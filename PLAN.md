# PLAN.md -- Zero-Touch Tengu PaaS Deployment

## Definition of Done

ONE clean `tengu-init --hetzner -y` run that:
1. Creates a Hetzner VM
2. Provisions it completely (no SSH failures, no script retries)
3. Deploys 3 test apps via `git push` (Ruby, Node, Static)
4. All 3 respond to HTTPS health checks on `*.tengu.host`
5. Zero manual corrections. Zero SSH-ing in. Zero retries.

---

## Architecture Overview

```
tengu-init --hetzner
    |
    |-- hcloud server create (cax41 ARM64, hel1)
    |-- wait_for_ssh (retry loop with backoff)
    |-- SCP tengu .deb to /tmp/
    |-- SSH: execute provision script
    |     |-- user setup, base packages, docker, postgres, ollama
    |     |-- tengu-caddy (with CF DNS plugin)
    |     |-- tengu .deb install + service start
    |     |-- postgres DB/user init
    |     |-- admin user creation
    |-- SSH: setup cloudflare tunnel
    |-- Cloudflare: update *.tengu.host A record to new IP
    '-- done
```

Two repos. Eight bugs. Four phases.

---

## Phase 1 "GM": Tengu Core -- Commit, Build, Ship

**Agent**: code-rust
**Repo**: ~/Projects/tengu (Rust, Cargo.toml)
**Blocks**: Phase 3 (the .deb must be built before provisioning can use it)

### Context

A previous agent run fixed 3 bugs in the tengu source but did NOT commit, push,
or rebuild the .deb. The fixes exist in the working tree. 368 tests pass.

### 1.1 Verify fixes are correct

Read the three files and confirm fixes match the described intent:

| File | Fix | Verify |
|------|-----|--------|
| `src/app/manifest.rs:48` | Ruby CMD: `bundle exec puma -p 5000` (was 9292) | grep for `puma -p 5000` |
| `src/app/manifest.rs:62` | Node build: `if [ -f package-lock.json ]; then npm ci; else npm install; fi` | grep for `npm install` fallback |
| `src/server/mod.rs:71-87` | PostgreSQL init: `pg_isready` instead of `sudo -u postgres psql` | grep for `pg_isready` |

### 1.2 Run tests

```bash
cd ~/Projects/tengu
cargo test
```

All 368 tests must pass. If any fail, stop and investigate.

### 1.3 Commit and push

```bash
git add -A
git commit -m 'fix: Ruby puma port, Node lockfile fallback, PostgreSQL pg_isready'
git push
```

### 1.4 Build ARM64 .deb

The production server is ARM64 (cax41). Build uses `cargo-zigbuild` + `cargo-deb`:

```bash
cargo zigbuild --release --target aarch64-unknown-linux-gnu
cargo deb --no-build --no-strip --target aarch64-unknown-linux-gnu
```

Output: `target/aarch64-unknown-linux-gnu/debian/tengu_0.2.0-1_arm64.deb`

### 1.5 Note the .deb path

tengu-init's `--deb-path` flag will SCP this to the VM during provisioning,
bypassing the GitHub release download (which has the old broken version).

### Success Criteria

- [x] 3 fixes present in source (already done)
- [ ] `cargo test` -- 368 pass, 0 fail
- [ ] committed and pushed
- [ ] ARM64 .deb exists at known path

---

## Phase 2 "Ball": tengu-init Bug Fixes

**Agent**: code-rust
**Repo**: ~/Projects/tengu-init (Rust, Cargo workspace)
**Blocks**: Phase 3 (tengu-init must be correct before the smoke test)
**Parallel with**: Phase 1 (different repo, no dependency)

### 2.1 SSH retry timing (Bug #1)

**File**: `crates/tengu-init/src/providers/ssh.rs` -- `wait_for_ssh()`

**Current behavior**: Retries every 2s with ConnectTimeout=5s, 30 attempts (60s).
After `hcloud server create`, a freshly booted VM needs 15-30s before sshd is up.
The retry loop already exists but the total window is too short for slow boots.

