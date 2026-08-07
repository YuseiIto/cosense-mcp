locals {
  # What clients actually address. The function URL is only that when nothing is
  # in front of it; reporting it once a proxy exists sends people to register the
  # origin instead of the endpoint, which then disagrees with the audience the
  # authorization server issues tokens for.
  public_base = var.public_base_url == "" ? trimsuffix(aws_lambda_function_url.this.function_url, "/") : var.public_base_url
}

output "function_url" {
  description = "The Lambda Function URL itself — the origin, not necessarily what clients address."
  value       = aws_lambda_function_url.this.function_url
}

output "mcp_endpoint" {
  description = "MCP endpoint to register in the client. Must equal the authorization server's API identifier."
  value       = "${local.public_base}/mcp"
}

output "oauth_protected_resource" {
  description = "RFC 9728 metadata endpoint. Also served at the path-suffixed form, which is what a client derives from the resource URI."
  value       = "${local.public_base}/.well-known/oauth-protected-resource"
}
