# rust-tenant-provisioning-poc-using-cloudflare

A proof-of-concept for automated multi-tenant SaaS infrastructure. When a
customer signs up, they get their own subdomain, SSL certificate, and login
page — provisioned automatically in under 10 seconds. Customers can also
connect a custom domain with automated SSL issuance.

Built with Rust, Postgres, Nginx, and Cloudflare. Runs entirely on localhost
during development. No cloud server required to test the full flow end-to-end.

---

## What It Does

```
Customer signs up at app.yoursaas.com
            ↓
acme.yoursaas.com is provisioned automatically
SSL certificate issued, DNS configured
            ↓
Customer logs in at acme.yoursaas.com
            ↓
Customer connects app.acme.com (optional)
CNAME verified, SSL issued via Cloudflare for SaaS
            ↓
app.acme.com routes to the same app, same tenant
```

---

## Stack

| Layer | Technology | Notes |
|---|---|---|
| App | Rust + axum | Single binary, all tenants |
| Database | Postgres 16 | Shared schema, LISTEN/NOTIFY for pub/sub |
| Cache | Postgres unlogged table + DashMap | No Redis needed |
| Reverse proxy | Nginx | Preserves Host header |
| Tunnel | Cloudflare Tunnel | Free SSL, no open ports, no EC2 |
| Custom domain SSL | Cloudflare for SaaS | Automated cert issuance |
| Auth | JWT | Stateless, scoped to tenant |

---

## How Tenant Isolation Works

One Rust binary serves all tenants. The tenant is resolved from the `Host`
header on every request:

```
GET /dashboard HTTP/1.1
Host: acme.yoursaas.com
          ↓
Middleware extracts "acme"
          ↓
Looks up tenant_id (DashMap cache → Postgres fallback)
          ↓
Injects TenantContext into request
          ↓
All DB queries scoped: WHERE tenant_id = $1
```

No separate deployments. No separate databases. No container orchestration.

---

## Architecture

```
Browser
  │
  ▼
Cloudflare Edge (SSL terminated)
  │
  ▼
Cloudflare Tunnel (outbound WSS from your machine)
  │
  ▼
Nginx :80 (Host header preserved)
  │
  ▼
Rust axum :8080
  ├── Tenant middleware (Host → tenant_id)
  ├── Auth middleware (JWT validation)
  ├── Handlers (business logic)
  └── Postgres :5432
        ├── tenants
        ├── users
        ├── custom_domains
        ├── kv_cache (unlogged)
        ├── acme_challenges
        ├── sessions
        └── rate_limits (unlogged)
```

---

## Project Structure

```
.
├── Makefile
├── docker-compose.yml
├── .env.example
├── migrations/
│   ├── 001_tenants.sql
│   ├── 002_users.sql
│   ├── 003_kv_cache.sql
│   ├── 004_custom_domains.sql
│   ├── 005_acme_challenges.sql
│   ├── 006_sessions.sql
│   └── 007_rate_limits.sql
├── src/
│   ├── main.rs
│   ├── config.rs          # all env vars in one place
│   ├── state.rs           # shared app state (db pool, caches)
│   ├── error.rs           # AppError → HTTP response
│   ├── db/mod.rs          # raw DB queries
│   ├── handlers/
│   │   ├── auth.rs        # register, login, me
│   │   ├── health.rs      # health check, static file serving
│   │   ├── domains.rs     # custom domain management
│   │   └── provisioning.rs # SSE status stream
│   ├── middleware/
│   │   ├── tenant.rs      # Host header → TenantContext
│   │   └── auth.rs        # JWT validation
│   └── services/
│       ├── jwt.rs         # issue + verify tokens
│       ├── provisioning.rs # 6-step tenant setup pipeline
│       ├── custom_domain.rs # Cloudflare for SaaS integration
│       └── rate_limit.rs  # Postgres-backed rate limiting
├── static/
│   ├── index.html         # signup page (app.yoursaas.com only)
│   ├── status.html        # provisioning waiting room
│   ├── login.html         # tenant login
│   └── dashboard.html     # tenant dashboard + domain management
├── nginx/
│   └── nginx.conf
├── cloudflared/
│   └── config.yml.template
└── docs/
    ├── TUTORIAL.md
    └── CUSTOM_DOMAIN_TROUBLESHOOTING.md
```

---

## Prerequisites

```bash
# required
brew install cloudflared sqlx-cli

# verify
cloudflared --version   # 2024.x or later
sqlx --version          # 0.7.x or later
cargo --version         # 1.75 or later
docker --version        # 24.x or later
```

You also need:
- A domain managed by Cloudflare (e.g. `yoursaas.com`)
- A Cloudflare account (free tier works)

---

## Quick Start

### 1. Clone and configure

```bash
git clone https://github.com/yourname/rust-tenant-provisioning-cloudflare
cd rust-tenant-provisioning-cloudflare
make setup
```

`make setup` copies `.env.example` → `.env`, starts Docker, and runs migrations.

Edit `.env` with your values:

