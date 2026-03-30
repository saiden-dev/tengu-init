//! SSH provider - provision servers via SSH
//!
//! Connects to an existing server via SSH, uploads a bash script,
//! and executes it with real-time progress streaming.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use tengu_provision::{BashRenderer, Manifest, Renderer, TenguConfig};

/// Configuration for Cloudflare Tunnel setup
pub struct TunnelConfig {
    /// The platform domain (e.g., "tengu.to")
    pub domain_platform: String,
    /// The tunnel name (e.g., "tengu")
    pub tunnel_name: String,
}

/// Server provisioning via SSH
pub struct SshProvider {
    /// SSH host
    pub host: String,
    /// SSH user (must have sudo access)
    pub user: String,
    /// SSH port
    pub port: u16,
}

impl SshProvider {
    /// Create a new SSH provider from a host specification
    ///
    /// Host can be:
    /// - `hostname` (uses current username)
    /// - `user@hostname` (extracts user)
    ///
    /// The user must have passwordless sudo access on the target server.
    pub fn new(host: &str, port: u16) -> Self {
        let (user, hostname) = if let Some((u, h)) = host.split_once('@') {
            (u.to_string(), h.to_string())
        } else {
            // Use current username as default
            let current_user = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "chi".to_string());
            (current_user, host.to_string())
        };

