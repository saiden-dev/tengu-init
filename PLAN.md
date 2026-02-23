# PLAN.md — tengu-provision Implementation Plan

## Overview

Add baremetal server provisioning to `tengu-init` by extracting installation logic into a reusable `tengu-provision` crate. This crate will model installation steps as Rust types that can render to either cloud-init YAML (for cloud providers) or idempotent shell scripts (for SSH execution on baremetal).

## Goals

1. **Extract**: Move installation logic from Tera templates to Rust structs
2. **Idempotent**: Every step handles re-runs gracefully
3. **Multi-target**: Render to cloud-init YAML or bash script
4. **Provider agnostic**: Support Hetzner Cloud (existing), Baremetal (new), and future providers

---

## Phase 1 "Zaku II": Project Structure

### Workspace Layout

Convert to a Cargo workspace with two crates:

```
tengu-init/
├── Cargo.toml              # Workspace root
├── PLAN.md
├── README.md
├── crates/
│   ├── tengu-init/         # CLI binary (existing code, refactored)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── cli.rs      # CLI args/commands
│   │       └── providers/
│   │           ├── mod.rs
│   │           ├── hetzner.rs
│   │           └── baremetal.rs
│   │
│   └── tengu-provision/    # Library crate (new)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── steps/      # Installation step definitions
│           │   ├── mod.rs
│           │   ├── package.rs
│           │   ├── user.rs
│           │   ├── file.rs
│           │   ├── service.rs
│           │   ├── firewall.rs
│           │   └── command.rs
│           ├── render/     # Output renderers
│           │   ├── mod.rs
│           │   ├── cloud_init.rs
│           │   └── bash.rs
│           ├── config.rs   # Configuration types
│           └── manifest.rs # Full installation manifest
```

### Workspace Cargo.toml

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
edition = "2024"
rust-version = "1.93"
license = "MIT"
authors = ["Chi <chi@localhost>"]
repository = "https://github.com/saiden-dev/tengu-init"

[workspace.dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
```

---

## Phase 2 "Gouf": Core Types and Traits

### Step Trait (`tengu-provision/src/steps/mod.rs`)

Every installation step implements this trait:

```rust
/// Result of running a step
pub enum StepResult {
    /// Step executed successfully
    Applied,
    /// Step was already satisfied, skipped
    Skipped,
    /// Step failed
    Failed(String),
}

/// A single installation step
pub trait Step: Send + Sync {
    /// Human-readable description
    fn description(&self) -> &str;

    /// Render as cloud-init YAML fragment
    fn to_cloud_init(&self) -> CloudInitFragment;

    /// Render as idempotent bash commands
    fn to_bash(&self) -> Vec<String>;

    /// Check if step is already satisfied (for idempotency)
    fn check_command(&self) -> Option<String>;
}

/// Fragment that can be merged into a cloud-init config
pub struct CloudInitFragment {
    pub packages: Vec<String>,
    pub write_files: Vec<WriteFile>,
    pub runcmd: Vec<String>,
}

pub struct WriteFile {
    pub path: String,
    pub content: String,
    pub permissions: Option<String>,
    pub owner: Option<String>,
}
```

### Render Trait (`tengu-provision/src/render/mod.rs`)

```rust
pub trait Renderer {
    type Output;

    fn render(&self, manifest: &Manifest) -> Result<Self::Output>;
}

pub struct CloudInitRenderer;
pub struct BashRenderer;
```

---

## Phase 3 "Dom": Step Implementations

### Package Installation (`steps/package.rs`)

```rust
pub struct InstallPackage {
    pub name: String,
    pub repository: Option<Repository>,
}

pub struct Repository {
    pub key_url: String,
    pub repo_line: String,
    pub keyring_path: String,
}

impl Step for InstallPackage {
    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![];

        // Add repo if specified
        if let Some(repo) = &self.repository {
            cmds.push(format!(
                "if [ ! -f {} ]; then \
                    curl -fsSL {} | gpg --dearmor -o {}; \
                fi",
                repo.keyring_path, repo.key_url, repo.keyring_path
            ));
            cmds.push(format!(
                "if ! grep -q '{}' /etc/apt/sources.list.d/*.list 2>/dev/null; then \
                    echo '{}' > /etc/apt/sources.list.d/{}.list; \
                    apt-get update; \
                fi",
                repo.repo_line, repo.repo_line, self.name
            ));
        }

        // Idempotent install
        cmds.push(format!(
            "dpkg -s {} >/dev/null 2>&1 || apt-get install -y {}",
            self.name, self.name
        ));

        cmds
    }

    fn check_command(&self) -> Option<String> {
        Some(format!("dpkg -s {} >/dev/null 2>&1", self.name))
    }
}
```

### User Management (`steps/user.rs`)

```rust
pub struct EnsureUser {
    pub name: String,
    pub groups: Vec<String>,
    pub shell: String,
    pub sudo: Option<String>,
    pub ssh_keys: Vec<String>,
}