**Fix**: Increase to retry every 5s, 24 attempts (120s total timeout). Also increase
ConnectTimeout to 10s for slow network conditions on fresh VMs.

```rust
// In wait_for_ssh():
let max_attempts = 24;  // was 30
// ...
"-o", "ConnectTimeout=10",  // was 5
// ...
std::thread::sleep(Duration::from_secs(5));  // was 2
```

### 2.2 First provision run fails ~step 31 (Bug #2)

**File**: `crates/tengu-init/src/providers/ssh.rs` -- `provision()` and
`crates/tengu-provision/src/steps/service.rs`

**Root cause analysis**: The provision script runs with `set -euo pipefail`.
Around step 31, multiple services are started in sequence (docker.socket, docker,
postgresql, fail2ban, caddy, ollama). The current service start has a single
2-second retry:

```bash
systemctl start foo || { sleep 2 && systemctl start foo; }
```

This fails when:
- apt dpkg locks are held by unattended-upgrades (common on fresh Ubuntu VMs)
- postgresql needs time to initialize data directory on first boot
- docker.socket needs to be ready before docker.service can start

**Fixes (two-part)**:

**Part A** -- Kill unattended-upgrades before apt operations.
In `crates/tengu-provision/src/render/bash.rs`, add a preamble to the generated
script that waits for apt locks:

```bash
# Wait for any apt locks (unattended-upgrades on fresh Ubuntu VMs)
while fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; do
    echo "Waiting for dpkg lock..."
    sleep 5
done
```

**Part B** -- Improve service start retry in `service.rs`.
Change the retry to a proper wait loop (5 attempts, 3s apart):

```bash
systemctl is-active foo >/dev/null 2>&1 || \
  systemctl start foo || \
  { for i in 1 2 3 4 5; do sleep 3; systemctl start foo && break; done; }
```

**Part C** -- Remove the blind retry in `provision()` (ssh.rs lines 346-355).
The "first run fails, retry" approach masks real errors. With Parts A and B,
the script should succeed on the first run. If it fails, we want to see the real
error, not retry and hide it.

### 2.3 Caddy DNS-01 API key mismatch (Bug #3)

**File**: `crates/tengu-provision/src/config.rs` -- `caddyfile()` and `caddy_cloudflare_env()`

**Current state**: The Caddyfile uses `{env.CF_API_TOKEN}` (scoped token format)
but the systemd drop-in passes a **Global API Key** as `CF_API_TOKEN`. Caddy's
Cloudflare DNS plugin actually accepts both Global API Key and scoped API Token,
but the env var naming is misleading.

The real issue: `caddy-cloudflare` plugin (the Caddy build with CF DNS support)
uses the `cloudflare` module which checks for `CF_API_TOKEN` first. When a Global
API Key is passed as `CF_API_TOKEN`, the Cloudflare API rejects it because it
expects a scoped token in the `Authorization: Bearer` header.

**Fix**: Use the Global API Key correctly. The Caddy Cloudflare DNS module also
supports `CF_API_KEY` + `CF_API_EMAIL` for global keys. Change the Caddyfile
snippet and the systemd env drop-in:

In `config.rs` -- `caddyfile()`:
```rust
// Change from:
//   dns cloudflare {env.CF_API_TOKEN}
// To:
(cf_tls) {{
    tls {{
        dns cloudflare {{
            api_token {{env.CF_API_TOKEN}}
        }}
    }}
}}
```

Actually, the cleaner fix is to just keep using the token format but document
that the user needs a **scoped API token** (not global key). However since the
`init.toml` config field is called `api_key` and 1Password stores a global key,
the pragmatic fix is:

In `config.rs` -- `caddy_cloudflare_env()`, set both env vars:
```rust
pub fn caddy_cloudflare_env(&self) -> String {
    format!(
        "[Service]\nEnvironment=\"CF_API_TOKEN={key}\"\nEnvironment=\"CF_API_KEY={key}\"\nEnvironment=\"CF_API_EMAIL={email}\"\n",
        key = self.cf_api_key,
        email = self.cf_email,
    )
}
```

