# Deploying cosense-mcp to AWS Lambda

The HTTP transport runs as a single arm64 binary on a Lambda Function URL,
protected by in-app Auth0 JWT validation. State lives in S3 with native
locking (no DynamoDB table).

## Prerequisites

- `cargo-lambda` (bundles Zig) and the `aarch64-unknown-linux-gnu` target
- Terraform >= 1.11
- AWS credentials with permission to manage Lambda, IAM, and CloudWatch Logs
- An existing S3 bucket for Terraform state

## 1. Build the binary

Run from the repo root. Terraform reads the zip at plan time, so build first:

```sh
cargo lambda build --release --arm64 --bin lambda --output-format zip
# -> target/lambda/lambda/bootstrap.zip
```

## 2. Configure

```sh
cd infra
cp backend.hcl.example backend.hcl          # set your state bucket
cp terraform.tfvars.example terraform.tfvars # set projects, SID, Auth0
```

`cosense_sid` is sensitive: it is written to the Lambda environment and to
(encrypted) state. Keep `terraform.tfvars` and the state bucket private.

## 3. Apply

```sh
terraform init -backend-config=backend.hcl
terraform apply
```

Outputs include `mcp_endpoint` (register this in claude.ai) and
`oauth_protected_resource`.

## 4. Auth0 (one-time, manual)

claude.ai authenticates via CIMD — Anthropic hosts the client metadata
document, so no app-side client setup is needed. In the Auth0 dashboard:

1. Settings > Advanced: enable **Client ID Metadata Document Registration**.
2. Applications > Create Application > **Import from URL**:
   `https://claude.ai/oauth/mcp-oauth-client-metadata`.
3. Create an API whose identifier matches `auth0_audience`; set it as the
   default audience.
4. Promote the login connection to the domain level.

## Redeploying after code changes

Rebuild (step 1) then `terraform apply` — `code_sha256` changes, so Terraform
updates the function.
