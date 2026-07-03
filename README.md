# cosense-mcp

An MCP server for [Cosense](https://scrapbox.io/) (formerly Scrapbox). Exposes
read-only tools for exploring project content: list pages, fetch a page as
Markdown with its 1-hop / 2-hop link graph, and full-text search.

## Tools

| Tool           | Description                                                        |
| -------------- | ------------------------------------------------------------------ |
| `list_pages`   | List pages in a project (sorted by recent update by default)       |
| `get_page`     | Get a page rendered as Markdown plus related page titles           |
| `search_pages` | Full-text search within a project                                  |

## Configuration

All configuration is via environment variables.

| Variable                  | Required | Description                                                                 |
| ------------------------- | -------- | --------------------------------------------------------------------------- |
| `COSENSE_PROJECTS`        | yes      | Comma-separated allowlist of project names, e.g. `my-notes,team-wiki`       |
| `COSENSE_DEFAULT_PROJECT` | no       | Default project when a tool call omits `project` (defaults to the first)    |
| `COSENSE_SID`             | no       | `connect.sid` cookie value, needed to read private projects                 |
| `COSENSE_API_BASE`        | no       | API base URL (defaults to `https://scrapbox.io/api`)                        |
| `COSENSE_MCP_TRANSPORT`   | no       | `stdio` (default) or `http`                                                 |
| `COSENSE_MCP_BIND`        | no       | HTTP mode: bind address (defaults to `127.0.0.1`)                           |
| `PORT`                    | no       | HTTP mode: listen port (defaults to `8080`)                                 |

## Local usage (stdio)

```sh
cargo build --release
```

Register with Claude Code:

```sh
claude mcp add cosense \
  --env COSENSE_PROJECTS=my-notes \
  --env COSENSE_SID=... \
  -- /path/to/target/release/cosense-mcp
```

Or in `claude_desktop_config.json` / any MCP client config:

```json
{
  "mcpServers": {
    "cosense": {
      "command": "/path/to/target/release/cosense-mcp",
      "env": {
        "COSENSE_PROJECTS": "my-notes",
        "COSENSE_SID": "..."
      }
    }
  }
}
```

## Remote deployment (OAuth via Cloudflare Access)

In HTTP mode the server speaks plain [Streamable HTTP](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports#streamable-http)
at `/mcp` and performs **no authentication itself**. OAuth — which MCP clients
such as claude.ai custom connectors require — is delegated to
[Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/access-controls/ai-controls/secure-mcp-servers/):
an Access *MCP server* application with **Managed OAuth** enabled sits in front
of the origin, runs the OAuth flow against your identity provider, and only
forwards requests from identities that pass your Access policies.

```
MCP client (claude.ai, ...)
   │  OAuth 2.0 (handled entirely by Cloudflare Access)
   ▼
Cloudflare edge ── Access policy check
   │  Cloudflare Tunnel
   ▼
cloudflared ──► cosense-mcp (http mode, 127.0.0.1:8080) ──► Cosense API
```

### Prerequisites

- A domain on Cloudflare (an active zone in your account).
- A [Zero Trust organization](https://developers.cloudflare.com/cloudflare-one/setup/)
  (free plan is fine) with an identity provider configured — the built-in
  [One-time PIN](https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/one-time-pin/)
  works if you don't want to wire up Google/GitHub/etc.

### 1. Run the server in HTTP mode

On the host that will run the tunnel connector:

```sh
COSENSE_MCP_TRANSPORT=http \
PORT=8080 \
COSENSE_PROJECTS=my-notes \
COSENSE_SID=... \
cosense-mcp
```

Keep the default bind address (`127.0.0.1`) so the listener is only reachable
by the local `cloudflared` process, never directly from the network. Only set
`COSENSE_MCP_BIND=0.0.0.0` when the tunnel connector runs in a separate
container, and make sure the port is not exposed publicly.

### 2. Connect a Cloudflare Tunnel

1. In [Zero Trust](https://one.dash.cloudflare.com/), go to
   **Networks** → **Tunnels** → **Create a tunnel** → **Cloudflared**, and
   install the connector on the same host.
2. Add a **Public hostname**: e.g. `cosense-mcp.example.com` →
   service `http://localhost:8080`.

`https://cosense-mcp.example.com/healthz` should now return `ok`.
At this point `/mcp` is still unauthenticated — finish step 3 before putting
anything sensitive behind it (or create the Access application first and add
the hostname to it, then the tunnel).

### 3. Create the Access MCP server application

1. In Zero Trust, go to **Access controls** → **AI controls** →
   **MCP servers** tab → **Add an MCP server**.
2. Name it, and set **HTTP URL** to `https://cosense-mcp.example.com/mcp`.
3. Configure an [Access policy](https://developers.cloudflare.com/cloudflare-one/access-controls/policies/)
   for who may use it (e.g. *Allow* → *Emails* → your address), pick your
   identity provider, and select **Save and connect server**.
4. Go to **Access controls** → **Applications**, open the application that was
   generated for the MCP server, and:
   - under **Basic information**, add `cosense-mcp.example.com` as a public
     hostname (so unauthenticated traffic to the origin is blocked at the edge);
   - under **Advanced settings**, turn on **Managed OAuth** (this is what lets
     MCP clients authenticate with a standard OAuth 2.0 flow instead of a
     browser redirect).

### 4. Connect from claude.ai

In claude.ai: **Settings** → **Connectors** → **Add custom connector**, with
URL `https://cosense-mcp.example.com/mcp`. The OAuth consent screen is served
by Cloudflare Access; log in with an identity allowed by your policy. Other
remote-MCP clients ([MCP inspector](https://github.com/modelcontextprotocol/inspector),
Workers AI Playground, etc.) work the same way.

### Security notes

- The origin trusts whatever reaches it, so it must be reachable *only*
  through the tunnel: loopback bind, no public port, and the hostname covered
  by the Access application.
- Cloudflare recommends that origins behind Managed OAuth additionally
  validate the `Cf-Access-Jwt-Assertion` header (JWT signature against your
  team's public keys, plus issuer/AUD claims) as defense in depth. This server
  does not do that yet; treat it as a hardening TODO before multi-tenant use.
- `COSENSE_SID` is your personal Cosense session: every identity allowed by
  the Access policy can read whatever it can read (within
  `COSENSE_PROJECTS`). Scope the allowlist accordingly.