impl Step for EnsureUser {
    fn to_bash(&self) -> Vec<String> {
        vec![
            // Check if user exists
            format!(
                "id {} >/dev/null 2>&1 || useradd -m -s {} {}",
                self.name, self.shell, self.name
            ),
            // Ensure groups
            format!(
                "for g in {}; do \
                    getent group $g >/dev/null && usermod -aG $g {} 2>/dev/null; \
                done",
                self.groups.join(" "), self.name
            ),
            // Ensure sudoers
            if let Some(sudo) = &self.sudo {
                format!(
                    "echo '{} {}' > /etc/sudoers.d/{} && chmod 440 /etc/sudoers.d/{}",
                    self.name, sudo, self.name, self.name
                )
            } else {
                String::new()
            },
            // Ensure SSH keys
            format!(
                "mkdir -p /home/{}/.ssh && chmod 700 /home/{}/.ssh",
                self.name, self.name
            ),
            // Idempotent key addition
            self.ssh_keys.iter().map(|key| {
                format!(
                    "grep -qF '{}' /home/{}/.ssh/authorized_keys 2>/dev/null || \
                     echo '{}' >> /home/{}/.ssh/authorized_keys",
                    key, self.name, key, self.name
                )
            }).collect::<Vec<_>>().join("; "),
        ].into_iter().filter(|s| !s.is_empty()).collect()
    }

    fn check_command(&self) -> Option<String> {
        Some(format!("id {} >/dev/null 2>&1", self.name))
    }
}
```

### File Management (`steps/file.rs`)

```rust
pub struct WriteFileStep {
    pub path: String,
    pub content: String,
    pub permissions: Option<String>,
    pub owner: Option<String>,
}

impl Step for WriteFileStep {
    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![];

        // Create parent directory
        cmds.push(format!(
            "mkdir -p $(dirname {})",
            self.path
        ));

        // Compute checksum and compare (idempotent)
        // Only write if content differs
        let content_escaped = self.content.replace("'", "'\\''");
        cmds.push(format!(
            "EXPECTED=$(echo -n '{}' | sha256sum | cut -d' ' -f1); \
             CURRENT=$(sha256sum {} 2>/dev/null | cut -d' ' -f1 || echo 'none'); \
             if [ \"$EXPECTED\" != \"$CURRENT\" ]; then \
                 echo '{}' > {}; \
             fi",
            content_escaped, self.path, content_escaped, self.path
        ));

        if let Some(perms) = &self.permissions {
            cmds.push(format!("chmod {} {}", perms, self.path));
        }

        if let Some(owner) = &self.owner {
            cmds.push(format!("chown {} {}", owner, self.path));
        }

        cmds
    }

    fn check_command(&self) -> Option<String> {
        Some(format!("[ -f {} ]", self.path))
    }
}
```

### Service Management (`steps/service.rs`)

```rust
pub struct EnsureService {
    pub name: String,
    pub enabled: bool,
    pub started: bool,
}

