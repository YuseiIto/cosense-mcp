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
use crate::mcp::auth::{AuthState, auth_middleware};
use crate::mcp::request_url::public_base_url;

/// RFC 9728 protected-resource metadata. `resource` is the canonical MCP server
/// URI (`<base>/mcp`), derived per-request because the public origin is unknown
/// at deploy time behind a Function URL. Errs 500 if the origin can't be derived.
fn protected_resource_metadata(
    headers: &HeaderMap,
    auth_server: &str,
) -> Result<Value, StatusCode> {
    let base = public_base_url(headers).ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [auth_server],
        "scopes_supported": [],
        "bearer_methods_supported": ["header"],
    }))
}

pub fn build_router(
    server: CosenseServer,
    http_config: HttpConfig,
    auth_state: Arc<AuthState>,
) -> Router {
    let mcp_service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts(),
    );

    let protected = Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(auth_state, auth_middleware));

    let auth_server = http_config.authorization_server();
    let public = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(move |headers: HeaderMap| {
                let auth_server = auth_server.clone();
                async move { protected_resource_metadata(&headers, &auth_server).map(axum::Json) }
            }),
        )
        .route("/health", get(|| async { StatusCode::OK }));

    let cors = CorsLayer::new()
        .allow_origin("https://claude.ai".parse::<HeaderValue>().unwrap())
        .allow_methods([Method::POST, Method::GET, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("mcp-protocol-version"),
        ]);

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
        let v = protected_resource_metadata(&h, "https://test.auth0.com").unwrap();
        assert_eq!(
            v["resource"],
            "https://abc.lambda-url.ap-northeast-1.on.aws/mcp"
        );
        assert_eq!(v["authorization_servers"][0], "https://test.auth0.com");
        assert_eq!(v["bearer_methods_supported"][0], "header");
    }

    #[test]
    fn missing_host_errs_500() {
        let err =
            protected_resource_metadata(&HeaderMap::new(), "https://test.auth0.com").unwrap_err();
        assert_eq!(err, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
