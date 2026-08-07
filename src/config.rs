use anyhow::{Result, bail};
use url::{Host, Url};

const DEFAULT_API_BASE: &str = "https://scrapbox.io/api";

/// Configuration for the HTTP (Lambda) transport. Read only by the lambda binary.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Auth0 tenant domain, e.g. `yuseiito-mcp.us.auth0.com`.
    pub auth0_domain: String,
    /// Auth0 API identifier used as the JWT `aud` claim, e.g. `https://cosense-mcp.local/api`.
    pub auth0_audience: String,
    /// Public origin this server publishes as its own identity, e.g.
    /// `https://mcp.example.com`. `None` falls back to the request `Host`.
    ///
    /// Set this whenever a proxy sits in front: the value it forwards as `Host`
    /// belongs to that proxy's configuration, and a CDN may rewrite it to match
    /// the TLS name it uses upstream. It should agree with `auth0_audience`
    /// minus the path.
    pub public_base_url: Option<String>,
}

impl HttpConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_raw(
            std::env::var("AUTH0_DOMAIN").ok().as_deref(),
            std::env::var("AUTH0_AUDIENCE").ok().as_deref(),
            std::env::var("PUBLIC_BASE_URL").ok().as_deref(),
        )
    }

    pub fn from_raw(
        domain: Option<&str>,
        audience: Option<&str>,
        public_base_url: Option<&str>,
    ) -> Result<Self> {
        let auth0_domain = domain
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("AUTH0_DOMAIN env var is required"))?;
        let auth0_audience = audience
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("AUTH0_AUDIENCE env var is required"))?;

        // AUTH0_AUDIENCE sitting next to it *is* a URL, which makes writing a
        // scheme here the natural mistake. Unchecked it produces
        // `https://https://tenant/.well-known/jwks.json`, every request 401s,
        // and the startup log prints the value back looking perfectly correct.
        validate_auth0_domain(auth0_domain)?;

        Ok(Self {
            auth0_domain: auth0_domain.to_string(),
            auth0_audience: auth0_audience.to_string(),
            public_base_url: public_base_url
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string()),
        })
    }

    /// `https://<domain>/.well-known/jwks.json`
    pub fn jwks_url(&self) -> String {
        format!("https://{}/.well-known/jwks.json", self.auth0_domain)
    }

    /// Auth0 JWT issuer — includes trailing slash per Auth0 spec.
    pub fn issuer(&self) -> String {
        format!("https://{}/", self.auth0_domain)
    }

    /// `https://<domain>` — used as the `authorization_server` entry in RFC 9728 metadata.
    pub fn authorization_server(&self) -> String {
        format!("https://{}", self.auth0_domain)
    }
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

        // Trimmed like every other field. A secret pulled from a file or a
        // parameter store routinely carries a trailing newline, which makes an
        // invalid header value and fails every call with a network error that
        // names nothing. A whitespace-only value is worse: untrimmed it looks
        // set, so it arms the SID-over-plaintext guard while authenticating as
        // nobody.
        let sid = sid
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

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

/// A bare `host[:port]`, since it is interpolated into `https://{domain}/...`.
fn validate_auth0_domain(domain: &str) -> Result<()> {
    if domain.contains("://") {
        bail!("AUTH0_DOMAIN must be a bare hostname, not a URL (got '{domain}')");
    }
    if domain.contains('/') {
        bail!("AUTH0_DOMAIN must not contain a path (got '{domain}')");
    }
    if !domain
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
    {
        bail!("AUTH0_DOMAIN contains characters that are not valid in a hostname (got '{domain}')");
    }
    Ok(())
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
    fn sid_is_trimmed_and_blank_is_treated_as_unset() {
        let c = cfg(Some("x"), None, Some("  abc\n"), None).unwrap();
        assert_eq!(c.sid.as_deref(), Some("abc"));

        // Must be unset, not Some("   "): a blank value that survives would arm
        // the SID-over-plaintext guard while carrying no credential.
        let c = cfg(Some("x"), None, Some("  \n"), None).unwrap();
        assert_eq!(c.sid, None);
        assert!(
            cfg(
                Some("x"),
                None,
                Some(" \t "),
                Some("http://example.com/api")
            )
            .is_ok()
        );
    }

    #[test]
    fn auth0_domain_must_be_a_bare_host() {
        for bad in [
            "https://tenant.us.auth0.com",
            "tenant.us.auth0.com/",
            "tenant.us.auth0.com/api",
            "tenant .us.auth0.com",
        ] {
            let r = HttpConfig::from_raw(Some(bad), Some("https://api.example.com/mcp"), None);
            assert!(r.is_err(), "expected rejection for {bad:?}: {r:?}");
        }
    }

    #[test]
    fn auth0_config_trims_and_derives_urls() {
        let c = HttpConfig::from_raw(
            Some("  tenant.us.auth0.com  "),
            Some("  https://api.example.com/mcp  "),
            None,
        )
        .unwrap();
        assert_eq!(c.auth0_domain, "tenant.us.auth0.com");
        assert_eq!(c.auth0_audience, "https://api.example.com/mcp");
        assert_eq!(
            c.jwks_url(),
            "https://tenant.us.auth0.com/.well-known/jwks.json"
        );
        // Auth0 issuers carry the trailing slash; the audience must not gain one.
        assert_eq!(c.issuer(), "https://tenant.us.auth0.com/");
    }

    #[test]
    fn auth0_config_requires_both_values() {
        assert!(HttpConfig::from_raw(None, Some("https://a/mcp"), None).is_err());
        assert!(HttpConfig::from_raw(Some("t.auth0.com"), None, None).is_err());
        assert!(HttpConfig::from_raw(Some("  "), Some("https://a/mcp"), None).is_err());
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