impl Step for EnsureService {
    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![];

        if self.enabled {
            cmds.push(format!(
                "systemctl is-enabled {} >/dev/null 2>&1 || systemctl enable {}",
                self.name, self.name
            ));
        }

        if self.started {
            cmds.push(format!(
                "systemctl is-active {} >/dev/null 2>&1 || systemctl start {}",
                self.name, self.name
            ));
        }

        cmds
    }

    fn check_command(&self) -> Option<String> {
        if self.started {
            Some(format!("systemctl is-active {} >/dev/null 2>&1", self.name))
        } else {
            None
        }
    }
}
```

### Firewall Rules (`steps/firewall.rs`)

```rust
pub struct UfwRule {
    pub allow: String,  // e.g., "22/tcp", "80/tcp"
}

pub struct EnsureFirewall {
    pub rules: Vec<UfwRule>,
    pub default_incoming: &'static str,  // "deny" or "allow"
    pub default_outgoing: &'static str,
}

impl Step for EnsureFirewall {
    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![
            format!("ufw default {} incoming", self.default_incoming),
            format!("ufw default {} outgoing", self.default_outgoing),
        ];

        for rule in &self.rules {
            // Idempotent: ufw allow is already idempotent
            cmds.push(format!("ufw allow {}", rule.allow));
        }

        // Enable if not already
        cmds.push("ufw status | grep -q 'Status: active' || ufw --force enable".to_string());

        cmds
    }

    fn check_command(&self) -> Option<String> {
        Some("ufw status | grep -q 'Status: active'".to_string())
    }
}
```

### Generic Command (`steps/command.rs`)

For one-off commands that need idempotency guards:

```rust
pub struct RunCommand {
    pub description: String,
    pub command: String,
    /// If this command succeeds, skip running `command`
    pub unless: Option<String>,
}

impl Step for RunCommand {
    fn to_bash(&self) -> Vec<String> {
        if let Some(unless) = &self.unless {
            vec![format!("{} || {{ {}; }}", unless, self.command)]
        } else {
            vec![self.command.clone()]
        }
    }

    fn check_command(&self) -> Option<String> {
        self.unless.clone()
    }
}
```

---

## Phase 4 "Gelgoog": Installation Manifest

### Manifest Structure (`tengu-provision/src/manifest.rs`)

```rust
use crate::steps::*;

/// Complete Tengu installation manifest
pub struct Manifest {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub steps: Vec<Box<dyn Step>>,
}

