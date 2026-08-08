use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use axum::extract::Request;
use axum::{
    body::Body,
    extract::State,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;

use crate::config::HttpConfig;
use crate::mcp::request_url::PublicOrigin;

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    /// `sig` or `enc` (RFC 7517 §4.2). Absent means unrestricted.
    #[serde(rename = "use")]
    key_use: Option<String>,
    alg: Option<String>,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

impl Jwk {
    /// RFC 7517 requires honouring `use` and `alg`: accepting everything
    /// RSA-shaped would admit an encryption key into the signature keyset.
    fn is_rs256_signing_key(&self) -> bool {
        self.kty == "RSA"
            && self.key_use.as_deref().is_none_or(|u| u == "sig")
            && self.alg.as_deref().is_none_or(|a| a == "RS256")
    }
}

#[derive(Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// How long a failed `kid` lookup suppresses another JWKS fetch, bounding how
/// hard an attacker-chosen `kid` can drive requests to the authorization server.
const REFETCH_INTERVAL: Duration = Duration::from_mins(1);

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
        let resp: JwksResponse = self.client.get(&self.jwks_url).send().await?.json().await?;
        let mut map = HashMap::new();
        for jwk in resp.keys {
            if !jwk.is_rs256_signing_key() {
                continue;
            }
            if let (Some(kid), Some(n), Some(e)) = (jwk.kid, jwk.n, jwk.e)
                && let Ok(key) = DecodingKey::from_rsa_components(&n, &e)
            {
                map.insert(kid, key);
            }
        }
        Ok(map)
    }

    pub async fn get_key(&self, kid: &str) -> KeyLookup {
        {
            let keys = self.keys.load();
            if let Some(k) = keys.get(kid) {
                return KeyLookup::Found(k.clone());
            }
        }

        // Rate-limited refetch for key rotation. Check and stamp under one lock:
        // reading first and writing after leaves a window where every concurrent
        // request sees a stale timestamp and refetches — the stampede this limit
        // exists to prevent.
        let should_fetch = {
            let mut guard = self.last_fetch.lock().unwrap();
            let stale = guard.is_none_or(|t| t.elapsed() > REFETCH_INTERVAL);
            if stale {
                *guard = Some(Instant::now());
            }
            stale
        };

        if should_fetch {
            match self.do_fetch().await {
                Ok(new_keys) => {
                    let key = new_keys.get(kid).cloned();
                    self.keys.store(Arc::new(new_keys));
                    return key.map_or(KeyLookup::UnknownKid, KeyLookup::Found);
                }
                Err(e) => {
                    tracing::warn!("JWKS refetch failed: {e}");
                    return KeyLookup::Unavailable;
                }
            }
        }

        // Nothing cached inside the refetch window: an unknown `kid` and an
        // outage are indistinguishable here. Report the outage — the opposite
        // makes a client discard a valid token and redo the whole OAuth flow.
        if self.keys.load().is_empty() {
            KeyLookup::Unavailable
        } else {
            KeyLookup::UnknownKid
        }
    }
}

/// Outcome of resolving a `kid`. "Unknown key" and "could not ask" map to
/// different statuses, deciding whether a client retries or re-authenticates.
pub enum KeyLookup {
    Found(DecodingKey),
    UnknownKid,
    Unavailable,
}

pub struct AuthState {
    pub jwks: JwksCache,
    validation: Validation,
    public_origin: PublicOrigin,
}

impl AuthState {
    pub fn new(config: &HttpConfig) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(std::slice::from_ref(&config.auth0_audience));
        validation.set_issuer(&[config.issuer()]);
        validation.leeway = 60;
        // `aud`/`iss` are only compared when present, and the default required
        // set is `exp` alone: without this a token signed by the same JWKS but
        // carrying neither claim passes both checks.
        validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
        // Requiring "nbf" above only asserts the claim exists; the comparison is
        // gated on this flag, off by default, so without it a future-dated token
        // verifies today.
        validation.validate_nbf = true;

        Self {
            jwks: JwksCache::new(config.jwks_url()),
            validation,
            public_origin: PublicOrigin::configured(config.public_base_url.as_deref()),
        }
    }
}

