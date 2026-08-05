use std::sync::Arc;

use axum::body::Body;
use cosense_mcp::{
    config::{Config, HttpConfig},
    cosense::{AuthMethod, CosenseClient},
    mcp::{CosenseServer, auth::AuthState, http::build_router},
};
use lambda_http::{Body as LambdaBody, Error, Request, Response, run, service_fn};
use tower::ServiceExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Ceiling on how much of a response is buffered before handing it to Lambda.
///
/// Lambda refuses payloads over 6 MB anyway, so anything larger cannot be
/// delivered; failing here turns "silently allocate whatever the handler
/// produced" into a bounded error. A tool that manages to generate more than
/// this has gone wrong upstream.
const MAX_RESPONSE_BYTES: usize = 6 * 1024 * 1024;

async fn handle(req: Request, router: axum::Router) -> Result<Response<LambdaBody>, Error> {
    // Convert Lambda request body to axum body.
    let (parts, lambda_body) = req.into_parts();
    let bytes: Vec<u8> = match lambda_body {
        LambdaBody::Empty => vec![],
        LambdaBody::Text(s) => s.into_bytes(),
        LambdaBody::Binary(b) => b,
    };
    let axum_req = http::Request::from_parts(parts, Body::from(bytes));

    // Route through axum (Router::Error = Infallible).
    let axum_resp = router.oneshot(axum_req).await.unwrap();

    // Convert axum response body back to Lambda body.
    let (resp_parts, resp_body) = axum_resp.into_parts();
    let resp_bytes = axum::body::to_bytes(resp_body, MAX_RESPONSE_BYTES)
        .await
        .map_err(|e| format!("{e}"))?;

    let lambda_body = if resp_bytes.is_empty() {
        LambdaBody::Empty
    } else {
        LambdaBody::Binary(resp_bytes.to_vec())
    };

    Ok(Response::from_parts(resp_parts, lambda_body))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env().map_err(|e| e.to_string())?;
    let http_config = HttpConfig::from_env().map_err(|e| e.to_string())?;

    tracing::info!(
        projects = ?config.projects,
        default = %config.default_project,
        sid_set = config.sid.is_some(),
        auth0_domain = %http_config.auth0_domain,
        "starting cosense-mcp lambda"
    );

    let auth_method = config
        .sid
        .clone()
        .map_or(AuthMethod::None, AuthMethod::SidCookie);
    let client =
        CosenseClient::new(config.api_base.clone(), auth_method).map_err(|e| e.to_string())?;
    let server = CosenseServer::new(client, config);

    let auth_state = Arc::new(AuthState::new(&http_config));
    if let Err(e) = auth_state.jwks.initialize().await {
        tracing::warn!("Initial JWKS fetch failed, will retry on first request: {e}");
    }

    let router = build_router(server, &http_config, auth_state);

    run(service_fn(move |req: Request| {
        let router = router.clone();
        async move { handle(req, router).await }
    }))
    .await
}
