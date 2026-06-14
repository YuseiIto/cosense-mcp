output "function_url" {
  description = "Public base URL of the Lambda Function URL (trailing slash)."
  value       = aws_lambda_function_url.this.function_url
}

output "mcp_endpoint" {
  description = "MCP endpoint to register in claude.ai."
  value       = "${trimsuffix(aws_lambda_function_url.this.function_url, "/")}/mcp"
}

output "oauth_protected_resource" {
  description = "RFC 9728 metadata endpoint claude.ai discovers for OAuth."
  value       = "${trimsuffix(aws_lambda_function_url.this.function_url, "/")}/.well-known/oauth-protected-resource"
}
