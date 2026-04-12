//! Tengu Provision - Installation Step Library
//!
//! This crate provides types and traits for defining idempotent installation steps
//! that can be rendered to executable bash scripts.
//!
//! # Architecture
//!
//! - [`Step`] trait: Common interface for all installation steps
//! - [`steps`] module: Concrete step implementations (packages, users, files, etc.)
//! - [`render`] module: Output renderers (bash)
//! - [`Manifest`]: Complete installation manifest combining multiple steps
//! - [`Config`]: Configuration types for Tengu installation
//!
//! # Example
//!
//! ```ignore
//! use tengu_provision::{Manifest, TenguConfig, BashRenderer};
//!
//! let config = TenguConfig::builder()
//!     .user("tengu")
//!     .domain_platform("tengu.to")
//!     .build();
//!
//! let manifest = Manifest::tengu(&config);
//! let renderer = BashRenderer::new().verbose(true);
//! let script = renderer.render(&manifest)?;
//! ```

pub mod config;
pub mod manifest;
pub mod render;
pub mod steps;

pub use config::TenguConfig;
pub use manifest::Manifest;
pub use render::{BashRenderer, Renderer};
pub use steps::Step;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::{
        EnsureDirectory, EnsureService, EnsureUser, InstallPackage, RunCommand, WriteFile,
    };

    #[test]
    fn test_install_package_idempotent() {
        let step = InstallPackage::new("vim");
        let bash = step.to_bash();

        assert_eq!(bash.len(), 1);
        assert!(bash[0].contains("dpkg -s vim"));
        assert!(bash[0].contains("apt-get install -y vim"));
    }

    #[test]
    fn test_ensure_user_creates_user() {
        let step = EnsureUser::new("testuser")
            .with_groups(["docker", "sudo"])
            .with_sudo("ALL=(ALL) NOPASSWD:ALL");

        let bash = step.to_bash();

        // Check user creation
        assert!(
            bash.iter()
                .any(|c| c.contains("id testuser") && c.contains("useradd"))
        );
        // Check groups
        assert!(
            bash.iter()
                .any(|c| c.contains("docker") && c.contains("usermod"))
        );
        // Check sudoers
        assert!(bash.iter().any(|c| c.contains("/etc/sudoers.d/testuser")));
    }

    #[test]
    fn test_write_file_uses_checksum() {
        let step = WriteFile::new("/etc/test.conf", "test content").with_permissions("0644");

        let bash = step.to_bash();
        let check = step.check_command();

        // Should use sha256sum for comparison
        assert!(bash.iter().any(|c| c.contains("sha256sum")));
        // Check command should verify hash
        assert!(check.is_some());
        assert!(check.unwrap().contains("sha256sum"));
    }

    #[test]
    fn test_ensure_directory_idempotent() {
        let step = EnsureDirectory::new("/var/lib/tengu")
            .with_permissions("0755")
            .with_owner("root:root");

        let check = step.check_command();
        assert!(check.is_some());
        assert!(check.unwrap().contains("[ -d /var/lib/tengu ]"));
    }

    #[test]
    fn test_ensure_service_idempotent() {
        let step = EnsureService::new("docker");
        let bash = step.to_bash();

        // Should check before enabling/starting
        assert!(bash.iter().any(|c| c.contains("systemctl is-enabled")));
        assert!(bash.iter().any(|c| c.contains("systemctl is-active")));
    }

    #[test]
    fn test_run_command_with_unless() {
        let step = RunCommand::new("Create directory", "mkdir /test").unless("[ -d /test ]");

        let bash = step.to_bash();
        let check = step.check_command();

        assert!(bash[0].contains("[ -d /test ] || { mkdir /test; }"));
        assert_eq!(check, Some("[ -d /test ]".into()));
    }

    #[test]
    fn test_manifest_tengu_has_all_phases() {
        let config = TenguConfig::test_config();
        let manifest = Manifest::tengu(&config);

        // Should have many steps
        assert!(
            manifest.steps.len() > 20,
            "Expected many steps, got {}",
            manifest.steps.len()
        );

        // Check for key steps by description
        let descriptions: Vec<&str> = manifest.steps.iter().map(|s| s.description()).collect();

        // User setup
        assert!(descriptions.iter().any(|d| d.contains("user")));
        // Packages
        assert!(descriptions.iter().any(|d| d.contains("curl")));
        // Docker
        assert!(descriptions.iter().any(|d| d.contains("docker")));
        // PostgreSQL
        assert!(descriptions.iter().any(|d| d.contains("postgresql")));
        // Firewall
        assert!(
            descriptions
                .iter()
                .any(|d| d.contains("firewall") || d.contains("Firewall"))
        );
        // sshd configuration
        assert!(descriptions.iter().any(|d| d.contains("sshd_config")));
        // tengu user shell
        assert!(descriptions.iter().any(|d| d.contains("/bin/bash")));
    }

    #[test]
    fn test_bash_renderer_verbose() {
        let config = TenguConfig::test_config();
        let manifest = Manifest::tengu(&config);
        let renderer = BashRenderer::new().verbose(true);

        let script = renderer.render(&manifest).unwrap();

        // Should have progress markers
        assert!(script.contains("TENGU_STEP:START"));
        assert!(script.contains("TENGU_STEP:DONE"));
        assert!(script.contains("TENGU_STEP:SKIP"));
        // Should have color codes by default
        assert!(script.contains("GREEN="));
    }

    #[test]
    fn test_bash_renderer_no_color() {
        let config = TenguConfig::test_config();
        let manifest = Manifest::tengu(&config);
        let renderer = BashRenderer::new().verbose(true).color(false);

        let script = renderer.render(&manifest).unwrap();

        // Should have progress markers
        assert!(script.contains("TENGU_STEP:START"));
        // Should NOT have color codes
        assert!(!script.contains("GREEN="));
    }
}
