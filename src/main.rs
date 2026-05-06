mod config;
mod cosense;
mod mcp;

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::Config;
use crate::cosense::{AuthMethod, CosenseClient};
use crate::mcp::CosenseServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Logs MUST go to stderr — stdout is reserved for MCP framing.
    fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    tracing::info!(
        projects = ?config.projects,
        default = %config.default_project,
        sid_set = config.sid.is_some(),
        api_base = %config.api_base,
        "starting cosense-mcp"
    );

    let auth = match config.sid.clone() {
        Some(sid) => AuthMethod::SidCookie(sid),
        None => AuthMethod::None,
    };
    let client = CosenseClient::new(config.api_base.clone(), auth)?;
    let server = CosenseServer::new(client, config);

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
