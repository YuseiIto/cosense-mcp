use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{Result, bail};
use url::{Host, Url};

const DEFAULT_API_BASE: &str = "https://scrapbox.io/api";
const DEFAULT_HTTP_PORT: u16 = 8080;
// Loopback by default: in the intended deployment only a local cloudflared
// (or similar reverse proxy) should reach the HTTP listener directly.
const DEFAULT_HTTP_BIND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
}

impl Transport {
    pub fn from_env() -> Result<Self> {
        Self::from_raw(std::env::var("COSENSE_MCP_TRANSPORT").ok().as_deref())
    }

    pub fn from_raw(raw: Option<&str>) -> Result<Self> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("stdio") => Ok(Self::Stdio),
            Some("http") => Ok(Self::Http),
            Some(other) => {
                bail!("COSENSE_MCP_TRANSPORT must be 'stdio' (default) or 'http', got '{other}'")
            }
        }
    }
}

/// Settings for the remote (Streamable HTTP) transport. Authentication is NOT
/// handled here — put the listener behind an authenticating proxy such as
/// Cloudflare Access (see README).
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub projects: Vec<String>,
    pub default_project: String,
    pub sid: Option<String>,
    pub api_base: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_raw(
            std::env::var("COSENSE_PROJECTS").ok().as_deref(),
            std::env::var("COSENSE_DEFAULT_PROJECT").ok().as_deref(),
            std::env::var("COSENSE_SID").ok().as_deref(),
            std::env::var("COSENSE_API_BASE").ok().as_deref(),
        )
    }

    pub fn from_raw(
        projects: Option<&str>,
        default: Option<&str>,
        sid: Option<&str>,
        api_base: Option<&str>,
    ) -> Result<Self> {
        let projects_raw = projects
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!(
                "COSENSE_PROJECTS env var is required (comma-separated list of allowed Cosense project names)"
            ))?;

        let projects: Vec<String> = projects_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if projects.is_empty() {
            bail!("COSENSE_PROJECTS must contain at least one project name");
        }

        let default_project = match default.map(str::trim).filter(|s| !s.is_empty()) {
            Some(v) => {
                if !projects.iter().any(|p| p == v) {
                    bail!(
                        "COSENSE_DEFAULT_PROJECT='{v}' is not listed in COSENSE_PROJECTS ({})",
                        projects.join(", ")
                    );
                }
                v.to_string()
            }
            None => projects[0].clone(),
        };

        let sid = sid.filter(|s| !s.is_empty()).map(str::to_owned);

        let api_base = validate_api_base(api_base, sid.is_some())?;

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

impl HttpConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_raw(
            std::env::var("COSENSE_MCP_BIND").ok().as_deref(),
            std::env::var("PORT").ok().as_deref(),
        )
    }

    pub fn from_raw(bind: Option<&str>, port: Option<&str>) -> Result<Self> {
        let bind = match bind.map(str::trim).filter(|s| !s.is_empty()) {
            Some(b) => b
                .parse::<IpAddr>()
                .map_err(|_| anyhow::anyhow!("COSENSE_MCP_BIND='{b}' is not a valid IP address"))?,
            None => DEFAULT_HTTP_BIND,
        };
        let port = match port.map(str::trim).filter(|s| !s.is_empty()) {
            Some(p) => p
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("PORT='{p}' is not a valid port number"))?,
            None => DEFAULT_HTTP_PORT,
        };
        Ok(Self {
            addr: SocketAddr::new(bind, port),
        })
    }
}