impl Manifest {
    /// Create the standard Tengu installation manifest
    pub fn tengu(config: &TenguConfig) -> Self {
        let mut steps: Vec<Box<dyn Step>> = vec![];

        // 1. System user
        steps.push(Box::new(EnsureUser {
            name: config.user.clone(),
            groups: vec!["sudo".into(), "docker".into()],
            shell: "/bin/bash".into(),
            sudo: Some("ALL=(ALL) NOPASSWD:ALL".into()),
            ssh_keys: config.ssh_keys.clone(),
        }));

        // 2. Base packages
        for pkg in &["curl", "wget", "git", "jq", "htop", "vim", "fail2ban", "ufw"] {
            steps.push(Box::new(InstallPackage {
                name: pkg.to_string(),
                repository: None,
            }));
        }

        // 3. Docker (from official repo)
        steps.push(Box::new(InstallPackage {
            name: "docker-ce".into(),
            repository: Some(Repository::docker()),
        }));
        steps.push(Box::new(InstallPackage {
            name: "docker-ce-cli".into(),
            repository: None,  // Already added
        }));
        steps.push(Box::new(InstallPackage {
            name: "containerd.io".into(),
            repository: None,
        }));
        steps.push(Box::new(InstallPackage {
            name: "docker-compose-plugin".into(),
            repository: None,
        }));

        // 4. PostgreSQL 18 + pgvector
        steps.push(Box::new(InstallPackage {
            name: "postgresql-18".into(),
            repository: Some(Repository::postgresql()),
        }));
        steps.push(Box::new(InstallPackage {
            name: "postgresql-18-pgvector".into(),
            repository: None,
        }));

        // 5. Ollama
        steps.push(Box::new(RunCommand {
            description: "Install Ollama".into(),
            command: "curl -fsSL https://ollama.com/install.sh | sh".into(),
            unless: Some("command -v ollama >/dev/null 2>&1".into()),
        }));

        // 6. tengu-caddy
        steps.push(Box::new(InstallDebFromUrl {
            name: "tengu-caddy".into(),
            url_template: "https://github.com/saiden-dev/tengu-caddy/releases/latest/download/tengu-caddy_2.10.2-1_{arch}.deb".into(),
            check_command: "dpkg -s tengu-caddy >/dev/null 2>&1".into(),
        }));

        // 7. Configuration files
        steps.push(Box::new(WriteFileStep {
            path: "/etc/fail2ban/jail.local".into(),
            content: config.fail2ban_config(),
            permissions: Some("0644".into()),
            owner: None,
        }));

        steps.push(Box::new(WriteFileStep {
            path: "/etc/tengu/config.toml".into(),
            content: config.tengu_config_toml(),
            permissions: Some("0640".into()),
            owner: Some("root:tengu".into()),
        }));

        steps.push(Box::new(WriteFileStep {
            path: "/etc/caddy/Caddyfile".into(),
            content: config.caddyfile(),
            permissions: Some("0644".into()),
            owner: None,
        }));

        // 8. Caddy Cloudflare env
        steps.push(Box::new(EnsureDirectory {
            path: "/etc/systemd/system/caddy.service.d".into(),
        }));
        steps.push(Box::new(WriteFileStep {
            path: "/etc/systemd/system/caddy.service.d/cloudflare.conf".into(),
            content: format!("[Service]\nEnvironment=\"CF_API_TOKEN={}\"", config.cf_api_key),
            permissions: Some("0640".into()),
            owner: None,
        }));

        // 9. Services
        for svc in &["docker", "caddy", "postgresql", "fail2ban", "ollama"] {
            steps.push(Box::new(EnsureService {
                name: svc.to_string(),
                enabled: true,
                started: true,
            }));
        }

        // 10. Firewall
        steps.push(Box::new(EnsureFirewall {
            rules: vec![
                UfwRule { allow: "22/tcp".into() },
                UfwRule { allow: "80/tcp".into() },
                UfwRule { allow: "443/tcp".into() },
            ],
            default_incoming: "deny",
            default_outgoing: "allow",
        }));

        // 11. Tengu package
        steps.push(Box::new(InstallDebFromUrl {
            name: "tengu".into(),
            url_template: format!(
                "https://github.com/saiden-dev/tengu-deb/releases/download/{}/tengu_0.1.0-1_{{arch}}.deb",
                config.release
            ),
            check_command: format!(
                "dpkg -s tengu >/dev/null 2>&1 && dpkg-query -W -f='${{Version}}' tengu | grep -q '{}'",
                config.release
            ),
        }));

        steps.push(Box::new(EnsureService {
            name: "tengu".into(),
            enabled: true,
            started: true,
        }));

        // 12. Post-install commands
        steps.push(Box::new(RunCommand {
            description: "Configure Cloudflare DNS".into(),
            command: "tengu cloudflare config".into(),
            unless: None,  // Always run to ensure DNS is current
        }));

        steps.push(Box::new(RunCommand {
            description: "Create admin user".into(),
            command: format!(
                "tengu user list --json | jq -e '.[] | select(.name == \"{}\")' >/dev/null || \
                 tengu user add {} --key '{}' --admin",
                config.user, config.user, config.ssh_keys.first().unwrap_or(&String::new())
            ),
            unless: Some(format!(
                "tengu user list --json | jq -e '.[] | select(.name == \"{}\")' >/dev/null 2>&1",
                config.user
            )),
        }));

        Self {
            hostname: "tengu".into(),
            timezone: "Europe/Warsaw".into(),
            locale: "en_US.UTF-8".into(),
            steps,
        }
    }
}
```

---

## Phase 5 "Rick Dom": Renderers

### Cloud-Init Renderer (`render/cloud_init.rs`)

```rust
use serde::Serialize;

