# Deploying cosense-mcp to AWS Lambda

The HTTP transport runs as a single arm64 binary behind a Lambda Function URL.
Access is decided in two places: the authorization server issues a token, and
this server requires that token to carry the `read:cosense` permission. State
lives in S3 with native locking (no DynamoDB table).

Anything under `*.example` is a template. The real files (`backend.hcl`,
`terraform.tfvars`) are gitignored — this repository is public, and those files
name an account, a bucket, a tenant and a hostname.

## Prerequisites

- `cargo-lambda` (bundles Zig) and the `aarch64-unknown-linux-gnu` target
- Terraform >= 1.11
- AWS credentials able to manage Lambda, IAM and CloudWatch Logs
- An existing S3 bucket for Terraform state
- An OAuth 2.0 authorization server that can issue RS256 JWTs with an RBAC
  permissions claim. The instructions below use Auth0.

## 0. Settle the public hostname first

The API identifier on the authorization server must equal the canonical MCP
endpoint URI, because that is the value MCP clients send as the RFC 8707
`resource` parameter. **On Auth0 that identifier cannot be changed after
creation**, so choosing it later means recreating the API, its permission, its
role and the connector registration.

A Lambda Function URL is a poor identifier: it is tied to the lifetime of the
URL configuration, and recreating it hands you a new hostname. Put a hostname
you control in front of it — a proxy or CDN — and use that.

The rest of this document writes it as `https://mcp.example.com`.

## 1. Configure the authorization server

Create an **API** whose identifier is `https://mcp.example.com/mcp`, with:

- RBAC policy enforcement enabled
- an access token dialect that includes the `permissions` claim
- offline access enabled, or clients never receive a refresh token
- a permission named `read:cosense`

Create a **role** holding that permission and assign it to the accounts that
should have access. This assignment is the authorization boundary: anyone able
to sign in gets a valid token, and this server rejects the ones that arrive
without the permission. Widening who may sign in — enabling signups on a
connection, say — does not widen who can reach the tools.

Register the MCP client. Claude publishes its metadata at
`https://claude.ai/oauth/mcp-oauth-client-metadata`; read it rather than
transcribing from here, since it is the authoritative source for the callback
URL. At time of writing it declares:

- callback `https://claude.ai/api/mcp/auth_callback`
- grant types `authorization_code` and `refresh_token`
- `token_endpoint_auth_method: "none"` — a public client, so **no client secret
  is involved**

Pre-registering the client this way keeps it first-party. Auth0's Client ID
Metadata Document registration is the documented alternative, but a CIMD client
is treated as third-party and can then only use domain-level connections, which
opens that connection to the whole tenant.

**Do not set a default audience on the tenant.** Auth0's resource parameter
compatibility profile only falls back to `resource` when `audience` is absent; a
default audience is equivalent to sending `audience` on every request, and the
token would be minted for the wrong API.

## 2. Build the binary

Run from the repository root. Terraform reads the zip at plan time, so build
first — a clean checkout cannot plan.

```sh
cargo lambda build --release --arm64 --bin lambda --output-format zip
# -> target/lambda/lambda/bootstrap.zip
```

## 3. Configure

```sh
cd infra
cp backend.hcl.example backend.hcl           # state bucket
cp terraform.tfvars.example terraform.tfvars # projects, SID, Auth0, account guard
```

Set `auth0_audience` to the same identifier used in step 1.

Set `aws_account_id`. Credentials for several accounts are often available at
once, and a function created in the wrong one costs nothing and breaks nothing,
so nothing tells you about it.

`cosense_sid` is written to the Lambda environment and to state. State is
encrypted at rest by the bucket; treat the bucket as the thing protecting it,
and keep `terraform.tfvars` out of version control.

## 4. Apply

```sh
terraform init -backend-config=backend.hcl
terraform plan -out=tfplan
terraform apply tfplan
rm -f tfplan
```

Plan files contain secrets in plaintext. Write them outside the repository or
delete them straight after.

Outputs include `mcp_endpoint` and `oauth_protected_resource`.

## 5. Point the hostname at the function

Put `mcp.example.com` in front of the Function URL. A plain CNAME will not work:
the function URL serves a certificate for `*.lambda-url.<region>.on.aws`, and
`AWS::Lambda::Url` exposes no way to attach your own. Terminate TLS at a proxy or
CDN and forward to the function URL as origin.

Two things to verify before going further:

- **The `Host` header must survive.** This server derives the metadata it
  publishes from it, so a rewritten `Host` makes the advertised `resource`
  disagree with the URL the client used, and authorization fails.
- **The edge must not block the client.** Bot-management and WAF rules in front
  of an MCP endpoint are a common cause of connection failures; exempt the MCP
  and OAuth paths, or allowlist the client's published egress range.

```sh
curl -sS -o /dev/null -w '%{http_code}\n' https://mcp.example.com/health
curl -sS https://mcp.example.com/.well-known/oauth-protected-resource/mcp | jq .
```

The `resource` field must read `https://mcp.example.com/mcp` exactly.

CloudFront origin access control is not usable here. It signs origin requests
with SigV4 in the `Authorization` header, which is the same header MCP uses for
the bearer token; the setting that preserves the client's header leaves the
request unsigned, and an `AWS_IAM` function URL then rejects it.

## 6. Register the connector

Add a custom connector for `https://mcp.example.com/mcp`. Under advanced
settings, supply the client ID from step 1 and leave the secret empty.

Sign in with an account that holds the role. An account without it completes
sign-in and then receives `403` with `insufficient_scope` — which is the
intended behaviour, and worth confirming once with a second account.

## Redeploying after code changes

Rebuild (step 2) then `terraform apply`. `code_sha256` changes, so Terraform
updates the function.

## Notes

- **The function URL is public.** Authorization happens inside the application,
  so every request is a billed invocation, including rejected ones.
  `reserved_concurrency` bounds that: it caps concurrency and, at ten times its
  value, the request rate. Set it to `0` to shut the endpoint off.
- **Terraform reads the zip at plan time.** Build before planning, in CI too.
- For this deployment the authorization server is declared in a separate,
  private infrastructure repository. Changing who may use this server is a
  change there, not here.
