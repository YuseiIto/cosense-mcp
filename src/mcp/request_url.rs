//! This server's public base URL, which it publishes as its own identity in
//! OAuth discovery (the RFC 9728 `resource` and the `WWW-Authenticate` metadata
//! URL).
//!
//! Prefer configuring it. Deriving it from the request `Host` means the server
//! learns who it is from the caller, and behind a proxy that value is whatever
//! the proxy chose to forward — a detail outside this configuration, and one
//! that a CDN may legitimately rewrite to match the TLS name it uses upstream.
//! When the public hostname is fixed (and it must be: an authorization server's
//! API identifier is usually immutable and has to equal this), configuration is
//! both simpler and truthful.
//!
//! The `Host` fallback remains for local runs and for a bare Lambda Function
//! URL, whose hostname is not known until the URL exists.
//! Ref: <https://docs.aws.amazon.com/lambda/latest/dg/urls-invocation.html>
//!
//! Either way the scheme is pinned to `https` (never trust `X-Forwarded-Proto`)
//! and a derived host is rejected unless it looks like a bare hostname, so it
//! cannot smuggle a path or whitespace into the emitted URL.

use axum::http::{HeaderMap, header};

/// Configured public base URL, if any. Takes precedence over the `Host` header.
#[derive(Debug, Clone, Default)]
pub struct PublicOrigin(Option<String>);

impl PublicOrigin {
    /// `base` is trusted as given; it comes from configuration, not the network.
    /// A trailing slash is trimmed so paths join cleanly.
    pub fn configured(base: Option<&str>) -> Self {
        Self(
            base.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_end_matches('/').to_string()),
        )
    }

    pub fn resolve(&self, headers: &HeaderMap) -> Option<String> {
        self.0.clone().or_else(|| public_base_url(headers))
    }
}

/// `https://<host>` from the request `Host` header, or `None` if it is absent or
/// not a plausible `host[:port]` (alphanumerics, `.`, `-`, `:` only).
pub fn public_base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    if host.is_empty() || !host.bytes().all(is_host_byte) {
        return None;
    }
    Some(format!("https://{host}"))
}

fn is_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn builds_https_url_from_host() {
        let h = headers(&[("host", "abc.lambda-url.ap-northeast-1.on.aws")]);
        assert_eq!(
            public_base_url(&h).as_deref(),
            Some("https://abc.lambda-url.ap-northeast-1.on.aws")
        );
    }

    #[test]
    fn pins_scheme_to_https_ignoring_forwarded_proto() {
        // Function URLs are always HTTPS; never let a header downgrade the emitted URL.
        let h = headers(&[("host", "example.com"), ("x-forwarded-proto", "http")]);
        assert_eq!(public_base_url(&h).as_deref(), Some("https://example.com"));
    }

    #[test]
    fn accepts_host_with_port() {
        let h = headers(&[("host", "localhost:8080")]);
        assert_eq!(
            public_base_url(&h).as_deref(),
            Some("https://localhost:8080")
        );
    }

    #[test]
    fn returns_none_when_host_absent() {
        let h = headers(&[("x-forwarded-proto", "https")]);
        assert_eq!(public_base_url(&h), None);
    }

    #[test]
    fn rejects_host_with_non_hostname_characters() {
        for bad in ["evil.com/path", "foo bar", "a@b.com", "x.com\\y"] {
            let h = headers(&[("host", bad)]);
            assert_eq!(public_base_url(&h), None, "should reject host {bad:?}");
        }
    }
}
