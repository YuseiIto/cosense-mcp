use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
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

/// Sort orders the Cosense API accepts.
///
/// An enum rather than a string so the tool schema carries the closed set: an
/// unsupported value is then rejected before it reaches the API, instead of
/// coming back as an opaque upstream failure the model cannot act on.
/// Ref: <https://scrapbox.io/scrapboxlab/Scrapbox_REST_API%E3%81%AE%E4%B8%80%E8%A6%A7>
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Updated,
    Created,
    Accessed,
    Linked,
    Views,
    Title,
}

impl SortOrder {
    fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::Created => "created",
            Self::Accessed => "accessed",
            Self::Linked => "linked",
            Self::Views => "views",
            Self::Title => "title",
        }
    }
}

/// Range the upstream API accepts. Values outside it are an error there, so
/// they are an error here where the message can say what was wrong.
const LIMIT_MIN: u32 = 1;
const LIMIT_MAX: u32 = 1000;
const LIMIT_DEFAULT: u32 = 50;

fn validate_limit(limit: Option<u32>) -> Result<u32, String> {
    let limit = limit.unwrap_or(LIMIT_DEFAULT);
    if (LIMIT_MIN..=LIMIT_MAX).contains(&limit) {
        Ok(limit)
    } else {
        Err(format!(
            "limit must be between {LIMIT_MIN} and {LIMIT_MAX} (got {limit})"
        ))
    }
}

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
    /// Sort order. Defaults to `updated`.
    #[serde(default)]
    pub sort: Option<SortOrder>,
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
        let limit = validate_limit(args.limit).map_err(|e| McpError::invalid_params(e, None))?;
        let sort = args.sort.unwrap_or(SortOrder::Updated);

        let result = self
            .client
            .list_pages(&project, Some(limit), args.skip, Some(sort.as_str()))
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
        // A serialization failure must not become an empty success. The model
        // would read `""` as "the page is empty" or "nothing matched" and say so
        // confidently, which is worse than an error it can report or retry.
        Ok(value) => match serde_json::to_string(&value) {
            Ok(json) => CallToolResult::success(vec![Content::text(json)]),
            Err(err) => {
                tracing::error!("failed to serialize tool response: {err}");
                CallToolResult::error(vec![Content::text(format!(
                    "internal error: could not serialize the response: {err}"
                ))])
            }
        },
        Err(err) => CallToolResult::error(vec![Content::text(err.to_string())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_defaults_and_bounds_are_enforced() {
        assert_eq!(validate_limit(None).unwrap(), LIMIT_DEFAULT);
        assert_eq!(validate_limit(Some(LIMIT_MIN)).unwrap(), LIMIT_MIN);
        assert_eq!(validate_limit(Some(LIMIT_MAX)).unwrap(), LIMIT_MAX);

        // Rejected here rather than passed upstream, where the failure would
        // come back as an opaque API error.
        assert!(validate_limit(Some(0)).is_err());
        assert!(validate_limit(Some(LIMIT_MAX + 1)).is_err());
    }

    #[test]
    fn sort_order_matches_the_api_spelling() {
        // These strings go into the query string verbatim.
        assert_eq!(SortOrder::Updated.as_str(), "updated");
        assert_eq!(SortOrder::Created.as_str(), "created");
        assert_eq!(SortOrder::Accessed.as_str(), "accessed");
        assert_eq!(SortOrder::Linked.as_str(), "linked");
        assert_eq!(SortOrder::Views.as_str(), "views");
        assert_eq!(SortOrder::Title.as_str(), "title");
    }

    #[test]
    fn sort_order_deserializes_from_the_wire_form() {
        let parsed: SortOrder = serde_json::from_str("\"views\"").unwrap();
        assert_eq!(parsed.as_str(), "views");
        assert!(serde_json::from_str::<SortOrder>("\"Views\"").is_err());
        assert!(serde_json::from_str::<SortOrder>("\"nonsense\"").is_err());
    }
}