```bash
nano .env
```

Minimum required for local testing with mock Cloudflare:

```bash
DATABASE_URL=postgresql://db_username:secret@localhost:5432/db_name
JWT_SECRET=any-long-random-string-here
BASE_DOMAIN=yoursaas.com
MOCK_CLOUDFLARE=true
```

### 2. Start the app

```bash
# terminal 1
make dev
```

### 3. Start the tunnel (real subdomains + SSL)

First, set up the tunnel once:

```bash
cloudflared tunnel login                           # authenticate
cloudflared tunnel create tunnel-name              # create tunnel
cloudflared tunnel route dns tunnel-name "*.yoursaas.com"
cloudflared tunnel route dns tunnel-name "app.yoursaas.com"
```

Add to `.env`:
```bash
CLOUDFLARE_TUNNEL_NAME=tunnel-name
CLOUDFLARE_TUNNEL_ID=<id from tunnel create output>
CLOUDFLARE_API_TOKEN=<token with Zone DNS Edit>
CLOUDFLARE_ZONE_ID=<your zone id>
```

Then start:
```bash
# terminal 2
make tunnel
```

### 4. Test the full flow

```bash
# register a tenant
curl -X POST https://app.yoursaas.com/api/register \
  -H "Content-Type: application/json" \
  -d '{
    "company": "Acme Corp",
    "subdomain": "acme",
    "email": "admin@acme.com",
    "password": "secret123"
  }'

# open the browser
open https://app.yoursaas.com
```

Or open `https://app.yoursaas.com` in a browser, fill in the signup form,
and watch the provisioning status page.

---

## Make Commands

```bash
make setup        # first time: copy .env, start docker, run migrations
make dev          # start docker + cargo run
make tunnel       # generate cloudflared config and start tunnel
make stop         # stop docker containers
make reset        # wipe database and re-run migrations
make migrate      # run pending migrations
make logs         # tail docker logs
make db           # open psql
make check        # cargo check
make build        # cargo build --release
```

---

## API Reference

### Public (no auth required)

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Health check, confirms DB reachable |
| `POST` | `/api/register` | Create tenant + start provisioning |
| `POST` | `/api/login` | Authenticate, returns JWT |
| `GET` | `/api/provisioning/stream/:tenant_id` | SSE stream for provisioning status |
| `GET` | `/api/domains/stream/:domain_id` | SSE stream for custom domain status |
| `GET` | `/.well-known/acme-challenge/:token` | ACME HTTP challenge (SSL validation) |
| `GET` | `/.well-known/cf-custom-hostname-challenge/:token` | CF ownership challenge |

### Protected (requires `Authorization: Bearer <token>`)

All protected routes also require a valid tenant subdomain or custom domain
in the `Host` header.

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/me` | Current user + session info |
| `POST` | `/api/domains` | Add custom domain |
| `GET` | `/api/domains` | List tenant's custom domains |
| `GET` | `/api/domains/:id/status` | Live domain status (hits CF API) |
| `POST` | `/api/domains/:id/verify` | Trigger CF recheck |
| `DELETE` | `/api/domains/:id` | Remove custom domain |

### Register payload

```json
{
  "company": "Acme Corp",
  "subdomain": "acme",
  "email": "admin@acme.com",
  "password": "secret123"
}
```

### Login payload

```json
{
  "email": "admin@acme.com",
  "password": "secret123"
}
```

### Add domain payload

```json
{
  "domain": "app.acme.com"
}
```

---

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | ✓ | — | Postgres connection string |
| `JWT_SECRET` | ✓ | — | Secret for signing JWTs |
| `BASE_DOMAIN` | — | `yoursaas.com` | Your root domain |
| `APP_SUBDOMAIN` | — | `app` | Subdomain for signup page |
| `APP_HOST` | — | `0.0.0.0` | Bind address |
| `APP_PORT` | — | `8080` | Bind port |
| `POSTGRES_DB` | — | — | Postgres database name (Docker) |
| `POSTGRES_USER` | — | — | Postgres user (Docker) |
| `POSTGRES_PASSWORD` | — | — | Postgres password (Docker) |
| `NGINX_PORT` | — | `80` | Nginx port (Docker) |
| `CLOUDFLARE_API_TOKEN` | — | — | CF token (Zone DNS Edit) |
| `CLOUDFLARE_ZONE_ID` | — | — | CF zone ID for your domain |
| `CLOUDFLARE_TUNNEL_ID` | — | — | Tunnel UUID |
| `CLOUDFLARE_TUNNEL_NAME` | — | — | Tunnel name (e.g. garageos-dev) |
| `MOCK_CLOUDFLARE` | — | `true` | Skip real CF API calls |
| `DB_MAX_CONNECTIONS` | — | `5` | Postgres pool size |
| `JWT_EXPIRY_DAYS` | — | `7` | JWT lifetime in days |
| `RATE_LIMIT_REGISTER_MAX` | — | `5` | Max signups per IP per window |
| `RATE_LIMIT_REGISTER_WINDOW_SECS` | — | `3600` | Register rate limit window |
| `RATE_LIMIT_LOGIN_MAX` | — | `10` | Max login attempts per IP |
| `RATE_LIMIT_LOGIN_WINDOW_SECS` | — | `300` | Login rate limit window |
| `PROVISION_CNAME_MAX_ATTEMPTS` | — | `40` | CNAME poll attempts |
| `PROVISION_CNAME_POLL_SECS` | — | `30` | Seconds between CNAME polls |
| `PROVISION_SSL_MAX_ATTEMPTS` | — | `40` | SSL status poll attempts |
| `PROVISION_SSL_POLL_SECS` | — | `15` | Seconds between SSL polls |
| `PROVISION_TXT_MAX_ATTEMPTS` | — | `40` | TXT record poll attempts |
| `PROVISION_TXT_POLL_SECS` | — | `30` | Seconds between TXT polls |
| `RUST_LOG` | — | `info` | Log level |

---

## Custom Domain Setup

When a tenant adds a custom domain, the system walks through this flow
automatically:

```
1. Create Cloudflare custom hostname (CF for SaaS API)
2. Show user two DNS records to add:
     CNAME: subdomain → <tunnel>.cfargotunnel.com  [grey cloud]
     TXT:   _cf-custom-hostname.subdomain → <uuid>
