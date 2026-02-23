//! Installation manifest - complete step sequence

use crate::config::TenguConfig;
use crate::steps::{
    EnsureDirectory, EnsureFirewall, EnsureService, EnsureUser, InstallDebFromUrl, InstallPackage,
    Repository, RunCommand, Step, WriteFile,
};

/// Complete Tengu installation manifest
pub struct Manifest {
    /// Server hostname
    pub hostname: String,
    /// Fully qualified domain name
    pub fqdn: Option<String>,
    /// Timezone
    pub timezone: String,
    /// Locale
    pub locale: String,
    /// Ordered list of installation steps
    pub steps: Vec<Box<dyn Step>>,
}

impl Manifest {
    /// Create a new empty manifest
    pub fn new(hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            fqdn: None,
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            steps: vec![],
        }
    }

    /// Set the FQDN
    pub fn with_fqdn(mut self, fqdn: impl Into<String>) -> Self {
        self.fqdn = Some(fqdn.into());
        self
    }

    /// Set the timezone
    pub fn with_timezone(mut self, timezone: impl Into<String>) -> Self {
        self.timezone = timezone.into();
        self
    }

    /// Set the locale
    pub fn with_locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = locale.into();
        self
    }

    /// Add a step to the manifest
    pub fn add_step<S: Step + 'static>(&mut self, step: S) {
        self.steps.push(Box::new(step));
    }

    /// Add a step fluently
    pub fn with_step<S: Step + 'static>(mut self, step: S) -> Self {
        self.add_step(step);
        self
    }

    /// Create a complete Tengu installation manifest
    ///
    /// This builds the full installation sequence including:
    /// - User setup with SSH keys and sudo
    /// - Base packages (curl, wget, git, jq, htop, vim, fail2ban, ufw)
    /// - Docker from Ubuntu repositories (docker.io)
    /// - `PostgreSQL` 18 with pgvector extension
    /// - Ollama for AI/ML
    /// - tengu-caddy (custom Caddy build with Cloudflare DNS)
    /// - Tengu configuration files
    /// - Firewall rules
    /// - Tengu .deb package installation
    #[allow(clippy::too_many_lines)]
    pub fn tengu(config: &TenguConfig) -> Self {
        let mut manifest = Self::new("tengu")
            .with_fqdn(format!("api.{}", config.domain_platform))
            .with_timezone("UTC");

        // =========================================================
        // Phase 1: User Setup
        // =========================================================
        manifest.add_step(
            EnsureUser::new(&config.user)
                .with_groups(["docker", "sudo"])
                .with_sudo("ALL=(ALL) NOPASSWD:ALL")
                .with_ssh_keys(config.ssh_keys.clone()),
        );

        // =========================================================
        // Phase 2: Base Packages
        // =========================================================
        let base_packages = [
            "curl",
            "wget",
            "git",
            "jq",
            "htop",
            "vim",
            "fail2ban",
            "ufw",
            "ca-certificates",
            "gnupg",
            "lsb-release",
            "unzip",
        ];

        for pkg in base_packages {
            manifest.add_step(InstallPackage::new(pkg));
        }

        // =========================================================
        // Phase 3: Docker from Ubuntu Repositories
        // =========================================================
        manifest.add_step(InstallPackage::new("docker.io"));
        manifest.add_step(InstallPackage::new("docker-compose"));

        // =========================================================
        // Phase 4: PostgreSQL 18 with pgvector
        // =========================================================
        manifest.add_step(
            InstallPackage::new("postgresql-18").with_repository(Repository::postgresql()),
        );
        manifest.add_step(InstallPackage::new("postgresql-18-pgvector"));

        // =========================================================
        // Phase 5: Ollama
        // =========================================================
        manifest.add_step(InstallDebFromUrl::ollama());

        // =========================================================
        // Phase 6: tengu-caddy (Caddy with Cloudflare DNS plugin)
        // =========================================================
        manifest.add_step(InstallDebFromUrl::tengu_caddy());

        // =========================================================
        // Phase 7: Tengu Directories
        // =========================================================
        manifest.add_step(
            EnsureDirectory::new("/etc/tengu")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            EnsureDirectory::new("/var/lib/tengu")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            EnsureDirectory::new("/var/lib/tengu/apps")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            EnsureDirectory::new("/var/lib/tengu/repos")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            EnsureDirectory::new("/var/log/tengu")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            EnsureDirectory::new("/etc/caddy/sites")
                .with_permissions("0755")
                .with_owner("root:root"),
        );

        // =========================================================
        // Phase 8: Configuration Files
        // =========================================================

        // Tengu config.toml
        manifest.add_step(
            WriteFile::new("/etc/tengu/config.toml", config.tengu_config_toml())
                .with_permissions("0600")
                .with_owner("root:root"),
        );

        // Caddyfile
        manifest.add_step(
            WriteFile::new("/etc/caddy/Caddyfile", config.caddyfile())
                .with_permissions("0644")
                .with_owner("root:root"),
        );

        // fail2ban configuration
        manifest.add_step(
            WriteFile::new("/etc/fail2ban/jail.local", config.fail2ban_config())
                .with_permissions("0644")
                .with_owner("root:root"),
        );

        // =========================================================
        // Phase 9: Firewall Rules
        // =========================================================
        manifest.add_step(
            EnsureFirewall::new()
                .allow("22/tcp") // SSH
                .allow("80/tcp") // HTTP
                .allow("443/tcp"), // HTTPS
        );

        // =========================================================
        // Phase 10: Enable and Start Services
        // =========================================================
        manifest.add_step(EnsureService::new("docker"));
        manifest.add_step(EnsureService::new("postgresql"));
        manifest.add_step(EnsureService::new("fail2ban"));
        manifest.add_step(EnsureService::new("caddy"));

        // Ollama runs as a user service by default, or systemd service if installed via deb
        manifest.add_step(
            RunCommand::new("Enable ollama service", "systemctl enable ollama || true")
                .unless("systemctl is-enabled ollama >/dev/null 2>&1"),
        );
        manifest.add_step(
            RunCommand::new("Start ollama service", "systemctl start ollama || true")
                .unless("systemctl is-active ollama >/dev/null 2>&1"),
        );

        // =========================================================
        // Phase 11: Install Tengu .deb Package
        // =========================================================
        let tengu_deb_url =
            "https://github.com/tengu-apps/tengu-deb/releases/latest/download/tengu_0.1.0-1_{arch}.deb";
        manifest.add_step(InstallDebFromUrl::new("tengu", tengu_deb_url));

        // Enable and start tengu service
        manifest.add_step(EnsureService::new("tengu"));

        // =========================================================
        // Phase 12: Post-Install Setup
        // =========================================================

        // Initialize PostgreSQL database for Tengu
        manifest.add_step(
            RunCommand::new(
                "Create tengu PostgreSQL database",
                r#"sudo -u postgres psql -c "CREATE DATABASE tengu;" 2>/dev/null || true"#,
            )
            .unless(r"sudo -u postgres psql -lqt | cut -d \| -f 1 | grep -qw tengu"),
        );

        // Create tengu PostgreSQL user
        manifest.add_step(
            RunCommand::new(
                "Create tengu PostgreSQL user",
                r#"sudo -u postgres psql -c "CREATE USER tengu WITH PASSWORD 'tengu';" 2>/dev/null || true"#,
            )
            .unless(r#"sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='tengu'" | grep -q 1"#),
        );

        // Grant privileges
        manifest.add_step(RunCommand::new(
            "Grant PostgreSQL privileges to tengu",
            r#"sudo -u postgres psql -c "GRANT ALL PRIVILEGES ON DATABASE tengu TO tengu;""#,
        ));

        // Enable pgvector extension
        manifest.add_step(
            RunCommand::new(
                "Enable pgvector extension",
                r#"sudo -u postgres psql -d tengu -c "CREATE EXTENSION IF NOT EXISTS vector;""#,
            )
            .unless(r#"sudo -u postgres psql -d tengu -tAc "SELECT 1 FROM pg_extension WHERE extname='vector'" | grep -q 1"#),
        );

        manifest
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new("tengu")
    }
}
