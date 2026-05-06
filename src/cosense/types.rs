use serde::Deserialize;

/// Subset of fields returned by `GET /pages/:project`.
#[derive(Debug, Deserialize)]
pub struct PageList {
    pub count: u32,
    #[serde(rename = "skip")]
    pub _skip: Option<u32>,
    #[serde(rename = "limit")]
    pub _limit: Option<u32>,
    pub pages: Vec<PageSummary>,
}

#[derive(Debug, Deserialize)]
pub struct PageSummary {
    pub title: String,
    #[serde(default)]
    pub descriptions: Vec<String>,
    pub updated: i64,
    #[serde(default)]
    pub views: Option<u64>,
}

/// Subset of fields returned by `GET /pages/:project/:title`.
#[derive(Debug, Deserialize)]
pub struct Page {
    pub title: String,
    pub updated: i64,
    pub lines: Vec<PageLine>,
    #[serde(rename = "relatedPages", default)]
    pub related_pages: Option<RelatedPages>,
}

#[derive(Debug, Deserialize)]
pub struct PageLine {
    pub text: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct RelatedPages {
    #[serde(rename = "links1hop", default)]
    pub links_1hop: Vec<RelatedPage>,
    #[serde(rename = "links2hop", default)]
    pub links_2hop: Vec<RelatedPage>,
}

#[derive(Debug, Deserialize)]
pub struct RelatedPage {
    pub title: String,
}

/// Subset of fields returned by `GET /pages/:project/search/query`.
#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub count: u32,
    pub pages: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub title: String,
    #[serde(default)]
    pub words: Vec<String>,
    #[serde(default)]
    pub lines: Vec<String>,
}
