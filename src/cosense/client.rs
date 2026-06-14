use reqwest::{Client, RequestBuilder, Response, StatusCode};
use thiserror::Error;

use super::types::{Page, PageList, SearchResult};

#[derive(Debug, Clone)]
pub enum AuthMethod {
    None,
    SidCookie(String),
    // Phase 2 will add: OAuthBearer(String)
}

#[derive(Debug, Error)]
pub enum CosenseError {
    #[error("project '{project}' does not exist on Cosense (HTTP 404)")]
    ProjectNotFound { project: String },

    #[error(
        "page '{title}' not found in project '{project}'. Use list_pages or search_pages to find existing titles."
    )]
    PageNotFound { project: String, title: String },

    #[error(
        "access denied to project '{project}'. This project may be private — set COSENSE_SID env var."
    )]
    Unauthorized { project: String },

    #[error("Cosense API request failed: {0}")]
    Network(#[from] reqwest::Error),
}

pub struct CosenseClient {
    http: Client,
    api_base: String,
    auth: AuthMethod,
}

impl CosenseClient {
    pub fn new(api_base: impl Into<String>, auth: AuthMethod) -> reqwest::Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("cosense-mcp/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            api_base: api_base.into(),
            auth,
        })
    }

    fn request(&self, url: String) -> RequestBuilder {
        let mut req = self.http.get(url);
        if let AuthMethod::SidCookie(sid) = &self.auth {
            req = req.header("Cookie", format!("connect.sid={sid}"));
        }
        req
    }

    pub async fn list_pages(
        &self,
        project: &str,
        limit: Option<u32>,
        skip: Option<u32>,
        sort: Option<&str>,
    ) -> Result<PageList, CosenseError> {
        let url = format!("{}/pages/{}", self.api_base, urlencoding::encode(project));
        let resp = self
            .request(url)
            .query(&query_params(&[
                ("limit", limit.map(|v| v.to_string())),
                ("skip", skip.map(|v| v.to_string())),
                ("sort", sort.map(str::to_owned)),
            ]))
            .send()
            .await?;
        map_status(project, None, resp)?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn get_page(&self, project: &str, title: &str) -> Result<Page, CosenseError> {
        let url = format!(
            "{}/pages/{}/{}",
            self.api_base,
            urlencoding::encode(project),
            urlencoding::encode(title),
        );
        let resp = self.request(url).send().await?;
        // Cosense returns 404 for both missing project and missing page on this endpoint;
        // we surface PageNotFound either way since list_pages catches project-level 404s.
        map_status(project, Some(title), resp)?
            .json()
            .await
            .map_err(Into::into)
    }

    pub async fn search_pages(
        &self,
        project: &str,
        query: &str,
    ) -> Result<SearchResult, CosenseError> {
        let url = format!(
            "{}/pages/{}/search/query",
            self.api_base,
            urlencoding::encode(project),
        );
        let resp = self.request(url).query(&[("q", query)]).send().await?;
        map_status(project, None, resp)?
            .json()
            .await
            .map_err(Into::into)
    }
}

fn query_params(pairs: &[(&'static str, Option<String>)]) -> Vec<(&'static str, String)> {
    pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().map(|v| (*k, v.clone())))
        .collect()
}

/// Map HTTP errors to typed `CosenseError`. Pass `title` only for endpoints that target a specific
/// page; in that case 404 is interpreted as a page-not-found rather than project-not-found.
fn map_status(
    project: &str,
    title: Option<&str>,
    resp: Response,
) -> Result<Response, CosenseError> {
    match resp.status() {
        StatusCode::NOT_FOUND => Err(match title {
            Some(t) => CosenseError::PageNotFound {
                project: project.to_string(),
                title: t.to_string(),
            },
            None => CosenseError::ProjectNotFound {
                project: project.to_string(),
            },
        }),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(CosenseError::Unauthorized {
            project: project.to_string(),
        }),
        s if s.is_success() => Ok(resp),
        _ => Ok(resp.error_for_status()?),
    }
}
