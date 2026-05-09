use std::sync::Arc;

use axum::{
    Router,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware,
    routing::get,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use tower_http::cors::CorsLayer;

use crate::config::HttpConfig;
use crate::mcp::CosenseServer;
use crate::mcp::auth::{AuthState, auth_middleware};

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

    let resource_url = http_config.resource_server_url.clone();
    let auth_server = http_config.authorization_server();
    let public = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let resource_url = resource_url.clone();
                let auth_server = auth_server.clone();
                async move {
                    axum::Json(json!({
                        "resource": resource_url,
                        "authorization_servers": [auth_server],
                        "scopes_supported": [],
                        "bearer_methods_supported": ["header"],
                    }))
                }
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