/// Base `WWW-Authenticate` value: where our metadata lives, and which scope to
/// request.
///
/// MCP's scope-selection strategy consults the challenge's `scope` *before*
/// `scopes_supported`, and a client that authorizes without requesting any scope
/// receives a token with an empty `scope` claim, which this server rejects.
/// Naming the scope on the first 401 steers it to a token that works.
fn www_authenticate(base_url: &str) -> String {
    format!(
        r#"Bearer realm="cosense-mcp", scope="{REQUIRED_SCOPE}", resource_metadata="{base_url}/.well-known/oauth-protected-resource""#
    )
}

/// Scope a token must carry to reach any tool — the authorization boundary.
///
/// `scope` expresses what the user delegated *to this client*: the authorization
/// server fills it with the intersection of what the client requested and what
/// the user's roles grant. Why not the `permissions` claim (the previous
/// design): it lists the user's full grants regardless of what the client
/// requested, so it stops modelling delegation once a second client exists.
/// Not an identity allowlist either — who may use this server is a role
/// assignment on the authorization server, not baked into the binary.
pub const REQUIRED_SCOPE: &str = "read:cosense";

#[derive(Deserialize)]
struct Claims {
    /// Space-delimited per RFC 6749 §3.3. Absent when the client requested no
    /// scopes, or was granted none of them — both fail closed.
    #[serde(default)]
    scope: Option<String>,
}

fn scope_grants(scope: Option<&str>) -> bool {
    scope.is_some_and(|s| s.split_ascii_whitespace().any(|s| s == REQUIRED_SCOPE))
}

/// Split the credential off an `Authorization` header value.
///
/// RFC 7235 §2.1 makes the scheme case-insensitive. Matching `Bearer` literally
/// turns a conforming `bearer` request into "no credentials sent", and the
/// client answers the bare 401 by re-running OAuth for the identical token.
fn bearer_credential(value: &str) -> Option<&str> {
    let (scheme, credential) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("Bearer")
        .then(|| credential.trim_start())
}

/// Why a request was rejected: 401 tells the client to obtain a token, 403 that
/// re-authenticating will not help until the permission is granted.
enum AuthFailure {
    InvalidToken,
    InsufficientScope,
    /// The token could not be judged either way. Distinct from `InvalidToken`,
    /// whose 401 makes a client throw away a possibly-good token.
    VerificationUnavailable,
}

