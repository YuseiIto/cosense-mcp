use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo,
};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::cosense::{CosenseClient, CosenseError, markdown};

#[derive(Clone)]
pub struct CosenseServer {
    client: Arc<CosenseClient>,
    config: Arc<Config>,
}

impl CosenseServer {
    pub fn new(client: CosenseClient, config: Config) -> Self {
        Self {
            client: Arc::new(client),
            config: Arc::new(config),
        }
    }

    fn resolve_project(&self, requested: Option<&str>) -> Result<String, McpError> {
        let project = self.config.resolve_project(requested);
        if !self.config.is_allowed(project) {
            return Err(McpError::invalid_params(
                format!(
                    "Project '{}' is not configured. Available: {}",
                    project,
                    self.config.projects.join(", ")
                ),
                None,
            ));
        }
        Ok(project.to_owned())
    }

    fn build_instructions(&self) -> String {
        format!(
            "This server exposes Cosense (Scrapbox) project content. \
             Available projects: {}. Default: `{}`. Pass the `project` argument to target a \
             specific one; omit it to use the default. \
             \n\nIntended usage: explore a project by following links — start with `list_pages` \
             (sorted by recent updates by default) or `search_pages`, then call `get_page` to read \
             content. Each `get_page` response includes Markdown body plus 1-hop / 2-hop related \
             page titles, which you should follow to deepen context.",
            self.config
                .projects
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", "),
            self.config.default_project,
        )
    }
}

// -------- Tool argument types --------

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPagesArgs {
    /// Cosense project name. Omit to use the configured default.
    #[serde(default)]
    pub project: Option<String>,
    /// Maximum number of pages to return (1-1000). Default 50.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Pagination offset.
    #[serde(default)]
    pub skip: Option<u32>,
    /// Sort order: `updated` (default), `created`, `accessed`, `linked`, `views`, or `title`.
    #[serde(default)]
    pub sort: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetPageArgs {
    /// Cosense project name. Omit to use the configured default.
    #[serde(default)]
    pub project: Option<String>,
    /// Page title (case-sensitive, matches the Cosense URL slug after URL decoding).
    pub title: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchPagesArgs {
    /// Cosense project name. Omit to use the configured default.
    #[serde(default)]
    pub project: Option<String>,
    /// Full-text search query. Supports AND, exclusion (`-term`), and phrase search (`"..."`).
    pub query: String,
}

// -------- Tool response types --------

#[derive(Debug, Serialize)]
struct ListPagesResponse {
    project: String,
    /// Total number of pages in the project (not the number returned in `pages`).
    total_pages_in_project: u32,
    pages: Vec<ListPageItem>,
}

#[derive(Debug, Serialize)]
struct ListPageItem {
    title: String,
    description: String,
    updated: i64,
}

#[derive(Debug, Serialize)]
struct GetPageResponse {
    project: String,
    title: String,
    updated: i64,
    body_markdown: String,
    links_1hop: Vec<String>,
    links_2hop: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SearchPagesResponse {
    project: String,
    /// Total number of matching pages (not the number returned in `hits`).
    total_hits: u32,
    hits: Vec<SearchHitItem>,
}

#[derive(Debug, Serialize)]
struct SearchHitItem {
    title: String,
    snippet: String,
}

// -------- Tools --------

#[tool_router]
impl CosenseServer {
    #[tool(
        description = "List pages in a Cosense project. Defaults to the most recently updated pages — useful as an entry point for exploration. Returns title, the first description line, and update timestamp. `total_pages_in_project` is the project-wide count; the actual returned slice is `pages` (length up to `limit`). Omit `project` to use the default project."
    )]
    async fn list_pages(
        &self,
        Parameters(args): Parameters<ListPagesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.resolve_project(args.project.as_deref())?;
        let sort = args.sort.as_deref().or(Some("updated"));
        let limit = args.limit.or(Some(50));

        let result = self
            .client
            .list_pages(&project, limit, args.skip, sort)
            .await
            .map(|list| ListPagesResponse {
                total_pages_in_project: list.count,
                pages: list
                    .pages
                    .into_iter()
                    .map(|p| ListPageItem {
                        description: p.descriptions.first().cloned().unwrap_or_default(),
                        title: p.title,
                        updated: p.updated,
                    })
                    .collect(),
                project: project.clone(),
            });
        Ok(into_call_result(result))
    }

    #[tool(
        description = "Get a Cosense page rendered as Markdown, plus 1-hop and 2-hop related page titles. The 1-hop links are pages directly linked from / to this page; 2-hop links surface co-occurrence clusters in the project. Follow these to traverse the link graph. Omit `project` to use the default project."
    )]
    async fn get_page(
        &self,
        Parameters(args): Parameters<GetPageArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.resolve_project(args.project.as_deref())?;
        let result = self
            .client
            .get_page(&project, &args.title)
            .await
            .map(|page| {
                let lines: Vec<String> = page.lines.into_iter().map(|l| l.text).collect();
                let related = page.related_pages.unwrap_or_default();
                GetPageResponse {
                    title: page.title,
                    updated: page.updated,
                    body_markdown: markdown::render(&project, &lines),
                    links_1hop: related.links_1hop.into_iter().map(|p| p.title).collect(),
                    links_2hop: related.links_2hop.into_iter().map(|p| p.title).collect(),
                    project: project.clone(),
                }
            });
        Ok(into_call_result(result))
    }

    #[tool(
        description = "Full-text search within a Cosense project. Returns hit titles and snippet lines. `total_hits` is the project-wide match count; up to ~100 are returned in `hits` due to API limits. Use this when you need to find pages by content rather than browse by recent update. Omit `project` to use the default project."
    )]
    async fn search_pages(
        &self,
        Parameters(args): Parameters<SearchPagesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let project = self.resolve_project(args.project.as_deref())?;
        let result = self
            .client
            .search_pages(&project, &args.query)
            .await
            .map(|result| SearchPagesResponse {
                total_hits: result.count,
                hits: result
                    .pages
                    .into_iter()
                    .map(|h| SearchHitItem {
                        snippet: build_snippet(&h.lines),
                        title: h.title,
                    })
                    .collect(),
                project: project.clone(),
            });
        Ok(into_call_result(result))
    }
}

const SEARCH_SNIPPET_LINES: usize = 3;
const SEARCH_SNIPPET_MAX_CHARS: usize = 400;

fn build_snippet(lines: &[String]) -> String {
    let mut s = lines
        .iter()
        .take(SEARCH_SNIPPET_LINES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" / ");
    if s.chars().count() > SEARCH_SNIPPET_MAX_CHARS {
        let truncated: String = s.chars().take(SEARCH_SNIPPET_MAX_CHARS).collect();
        s = format!("{truncated}…");
    }
    s
}

#[tool_handler]
impl ServerHandler for CosenseServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(self.build_instructions())
    }
}

fn into_call_result<T: Serialize>(result: Result<T, CosenseError>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(vec![Content::text(
            serde_json::to_string(&value).unwrap_or_default(),
        )]),
        Err(err) => CallToolResult::error(vec![Content::text(err.to_string())]),
    }
}