3. Poll until CNAME resolves correctly (every 30s)
4. Poll until TXT record propagates (every 30s)
5. Trigger CF ownership verification
6. Poll until SSL certificate is issued (every 15s)
7. Warm in-process cache
8. Mark domain active, notify browser via SSE
```

The dashboard shows live status for each step with a built-in debug panel
showing SSL status, certificate authority, ACME challenges, and verification
errors directly from the Cloudflare API.

### Cloudflare for SaaS — One-Time Setup

Before custom domains work, enable CF for SaaS on your zone:

```
Cloudflare Dashboard
  → yoursaas.com
  → SSL/TLS
  → Custom Hostnames
  → Enable Cloudflare for SaaS
  → Fallback origin: app.yoursaas.com
```

Free tier includes 100 custom hostnames.

### Known Limitation

Custom domains hosted on Cloudflare (same platform) cannot use CF for SaaS
SSL issuance — this is a Cloudflare platform limitation. Two paths exist:

- **Non-Cloudflare domains** — full automated SSL via CF for SaaS
- **Cloudflare domains** — SSL comes from the customer's existing wildcard
  cert automatically, no CF for SaaS needed

See `docs/CUSTOM_DOMAIN_TROUBLESHOOTING.md` for the complete guide.

---

## Provisioning Pipeline

Six steps run as a Tokio background task after signup:

```
Step 1  Insert tenant row (status: pending)
Step 2  Cloudflare DNS → create CNAME subdomain.yoursaas.com → tunnel
Step 3  Warm kv_cache → subdomain:acme = tenant_id (TTL 24h)
Step 4  Mark tenant active (status: active)
Step 5  pg_notify → SSE handler receives event
Step 6  Browser redirects to acme.yoursaas.com/login.html
```

With `MOCK_CLOUDFLARE=true`, steps 2 simulates a 2-second network delay.
The HTTP response returns in milliseconds — provisioning runs entirely in
the background.

---

## Moving to Production

The only changes needed to go from localhost to EC2:

**1. Change the wildcard DNS record**
```
# local
*.yoursaas.com  CNAME  <tunnel>.cfargotunnel.com  [proxied]

# production
*.yoursaas.com  A  <ec2-ip>  [proxied]
```

**2. Remove cloudflared**
Not needed on EC2 — traffic routes directly via DNS.

**3. Build the release binary**
```bash
make build
# copy target/release/garageos to EC2
```

**4. Run Nginx and Postgres as services**
Same `nginx.conf` — no changes. Point `DATABASE_URL` at your hosted
Postgres (Supabase, RDS, etc).

The Rust code does not change. No redeployment of configuration.
Recommended instance: `t4g.small` (~$12/month, 2GB RAM, ARM).

---

## Rate Limiting

Implemented in Postgres using an unlogged table — no Redis needed.

| Endpoint | Limit | Window |
|---|---|---|
| `POST /api/register` | 5 requests | per IP per hour |
| `POST /api/login` | 10 requests | per IP per 5 minutes |

Limits are configurable via environment variables.

---

## Security Notes

- Passwords hashed with bcrypt (cost factor 12)
- JWTs signed with HS256, expiry configurable
- `tenant_id` injected by middleware — handlers cannot spoof it
- Subdomain blocklist prevents squatting on reserved names
- Rate limiting on all auth endpoints
- Custom domain ownership verified via CNAME + TXT before SSL issuance
- ACME challenge tokens stored and served automatically

---

## Documentation

| Document | Description |
|---|---|
| `docs/CUSTOM_DOMAIN_TROUBLESHOOTING.md` | Known issues, causes, and fixes for custom domain SSL |

---

## License

MIT