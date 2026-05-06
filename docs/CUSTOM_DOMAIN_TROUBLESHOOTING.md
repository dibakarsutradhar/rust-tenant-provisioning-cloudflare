# Custom Domain Troubleshooting Guide

## Overview

GarageOS supports custom domains via **Cloudflare for SaaS**. When a tenant adds
a custom domain (e.g. `app.ggdhaka.com`), the system:

1. Verifies the CNAME record points to the tunnel
2. Creates a Cloudflare custom hostname for SSL
3. Verifies domain ownership via TXT record
4. Waits for SSL certificate issuance
5. Marks the domain active and routes traffic

This guide documents known issues, causes, and solutions encountered during
development and testing.

---

## Issue Index

1. [Custom hostname does not CNAME to this zone](#1-custom-hostname-does-not-cname-to-this-zone)
2. [SSL stuck at pending_validation](#2-ssl-stuck-at-pending_validation)
3. [SSL stuck at ssl_com CA](#3-ssl-stuck-at-ssl_com-ca)
4. [Certificate Authority selection error](#4-certificate-authority-selection-error)
5. [ACME challenge not served](#5-acme-challenge-not-served)
6. [Local DNS not resolving custom domain](#6-local-dns-not-resolving-custom-domain)
7. [Domain works with --resolve but not directly](#7-domain-works-with---resolve-but-not-directly)
8. [Cloudflare-to-Cloudflare domain limitation](#8-cloudflare-to-cloudflare-domain-limitation)
9. [TXT ownership value rotates](#9-txt-ownership-value-rotates)
10. [SSE stream misses events on reconnect](#10-sse-stream-misses-events-on-reconnect)

---

## 1. Custom hostname does not CNAME to this zone

### Symptom
```
verification_errors: ["custom hostname does not CNAME to this zone."]
status: "pending"
```

### Cause
The custom domain's CNAME record is either:
- Not yet created at the customer's DNS provider
- Proxied through Cloudflare (orange cloud) when it should be grey
- Pointing to the wrong target

### Solution
The CNAME must point directly to your tunnel and be **unproxied (grey cloud)**:

```
Type:    CNAME
Name:    app (or subdomain)
Target:  <tunnel-id>.cfargotunnel.com
Proxy:   OFF — grey cloud only
```

Verify with:
```bash
dig +short CNAME app.yourdomain.com @1.1.1.1
# must return: <tunnel-id>.cfargotunnel.com.
# NOT an IP address (IP = proxied = wrong)
```

### Exception
If the customer's domain is **also on Cloudflare**, see [Issue #8](#8-cloudflare-to-cloudflare-domain-limitation).

---

## 2. SSL stuck at pending_validation

### Symptom
```json
"ssl": {
  "status": "pending_validation",
  "validation_records": [
    { "status": "pending", "http_url": "http://domain/.well-known/acme-challenge/TOKEN" }
  ]
}
```

### Cause
Cloudflare's CA needs to reach the ACME HTTP challenge URL on your server but either:
- The challenge token is not stored in your DB
- Your tunnel is not running
- The domain doesn't resolve to your tunnel
- "Always Use HTTPS" is redirecting HTTP to HTTPS before CF can validate

### Solution

**Step 1** — Verify the challenge is being served:
```bash
curl http://yourdomain.com/.well-known/acme-challenge/TOKEN
# must return the exact http_body value
```

**Step 2** — If not served, insert into DB manually:
```bash
psql $DATABASE_URL -c "
INSERT INTO acme_challenges (token, response)
VALUES ('TOKEN', 'BODY')
ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response;
"
```

**Step 3** — Trigger CF recheck:
```bash
curl -X PATCH "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames/$CF_HOSTNAME_ID" \
  -H "Authorization: Bearer $CF_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

**Step 4** — If using Cloudflare on the customer domain, disable "Always Use HTTPS"
for the ACME path via a Page Rule:
```
URL: http://yourdomain.com/.well-known/acme-challenge/*
Setting: SSL → Off
```

### Notes
- Google Trust Services (fast, 1-3 min) and SSL.com (slow, 10-30 min) are
  assigned automatically by Cloudflare — you cannot choose on non-Enterprise plans
- New tokens are generated each time CF retries — the system auto-stores them
  on every status poll
- Two validation records are normal — both must be served

---

## 3. SSL stuck at ssl_com CA

### Symptom
```json
"certificate_authority": "ssl_com",
"status": "pending_validation"
```
Stays pending for 15+ minutes despite challenges being served correctly.

### Cause
SSL.com is significantly slower than Google Trust Services. Cloudflare assigns
the CA automatically — it cannot be controlled on free/pro plans.

### Solution
Delete the custom hostname and recreate — Cloudflare will assign a different CA:

```bash
# delete
curl -X DELETE "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames/$CF_HOSTNAME_ID" \
  -H "Authorization: Bearer $CF_TOKEN"

# recreate
curl -X POST "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames" \
  -H "Authorization: Bearer $CF_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "hostname": "yourdomain.com",
    "ssl": { "method": "http", "type": "dv", "settings": { "min_tls_version": "1.2" } }
  }'
```

Keep recreating until you get `"certificate_authority": "google"` in the response.

### Expected timing
- Google Trust Services: 1-3 minutes
- SSL.com: 10-30+ minutes (sometimes fails with internal errors)

---

## 4. Certificate Authority selection error

### Symptom
```json
{
  "code": 1459,
  "message": "Certificate Authority selection is only available on an Enterprise plan."
}
```

### Cause
The code was passing `"certificate_authority": "google"` in the custom hostname
creation request, which is an Enterprise-only feature.

### Solution
Remove `certificate_authority` from the request body entirely. Cloudflare assigns
it automatically:

```rust
// WRONG
let body = serde_json::json!({
    "hostname": domain,
    "ssl": {
        "method": "http",
        "type": "dv",
        "certificate_authority": "google",  // ← remove this
    }
});

// CORRECT
let body = serde_json::json!({
    "hostname": domain,
    "ssl": {
        "method": "http",
        "type": "dv",
        "settings": { "min_tls_version": "1.2" }
    }
});
```

---

## 5. ACME challenge not served

### Symptom
```bash
curl http://yourdomain.com/.well-known/acme-challenge/TOKEN
# {"error":"challenge not found"}
# or: connection refused
# or: redirect to https
```

### Cause A — Token not in DB
The system stores tokens automatically during SSL polling, but if the server
restarted or the token is new, it may not be stored yet.

**Solution:** Insert manually:
```bash
psql $DATABASE_URL -c "
INSERT INTO acme_challenges (token, response)
VALUES ('TOKEN', 'FULL_BODY_VALUE')
ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response;
"
```

### Cause B — HTTP redirecting to HTTPS
CF validates over HTTP. If your domain forces HTTPS, the challenge fails.

**Solution:** The tunnel handles SSL termination. Ensure Nginx is not redirecting
HTTP to HTTPS for the `/.well-known/` path.

### Cause C — Tunnel not running
**Solution:**
```bash
make tunnel
# verify: cloudflared tunnel --config cloudflared/config.yml run
```

### Cause D — Wrong route in axum
The ACME challenge route must be in `public_routes`, not behind auth middleware.

**Solution:** Verify in `main.rs`:
```rust
let public_routes = Router::new()
    .route("/.well-known/acme-challenge/:token",
        get(handlers::domains::acme_challenge))
    .route("/.well-known/cf-custom-hostname-challenge/:token",
        get(handlers::domains::acme_challenge));
```

---

## 6. Local DNS not resolving custom domain

### Symptom
```bash
curl https://hybrid.dibakar.me/health
# curl: (6) Could not resolve host: hybrid.dibakar.me
```
But `dig +short hybrid.dibakar.me @1.1.1.1` returns an IP.

### Cause
Your Mac's DNS resolver (usually your router at `192.168.0.1`) is caching
old DNS records. Flushing the Mac cache doesn't help because the router
itself has a cache.

### Solution

**Option A — Change Mac DNS to bypass router:**
```
System Settings → Network → WiFi → Details → DNS
→ Add: 1.1.1.1 and 1.0.0.1 at the top
```

Then flush:
```bash
sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder
```

**Option B — Use --resolve for testing (doesn't fix DNS, just bypasses):**
```bash
curl --resolve hybrid.dibakar.me:443:104.21.15.40 \
  https://hybrid.dibakar.me/health
```

**Option C — Add to /etc/hosts temporarily:**
```
127.0.0.1   hybrid.dibakar.me
```
⚠ Remember to remove this before testing real external access.

### Note
Cloudflare's validation servers use their own DNS — your local DNS issue
does not affect CF's ability to validate. This is a local dev inconvenience only.

---

## 7. Domain works with --resolve but not directly

### Symptom
```bash
curl --resolve domain:443:104.21.15.40 https://domain/health  # works
curl https://domain/health  # curl: (6) Could not resolve host
```

### Cause
Same as Issue #6 — local DNS cache. The domain resolves correctly globally
but not on your machine.

### Solution
See Issue #6. This is a local issue only and does not affect production.

---

## 8. Cloudflare-to-Cloudflare domain limitation

### Symptom
Customer's domain is also on Cloudflare. Custom hostname stays in `pending`
status with:
```
"custom hostname does not CNAME to this zone."
```
Even with grey cloud CNAME.

### Cause
Cloudflare for SaaS cannot issue SSL certificates for custom hostnames where
the customer's domain is proxied through a different Cloudflare zone. This is
a platform limitation — full support requires Cloudflare Enterprise
("Cloudflare for SaaS with Cloudflare customers").

### Two-path solution

**Path 1 — Customer domain NOT on Cloudflare** (Namecheap, GoDaddy, Route53):
```
CNAME: subdomain → <tunnel-id>.cfargotunnel.com  [grey cloud]
TXT:   _cf-custom-hostname.subdomain → <ownership-uuid>
SSL:   issued by Cloudflare for SaaS (Google Trust Services or SSL.com)
```

**Path 2 — Customer domain IS on Cloudflare**:
```
CNAME: subdomain → <tunnel-id>.cfargotunnel.com  [orange cloud OK]
TXT:   not needed
SSL:   customer's existing Cloudflare wildcard cert (*.domain.com)
       already covers the subdomain automatically
```

Path 2 already works — the `*.dibakar.me` wildcard cert covers
`hybrid.dibakar.me` automatically. No CF for SaaS needed.

### Detection logic (to implement)
```rust
// Check if domain resolves to CF IPs → customer is on Cloudflare
// If yes → skip CF for SaaS, just verify CNAME and mark active
// If no  → proceed with CF for SaaS custom hostname flow
async fn is_cloudflare_proxied(domain: &str) -> bool {
    // dig A domain @1.1.1.1 → check if IPs are in CF ranges
    // 172.67.x.x, 104.21.x.x, 104.16-20.x.x etc
}
```

### Current workaround
For testing with a Cloudflare domain:
1. Set CNAME to orange cloud
2. Skip the CF for SaaS custom hostname creation
3. Manually mark domain as active in DB
4. Tenant middleware already resolves it correctly

---

## 9. TXT ownership value rotates

### Symptom
```bash
dig +short TXT _cf-custom-hostname.domain.com @1.1.1.1
# returns old UUID

# CF dashboard shows different UUID
```

### Cause
Every time a custom hostname is deleted and recreated, Cloudflare generates a
new ownership verification UUID. The old TXT record becomes invalid.

### Solution
After any delete+recreate cycle:
1. Check new UUID from CF API or dashboard
2. Update TXT record at customer's DNS provider
3. Update DB:
```bash
psql $DATABASE_URL -c "
UPDATE custom_domains
SET cf_hostname_id = 'NEW_CF_ID',
    ownership_txt_value = 'NEW_UUID'
WHERE domain = 'yourdomain.com';
"
```

### Prevention
The `refresh_ownership_info` function in `custom_domain.rs` automatically
fetches and stores the latest ownership values whenever the hostname already
exists. This prevents stale values in the DB.

---

## 10. SSE stream misses events on reconnect

### Symptom
User opens the domain status modal after provisioning has already progressed.
The TXT record card remains locked showing "waiting for CNAME..." even though
the domain is already in `ssl_pending` state.

### Cause
SSE is a one-way push — events fired before the client connected are lost.
The `records:` event containing the TXT name and value was sent when CNAME
was first detected, but the client wasn't connected yet.

### Solution
The `domain_stream` handler checks the current DB status on connect and
immediately re-sends the appropriate events:

```rust
match row.status.as_str() {
    "ssl_pending" => {
        // re-send records: event immediately with stored TXT values
        let records_event = format!("{name}={value}");
        tx.send(Event::default().event("records").data(records_event)).await;
        // then continue listening for future events
    }
    "active" => { /* send done: immediately */ }
    "failed" => { /* send failed: immediately */ }
    _ => { /* normal listen */ }
}
```

---

## Quick Reference — Manual Debug Commands

```bash
# check custom hostname status
curl -s "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames?hostname=DOMAIN" \
  -H "Authorization: Bearer $CF_TOKEN" | python3 -m json.tool

# trigger CF recheck
curl -X PATCH "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames/CF_HOSTNAME_ID" \
  -H "Authorization: Bearer $CF_TOKEN" -H "Content-Type: application/json" -d '{}'

# check CNAME
dig +short CNAME domain.com @1.1.1.1

# check TXT
dig +short TXT _cf-custom-hostname.domain.com @1.1.1.1

# verify ACME challenge served
curl http://domain.com/.well-known/acme-challenge/TOKEN

# verify cf-custom-hostname-challenge served
curl http://domain.com/.well-known/cf-custom-hostname-challenge/CF_HOSTNAME_ID

# insert ACME challenge manually
psql $DATABASE_URL -c "
INSERT INTO acme_challenges (token, response) VALUES ('TOKEN', 'BODY')
ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response;"

# mark domain active manually
psql $DATABASE_URL -c "
UPDATE custom_domains SET status = 'active' WHERE domain = 'DOMAIN';
INSERT INTO kv_cache (key, value, expires_at)
SELECT 'custom:DOMAIN', tenant_id::text, now() + interval '24 hours'
FROM custom_domains WHERE domain = 'DOMAIN'
ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, expires_at = EXCLUDED.expires_at;"

# delete and restart domain
psql $DATABASE_URL -c "DELETE FROM custom_domains WHERE domain = 'DOMAIN';"
curl -X DELETE "https://api.cloudflare.com/client/v4/zones/$CF_ZONE/custom_hostnames/CF_HOSTNAME_ID" \
  -H "Authorization: Bearer $CF_TOKEN"
```

---

## Decision Tree

```
User adds custom domain
        │
        ▼
Does CNAME resolve? ──No──→ Wait / ask user to add CNAME
        │
       Yes
        │
        ▼
Is it a CF IP? ──Yes──→ Customer is on Cloudflare
        │                    │
        │                    ▼
        │              Skip CF for SaaS
        │              Mark active directly
        │              SSL = customer's wildcard cert ✓
        │
       No (non-CF domain)
        │
        ▼
Create CF custom hostname
        │
        ▼
Got Google Trust Services? ──No (SSL.com)──→ Delete + recreate
        │
       Yes
        │
        ▼
Ownership TXT added? ──No──→ Show user TXT record to add
        │
       Yes
        │
        ▼
ACME challenges stored? ──No──→ Auto-stored on next poll
        │
       Yes
        │
        ▼
Wait 1-3 min → SSL active ✓
```

---

## Environment Variables Relevant to Custom Domains

```bash
CLOUDFLARE_API_TOKEN      # Zone DNS Edit permission
CLOUDFLARE_ZONE_ID        # thegarageos.com zone ID
CLOUDFLARE_TUNNEL_ID      # tunnel ID for CNAME target
BASE_DOMAIN               # thegarageos.com
MOCK_CLOUDFLARE           # true = skip all CF API calls
```