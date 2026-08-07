# Alarms on the gateway's error metrics, delivered to Discord without running
# any code of ours: CloudWatch alarm state changes flow into EventBridge on
# their own, a rule matches them, and an API destination POSTs the webhook.
#
# The two alarms are deliberately asymmetric. 4xx measures OTHER PEOPLE's
# behaviour, and a baseline of it is normal (a client's first 401 is part of
# the OAuth flow), so only a flood is anomalous. 5xx measures OUR failure and
# has a baseline of zero, so a single one is worth hearing about. Tuning them
# to the same threshold would misread one of the two.
#
# Everything here is created only when a webhook URL is configured, so a public
# clone of this repository deploys cleanly without it.

locals {
  # The URL is sensitive; whether one was provided is not, and for_each/count
  # refuse sensitive values outright.
  alerting_enabled = nonsensitive(var.discord_webhook_url != "")

  gateway_alarms = {
    "4xx" = {
      # High on purpose: legitimate use produces a trickle of 4xx, and an
      # attack produces thousands. 300/5min cannot be reached by one user.
      threshold   = 300
      description = "4xx flood at the gateway - scan, abuse, or a broken client"
    }
    "5xx" = {
      # A personal service has a 5xx baseline of exactly zero. One is news.
      threshold   = 1
      description = "5xx at the gateway - the function is failing or throttled"
    }
  }
}

# NOTE: HTTP APIs publish `4xx`/`5xx` (dimension ApiId), unlike REST APIs'
# `4XXError`/`5XXError`. The names are easy to cross-wire and a wrong name
# alarms on a metric that never has data — which treat_missing_data would then
# silently mask. Verify with `aws cloudwatch list-metrics` after first traffic.
resource "aws_cloudwatch_metric_alarm" "gateway" {
  for_each = local.alerting_enabled ? local.gateway_alarms : {}

  alarm_name          = "${var.function_name}-gateway-${each.key}"
  alarm_description   = each.value.description
  namespace           = "AWS/ApiGateway"
  metric_name         = each.key
  dimensions          = { ApiId = aws_apigatewayv2_api.this.id }
  statistic           = "Sum"
  period              = 300
  evaluation_periods  = 1
  threshold           = each.value.threshold
  comparison_operator = "GreaterThanOrEqualToThreshold"
  # No requests means no datapoints, and that is the healthy idle state.
  treat_missing_data = "notBreaching"
}

# EventBridge insists a connection carries SOME auth; Discord webhooks carry
# theirs in the URL and ignore unknown headers, so this one is a no-op header.
# The Content-Type is set here explicitly rather than trusted to defaults —
# Discord rejects bodies without it.
resource "aws_cloudwatch_event_connection" "discord" {
  count = local.alerting_enabled ? 1 : 0

  name               = "${var.function_name}-discord"
  authorization_type = "API_KEY"

  auth_parameters {
    api_key {
      key   = "x-eventbridge-auth"
      value = "unused"
    }
    invocation_http_parameters {
      header {
        key   = "Content-Type"
        value = "application/json"
      }
    }
  }
}

# The webhook URL is a credential: anyone holding it can post to the channel.
# It reaches here from a gitignored tfvars file and ends up in state and in
# this resource's attributes, both of which sit behind the state bucket's
# encryption — same treatment as COSENSE_SID.
resource "aws_cloudwatch_event_api_destination" "discord" {
  count = local.alerting_enabled ? 1 : 0

  name                             = "${var.function_name}-discord"
  connection_arn                   = aws_cloudwatch_event_connection.discord[0].arn
  invocation_endpoint              = var.discord_webhook_url
  http_method                      = "POST"
  invocation_rate_limit_per_second = 1
}

resource "aws_cloudwatch_event_rule" "alarm_to_discord" {
  count = local.alerting_enabled ? 1 : 0

  name = "${var.function_name}-alarms"
  # ALARM transitions only. OK/recovery notifications are noise at this scale,
  # and INSUFFICIENT_DATA is the normal idle state (see treat_missing_data).
  event_pattern = jsonencode({
    source      = ["aws.cloudwatch"]
    detail-type = ["CloudWatch Alarm State Change"]
    detail = {
      alarmName = [for k in keys(local.gateway_alarms) : "${var.function_name}-gateway-${k}"]
      state     = { value = ["ALARM"] }
    }
  })
}

resource "aws_iam_role" "alarm_to_discord" {
  count = local.alerting_enabled ? 1 : 0

  name = "${var.function_name}-alerts"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Action    = "sts:AssumeRole"
      Principal = { Service = "events.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role_policy" "alarm_to_discord" {
  count = local.alerting_enabled ? 1 : 0

  name = "invoke-api-destination"
  role = aws_iam_role.alarm_to_discord[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = "events:InvokeApiDestination"
      Resource = aws_cloudwatch_event_api_destination.discord[0].arn
    }]
  })
}

resource "aws_cloudwatch_event_target" "discord" {
  count = local.alerting_enabled ? 1 : 0

  rule     = aws_cloudwatch_event_rule.alarm_to_discord[0].name
  arn      = aws_cloudwatch_event_api_destination.discord[0].arn
  role_arn = aws_iam_role.alarm_to_discord[0].arn

  # Values are substituted verbatim; a reason containing a double quote would
  # break the JSON. CloudWatch's generated reasons don't contain them, and a
  # malformed one costs a single lost notification, not a stuck rule.
  input_transformer {
    input_paths = {
      alarm  = "$.detail.alarmName"
      reason = "$.detail.state.reason"
    }
    input_template = "{\"content\": \"🚨 <alarm>: <reason>\"}"
  }
}