        Self {
            host: hostname,
            user,
            port,
        }
    }

    /// Generate the provisioning bash script
    pub fn generate_script(config: &TenguConfig) -> Result<String> {
        let manifest = Manifest::tengu(config);
        let renderer = BashRenderer::new().verbose(true).color(true);
        renderer
            .render(&manifest)
            .map_err(|e| anyhow::anyhow!("Failed to render script: {e:?}"))
    }

    /// Generate a removal script that undoes everything tengu-init installed
    pub fn generate_removal_script() -> String {
        r#"#!/bin/bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

step=0
total=17

progress() {
    step=$((step + 1))
    echo -e "${YELLOW}[$step/$total]${NC} $1"
}

echo ""
echo -e "${RED}╔═══════════════════════════════════════╗${NC}"
echo -e "${RED}║        TENGU REMOVAL IN PROGRESS      ║${NC}"
echo -e "${RED}╚═══════════════════════════════════════╝${NC}"
echo ""

# Get the actual user's home directory (not root's when running with sudo)
ACTUAL_USER="${SUDO_USER:-$USER}"
ACTUAL_HOME=$(getent passwd "$ACTUAL_USER" | cut -d: -f6)

# Phase 0: Cloudflare Tunnel cleanup
progress "Stopping cloudflared service..."
sudo systemctl stop cloudflared 2>/dev/null || true
sudo systemctl disable cloudflared 2>/dev/null || true
sudo cloudflared service uninstall 2>/dev/null || true
sudo rm -rf /etc/cloudflared

progress "Deleting cloudflare tunnel..."
# Run as actual user to access their credentials
sudo -u "$ACTUAL_USER" cloudflared tunnel delete tengu 2>/dev/null || true

progress "Removing cloudflared credentials..."
rm -rf "$ACTUAL_HOME/.cloudflared"

progress "Removing cloudflared package..."
sudo dpkg --purge cloudflared 2>/dev/null || true

# Phase 1: Stop services
progress "Stopping tengu service..."
systemctl stop tengu 2>/dev/null || true
systemctl disable tengu 2>/dev/null || true

progress "Stopping caddy service..."
systemctl stop caddy 2>/dev/null || true
systemctl disable caddy 2>/dev/null || true

progress "Stopping ollama service..."
systemctl stop ollama 2>/dev/null || true
systemctl disable ollama 2>/dev/null || true

progress "Stopping PostgreSQL service..."
systemctl stop postgresql 2>/dev/null || true
systemctl disable postgresql 2>/dev/null || true

progress "Stopping Docker services..."
systemctl stop docker 2>/dev/null || true
systemctl stop docker.socket 2>/dev/null || true
systemctl disable docker 2>/dev/null || true
systemctl disable docker.socket 2>/dev/null || true

progress "Stopping fail2ban service..."
systemctl stop fail2ban 2>/dev/null || true
systemctl disable fail2ban 2>/dev/null || true

# Phase 2: Remove packages
progress "Removing tengu packages..."
dpkg --purge tengu 2>/dev/null || true
dpkg --purge tengu-caddy 2>/dev/null || true

progress "Removing installed packages..."
export DEBIAN_FRONTEND=noninteractive
TENGU_INSTALLED_FILE="/etc/tengu/installed-by-tengu.txt"
if [ -f "$TENGU_INSTALLED_FILE" ]; then
    echo "  Packages to remove: $(cat "$TENGU_INSTALLED_FILE" | tr '\n' ' ')"
    while IFS= read -r pkg || [ -n "$pkg" ]; do
        [ -z "$pkg" ] && continue
        echo "  Removing $pkg..."
        apt-get purge -y "$pkg" 2>/dev/null || true
    done < "$TENGU_INSTALLED_FILE"
else
    echo "  No package tracking file found, removing known packages..."
    apt-get purge -y \
        tengu tengu-caddy ollama cloudflared \
        postgresql-16 postgresql-16-pgvector postgresql-client-16 postgresql-common postgresql-client-common \
        docker.io docker-compose docker-compose-v2 containerd runc \
        fail2ban ufw \
        2>/dev/null || true
fi
apt-get autoremove -y 2>/dev/null || true

# Phase 3: Remove config and data directories
progress "Removing Tengu configuration and data..."
rm -rf /etc/tengu
rm -rf /var/lib/tengu
rm -rf /var/log/tengu
rm -rf /etc/caddy
rm -rf /etc/fail2ban/jail.local
rm -rf /etc/systemd/system/caddy.service.d
# Note: /var/lib/postgresql and /var/lib/docker are left intact
# Removing them would require cluster reinitialization on next provision

# Phase 4: Remove tengu user
progress "Removing tengu user..."
userdel -r tengu 2>/dev/null || true
groupdel tengu 2>/dev/null || true

# Phase 5: Remove APT repositories added during install
progress "Cleaning up APT repositories..."
rm -f /etc/apt/sources.list.d/pgdg.list
rm -f /usr/share/keyrings/postgresql-archive-keyring.gpg
apt-get update -qq 2>/dev/null || true

systemctl daemon-reload

echo ""
echo -e "${GREEN}╔═══════════════════════════════════════╗${NC}"
echo -e "${GREEN}║          REMOVAL COMPLETE             ║${NC}"
echo -e "${GREEN}╚═══════════════════════════════════════╝${NC}"
echo ""
echo -e "All Tengu components have been removed."
echo -e "Base packages (curl, git, etc.) were left intact."
echo ""
"#
        .to_string()
    }

    /// Remove Tengu and all installed dependencies from the server
    pub fn remove(&self) -> Result<()> {
        let script = Self::generate_removal_script();

        // Wait for SSH
        self.wait_for_ssh()?;

        // Upload removal script
        println!(
            "{} Uploading removal script to {}...",
            style("*").cyan(),
            self.ssh_destination()
        );
        self.upload_removal_script(&script)?;

        // Execute
        println!("{} Executing removal...\n", style("*").cyan());
        self.execute_removal()?;

        // Cleanup
        self.cleanup_removal_script()?;

        Ok(())
    }

    /// Upload the removal script
    fn upload_removal_script(&self, script: &str) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push("cat > /tmp/tengu-remove.sh && chmod +x /tmp/tengu-remove.sh".into());

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSH for upload")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .context("Failed to write script to SSH")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to upload removal script")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to upload removal script: {stderr}");
        }

        Ok(())
    }

    /// Execute the removal script with live output
    fn execute_removal(&self) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push("sudo /tmp/tengu-remove.sh".into());

        let mut child = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to execute removal script")?;

        let stdout = child.stdout.take().context("No stdout")?;
        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            let Ok(line) = line else { continue };
            println!("  {line}");
        }

        let status = child.wait().context("Failed to wait for removal script")?;

        if !status.success() {
            bail!("Removal script failed with exit code: {status}");
        }

        Ok(())
    }

    /// Remove the temporary removal script
    #[allow(clippy::unnecessary_wraps)]
    fn cleanup_removal_script(&self) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push("rm -f /tmp/tengu-remove.sh".into());

        let _ = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        Ok(())
    }

    /// Provision the server
    ///
    /// 1. Generate bash script from config
    /// 2. Upload to /tmp/tengu-provision.sh via SSH
    /// 3. Execute with sudo, streaming output
    /// 4. Parse progress markers and display pretty progress
    /// 5. Cleanup temp script
    pub fn provision(&self, config: &TenguConfig) -> Result<()> {
        // Generate script
        println!("\n{} Generating provisioning script...", style("*").cyan());
        let script = Self::generate_script(config)?;

        // Count steps from manifest
        let manifest = Manifest::tengu(config);
        let total_steps = manifest.steps.len();

        // Wait for SSH
        self.wait_for_ssh()?;

        // Upload script
        println!(
            "{} Uploading script to {}...",
            style("*").cyan(),
            self.ssh_destination()
        );
        self.upload_script(&script)?;

        // Execute script
        println!("{} Executing provisioning script...\n", style("*").cyan());
        println!("{}", style("-".repeat(50)).dim());
        self.execute_script(total_steps)?;
        println!("{}", style("-".repeat(50)).dim());

        // Cleanup
        println!("{} Cleaning up...", style("*").cyan());
        self.cleanup_script()?;

        Ok(())
    }

    /// Set up a Cloudflare Tunnel on the remote server
    ///
    /// Steps:
    /// 1. Install cloudflared (from GitHub .deb release)
    /// 2. Upload local cert.pem to remote
    /// 3. Delete any existing tunnel with the given name
    /// 4. Create a new tunnel and capture its ID
    /// 5. Write the tunnel config.yml
    /// 6. Create DNS CNAME routes for subdomains
    /// 7. Install and start cloudflared as a systemd service
    pub fn setup_tunnel(&self, tunnel_config: &TunnelConfig) -> Result<()> {
        let home = format!("/home/{}", self.user);
        let cf_dir = format!("{home}/.cloudflared");

        // Step 1: Install cloudflared
        println!(
            "\n{} Installing cloudflared...",
            style("*").cyan()
        );
        self.run_ssh_command(
            "if command -v cloudflared >/dev/null 2>&1; then \
                echo 'cloudflared already installed'; \
            else \
                ARCH=$(dpkg --print-architecture) && \
                curl -fsSL -o /tmp/cloudflared.deb \
                    \"https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-${ARCH}.deb\" && \
                sudo dpkg -i /tmp/cloudflared.deb && \
                rm -f /tmp/cloudflared.deb; \
            fi",
        )?;
        println!("  {} cloudflared installed", style("v").green());

        // Step 2: Upload cert.pem
        println!(
            "{} Uploading Cloudflare tunnel credentials...",
            style("*").cyan()
        );
        let local_cert = PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".cloudflared")
            .join("cert.pem");
        if !local_cert.exists() {
            bail!(
                "Local cert.pem not found at {}. Run 'cloudflared tunnel login' first.",
                local_cert.display()
            );
        }
        let cert_content = std::fs::read_to_string(&local_cert)
            .context("Failed to read local cert.pem")?;

        // Create remote .cloudflared dir and write cert.pem
        self.run_ssh_command(&format!("mkdir -p {cf_dir}"))?;
        self.upload_file_content(&cert_content, &format!("{cf_dir}/cert.pem"))?;
        println!("  {} cert.pem uploaded", style("v").green());

        // Step 3: Clean up any previous tunnel installation
        println!(
            "{} Configuring tunnel '{}'...",
            style("*").cyan(),
            tunnel_config.tunnel_name
        );
        // Remove old systemd service and /etc/cloudflared if present
        self.run_ssh_command(
            "sudo systemctl stop cloudflared 2>/dev/null || true; \
             sudo systemctl disable cloudflared 2>/dev/null || true; \
             sudo cloudflared service uninstall 2>/dev/null || true; \
             sudo rm -rf /etc/cloudflared",
        )?;
        self.run_ssh_command(&format!(
            "cloudflared tunnel cleanup {} 2>/dev/null || true; \
             cloudflared tunnel delete {} 2>/dev/null || true",
            tunnel_config.tunnel_name, tunnel_config.tunnel_name
        ))?;

        // Step 4: Create tunnel and capture ID
        let create_output = self.run_ssh_command_output(&format!(
            "cloudflared tunnel create {}",
            tunnel_config.tunnel_name
        ))?;

        let tunnel_id = parse_tunnel_id(&create_output)
            .context("Failed to parse tunnel ID from cloudflared output")?;
        println!("  {} Tunnel created (ID: {})", style("v").green(), &tunnel_id[..8]);

        // Step 5: Write config.yml
        let config_yml = format!(
            "tunnel: {tunnel_id}\n\
             credentials-file: {cf_dir}/{tunnel_id}.json\n\
             \n\
             ingress:\n\
             \x20 - hostname: api.{domain}\n\
             \x20   service: http://localhost:8080\n\
             \x20 - hostname: docs.{domain}\n\
             \x20   service: http://localhost:8080\n\
             \x20 - hostname: git.{domain}\n\
             \x20   service: http://localhost:8080\n\
             \x20 - hostname: ssh.{domain}\n\
             \x20   service: ssh://localhost:2222\n\
             \x20 - service: http_status:404\n",
            domain = tunnel_config.domain_platform,
        );
        self.upload_file_content(&config_yml, &format!("{cf_dir}/config.yml"))?;
        println!("  {} config.yml written", style("v").green());

        // Step 6: Create DNS routes (delete stale records first)
        println!(
            "{} Creating DNS routes...",
            style("*").cyan()
        );
        for subdomain in &["api", "docs", "git", "ssh"] {
            let hostname = format!("{subdomain}.{}", tunnel_config.domain_platform);
            self.run_ssh_command(&format!(
                "cloudflared tunnel route dns --overwrite-dns {} {}",
                tunnel_config.tunnel_name, hostname
            ))?;
            println!("  {} {}", style("v").green(), hostname);
        }

        // Step 7: Install systemd service and start
        println!(
            "{} Installing cloudflared service...",
            style("*").cyan()
        );
        self.run_ssh_command(&format!(
            "sudo cloudflared --config {cf_dir}/config.yml service install && \
             sudo systemctl enable --now cloudflared"
        ))?;
        println!("  {} Cloudflare Tunnel ready", style("v").green());

        Ok(())
    }

    /// Run a command on the remote server via SSH (discard output)
    fn run_ssh_command(&self, command: &str) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push(command.to_string());

        let output = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to execute SSH command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!("SSH command failed: {command}\nstdout: {stdout}\nstderr: {stderr}");
        }

        Ok(())
    }

    /// Run a command on the remote server via SSH and return stdout
    fn run_ssh_command_output(&self, command: &str) -> Result<String> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push(command.to_string());

        let output = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to execute SSH command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!("SSH command failed: {command}\nstdout: {stdout}\nstderr: {stderr}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Upload file content to a remote path via SSH stdin
    fn upload_file_content(&self, content: &str, remote_path: &str) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push(format!("cat > {remote_path}"));

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSH for file upload")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(content.as_bytes())
                .context("Failed to write file content to SSH")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to upload file")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to upload to {remote_path}: {stderr}");
        }

        Ok(())
    }

    /// SSH destination string (user@host)
    fn ssh_destination(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// SSH command arguments (common options)
    fn ssh_args(&self) -> Vec<String> {
        vec![
            "-o".into(),
            "StrictHostKeyChecking=no".into(),
            "-o".into(),
            "UserKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "LogLevel=ERROR".into(),
            "-p".into(),
            self.port.to_string(),
        ]
    }

    /// Wait for SSH to become available
    fn wait_for_ssh(&self) -> Result<()> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .unwrap(),
        );
        spinner.set_message(format!("Connecting to {}:{}...", self.host, self.port));
        spinner.enable_steady_tick(Duration::from_millis(100));

        let mut attempts = 0;
        let max_attempts = 30;

        loop {
            let mut args = self.ssh_args();
            args.extend([
                "-o".into(),
                "ConnectTimeout=5".into(),
                "-o".into(),
                "BatchMode=yes".into(),
                self.ssh_destination(),
                "true".into(),
            ]);

            let status = Command::new("ssh")
                .args(&args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            if status.map(|s| s.success()).unwrap_or(false) {
                break;
            }

            attempts += 1;
            if attempts >= max_attempts {
                spinner.finish_with_message(format!(
                    "{} Failed to connect after {} attempts",
                    style("x").red(),
                    max_attempts
                ));
                bail!("Could not connect to {}:{} via SSH", self.host, self.port);
            }

            std::thread::sleep(Duration::from_secs(2));
        }

        spinner.finish_with_message(format!("{} SSH connection established", style("v").green()));
        Ok(())
    }

    /// Upload script to remote server
    fn upload_script(&self, script: &str) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push("cat > /tmp/tengu-provision.sh && chmod +x /tmp/tengu-provision.sh".into());

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start SSH for upload")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .context("Failed to write script to SSH")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to upload script")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to upload script: {stderr}");
        }

        Ok(())
    }

    /// Execute script and stream progress
    fn execute_script(&self, total_steps: usize) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        // Redirect stderr to /dev/null on remote — we parse progress from stdout markers.
        // Without this, stderr fills the pipe buffer and deadlocks the SSH process.
        args.push("sudo /tmp/tengu-provision.sh 2>/tmp/tengu-provision.err".into());

        let mut child = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Failed to execute script")?;

        let stdout = child.stdout.take().context("No stdout")?;
        let reader = BufReader::new(stdout);

        // Track current step for spinner
        let mut current_spinner: Option<ProgressBar> = None;

        for line in reader.lines() {
            let Ok(line) = line else { continue };

            // Parse progress markers
            if let Some(marker) = parse_progress_marker(&line) {
                match marker {
                    ProgressMarker::Start { step, desc } => {
                        // Finish previous spinner if any
                        if let Some(spinner) = current_spinner.take() {
                            spinner.finish_and_clear();
                        }

                        // Start new spinner
                        let spinner = ProgressBar::new_spinner();
                        spinner.set_style(
                            ProgressStyle::default_spinner()
                                .template(&format!(
                                    "{{spinner:.cyan}} [{step}/{total_steps}] {{msg}}"
                                ))
                                .unwrap(),
                        );
                        spinner.set_message(desc);
                        spinner.enable_steady_tick(Duration::from_millis(100));
                        current_spinner = Some(spinner);
                    }
                    ProgressMarker::Done { step, desc } => {
                        if let Some(spinner) = current_spinner.take() {
                            spinner.finish_and_clear();
                        }
                        println!("[{}/{}] {} {}", step, total_steps, style("v").green(), desc);
                    }
                    ProgressMarker::Skip { step, desc } => {
                        if let Some(spinner) = current_spinner.take() {
                            spinner.finish_and_clear();
                        }
                        println!(
                            "[{}/{}] {} {} {}",
                            step,
                            total_steps,
                            style("o").yellow(),
                            desc,
                            style("(skipped)").dim()
                        );
                    }
                    ProgressMarker::Fail { step, desc } => {
                        if let Some(spinner) = current_spinner.take() {
                            spinner.finish_and_clear();
                        }
                        println!("[{}/{}] {} {}", step, total_steps, style("x").red(), desc);
                    }
                    ProgressMarker::Complete { .. } => {
                        if let Some(spinner) = current_spinner.take() {
                            spinner.finish_and_clear();
                        }
                    }
                }
            }
            // Skip non-marker lines (we show progress via markers)
        }

        // Clean up any remaining spinner
        if let Some(spinner) = current_spinner.take() {
            spinner.finish_and_clear();
        }

        let status = child.wait().context("Failed to wait for script")?;

        if !status.success() {
            bail!("Provisioning script failed with exit code: {status}");
        }

        Ok(())
    }

    /// Remove the temporary script
    fn cleanup_script(&self) -> Result<()> {
        let mut args = self.ssh_args();
        args.push(self.ssh_destination());
        args.push("rm -f /tmp/tengu-provision.sh".into());

        let status = Command::new("ssh")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to cleanup script")?;

        if !status.success() {
            // Non-fatal, just warn
            eprintln!(
                "{} Warning: Could not remove temp script",
                style("!").yellow()
            );
        }

        Ok(())
    }
}