#[derive(Serialize)]
struct CloudInitConfig {
    hostname: String,
    fqdn: String,
    timezone: String,
    locale: String,
    users: Vec<CloudInitUser>,
    ssh_pwauth: bool,
    disable_root: bool,
    package_update: bool,
    package_upgrade: bool,
    packages: Vec<String>,
    write_files: Vec<CloudInitFile>,
    runcmd: Vec<String>,
    final_message: String,
}

impl CloudInitRenderer {
    pub fn render(&self, manifest: &Manifest, config: &TenguConfig) -> Result<String> {
        let mut packages = vec![];
        let mut write_files = vec![];
        let mut runcmd = vec![];

        for step in &manifest.steps {
            let fragment = step.to_cloud_init();
            packages.extend(fragment.packages);
            write_files.extend(fragment.write_files);
            runcmd.extend(fragment.runcmd);
        }

        let config = CloudInitConfig {
            hostname: manifest.hostname.clone(),
            fqdn: config.domain_platform.clone(),
            timezone: manifest.timezone.clone(),
            locale: manifest.locale.clone(),
            users: vec![/* ... */],
            ssh_pwauth: false,
            disable_root: true,
            package_update: true,
            package_upgrade: true,
            packages,
            write_files: write_files.into_iter().map(|f| f.into()).collect(),
            runcmd,
            final_message: "Tengu PaaS server ready!".into(),
        };

        // Render as YAML with #cloud-config header
        let yaml = serde_yaml::to_string(&config)?;
        Ok(format!("#cloud-config\n{}", yaml))
    }
}
```

### Bash Renderer (`render/bash.rs`)

```rust
pub struct BashRenderer {
    /// Include progress output
    pub verbose: bool,
}

impl BashRenderer {
    pub fn render(&self, manifest: &Manifest) -> Result<String> {
        let mut script = String::new();

        script.push_str("#!/bin/bash\n");
        script.push_str("# Tengu PaaS Installation Script\n");
        script.push_str("# Generated by tengu-provision\n");
        script.push_str("# Idempotent - safe to re-run\n\n");
        script.push_str("set -euo pipefail\n\n");

        // Color codes for output
        if self.verbose {
            script.push_str(r#"
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_step() { echo -e "${GREEN}[STEP]${NC} $1"; }
log_skip() { echo -e "${YELLOW}[SKIP]${NC} $1 (already done)"; }
"#);
        }

        for (i, step) in manifest.steps.iter().enumerate() {
            let desc = step.description();

            if self.verbose {
                script.push_str(&format!("\n# Step {}: {}\n", i + 1, desc));
            }

            // Wrap in idempotency check if available
            if let Some(check) = step.check_command() {
                script.push_str(&format!(
                    "if {}; then\n",
                    check
                ));
                if self.verbose {
                    script.push_str(&format!("  log_skip \"{}\"\n", desc));
                }
                script.push_str("else\n");
                if self.verbose {
                    script.push_str(&format!("  log_step \"{}\"\n", desc));
                }
                for cmd in step.to_bash() {
                    script.push_str(&format!("  {}\n", cmd));
                }
                script.push_str("fi\n");
            } else {
                if self.verbose {
                    script.push_str(&format!("log_step \"{}\"\n", desc));
                }
                for cmd in step.to_bash() {
                    script.push_str(&format!("{}\n", cmd));
                }
            }
        }

        script.push_str("\necho 'Tengu PaaS installation complete!'\n");

        Ok(script)
    }
}
```

---

## Phase 6 "Hyaku Shiki": Baremetal Provider

### SSH Executor (`tengu-init/src/providers/baremetal.rs`)

```rust
use std::process::{Command, Stdio};
use tengu_provision::{Manifest, BashRenderer};

pub struct Baremetal {
    pub host: String,
    pub user: String,
    pub port: u16,
}

impl Baremetal {
    pub fn provision(&self, config: &TenguConfig) -> Result<()> {
        // Generate manifest
        let manifest = Manifest::tengu(config);

        // Render to bash
        let renderer = BashRenderer { verbose: true };
        let script = renderer.render(&manifest)?;

        // Execute via SSH
        self.execute_script(&script)?;

        Ok(())
    }

