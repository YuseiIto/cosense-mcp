locals {
  # gateway.tf already refuses to plan without public_base_url, so there is no
  # fallback to compute here.
  public_base = var.public_base_url
}

output "mcp_endpoint" {
  description = "MCP endpoint to register in the client. Must equal the authorization server's API identifier."
  value       = "${local.public_base}/mcp"
}

output "oauth_protected_resource" {
  description = "RFC 9728 metadata endpoint. Also served at the path-suffixed form, which is what a client derives from the resource URI."
  value       = "${local.public_base}/.well-known/oauth-protected-resource"
}

# Copy these into the DNS zone (unproxied) promptly; see gateway.tf.
output "acm_validation_records" {
  description = "DNS records ACM needs to see before it issues the certificate."
  value = [for o in aws_acm_certificate.public.domain_validation_options : {
    name  = o.resource_record_name
    type  = o.resource_record_type
    value = o.resource_record_value
  }]
}

output "api_gateway_regional_domain" {
  description = "Where the public hostname's CNAME must point once the custom domain exists."
  value       = aws_apigatewayv2_domain_name.this.domain_name_configuration[0].target_domain_name
}
