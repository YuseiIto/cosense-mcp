# cosense-mcp

An MCP server that exposes a [Cosense (Scrapbox)](https://scrapbox.io) project as
readable, linkable context. Pages come back as Markdown together with their 1-hop
and 2-hop related titles, so a model can follow the link graph rather than
retrieve isolated documents.

Read-only. Nothing here writes to Cosense.

## Tools

| Tool | Purpose |
|---|---|
| `list_pages` | Browse a project, most recently updated first. A reasonable entry point. |
| `get_page` | Fetch one page as Markdown, plus its 1-hop and 2-hop related titles. |
| `search_pages` | Full-text search. Supports `AND`, exclusion (`-term`) and `"phrases"`. |

Every tool takes an optional `project`, restricted to the configured allowlist.

## Transports

Two binaries share the same tool implementation.

- **`stdio`** — for a local MCP client. Access is whatever the machine already has.
- **`lambda`** — an HTTP server for AWS Lambda, for remote clients such as
  claude.ai. Access is decided by OAuth 2.1: a request must carry a bearer token
  from the configured authorization server, and that token must include the
  `read:cosense` permission.

The second is not the first with a URL attached. A local server inherits the
trust of the machine running it; a remote one is reachable by anyone who finds
it, so authorization is part of the server rather than the environment.

## Configuration

Both transports read:

| Variable | |
|---|---|
| `COSENSE_PROJECTS` | **Required.** Comma-separated allowlist of project names. |
| `COSENSE_DEFAULT_PROJECT` | Used when a tool call omits `project`. Defaults to the first entry. |
| `COSENSE_SID` | `connect.sid` cookie, for private projects. Omit for public ones. |
| `COSENSE_API_BASE` | Defaults to `https://scrapbox.io/api`. Mainly for testing. |

Sending `COSENSE_SID` over plaintext HTTP to a non-loopback host is refused
rather than downgraded silently.

The `lambda` binary additionally reads `AUTH0_DOMAIN` and `AUTH0_AUDIENCE`.
`AUTH0_AUDIENCE` must equal the canonical URI of the deployed MCP endpoint,
because that is the value clients send as the RFC 8707 `resource` parameter.

## Running locally

```sh
COSENSE_PROJECTS=your-project cargo run --bin stdio
```

Logs go to stderr; stdout is reserved for MCP framing.

## Deploying the HTTP transport

See [`infra/README.md`](infra/README.md). It covers the authorization server
setup, the build, Terraform, and the hostname decision that has to be made
before anything else — the API identifier cannot be changed afterwards.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Lints are configured in `Cargo.toml` so a local run and CI agree.

## A note on trust

Page content reaches the model verbatim. A Cosense project is a place where
instructions can be written, and anything readable in an allowlisted project can
influence a model that reads it. Worth keeping in mind before allowlisting a
project other people can edit.