    fn execute_script(&self, script: &str) -> Result<()> {
        // Upload script to temp file
        let remote_path = "/tmp/tengu-provision.sh";

        // Copy script
        let mut child = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=accept-new",
                "-p", &self.port.to_string(),
                &format!("{}@{}", self.user, self.host),
                &format!("cat > {} && chmod +x {}", remote_path, remote_path),
            ])
            .stdin(Stdio::piped())
            .spawn()?;

        child.stdin.as_mut().unwrap().write_all(script.as_bytes())?;
        child.wait()?;

        // Execute with streaming output
        let status = Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=accept-new",
                "-p", &self.port.to_string(),
                "-t",  // Allocate PTY for colors
                &format!("{}@{}", self.user, self.host),
                &format!("sudo {}", remote_path),
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;

        if !status.success() {
            bail!("Provisioning failed");
        }

        // Cleanup
        Command::new("ssh")
            .args([
                "-p", &self.port.to_string(),
                &format!("{}@{}", self.user, self.host),
                &format!("rm -f {}", remote_path),
            ])
            .status()?;

        Ok(())
    }
}
```

---

## Phase 7 "Quebeley": CLI Updates

### New CLI Structure

```rust
#[derive(Parser)]
#[command(name = "tengu-init")]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,

    // Existing flags (for backwards compat with direct provisioning)
    #[arg(long)]
    dry_run: bool,
    // ...
}

#[derive(Subcommand)]
enum Commands {
    /// Provision on Hetzner Cloud (default, existing behavior)
    Hetzner {
        #[command(flatten)]
        server: ServerArgs,
    },

    /// Provision on existing baremetal server via SSH
    Baremetal {
        /// SSH destination (user@host). User must have passwordless sudo
        #[arg()]
        host: String,

        /// SSH port
        #[arg(short, long, default_value = "22")]
        port: u16,

        /// Generate script only, don't execute
        #[arg(long)]
        script_only: bool,

        #[command(flatten)]
        config: ConfigArgs,
    },

    /// Show generated cloud-init or bash script
    Show {
        /// Output format
        #[arg(value_enum)]
        format: OutputFormat,

        #[command(flatten)]
        config: ConfigArgs,
    },
}

#[derive(ValueEnum, Clone)]
enum OutputFormat {
    CloudInit,
    Bash,
}
```

### Example Usage

```bash
# Existing behavior (Hetzner Cloud)
tengu-init
tengu-init --dry-run

# New: Baremetal provisioning
tengu-init baremetal chi@192.168.1.100
tengu-init baremetal chi@my-server.com --port 2222

# New: Generate script without executing
tengu-init baremetal chi@server --script-only > provision.sh