pub async fn auth_middleware(
    State(state): State<Arc<AuthState>>,
    request: Request,
    next: Next,
) -> Response {
    // CorsLayer intercepts OPTIONS before us, but pass through defensively.
    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }

    // Our own origin, so the challenge can point the client at our metadata.
    // Unresolvable only when nothing is configured *and* the request carries no
    // usable Host — a proxy misconfiguration, not a client error.
    let Some(base_url) = state.public_origin.resolve(request.headers()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let www_auth = www_authenticate(&base_url);

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(bearer_credential)
        .map(str::to_owned);

    let Some(token) = token else {
        return challenge(StatusCode::UNAUTHORIZED, &www_auth);
    };

    match verify_jwt(&state, &token).await {
        Ok(()) => next.run(request).await,
        Err(AuthFailure::InvalidToken) => challenge(
            StatusCode::UNAUTHORIZED,
            &format!(r#"{www_auth}, error="invalid_token""#),
        ),
        // `scope` is already in the base challenge, and RFC 6750 forbids
        // repeating an auth-param, so only the error is appended here.
        Err(AuthFailure::InsufficientScope) => challenge(
            StatusCode::FORBIDDEN,
            &format!(r#"{www_auth}, error="insufficient_scope""#),
        ),
        // No challenge header: the credential is not in question, and one here
        // would invite the client to re-authenticate.
        Err(AuthFailure::VerificationUnavailable) => Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::RETRY_AFTER, "5")
            .body(Body::empty())
            .unwrap(),
    }
}

async fn verify_jwt(state: &AuthState, token: &str) -> Result<(), AuthFailure> {
    let header = decode_header(token).map_err(|_| AuthFailure::InvalidToken)?;
    let kid = header.kid.ok_or(AuthFailure::InvalidToken)?;
    let key = match state.jwks.get_key(&kid).await {
        KeyLookup::Found(key) => key,
        KeyLookup::UnknownKid => return Err(AuthFailure::InvalidToken),
        KeyLookup::Unavailable => return Err(AuthFailure::VerificationUnavailable),
    };
    let data = decode::<Claims>(token, &key, &state.validation).map_err(|e| {
        tracing::debug!("JWT verification failed: {e}");
        AuthFailure::InvalidToken
    })?;

    if !scope_grants(data.claims.scope.as_deref()) {
        return Err(AuthFailure::InsufficientScope);
    }
    Ok(())
}

fn challenge(status: StatusCode, www_auth: &str) -> Response {
    Response::builder()
        .status(status)
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
            public_base_url: None,
        }
    }

    fn test_router() -> Router {
        let state = Arc::new(AuthState::new(&test_config()));
        Router::new()
            .route("/mcp", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, auth_middleware))
    }

    const HOST: &str = "abc.lambda-url.ap-northeast-1.on.aws";

    #[tokio::test]
    async fn no_token_returns_401_with_derived_resource_metadata() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .uri("/mcp")
                    .header(header::HOST, HOST)
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(www.contains("Bearer realm="));
        assert!(
            www.contains(&format!(
                r#"resource_metadata="https://{HOST}/.well-known/oauth-protected-resource""#
            )),
            "resource_metadata should be derived from the request Host: {www}"
        );
        assert!(
            !www.contains("error="),
            "no-token 401 should not have error field"
        );
        assert!(
            www.contains(r#"scope="read:cosense""#),
            "the first 401 must already name the scope to request: {www}"
        );
    }

    #[tokio::test]
    async fn malformed_token_returns_401_with_invalid_token() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .uri("/mcp")
                    .header(header::HOST, HOST)
                    .header(header::AUTHORIZATION, "Bearer not.a.jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(www.contains(r#"error="invalid_token""#));
        assert!(www.contains(&format!(r#"resource_metadata="https://{HOST}/"#)));
    }

    #[tokio::test]
    async fn missing_host_returns_500() {
        let resp = test_router()
            .oneshot(
                http::Request::builder()
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A real RS256 sign-and-verify round trip.
    ///
    /// jsonwebtoken ships no crypto provider by default and panics at the first
    /// real signature verification — at runtime, on the success path only. Every
    /// other test here uses malformed or absent tokens, which fail before any
    /// cryptography runs, so the suite passed while deployed verification
    /// aborted the process.
    #[test]
    fn rs256_verification_round_trip_reaches_the_crypto_provider() {
        use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey};

        let mut rng = rand::thread_rng();
        let private = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("keygen");
        let public = rsa::RsaPublicKey::from(&private);

        let config = test_config();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = serde_json::json!({
            "iss": config.issuer(),
            "aud": config.auth0_audience,
            "sub": "auth0|test-user",
            "exp": now + 600,
            "scope": "openid read:cosense",
        });
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(Algorithm::RS256),
            &claims,
            &jsonwebtoken::EncodingKey::from_rsa_pem(
                private
                    .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap(),
        )
        .unwrap();

        let key = DecodingKey::from_rsa_pem(
            public
                .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let state = AuthState::new(&config);
        let data = decode::<Claims>(&token, &key, &state.validation).expect("verify");
        assert!(scope_grants(data.claims.scope.as_deref()));
    }

    #[test]
    fn scope_check_requires_an_exact_space_delimited_token() {
        assert!(scope_grants(Some(REQUIRED_SCOPE)));
        assert!(scope_grants(Some("openid profile read:cosense")));
        // A token issued without the grant carries no scope (or an empty one)
        // rather than a wrong one, so these are the cases that must fail closed.
        assert!(!scope_grants(None));
        assert!(!scope_grants(Some("")));
        assert!(!scope_grants(Some("read:cosens")));
        assert!(!scope_grants(Some("read:cosense:extra")));
        assert!(!scope_grants(Some("xread:cosense")));
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

        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "OPTIONS should bypass auth"
        );
    }
}
