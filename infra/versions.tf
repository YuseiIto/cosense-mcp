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

  # Partial config: the bucket, key, region, KMS key and state-access role are
  # supplied at init time via `terraform init -backend-config=backend.hcl`
  # (see backend.hcl.example). The state bucket must exist before the first init.
  #
  # This stays a partial config on purpose. This repository is public, and the
  # values identify an AWS account, a state bucket and a CMK. Inlining them the
  # way a private infrastructure repository would is not an option here.
  backend "s3" {
    use_lockfile = true
    encrypt      = true
  }
}

provider "aws" {
  region = var.aws_region

  # Empty means "use whatever the environment already provides", which is the
  # sensible default for anyone deploying their own copy.
  profile = var.aws_profile == "" ? null : var.aws_profile

  # Refuse to apply against an account other than the intended one. Credentials
  # for several accounts are commonly available at once, and a Lambda created in
  # the wrong one is easy to miss: it costs nothing, breaks nothing, and simply
  # sits there. An empty list disables the check.
  allowed_account_ids = var.aws_account_id == "" ? [] : [var.aws_account_id]
}
