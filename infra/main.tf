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

  environment {
    variables = local.env
  }

  depends_on = [aws_iam_role_policy_attachment.logs, aws_cloudwatch_log_group.lambda]
}

# Public Function URL. Requests (including OPTIONS) are forwarded to the app,
# which enforces Auth0 JWT on /mcp and emits CORS itself; AWS-side auth is off.
resource "aws_lambda_function_url" "this" {
  function_name      = aws_lambda_function.this.function_name
  authorization_type = "NONE"
}

resource "aws_lambda_permission" "function_url" {
  statement_id           = "FunctionURLAllowPublicAccess"
  action                 = "lambda:InvokeFunctionUrl"
  function_name          = aws_lambda_function.this.function_name
  principal              = "*"
  function_url_auth_type = "NONE"
}
