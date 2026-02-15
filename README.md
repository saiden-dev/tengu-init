<p align="center">
  <a href="https://tengu.to">
    <img src="logo.png" alt="Tengu" width="120" height="120">
  </a>
</p>

<h1 align="center">tengu-init</h1>

<p align="center">
  <strong>Provision Tengu PaaS on Hetzner Cloud</strong>
</p>

<p align="center">
  One command to spin up a fully configured <a href="https://tengu.to">Tengu</a> server with SSL, PostgreSQL, and git push deploys.
</p>

---

## Install

```bash
cargo install tengu-init
```

**Requires:** [hcloud CLI](https://github.com/hetznercloud/cli) configured with an API token.

## Usage

```bash
# Interactive provisioning with config file
tengu-init

# Preview without creating
tengu-init --dry-run

# Override server type
tengu-init --server-type cpx41
```

## Configuration

Create `~/.config/tengu/init.toml`:

```toml
[server]
name = "tengu"
type = "cax41"        # ARM64, 16 vCPU, 32GB RAM
location = "hel1"     # Helsinki
image = "ubuntu-24.04"

[domains]
platform = "tengu.to"
apps = "tengu.host"

[cloudflare]
api_key = "your-cloudflare-api-key"
email = "your-email@example.com"

[resend]
api_key = "re_xxx"

[ssh]
public_key = "ssh-ed25519 AAAA..."

[notifications]
email = "notify@example.com"
```

## Server Types

| Type | Arch | vCPU | RAM | Disk | Price |
|------|------|------|-----|------|-------|
| `cax11` | ARM64 | 2 | 4GB | 40GB | ~€4/mo |
| `cax21` | ARM64 | 4 | 8GB | 80GB | ~€8/mo |
| `cax31` | ARM64 | 8 | 16GB | 160GB | ~€15/mo |
| `cax41` | ARM64 | 16 | 32GB | 320GB | ~€30/mo |
| `cpx11` | x86 | 2 | 2GB | 40GB | ~€5/mo |
| `cpx21` | x86 | 3 | 4GB | 80GB | ~€9/mo |
| `cpx31` | x86 | 4 | 8GB | 160GB | ~€17/mo |
| `cpx41` | x86 | 8 | 16GB | 240GB | ~€32/mo |

ARM servers (cax*) are recommended—cheaper and Tengu builds for both architectures.

## What Gets Installed

- **Docker** - Container runtime
- **Caddy** - Automatic HTTPS reverse proxy
- **PostgreSQL 16** - Database with pgvector extension
- **Tengu** - PaaS server with SSH git endpoint

## License

MIT