# New: Just show the generated config
tengu-init show cloud-init
tengu-init show bash
```

---

## Phase 8 "Zeta Gundam": Idempotency Summary

Every step type has a clear idempotency strategy:

| Step Type | Idempotency Check |
|-----------|-------------------|
| `InstallPackage` | `dpkg -s PKG` |
| `EnsureUser` | `id USER` |
| `WriteFileStep` | SHA256 checksum comparison |
| `EnsureService` | `systemctl is-active`/`is-enabled` |
| `EnsureFirewall` | `ufw status | grep active` |
| `RunCommand` | Custom `unless` check |
| `InstallDebFromUrl` | `dpkg -s PKG` + version check |
| `EnsureDirectory` | `[ -d PATH ]` |

### Verification Script

The `BashRenderer` can generate a verify-only mode:

```rust
impl BashRenderer {
    pub fn render_verify(&self, manifest: &Manifest) -> Result<String> {
        let mut script = String::new();
        script.push_str("#!/bin/bash\n# Tengu Installation Verification\n\n");
        script.push_str("PASS=0; FAIL=0\n\n");

        for step in &manifest.steps {
            if let Some(check) = step.check_command() {
                script.push_str(&format!(
                    "if {}; then ((PASS++)); else echo 'FAIL: {}'; ((FAIL++)); fi\n",
                    check, step.description()
                ));
            }
        }

        script.push_str("\necho \"Passed: $PASS, Failed: $FAIL\"\n");
        script.push_str("[ $FAIL -eq 0 ]\n");

        Ok(script)
    }
}
```

---

## Phase 9 "Nu Gundam": Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_install_is_idempotent() {
        let step = InstallPackage {
            name: "curl".into(),
            repository: None,
        };

        let bash = step.to_bash();
        assert!(bash[0].contains("dpkg -s curl"));
        assert!(bash[0].contains("|| apt-get install"));
    }

    #[test]
    fn user_creation_is_idempotent() {
        let step = EnsureUser {
            name: "chi".into(),
            groups: vec!["sudo".into()],
            shell: "/bin/bash".into(),
            sudo: None,
            ssh_keys: vec![],
        };

        let bash = step.to_bash();
        assert!(bash[0].contains("id chi"));
        assert!(bash[0].contains("|| useradd"));
    }

    #[test]
    fn manifest_renders_to_cloud_init() {
        let config = TenguConfig::test_config();
        let manifest = Manifest::tengu(&config);
        let renderer = CloudInitRenderer;
        let yaml = renderer.render(&manifest, &config).unwrap();

        assert!(yaml.starts_with("#cloud-config"));
        assert!(yaml.contains("packages:"));
    }
}
```

### Integration Tests

```rust
// tests/integration.rs
// Requires Docker for running tests in isolated Ubuntu container

#[test]
#[ignore]  // Run with --ignored
fn baremetal_script_is_idempotent() {
    // 1. Start Ubuntu container
    // 2. Run provisioning script
    // 3. Verify all services running
    // 4. Run script again
    // 5. Verify no changes (all steps skipped)
}
```

---

## Migration Path

1. **Phase 1-2**: Set up workspace, define traits (no behavior change)
2. **Phase 3-4**: Implement step types and manifest
3. **Phase 5**: Implement renderers, verify cloud-init output matches current template
4. **Phase 6**: Add baremetal provider
5. **Phase 7**: Update CLI with subcommands
6. **Phase 8**: Add verification tooling
7. **Phase 9**: Add integration tests

### Backwards Compatibility

- Running `tengu-init` without subcommand maintains current Hetzner behavior
- All existing CLI flags continue to work
- Generated cloud-init should be byte-for-byte compatible with current template

---

## Open Questions

1. **SSH key management for baremetal**: Should we require the user's SSH key to already be on the server, or support password auth for initial setup?

2. **Partial re-runs**: Should we support `--step=N` to re-run from a specific step?

3. **Rollback**: Should steps implement a `rollback()` method for cleanup on failure?

4. **Progress reporting**: For baremetal, should we parse SSH output to show a progress bar, or just stream raw output?

---

## Dependencies to Add

```toml
# tengu-provision/Cargo.toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
sha2 = "0.10"  # For content checksums
thiserror = "2"
```

```toml
# tengu-init/Cargo.toml (additional)
[dependencies]
tengu-provision = { path = "../tengu-provision" }
```