And in `caddyfile()`, use the block syntax that tries token first, falls back to key:
```rust
(cf_tls) {{
    tls {{
        dns cloudflare {{env.CF_API_TOKEN}}
    }}
}}
```

The caddy-cloudflare module auto-detects: if the value looks like a scoped token
(starts with specific prefix), it uses Bearer auth. If it looks like a global key,
it uses X-Auth-Key + X-Auth-Email. Passing all three env vars covers both cases.

### 2.4 Hetzner SSH key selection (Bug #4)

**File**: `crates/tengu-init/src/providers/hetzner.rs` -- `create_server()` and
`crates/tengu-init/src/main.rs`

**Current state**: The code creates/checks for a single SSH key named `tengu-init`
in Hetzner. This key comes from `resolved.ssh_key` (the user's local SSH pubkey
detected from `~/.ssh/id_ed25519.pub`).

**Problem**: When running from the junkpile machine, the SSH key is the junkpile
key. But if tengu-init was previously run from a different machine (e.g., MacBook),
the Hetzner SSH key named `tengu-init` has the MacBook's pubkey. The key existence
check (`ssh_key_exists`) succeeds, so it skips creation. But the VM is created
with the wrong key.

**Fix**: After confirming the key exists in Hetzner, fetch its fingerprint and
compare. If the fingerprint doesn't match the local key, update it.

In `hetzner.rs`, add an `update_ssh_key()` function:
```rust
pub fn update_ssh_key(name: &str, public_key: &str) -> Result<()> {
    // Delete and recreate (hcloud doesn't have ssh-key update)
    Self::delete_ssh_key(name)?;
    Self::create_ssh_key(name, public_key)?;
    Ok(())
}
```

In `main.rs`, change the key handling:
```rust
if Hetzner::ssh_key_exists(SSH_KEY_NAME)? {
    // Key exists -- verify it matches local key
    let remote_fp = Hetzner::ssh_key_fingerprint(SSH_KEY_NAME)?;
    let local_fp = compute_ssh_fingerprint(&resolved.ssh_key);
    if remote_fp != local_fp {
        println!("{} SSH key mismatch, updating...", style("*").cyan());
        Hetzner::update_ssh_key(SSH_KEY_NAME, &resolved.ssh_key)?;
    }
} else {
    Hetzner::create_ssh_key(SSH_KEY_NAME, &resolved.ssh_key)?;
}
```

A simpler alternative (preferred): just always delete + recreate the key.
The SSH key in Hetzner is ephemeral -- it's only used during initial VM creation.
No harm in refreshing it every run:

```rust
// Ensure SSH key matches current machine
if Hetzner::ssh_key_exists(SSH_KEY_NAME)? {
    Hetzner::delete_ssh_key(SSH_KEY_NAME)?;
}
Hetzner::create_ssh_key(SSH_KEY_NAME, &resolved.ssh_key)?;
```

### 2.5 Hetzner x86 deprecation / architecture mismatch (Bug #7)

**File**: `crates/tengu-init/src/main.rs` -- `resolve_hetzner_params()`

**Current defaults**: `server_type: cax41` (ARM64), `location: hel1` (Helsinki).
This is correct for ARM. The .deb from Phase 1 is also ARM64.

**No code change needed** -- the current defaults are already ARM64 (cax41) in
an EU location (hel1) where ARM types are available. The x86 cpx types were
deprecated in EU but that doesn't affect us since we use cax (ARM).

**Documentation only**: Add a comment in `resolve_hetzner_params()` explaining
that x86 `cpx*` types are unavailable in EU since 2026-01-01. If a user wants
x86, they must use `--location ash` or `--location hil` (US locations).

### 2.6 Cloudflare wildcard DNS update (Bug #8)

**File**: `crates/tengu-init/src/main.rs` -- after `provider.setup_tunnel()`

**Current state**: After creating the VM and CF tunnel, the `*.tengu.host` A record
still points to the old IP. The tunnel handles `*.tengu.to` subdomains (api, docs,
git, ssh) but app traffic goes through `*.tengu.host` which needs a direct A record.

