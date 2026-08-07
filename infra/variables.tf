variable "aws_region" {
  description = "AWS region to deploy into."
  type        = string
  default     = "ap-northeast-1"
}

variable "aws_profile" {
  description = "AWS CLI profile to deploy with. Empty uses the ambient credentials."
  type        = string
  default     = ""
}

variable "aws_account_id" {
  description = "Account this must be applied to. Empty disables the guard."
  type        = string
  default     = ""

  validation {
    condition     = var.aws_account_id == "" || can(regex("^[0-9]{12}$", var.aws_account_id))
    error_message = "aws_account_id must be 12 digits, or empty to disable the guard."
  }
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

variable "reserved_concurrency" {
  description = "Reserved concurrent executions. Bounds cost and blast radius on a public endpoint. -1 removes the reservation; see the note in main.tf before setting it."
  type        = number
  default     = -1
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

variable "public_base_url" {
  description = "Origin this server publishes as its own identity, e.g. https://mcp.example.com. Empty falls back to the request Host. Set it whenever a proxy sits in front."
  type        = string
  default     = ""
}

variable "auth0_domain" {
  description = "Auth0 tenant domain, e.g. your-tenant.us.auth0.com."
  type        = string
}

variable "auth0_audience" {
  description = "Auth0 API identifier validated as the JWT aud claim."
  type        = string
}

variable "discord_webhook_url" {
  description = "Discord webhook for gateway alarms. A credential: whoever holds it can post to the channel. Empty disables alerting entirely."
  type        = string
  default     = ""
  sensitive   = true
}
