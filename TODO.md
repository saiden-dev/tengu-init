# TODO: Auto DNS via flarectl

## Phase 1: Dual Auth + DNS via flarectl

- [ ] Add `api_token: Option<String>` to `CloudflareConfig` struct
- [ ] Add `--cf-token` CLI arg to Args
- [ ] Add `cf_email`, `cf_api_key`, `cf_api_token` to `ResolvedConfig`
- [ ] Populate CF creds in `resolve_config()` for ALL modes (not just CF mode)
- [ ] New `create_dns_record()` using flarectl: `flarectl dns create-or-update --zone <domain> --type A --name <name> --content <ip>`
- [ ] New `setup_dns_records()` calling it for api/docs/wildcard
- [ ] Replace curl-based `update_wildcard_dns()` with flarectl version
- [ ] Call `setup_dns_records()` in Direct mode (if CF creds available, else print reminder)
- [ ] Build + test

## Phase 2: E2E Verify

- [ ] Destroy tengu-arm, re-provision with `--direct`
- [ ] Confirm A records in CF via `dig` or flarectl
- [ ] Confirm Caddy TLS certs obtained
- [ ] Leave VM running