/// Progress marker types
enum ProgressMarker {
    Start { step: usize, desc: String },
    Done { step: usize, desc: String },
    Skip { step: usize, desc: String },
    Fail { step: usize, desc: String },
    Complete { _total: usize },
}

/// Parse a progress marker from a line
///
/// Format: `TENGU_STEP:ACTION:step_num:description`
fn parse_progress_marker(line: &str) -> Option<ProgressMarker> {
    let line = line.trim();

    // Strip ANSI escape codes for parsing
    let clean = strip_ansi_codes(line);

    if !clean.starts_with("TENGU_STEP:") {
        return None;
    }

    let parts: Vec<&str> = clean.splitn(4, ':').collect();
    if parts.len() < 3 {
        return None;
    }

    let action = parts[1];
    let step: usize = parts[2].parse().ok()?;
    let desc = parts.get(3).unwrap_or(&"").to_string();

    match action {
        "START" => Some(ProgressMarker::Start { step, desc }),
        "DONE" => Some(ProgressMarker::Done { step, desc }),
        "SKIP" => Some(ProgressMarker::Skip { step, desc }),
        "FAIL" => Some(ProgressMarker::Fail { step, desc }),
        "COMPLETE" => Some(ProgressMarker::Complete { _total: step }),
        _ => None,
    }
}

/// Parse tunnel ID (UUID) from `cloudflared tunnel create` output
///
/// The output contains a line like: "Created tunnel tengu with id abcdef12-3456-7890-abcd-ef1234567890"
fn parse_tunnel_id(output: &str) -> Option<String> {
    let uuid_re = regex::Regex::new(
        r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    )
    .ok()?;

    uuid_re.find(output).map(|m| m.as_str().to_string())
}

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip escape sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Skip until we hit a letter (end of sequence)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}