**Fix**: After provisioning and tunnel setup, call the Cloudflare API to update
the `*.tengu.host` wildcard A record to the new VM's IP. This requires:

1. Add a `cloudflare.rs` module to tengu-init (or use `curl` via SSH)
2. After `setup_tunnel()`, call Cloudflare API:
   - GET zones for `tengu.host`
   - GET DNS records for `*.tengu.host`
   - PUT to update the A record to the new IP

The simplest implementation uses the `hcloud` IP already obtained and the CF
API credentials already in `resolved`:

```rust
// After setup_tunnel() in main.rs
fn update_wildcard_dns(cf_email: &str, cf_api_key: &str, domain: &str, ip: &str) -> Result<()>
```

This uses `curl` calls via the SSH provider (run on the VM) or directly from the
local machine. Prefer local execution since we already have credentials.

### 2.7 Build tengu-init binary

After all fixes, rebuild the tengu-init binary:

```bash
cd ~/Projects/tengu-init
cargo build --release
```

### Success Criteria

- [ ] SSH wait_for_ssh: 120s timeout, 5s intervals, 10s connect timeout
- [ ] apt lock wait preamble in generated provision script
- [ ] Service start retry: 5 attempts with 3s sleep
- [ ] Blind retry removed from provision()
- [ ] Caddy env drop-in includes CF_API_KEY + CF_API_EMAIL
- [ ] Hetzner SSH key always refreshed before VM create
- [ ] Wildcard DNS update after tunnel setup
- [ ] `cargo test` passes in tengu-init workspace
- [ ] `cargo build --release` succeeds

---

## Phase 3 "Zaku": Full Integration Smoke Test

**Agent**: devops-tengu
**Repo**: Both repos
**Depends on**: Phase 1 (ARM64 .deb), Phase 2 (fixed tengu-init)
**Blocks**: Phase 4

### 3.1 Nuke existing server

```bash
hcloud server delete tengu
```

### 3.2 Run tengu-init

```bash
cd ~/Projects/tengu-init
./target/release/tengu-init --hetzner -y \
    --deb-path ~/Projects/tengu/target/aarch64-unknown-linux-gnu/debian/tengu_0.2.0-1_arm64.deb
```

Expected flow:
1. Creates `tengu` server (cax41, hel1, ubuntu-24.04)
2. Waits for SSH (up to 120s)
3. SCPs the local .deb
4. Uploads and runs provision script (ONE pass, no retry)
5. Sets up CF tunnel
6. Updates `*.tengu.host` DNS

### 3.3 Verify provisioning

SSH in and check:
```bash
ssh chi@ssh.tengu.to
systemctl status tengu caddy postgresql docker
tengu version
```

### 3.4 Deploy 3 test apps

Create minimal test apps and push to verify the full pipeline:

**Ruby app** (test-ruby):
```yaml
# app.yml
runtime: ruby
```
```ruby
# config.ru
run ->(env) { [200, {'content-type' => 'text/plain'}, ['OK ruby']] }
```
```ruby
# Gemfile
source 'https://rubygems.org'
gem 'puma'
```

**Node app** (test-node):
```yaml
# app.yml
runtime: node
```
```javascript
// server.js
require('http').createServer((_, res) => {
  res.writeHead(200); res.end('OK node');
}).listen(5000);
```
```json
// package.json
{ "name": "test-node", "version": "1.0.0" }
```

**Static app** (test-static):
```yaml
# app.yml
runtime: static
static_dir: dist
```
```html
<!-- dist/index.html (pre-built, no build step needed) -->
<!DOCTYPE html><html><body>OK static</body></html>
```
```json
// package.json
{ "name": "test-static", "scripts": { "build": "echo ok" } }
```

Deploy each:
```bash
tengu app create test-ruby
tengu app create test-node
tengu app create test-static
# Then git push to each
```

### 3.5 Health checks

```bash
curl -sf https://test-ruby.tengu.host/ | grep "OK ruby"
curl -sf https://test-node.tengu.host/ | grep "OK node"
curl -sf https://test-static.tengu.host/ | grep "OK static"
```

