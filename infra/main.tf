locals {
  # Built by `cargo lambda build --release --arm64 --output-format zip` before apply.
  lambda_zip = "${path.module}/../target/lambda/lambda/bootstrap.zip"

  # COSENSE_DEFAULT_PROJECT is omitted when empty so the binary applies its own
  # "first project" fallback rather than receiving a blank value.
  env = merge(
    {
      COSENSE_PROJECTS = join(",", var.cosense_projects)
      COSENSE_SID      = var.cosense_sid
      AUTH0_DOMAIN     = var.auth0_domain
      AUTH0_AUDIENCE   = var.auth0_audience
    },
    var.cosense_default_project == "" ? {} : {
      COSENSE_DEFAULT_PROJECT = var.cosense_default_project
    },
    # Omitted when empty so the binary falls back to deriving the origin from
    # the request Host, rather than receiving a blank value it would have to
    # treat as meaningful.
    var.public_base_url == "" ? {} : {
      PUBLIC_BASE_URL = var.public_base_url
    },
  )
}

data "aws_iam_policy_document" "assume_role" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "lambda" {
  name               = "${var.function_name}-exec"
  assume_role_policy = data.aws_iam_policy_document.assume_role.json
}

# Grants CloudWatch Logs write access only; the SID lives in env, so no
# Secrets Manager / SSM permissions are needed.
resource "aws_iam_role_policy_attachment" "logs" {
  role       = aws_iam_role.lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_cloudwatch_log_group" "lambda" {
  name              = "/aws/lambda/${var.function_name}"
  retention_in_days = var.log_retention_days
}

resource "aws_lambda_function" "this" {
  function_name = var.function_name
  role          = aws_iam_role.lambda.arn

  filename    = local.lambda_zip
  code_sha256 = filebase64sha256(local.lambda_zip)

  # Custom runtime: the zip contains a single `bootstrap` arm64 binary.
  runtime       = "provided.al2023"
  handler       = "bootstrap"
  architectures = ["arm64"]

  memory_size = var.memory_size
  timeout     = var.timeout

  # The function URL is public and unauthenticated at the AWS layer, so every
  # request is a billed invocation — including the ones the app rejects with 401
  # — and nothing upstream rate-limits them. Concurrency is the bound available:
  # it caps the request rate too, since Lambda allows 10x the reserved
  # concurrency in RPS, and 0 is the documented emergency stop for a function URL.
  #
  # **Reserving requires headroom that a low account quota may not have.** Lambda
  # refuses any reservation that would leave fewer than 10 unreserved executions,
  # so an account whose limit *is* 10 cannot reserve anything above 0 — the
  # attempt fails at apply time with InvalidParameterValueException.
  #
  # Where that is the case the bound has not disappeared: the account-wide limit
  # is itself the ceiling, and it is a tighter one than most reservations would
  # be. But it is a property of the account rather than of this configuration, so
  # **raising that quota silently removes the protection**. Set this variable if
  # the limit is ever increased, or if another function starts sharing it.
  # Ref: https://docs.aws.amazon.com/lambda/latest/dg/urls-configuration.html
  reserved_concurrent_executions = var.reserved_concurrency

  environment {
    variables = local.env
  }

  depends_on = [aws_iam_role_policy_attachment.logs, aws_cloudwatch_log_group.lambda]
}
