//! Reconstructs this server's public base URL from the request `Host` header.
//!
//! Behind a Lambda Function URL the public origin is not known at deploy time
//! (it depends on the generated URL id), so we derive it per-request instead of
//! configuring it. Function URLs put the public domain in `Host`.
//! Ref: https://docs.aws.amazon.com/lambda/latest/dg/urls-invocation.html
//!
//! The derived value only feeds OAuth discovery hints (RFC 9728 `resource` and
//! the `WWW-Authenticate` metadata URL), reflected to the requester; it never
//! gates access — JWT `aud`/`iss` are validated against static config. Scheme is
//! pinned to `https` (never trust `X-Forwarded-Proto`) and the host is rejected
//! unless it looks like a bare hostname so it can't smuggle a path or whitespace
//! into the emitted URL.

use axum::http::{HeaderMap, header};

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
