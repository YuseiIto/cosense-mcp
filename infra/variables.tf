variable "aws_region" {
  description = "AWS region to deploy into."
  type        = string
  default     = "ap-northeast-1"
}

variable "function_name" {
  description = "Lambda function name; also used to derive the IAM role and log group names."
  type        = string
  default     = "cosense-mcp"
}

variable "memory_size" {
  description = "Lambda memory (MB). Higher memory also raises CPU share, which shortens the Rust cold start."
  type        = number
  default     = 256
}

variable "timeout" {
  description = "Lambda timeout (seconds). Covers a cold-start JWKS fetch plus the upstream Cosense API call."
  type        = number
  default     = 15
}

variable "log_retention_days" {
  description = "CloudWatch log retention. Avoids the default never-expire log group."
  type        = number
  default     = 14
}

variable "cosense_projects" {
  description = "Allowed Cosense project names (joined into COSENSE_PROJECTS)."
  type        = list(string)

  validation {
    condition     = length(var.cosense_projects) > 0
    error_message = "Provide at least one Cosense project."
  }
}

variable "cosense_default_project" {
  description = "Default project when a tool call omits one. Empty string falls back to the first of cosense_projects."
  type        = string
  default     = ""
}

variable "cosense_sid" {
  description = "Cosense connect.sid cookie for private projects. Empty disables private access. Stored in Lambda env and (encrypted) tfstate."
  type        = string
  default     = ""
  sensitive   = true
}

variable "auth0_domain" {
  description = "Auth0 tenant domain, e.g. your-tenant.us.auth0.com."
  type        = string
}

variable "auth0_audience" {
  description = "Auth0 API identifier validated as the JWT aud claim."
  type        = string
}
