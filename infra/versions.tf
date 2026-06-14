terraform {
  # >= 1.11 for native S3 state locking (use_lockfile), which removes the need
  # for a DynamoDB lock table.
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }

  # Partial config: bucket/key/region are supplied at init time via
  # `terraform init -backend-config=backend.hcl` (see backend.hcl.example).
  # The state bucket must exist before the first init.
  backend "s3" {
    use_lockfile = true
    encrypt      = true
  }
}

provider "aws" {
  region = var.aws_region
}