fn validate_api_base(raw: Option<&str>, sid_set: bool) -> Result<String> {
    let raw = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_API_BASE);
    let url = Url::parse(raw)
        .map_err(|e| anyhow::anyhow!("COSENSE_API_BASE='{raw}' is not a valid URL: {e}"))?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        bail!("COSENSE_API_BASE must use http or https scheme, got '{scheme}'");
    }
    let host = url
        .host()
        .ok_or_else(|| anyhow::anyhow!("COSENSE_API_BASE='{raw}' has no host"))?;

    if sid_set && scheme == "http" && !is_loopback(&host) {
        bail!(
            "refusing to send COSENSE_SID over plaintext HTTP to non-loopback host '{host}'. \
             Use https:// for production, or unset COSENSE_SID."
        );
    }

    // Trim a single trailing slash so endpoint paths join cleanly with `format!("{base}/...")`.
    let normalized = url.as_str().trim_end_matches('/').to_string();
    Ok(normalized)
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        // Some local DNS setups resolve "localhost" — accept it without a lookup.
        Host::Domain(name) => *name == "localhost",
        Host::Ipv4(addr) => addr.is_loopback(),
        Host::Ipv6(addr) => addr.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        projects: Option<&str>,
        default: Option<&str>,
        sid: Option<&str>,
        api_base: Option<&str>,
    ) -> Result<Config> {
        Config::from_raw(projects, default, sid, api_base)
    }

    #[test]
    fn projects_required() {
        assert!(cfg(None, None, None, None).is_err());
        assert!(cfg(Some(""), None, None, None).is_err());
        assert!(cfg(Some(" , , "), None, None, None).is_err());
    }

    #[test]
    fn default_falls_back_to_first_project() {
        let c = cfg(Some("a,b,c"), None, None, None).unwrap();
        assert_eq!(c.default_project, "a");
        assert_eq!(c.projects, vec!["a", "b", "c"]);
    }

    #[test]
    fn explicit_default_must_be_in_allowlist() {
        assert!(cfg(Some("a,b"), Some("c"), None, None).is_err());
        let c = cfg(Some("a,b"), Some("b"), None, None).unwrap();
        assert_eq!(c.default_project, "b");
    }

    #[test]
    fn api_base_defaults_to_scrapbox() {
        let c = cfg(Some("x"), None, None, None).unwrap();
        assert_eq!(c.api_base, "https://scrapbox.io/api");
    }

    #[test]
    fn api_base_rejects_non_http_schemes() {
        assert!(cfg(Some("x"), None, None, Some("file:///etc/passwd")).is_err());
        assert!(cfg(Some("x"), None, None, Some("ftp://example.com")).is_err());
        assert!(cfg(Some("x"), None, None, Some("not a url")).is_err());
    }

    #[test]
    fn sid_over_plaintext_http_to_remote_host_rejected() {
        let r = cfg(Some("x"), None, Some("abc"), Some("http://example.com/api"));
        assert!(r.is_err(), "expected SID-over-HTTP rejection: {r:?}");
    }

    #[test]
    fn sid_over_plaintext_http_to_loopback_allowed() {
        for host in [
            "http://localhost:5000",
            "http://127.0.0.1:8080",
            "http://127.5.5.5",
            "http://[::1]:9000",
        ] {
            let r = cfg(Some("x"), None, Some("abc"), Some(host));
            assert!(r.is_ok(), "expected loopback allowed for {host}: {r:?}");
        }
    }

    #[test]
    fn sid_over_plaintext_http_to_lookalike_host_rejected() {
        // "127.foo" matches the old `starts_with("127.")` heuristic but is not a real loopback.
        let r = cfg(Some("x"), None, Some("abc"), Some("http://127.foo/api"));
        assert!(r.is_err(), "expected rejection for 127.foo: {r:?}");
    }

    #[test]
    fn sid_over_https_to_any_host_allowed() {
        let r = cfg(
            Some("x"),
            None,
            Some("abc"),
            Some("https://example.com/api"),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn api_base_trailing_slash_normalized() {
        let c = cfg(Some("x"), None, None, Some("https://example.com/api/")).unwrap();
        assert_eq!(c.api_base, "https://example.com/api");
    }

    #[test]
    fn transport_parsing() {
        assert_eq!(Transport::from_raw(None).unwrap(), Transport::Stdio);
        assert_eq!(
            Transport::from_raw(Some("stdio")).unwrap(),
            Transport::Stdio
        );
        assert_eq!(Transport::from_raw(Some("http")).unwrap(), Transport::Http);
        assert!(Transport::from_raw(Some("sse")).is_err());
    }

    #[test]
    fn http_config_defaults_to_loopback_8080() {
        let c = HttpConfig::from_raw(None, None).unwrap();
        assert_eq!(c.addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn http_config_bind_and_port_parsing() {
        let c = HttpConfig::from_raw(Some("0.0.0.0"), Some("3000")).unwrap();
        assert_eq!(c.addr.to_string(), "0.0.0.0:3000");
        let c = HttpConfig::from_raw(Some("::1"), None).unwrap();
        assert_eq!(c.addr.to_string(), "[::1]:8080");
        assert!(HttpConfig::from_raw(Some("not-an-ip"), None).is_err());
        assert!(HttpConfig::from_raw(None, Some("notaport")).is_err());
        assert!(HttpConfig::from_raw(None, Some("99999")).is_err());
    }

    #[test]
    fn is_allowed_and_resolve_project() {
        let c = cfg(Some("a,b"), Some("b"), None, None).unwrap();
        assert!(c.is_allowed("a"));
        assert!(!c.is_allowed("c"));
        assert_eq!(c.resolve_project(None), "b");
        assert_eq!(c.resolve_project(Some("a")), "a");
    }
}
