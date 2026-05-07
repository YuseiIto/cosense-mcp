use serde::Deserialize;

/// Subset of fields returned by `GET /pages/:project`.
/// `count` is the **total number of pages in the project**, not the number returned in `pages`.
#[derive(Debug, Deserialize)]
pub struct PageList {
    pub count: u32,
    pub pages: Vec<PageSummary>,
}

#[derive(Debug, Deserialize)]
pub struct PageSummary {
    pub title: String,
    #[serde(default)]
    pub descriptions: Vec<String>,
    pub updated: i64,
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
/// `count` is the **total number of matching pages**, not the number returned in `pages`.
#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub count: u32,
    pub pages: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub title: String,
    #[serde(default)]
    pub lines: Vec<String>,
}
