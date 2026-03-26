<p align="center">
  <a href="https://tengu.to">
    <img src="logo.png" alt="Tengu" width="120" height="120">
  </a>
</p>

<h1 align="center">tengu-init</h1>

<p align="center">
  <strong>Provision Tengu PaaS servers</strong>
</p>

<p align="center">
  One command to spin up a fully configured <a href="https://tengu.to">Tengu</a> server with SSL, PostgreSQL, Docker, and git push deploys.
</p>

---

## Features

- **Hetzner Cloud** - Automatic VM provisioning via cloud-init
- **Baremetal** - SSH-based provisioning for existing servers
- **Removal** - Clean uninstall of Tengu and all dependencies
- **Idempotent** - Safe to re-run, only applies needed changes
- **Interactive** - Prompts for missing credentials with config file and env var support
- **Multi-arch** - Supports both ARM64 and x86_64

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/tengu-apps/tengu-init/master/install.sh | sh
```

Or download binaries directly from [releases](https://github.com/tengu-apps/tengu-init/releases).

## Usage

### Baremetal / Existing Server

```bash
# Provision existing server via SSH (interactive credential prompts)
tengu-init chi@192.168.1.100

# Custom SSH port
tengu-init chi@my-server.com --port 2222

# Generate script only (don't execute)
tengu-init chi@server --script-only > provision.sh

# Dry run - show config without provisioning
tengu-init chi@server --dry-run
```

### Hetzner Cloud

```bash
# Create Hetzner VPS and provision
tengu-init --hetzner

# Override server type and location
tengu-init --hetzner --server-type cpx41 --location fsn1

# Preview without creating
tengu-init --hetzner --dry-run

# Force recreate existing server
tengu-init --hetzner --force
```

**Requires:** [hcloud CLI](https://github.com/hetznercloud/cli) configured with an API token.

### Remove Tengu

```bash
# Remove Tengu and all installed dependencies
tengu-init chi@server --remove

# Skip confirmation prompt
tengu-init chi@server --remove --force

# Generate removal script only
tengu-init chi@server --remove --script-only
```

### Show Generated Config

```bash
# View cloud-init YAML
tengu-init show cloud-init

# View bash script
tengu-init show bash
```

## Configuration

Credentials are resolved in order: **CLI flags > environment variables > config file > interactive prompt**.

### Config File

Create `~/.config/tengu/init.toml`:

```toml
[server]
name = "tengu"
type = "cax41"        # ARM64, 16 vCPU, 32GB RAM
location = "hel1"     # Helsinki
image = "ubuntu-24.04"
release = "v0.1.0"    # Tengu release tag

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

### Environment Variables

| Variable | Description |
|----------|-------------|
| `CF_API_KEY` | Cloudflare API key |
| `CF_EMAIL` | Cloudflare email |
| `RESEND_API_KEY` | Resend API key |
| `SSH_PUBLIC_KEY` | SSH public key |

### CLI Flags

All config values can be passed as flags:

```bash
tengu-init chi@server \
  --cf-api-key "..." \
  --cf-email "..." \
  --resend-api-key "..." \
  --domain-platform tengu.to \
  --domain-apps tengu.host \
  --ssh-key "ssh-ed25519 AAAA..." \
  --notify-email admin@example.com \
  --release v0.1.0
```

## Server Types

| Type | Arch | vCPU | RAM | Disk | Price |
|------|------|------|-----|------|-------|
| `cax11` | ARM64 | 2 | 4GB | 40GB | ~EUR4/mo |
| `cax21` | ARM64 | 4 | 8GB | 80GB | ~EUR8/mo |
| `cax31` | ARM64 | 8 | 16GB | 160GB | ~EUR15/mo |
| `cax41` | ARM64 | 16 | 32GB | 320GB | ~EUR30/mo |
| `cpx11` | x86 | 2 | 2GB | 40GB | ~EUR5/mo |
| `cpx21` | x86 | 3 | 4GB | 80GB | ~EUR9/mo |
| `cpx31` | x86 | 4 | 8GB | 160GB | ~EUR17/mo |
| `cpx41` | x86 | 8 | 16GB | 240GB | ~EUR32/mo |

ARM servers (cax*) are recommended -- cheaper and Tengu builds for both architectures.

## What Gets Installed

| Component | Description |
|-----------|-------------|
| **Docker** | Container runtime (docker.io) |
| **tengu-caddy** | Caddy with Cloudflare DNS plugin for automatic HTTPS |
| **PostgreSQL 16** | Database with pgvector extension for AI/embeddings |
| **Ollama** | Local LLM inference for RAG features |
| **Tengu** | PaaS server with SSH git endpoint and API |
| **fail2ban** | Intrusion prevention |
| **ufw** | Firewall (ports 22, 80, 443) |

## Project Structure

```
tengu-init/
├── crates/
│   ├── tengu-init/        # CLI binary
│   │   └── templates/     # Tera templates for cloud-init
│   └── tengu-provision/   # Library for provisioning steps
│       └── src/
│           ├── steps/     # Idempotent installation steps
│           ├── render/    # Output renderers (cloud-init, bash)
│           └── manifest.rs
```

## License

MIT
