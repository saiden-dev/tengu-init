# TODO.md -- Zero-Touch Tengu PaaS Deployment

## Phase 1 "GM": Tengu Core -- Commit, Build, Ship
**Agent**: code-rust | **Repo**: ~/Projects/tengu

- [ ] Verify Ruby CMD fix in `src/app/manifest.rs:48` (`puma -p 5000`)
- [ ] Verify Node lockfile fallback in `src/app/manifest.rs:62`
- [ ] Verify pg_isready fix in `src/server/mod.rs:71-87`
- [ ] Run `cargo test` -- all 368 tests pass
- [ ] `git add -A && git commit -m 'fix: Ruby puma port, Node lockfile fallback, PostgreSQL pg_isready'`
- [ ] `git push`
- [ ] `cargo zigbuild --release --target aarch64-unknown-linux-gnu`
- [ ] `cargo deb --no-build --no-strip --target aarch64-unknown-linux-gnu`
- [ ] Confirm .deb exists: `target/aarch64-unknown-linux-gnu/debian/tengu_0.2.0-1_arm64.deb`

## Phase 2 "Ball": tengu-init Bug Fixes
**Agent**: code-rust | **Repo**: ~/Projects/tengu-init

### 2.1 SSH timing
- [ ] `ssh.rs`: Change `max_attempts` from 30 to 24
- [ ] `ssh.rs`: Change `ConnectTimeout` from 5 to 10
- [ ] `ssh.rs`: Change sleep from 2s to 5s

### 2.2 Provision script reliability
- [ ] `bash.rs`: Add apt lock wait preamble (`fuser /var/lib/dpkg/lock-frontend`)
- [ ] `service.rs`: Replace single retry with 5-attempt loop (3s sleep between)
- [ ] `ssh.rs`: Remove blind retry block in `provision()` (lines 346-355)

### 2.3 Caddy DNS-01 API key
- [ ] `config.rs` `caddy_cloudflare_env()`: Add `CF_API_KEY` and `CF_API_EMAIL` env vars alongside `CF_API_TOKEN`

### 2.4 Hetzner SSH key selection
- [ ] `hetzner.rs`: Add `delete_ssh_key()` function
- [ ] `main.rs`: Always delete + recreate SSH key before VM creation

### 2.5 Hetzner x86 deprecation
- [ ] `main.rs`: Add comment in `resolve_hetzner_params()` about cpx deprecation in EU

### 2.6 Cloudflare wildcard DNS update
- [ ] `main.rs`: Add `update_wildcard_dns()` function using CF API (curl or reqwest)
- [ ] `main.rs`: Call after `setup_tunnel()` to update `*.tengu.host` A record

### 2.7 Build
- [ ] `cargo test` -- all tests pass in tengu-init workspace
- [ ] `cargo build --release` -- tengu-init binary built

## Phase 3 "Zaku": Full Integration Smoke Test
**Agent**: devops-tengu | **Depends on**: Phase 1 + Phase 2

- [ ] `hcloud server delete tengu` (nuke existing)
- [ ] Run: `./target/release/tengu-init --hetzner -y --deb-path <path-to-arm64-deb>`
- [ ] Provision script completes in ONE pass (no retry)
- [ ] SSH in, verify services: `systemctl status tengu caddy postgresql docker`
- [ ] Create test-ruby app, git push, verify `curl https://test-ruby.tengu.host/`
- [ ] Create test-node app, git push, verify `curl https://test-node.tengu.host/`
- [ ] Create test-static app, git push, verify `curl https://test-static.tengu.host/`
- [ ] All 3 health checks return 200

## Phase 4 "Gelgoog": Cleanup and Hardening
**Agent**: code-rust + devops-tengu | **Depends on**: Phase 3

- [ ] Commit tengu-init fixes: `git add -A && git commit && git push`
- [ ] Delete test apps: `tengu app delete test-ruby test-node test-static -y`
- [ ] Update `DEFAULT_RELEASE` in `main.rs` to new tag
- [ ] Tag releases if warranted (`v0.2.0` for both repos)
