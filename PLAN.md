# Auto DNS A Records in Direct Mode

Add automatic Cloudflare DNS record creation to `--direct` mode using flarectl.

## Auth Design

Two auth paths in `init.toml`:

```toml
[cloudflare]
# Option 1: Scoped API Token (preferred, minimal permissions)
api_token = "cf_scoped_token_here"

# Option 2: Global API Key + email (legacy)
api_key = "global_key_here"
email = "adam@example.com"
```

Both work with flarectl:
- Token: `CF_API_TOKEN` env var
- Global key: `CF_API_KEY` + `CF_API_EMAIL` env vars

tengu-init passes whichever is configured to flarectl via env vars.

## DNS Records Created

For `--direct` mode with known server IP:

| Record | Type | Name | Value | Proxied |
|--------|------|------|-------|---------|
| Platform API | A | `api.<domain-platform>` | VM IP | No |
| Platform Docs | A | `docs.<domain-platform>` | VM IP | No |
| Apps wildcard | A | `*.<domain-apps>` | VM IP | No |

Non-proxied so Caddy HTTP-01 challenge works.

## Implementation

### Phase 1: Dual Auth + DNS via flarectl

1. Add `api_token` field to `CloudflareConfig` in main.rs
2. Add `--cf-token` CLI arg
3. Populate `cf_email`, `cf_api_key`, `cf_api_token` on `ResolvedConfig` from config/args/env in ALL modes
4. Replace curl-based `update_wildcard_dns()` with flarectl-based `create_dns_record()`
5. New `setup_dns_records()` creates all 3 A records via flarectl
6. Call `setup_dns_records()` in Direct mode post-provision branch (if any CF creds available)
7. Update existing CF mode to use the same function

### Phase 2: E2E Verify

1. Re-provision tengu-arm with `--direct`
2. Confirm A records created via `dig`
3. Confirm Caddy gets TLS certs