All three must return 200 with expected body.

### Success Criteria

- [ ] `tengu-init --hetzner` completes with zero failures
- [ ] Provision script runs once (no retry)
- [ ] All services running: tengu, caddy, postgresql, docker
- [ ] 3 test apps deployed via git push
- [ ] 3 HTTPS health checks pass
- [ ] Total time from `tengu-init` start to health checks: < 15 minutes

---

## Phase 4 "Gelgoog": Cleanup and Hardening

**Agent**: code-rust + devops-tengu
**Depends on**: Phase 3 (must pass before cleanup)

### 4.1 Commit tengu-init fixes

```bash
cd ~/Projects/tengu-init
git add -A
git commit -m 'fix: zero-touch provisioning (SSH timing, apt locks, CF DNS, key rotation)'
git push
```

### 4.2 Remove test apps

```bash
tengu app delete test-ruby -y
tengu app delete test-node -y
tengu app delete test-static -y
```

### 4.3 Tag release

If we're satisfied with the state:
```bash
# tengu
cd ~/Projects/tengu
git tag v0.2.0
git push --tags

# tengu-init
cd ~/Projects/tengu-init
git tag v0.2.0
git push --tags
```

### 4.4 Update DEFAULT_RELEASE in tengu-init

Update `main.rs` constant `DEFAULT_RELEASE` to point to the new tag so future
`tengu-init` runs without `--deb-path` download the correct version.

### Success Criteria

- [ ] Both repos committed and pushed
- [ ] Test apps cleaned up
- [ ] Releases tagged (if warranted)

---

## Dependency Graph

```
Phase 1 "GM"       Phase 2 "Ball"
(tengu .deb)       (tengu-init fixes)
     \                  /
      \                /
       v              v
      Phase 3 "Zaku"
      (smoke test)
            |
            v
      Phase 4 "Gelgoog"
      (cleanup)
```

Phases 1 and 2 are **fully parallel** (different repos, no shared files).
Phase 3 blocks on both.
Phase 4 blocks on Phase 3.

---

## Files Modified

### Phase 1 (tengu)
| File | Change |
|------|--------|
| `src/app/manifest.rs` | Already fixed (Ruby port, Node lockfile) |
| `src/server/mod.rs` | Already fixed (pg_isready) |
| `src/app/builder.rs` | No change needed (uses manifest correctly) |

### Phase 2 (tengu-init)
| File | Change |
|------|--------|
| `crates/tengu-init/src/providers/ssh.rs` | SSH timing (2.1), remove blind retry (2.2C) |
| `crates/tengu-provision/src/steps/service.rs` | Service start retry loop (2.2B) |
| `crates/tengu-provision/src/render/bash.rs` | apt lock wait preamble (2.2A) |
| `crates/tengu-provision/src/config.rs` | Caddy CF env vars (2.3) |
| `crates/tengu-init/src/providers/hetzner.rs` | SSH key refresh + delete helper (2.4) |
| `crates/tengu-init/src/main.rs` | Key handling, DNS update, arch comment (2.4, 2.5, 2.6) |

### Phase 3 (no code -- operational)
Test app source is ephemeral (created in /tmp, pushed, then deleted).

### Phase 4 (tengu-init)
| File | Change |
|------|--------|
| `crates/tengu-init/src/main.rs` | Update DEFAULT_RELEASE constant |

---

## Risk Mitigation

| Risk | Mitigation |
|------|-----------|
| ARM .deb fails to cross-compile on macOS | `cargo-zigbuild` is proven in deploy.sh. Fall back to CI build. |
| CF tunnel creation fails | cert.pem must exist locally. tengu-init already checks for this. |
| VM boot takes > 120s | Extremely unlikely for cax41. Fallback: re-run (SSH wait is the first step). |
| Caddy can't get TLS cert | CF DNS challenge needs working API key. Phase 2.3 fixes the env var mismatch. |
| Test app push fails | Indicates tengu service issue. Check `journalctl -u tengu` on VM. |
