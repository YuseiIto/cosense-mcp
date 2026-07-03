//! Remote transport: a plain Streamable HTTP MCP endpoint at `/mcp`.
//!
//! This listener performs NO authentication itself. It is designed to sit
//! behind an authenticating reverse proxy — the supported setup is Cloudflare
//! Access with Managed OAuth in front of a Cloudflare Tunnel (see README),
//! which handles the OAuth flow required by MCP clients such as claude.ai and
//! only forwards requests from identities that pass your Access policies.
//! Keep the bind address on loopback (the default) so that nothing but the
//! local tunnel daemon can reach the port directly.

use anyhow::Result;
use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::sync::Arc;

use crate::config::HttpConfig;
use crate::mcp::CosenseServer;

pub async fn serve(server: CosenseServer, config: HttpConfig) -> Result<()> {
    let mcp_service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .nest_service("/mcp", mcp_service);

    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    tracing::info!(addr = %config.addr, "listening; MCP endpoint at http://{}/mcp", config.addr);
    axum::serve(listener, app).await?;
    Ok(())
}
