//! Hetzner Cloud provider
//!
//! Currently uses the `hcloud` CLI. Requires:
//! ```sh
//! brew install hcloud
//! hcloud context create tengu
//! ```

use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use indicatif::{ProgressBar, ProgressStyle};

/// Server creation parameters
pub struct ServerParams<'a> {
    pub name: &'a str,
    pub server_type: &'a str,
    pub image: &'a str,
    pub location: &'a str,
    pub ssh_key_name: &'a str,
}

/// Hetzner Cloud provider (via hcloud CLI)
pub struct Hetzner;

impl Hetzner {
    /// Get server type info (cores, RAM, architecture)
    pub fn server_type_info(server_type: &str) -> Result<String> {
        let output = Command::new("hcloud")
            .args([
                "server-type",
                "describe",
                server_type,
                "-o",
                "format={{.Cores}} cores, {{.Memory}}GB RAM, {{.Architecture}}",
            ])
            .output()
            .context("Failed to run hcloud - is it installed?")?;

        if !output.status.success() {
            bail!("Unknown server type: {server_type}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check if a server with the given name exists
    pub fn server_exists(name: &str) -> Result<bool> {
        let status = Command::new("hcloud")
            .args(["server", "describe", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run hcloud")?;

        Ok(status.success())
    }

    /// Delete a server by name
    pub fn delete_server(name: &str) -> Result<()> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .unwrap(),
        );
        spinner.set_message(format!("Deleting {name}..."));
        spinner.enable_steady_tick(Duration::from_millis(100));

        let status = Command::new("hcloud")
            .args(["server", "delete", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to delete server")?;

        if !status.success() {
            spinner.finish_with_message(format!("{} Failed to delete server", style("✗").red()));
            bail!("Failed to delete server");
        }

        spinner.finish_with_message(format!("{} Deleted {}", style("✓").green(), name));
        thread::sleep(Duration::from_secs(2));
        Ok(())
    }

    /// Check if an SSH key exists in Hetzner
    pub fn ssh_key_exists(name: &str) -> Result<bool> {
        let status = Command::new("hcloud")
            .args(["ssh-key", "describe", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run hcloud")?;

        Ok(status.success())
    }

    /// Delete an SSH key from Hetzner by name
    pub fn delete_ssh_key(name: &str) -> Result<()> {
        let output = Command::new("hcloud")
            .args(["ssh-key", "delete", name])
            .output()
            .context("Failed to delete SSH key")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to delete SSH key: {stderr}");
        }

        Ok(())
    }

    /// Create an SSH key in Hetzner from a public key string
    pub fn create_ssh_key(name: &str, public_key: &str) -> Result<()> {
        let output = Command::new("hcloud")
            .args([
                "ssh-key",
                "create",
                "--name",
                name,
                "--public-key",
                public_key,
            ])
            .output()
            .context("Failed to create SSH key")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create SSH key: {stderr}");
        }

        Ok(())
    }

    /// Find the Hetzner SSH key name that matches a given public key content.
    /// Uses `hcloud ssh-key list` and compares fingerprints.
    pub fn find_key_name_by_content(public_key: &str) -> Result<Option<String>> {
        // Compute local fingerprint via ssh-keygen
        let mut child = Command::new("ssh-keygen")
            .args(["-l", "-f", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run ssh-keygen")?;
        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            let _ = stdin.write_all(public_key.as_bytes());
        }
        let output = child.wait_with_output()?;
        let _local_fp_line = String::from_utf8_lossy(&output.stdout);
        // Format: "256 SHA256:xxx comment (ED25519)"  — extract the middle part after MD5 colon-hex
        // hcloud uses MD5 fingerprint format (colon-separated hex)
        let local_fp_output = Command::new("ssh-keygen")
            .args(["-l", "-E", "md5", "-f", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn();
        let local_md5 = if let Ok(mut child2) = local_fp_output {
            if let Some(ref mut stdin) = child2.stdin {
                use std::io::Write;
                let _ = stdin.write_all(public_key.as_bytes());
            }
            let out2 = child2.wait_with_output()?;
            let line = String::from_utf8_lossy(&out2.stdout).to_string();
            // "256 MD5:xx:xx:xx... comment (ED25519)" — extract after "MD5:"
            line.split_whitespace()
                .nth(1)
                .map(|s| s.replace("MD5:", ""))
                .unwrap_or_default()
        } else {
            return Ok(None);
        };

        if local_md5.is_empty() {
            return Ok(None);
        }

        // List all hcloud SSH keys and find matching fingerprint
        let output = Command::new("hcloud")
            .args(["ssh-key", "list", "-o", "columns=name,fingerprint"])
            .output()
            .context("Failed to list SSH keys")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            // "NAME   FINGERPRINT"
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == local_md5 {
                return Ok(Some(parts[0].to_string()));
            }
        }

        Ok(None)
    }

    /// Create a new server, returns the IP address
    ///
    /// Creates a plain Ubuntu server with the specified SSH key.
    /// No cloud-init - provisioning happens via SSH after creation.
    pub fn create_server(params: &ServerParams) -> Result<String> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .unwrap(),
        );
        spinner.set_message(format!("Creating {} on Hetzner...", params.name));
        spinner.enable_steady_tick(Duration::from_millis(100));

        let output = Command::new("hcloud")
            .args([
                "server",
                "create",
                "--name",
                params.name,
                "--type",
                params.server_type,
                "--image",
                params.image,
                "--location",
                params.location,
                "--ssh-key",
                params.ssh_key_name,
            ])
            .output()
            .context("Failed to create server")?;

        if !output.status.success() {
            spinner.finish_with_message(format!("{} Failed to create server", style("✗").red()));
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create server: {stderr}");
        }

        spinner.finish_with_message(format!("{} Server created", style("✓").green()));

        // Get IP
        let output = Command::new("hcloud")
            .args(["server", "ip", params.name])
            .output()
            .context("Failed to get server IP")?;

        let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(ip)
    }

    /// Remove old SSH host key for an IP
    pub fn clear_host_key(ip: &str) {
        let _ = Command::new("ssh-keygen")
            .args(["-R", ip])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
