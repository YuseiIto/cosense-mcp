use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    middleware,
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::{Value, json};
use tower_http::cors::CorsLayer;

use crate::config::HttpConfig;
use crate::mcp::CosenseServer;
use crate::mcp::auth::{AuthState, REQUIRED_PERMISSION, auth_middleware};
use crate::mcp::request_url::PublicOrigin;

/// RFC 9728 protected-resource metadata. `resource` is the canonical MCP server
/// URI (`<base>/mcp`), derived per-request because the public origin is unknown
/// at deploy time behind a Function URL. Errs 500 if the origin can't be derived.
fn protected_resource_metadata(
    headers: &HeaderMap,
    auth_server: &str,
    public_origin: &PublicOrigin,
) -> Result<Value, StatusCode> {
    let base = public_origin
        .resolve(headers)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [auth_server],
        "scopes_supported": [REQUIRED_PERMISSION],
        "bearer_methods_supported": ["header"],
    }))
}

/// The only browser origin allowed to reach this server, for both CORS and the
/// transport's own origin check. Kept in one place so the two cannot drift: a
/// permissive CORS policy paired with a strict origin check (or the reverse)
/// reads as protection while providing none.
const ALLOWED_BROWSER_ORIGIN: &str = "https://claude.ai";

pub fn build_router(
    server: CosenseServer,
    http_config: &HttpConfig,
    auth_state: Arc<AuthState>,
) -> Router {
    let mcp_service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            // Host validation cannot be used here: it needs the public hostname
            // at construction time, and behind a Function URL that is only known
            // once the URL exists. The host arrives in a header we do not
            // control, so pinning it to a compile-time value would be theatre.
            .disable_allowed_hosts()
            // Origin validation can, and covers the attack Host validation was
            // there for. Requests without an `Origin` header pass — which is
            // what server-to-server MCP clients send — while a browser coaxed
            // into calling us from another site is rejected.
            .with_allowed_origins([ALLOWED_BROWSER_ORIGIN]),
    );

    let protected = Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(auth_state, auth_middleware));

    let auth_server = http_config.authorization_server();
    let public_origin = PublicOrigin::configured(http_config.public_base_url.as_deref());
    let metadata = move |headers: HeaderMap| {
        let auth_server = auth_server.clone();
        let public_origin = public_origin.clone();
        async move {
            protected_resource_metadata(&headers, &auth_server, &public_origin).map(axum::Json)
        }
    };

    // Served at both paths on purpose. RFC 9728 §3.1 inserts the well-known
    // segment between host and path, so a resource of `<base>/mcp` is discovered
    // at `/.well-known/oauth-protected-resource/mcp`. Clients that instead
    // follow the `resource_metadata` pointer in our `WWW-Authenticate` header
    // arrive at the bare path. Both are in active use.
    let public = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(metadata.clone()),
        )
        .route("/.well-known/oauth-protected-resource/mcp", get(metadata))
        .route("/health", get(|| async { StatusCode::OK }));

    let cors = CorsLayer::new()
        .allow_origin(ALLOWED_BROWSER_ORIGIN.parse::<HeaderValue>().unwrap())
        .allow_methods([Method::POST, Method::GET, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("mcp-protocol-version"),
        ])
        // `WWW-Authenticate` is not CORS-safelisted, so without this a browser
        // strips it from our 401 and the client sees a bare rejection with no
        // `resource_metadata` pointer — the discovery hop the whole auth flow
        // is built around. Harmless for clients that call us server-side, but
        // the CORS layer exists precisely for the ones that do not.
        .expose_headers([header::WWW_AUTHENTICATE]);

    Router::new().merge(protected).merge(public).layer(cors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn resource_is_derived_mcp_uri() {
        let h = headers(&[
            ("host", "abc.lambda-url.ap-northeast-1.on.aws"),
            ("x-forwarded-proto", "https"),
        ]);
        let v = protected_resource_metadata(&h, "https://test.auth0.com", &PublicOrigin::default())
            .unwrap();
        assert_eq!(
            v["resource"],
            "https://abc.lambda-url.ap-northeast-1.on.aws/mcp"
        );
        assert_eq!(v["authorization_servers"][0], "https://test.auth0.com");
        assert_eq!(v["bearer_methods_supported"][0], "header");
    }

    #[test]
    fn configured_origin_wins_over_the_host_header() {
        let h = headers(&[("host", "whatever-the-proxy-forwarded.example")]);
        let v = protected_resource_metadata(
            &h,
            "https://test.auth0.com",
            &PublicOrigin::configured(Some("https://mcp.example.com/")),
        )
        .unwrap();
        // Trailing slash trimmed, and the Host ignored entirely.
        assert_eq!(v["resource"], "https://mcp.example.com/mcp");
    }

    #[test]
    fn missing_host_errs_500() {
        let err = protected_resource_metadata(
            &HeaderMap::new(),
            "https://test.auth0.com",
            &PublicOrigin::default(),
        )
        .unwrap_err();
        assert_eq!(err, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
