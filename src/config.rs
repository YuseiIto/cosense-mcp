use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Config {
    pub projects: Vec<String>,
    pub default_project: String,
    pub sid: Option<String>,
    pub api_base: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let projects_raw = std::env::var("COSENSE_PROJECTS")
            .context("COSENSE_PROJECTS env var is required (comma-separated list of allowed Cosense project names)")?;
        let projects: Vec<String> = projects_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if projects.is_empty() {
            bail!("COSENSE_PROJECTS must contain at least one project name");
        }

        let default_project = match std::env::var("COSENSE_DEFAULT_PROJECT") {
            Ok(v) => {
                let v = v.trim().to_string();
                if !projects.contains(&v) {
                    bail!(
                        "COSENSE_DEFAULT_PROJECT='{v}' is not listed in COSENSE_PROJECTS ({})",
                        projects.join(", ")
                    );
                }
                v
            }
            Err(_) => projects[0].clone(),
        };

        let sid = std::env::var("COSENSE_SID").ok().filter(|s| !s.is_empty());
        let api_base = std::env::var("COSENSE_API_BASE")
            .unwrap_or_else(|_| "https://scrapbox.io/api".to_string());

        Ok(Self {
            projects,
            default_project,
            sid,
            api_base,
        })
    }

    pub fn is_allowed(&self, project: &str) -> bool {
        self.projects.iter().any(|p| p == project)
    }

    pub fn resolve_project<'a>(&'a self, project: Option<&'a str>) -> &'a str {
        project.unwrap_or(&self.default_project)
    }
}
