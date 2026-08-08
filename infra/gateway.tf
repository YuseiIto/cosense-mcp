# The public face of the HTTP transport: an HTTP API in front of the Lambda.
#
# It exists for what a bare Function URL cannot do: terminate TLS for our own
# hostname (a Function URL rejects a foreign Host and takes no certificate), pass
# WWW-Authenticate through un-renamed, throttle before the function runs, and log
# requests. It does NOT authenticate — the managed JWT authorizer answers 401
# with no resource_metadata pointer and a literal scope="", breaking the OAuth
# discovery flow, and HTTP APIs cannot customize gateway responses.

locals {
  # Single source for the public hostname. The certificate, the custom domain,
  # PUBLIC_BASE_URL and the Auth0 API identifier all have to agree on it.
  public_host = trimprefix(var.public_base_url, "https://")
}

# The validation CNAME lives in a DNS zone this repository does not manage, so
# there is deliberately no aws_acm_certificate_validation resource — it would
# hang waiting on a record another repository has to create. Apply this, copy the
# `acm_validation_records` output into the zone, and confirm the certificate is
# ISSUED before applying the custom domain below. ACM deletes certificates that
# stay unvalidated for 72 hours.
resource "aws_acm_certificate" "public" {
  domain_name       = local.public_host
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
    precondition {
      condition     = var.public_base_url != ""
      error_message = "public_base_url must be set: the gateway's hostname, certificate and audience all derive from it."
    }
  }
}

resource "aws_apigatewayv2_api" "this" {
  name          = var.function_name
  protocol_type = "HTTP"

  # The custom domain is the only intended door, so the {api-id}.execute-api
  # hostname is closed. The d-xxxx regional domain behind the CNAME stays
  # reachable to anyone sending our Host, so this narrows the surface rather
  # than forcing the proxy.
  disable_execute_api_endpoint = true

  # CORS lives here, not in the application: the gateway answers preflights
  # without invoking Lambda (no OPTIONS route needed) and ignores any CORS
  # header the integration returns, so an app-side layer would be dead code.
  # WWW-Authenticate is not CORS-safelisted, and it carries the
  # resource_metadata pointer and scope a browser client needs.
  cors_configuration {
    allow_origins  = ["https://claude.ai"]
    allow_methods  = ["GET", "POST", "DELETE", "OPTIONS"]
    allow_headers  = ["authorization", "content-type", "mcp-protocol-version"]
    expose_headers = ["www-authenticate"]
    max_age        = 300
  }
}

resource "aws_apigatewayv2_integration" "lambda" {
  api_id                 = aws_apigatewayv2_api.this.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.this.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
}

# An allowlist, not a catch-all: anything unlisted never invokes the function.
# GET and DELETE on /mcp are routed despite this server being stateless-only, so
# a client probing them gets the application's 405 rather than a gateway 404
# suggesting the endpoint does not exist.
resource "aws_apigatewayv2_route" "this" {
  for_each = toset([
    "POST /mcp",
    "GET /mcp",
    "DELETE /mcp",
    "GET /health",
    "GET /.well-known/oauth-protected-resource",
    "GET /.well-known/oauth-protected-resource/mcp",
  ])

  api_id    = aws_apigatewayv2_api.this.id
  route_key = each.value
  target    = "integrations/${aws_apigatewayv2_integration.lambda.id}"
}

# Requests the gateway rejects or answers itself appear nowhere else — the
# Lambda log only ever shows requests that reached the function.
resource "aws_cloudwatch_log_group" "api_access" {
  name              = "/aws/apigateway/${var.function_name}"
  retention_in_days = var.log_retention_days
}

# NOTE: the stage MUST be named $default. lambda_http prefixes any other stage
# name onto the request path before the router sees it, 404ing every route.
resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.this.id
  name        = "$default"
  auto_deploy = true

  # The only cost bound when reserved concurrency is unavailable (see main.tf).
  # Rate 2/s is ~10x actual personal use. The residual gap is a sustained flood
  # of slow requests (2/s x 15s timeout = 30 in flight > 10), where Lambda
  # throttling surfaces as gateway 500s — accepted, since the 5xx alarm firing
  # under attack is the correct outcome.
  default_route_settings {
    throttling_rate_limit  = 2
    throttling_burst_limit = 10
  }

  # Excludes the Authorization header and full claims — tokens and identity
  # payloads do not belong in logs. `sub` alone answers "who was it".
  access_log_settings {
    destination_arn = aws_cloudwatch_log_group.api_access.arn
    format = jsonencode({
      requestId               = "$context.requestId"
      ip                      = "$context.identity.sourceIp"
      requestTime             = "$context.requestTime"
      routeKey                = "$context.routeKey"
      status                  = "$context.status"
      responseLatency         = "$context.responseLatency"
      integrationErrorMessage = "$context.integrationErrorMessage"
      sub                     = "$context.authorizer.claims.sub"
    })
  }
}

# Created only after the certificate above is ISSUED — AWS rejects a domain
# name whose certificate is still pending validation.
resource "aws_apigatewayv2_domain_name" "this" {
  domain_name = local.public_host

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.public.arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }
}

resource "aws_apigatewayv2_api_mapping" "this" {
  api_id      = aws_apigatewayv2_api.this.id
  domain_name = aws_apigatewayv2_domain_name.this.id
  stage       = aws_apigatewayv2_stage.default.id
}

resource "aws_lambda_permission" "api_gateway" {
  statement_id  = "AllowApiGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.this.function_name
  principal     = "apigateway.amazonaws.com"
  # This API and stage only — the bare execution ARN prefix would also admit
  # other APIs in the account.
  source_arn = "${aws_apigatewayv2_api.this.execution_arn}/*/*"
}
