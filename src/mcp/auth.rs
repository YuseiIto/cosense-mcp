use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use axum::{
    body::Body,
    extract::State,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use axum::extract::Request;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;

use crate::config::HttpConfig;

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

pub struct JwksCache {
    keys: ArcSwap<HashMap<String, DecodingKey>>,
    last_fetch: Mutex<Option<Instant>>,
    jwks_url: String,
    client: reqwest::Client,
}

impl JwksCache {
    fn new(jwks_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest client build");
        Self {
            keys: ArcSwap::from_pointee(HashMap::new()),
            last_fetch: Mutex::new(None),
            jwks_url,
            client,
        }
    }

    pub async fn initialize(&self) -> anyhow::Result<()> {
        let keys = self.do_fetch().await?;
        self.keys.store(Arc::new(keys));
        *self.last_fetch.lock().unwrap() = Some(Instant::now());
        Ok(())
    }

    async fn do_fetch(&self) -> anyhow::Result<HashMap<String, DecodingKey>> {
        let resp: JwksResponse = self
            .client
            .get(&self.jwks_url)
            .send()
            .await?
            .json()
            .await?;
        let mut map = HashMap::new();
        for jwk in resp.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            if let (Some(kid), Some(n), Some(e)) = (jwk.kid, jwk.n, jwk.e) {
                if let Ok(key) = DecodingKey::from_rsa_components(&n, &e) {
                    map.insert(kid, key);
                }
            }
        }
        Ok(map)
    }

    pub async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        {
            let keys = self.keys.load();
            if let Some(k) = keys.get(kid) {
                return Some(k.clone());
            }
        }

        // Rate-limited refetch to handle key rotation (max once per 60 s).
        let should_fetch = {
            let guard = self.last_fetch.lock().unwrap();
            guard.map_or(true, |t| t.elapsed() > Duration::from_secs(60))
        };

        if should_fetch {
            // Update timestamp before the fetch so concurrent requests don't pile up.
            *self.last_fetch.lock().unwrap() = Some(Instant::now());
            match self.do_fetch().await {
                Ok(new_keys) => {
                    let key = new_keys.get(kid).cloned();
                    self.keys.store(Arc::new(new_keys));
                    return key;
                }
                Err(e) => {
                    tracing::warn!("JWKS refetch failed: {e}");
                }
            }
        }

        None
    }
}

pub struct AuthState {
    pub jwks: JwksCache,
    validation: Validation,
    /// WWW-Authenticate value for requests with no token.
    no_token_header: String,
    /// WWW-Authenticate value for requests with an invalid token.
    invalid_token_header: String,
}

impl AuthState {
    pub fn new(config: &HttpConfig) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[config.auth0_audience.clone()]);
        validation.set_issuer(&[config.issuer()]);
        validation.leeway = 60;

        let base = format!(
            r#"Bearer realm="cosense-mcp", resource_metadata="{}/.well-known/oauth-protected-resource""#,
            config.resource_server_url,
        );

        Self {
            jwks: JwksCache::new(config.jwks_url()),
            validation,
            invalid_token_header: format!(r#"{base}, error="invalid_token""#),
            no_token_header: base,
        }
    }
}

#[derive(Deserialize)]
struct EmptyClaims {}

pub async fn auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request,
    next: Next,
) -> Response {
    // CorsLayer intercepts OPTIONS before us, but pass through defensively.
    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);

    let token = match token {
        Some(t) => t,
        None => return build_401(&state.no_token_header),
    };

    if verify_jwt(&state, &token).await.is_err() {
        return build_401(&state.invalid_token_header);
    }

    next.run(request).await
}

async fn verify_jwt(state: &AuthState, token: &str) -> Result<(), ()> {
    let header = decode_header(token).map_err(|_| ())?;
    let kid = header.kid.ok_or(())?;
    let key = state.jwks.get_key(&kid).await.ok_or(())?;
    decode::<EmptyClaims>(token, &key, &state.validation).map_err(|e| {
        tracing::debug!("JWT verification failed: {e}");
    })?;
    Ok(())
}

fn build_401(www_auth: &str) -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, www_auth)
        .body(Body::empty())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, middleware, routing::get};
    use tower::ServiceExt;

    use crate::config::HttpConfig;

    fn test_config() -> HttpConfig {
        HttpConfig {
            auth0_domain: "test.auth0.com".to_string(),
            auth0_audience: "https://test.local/api".to_string(),
            resource_server_url: "https://test.lambda.aws".to_string(),
        }
    }

    fn test_router() -> Router {
        let state = Arc::new(AuthState::new(&test_config()));
        Router::new()
            .route("/mcp", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, auth_middleware))
    }

    #[tokio::test]
    async fn no_token_returns_401_without_error_field() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp.headers().get(header::WWW_AUTHENTICATE).unwrap().to_str().unwrap();
        assert!(www.contains("Bearer realm="));
        assert!(www.contains("resource_metadata="));
        assert!(!www.contains("error="), "no-token 401 should not have error field");
    }

    #[tokio::test]
    async fn malformed_token_returns_401_with_invalid_token() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, "Bearer not.a.jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp.headers().get(header::WWW_AUTHENTICATE).unwrap().to_str().unwrap();
        assert!(www.contains(r#"error="invalid_token""#));
    }

    #[tokio::test]
    async fn options_bypasses_auth_middleware() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .method("OPTIONS")
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED, "OPTIONS should bypass auth");
    }
}
