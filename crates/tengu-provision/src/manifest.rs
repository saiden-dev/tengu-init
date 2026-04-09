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
    /// - `PostgreSQL` 16 with pgvector extension
    /// - Ollama for AI/ML
    /// - tengu-caddy (custom Caddy build with Cloudflare DNS)
    /// - Tengu configuration files
    /// - Firewall rules
    /// - Tengu .deb package installation
    /// - OpenSSH configuration for git operations
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
        // Phase 4: PostgreSQL 16 with pgvector
        // =========================================================
        manifest.add_step(
            InstallPackage::new("postgresql-16").with_repository(Repository::postgresql()),
        );
        manifest.add_step(InstallPackage::new("postgresql-16-pgvector"));

        // =========================================================
        // Phase 5: Ollama
        // =========================================================
        manifest.add_step(
            RunCommand::new(
                "Install Ollama",
                "bash -c 'set +e; curl -fsSL https://ollama.com/install.sh | sh; exit 0'",
            )
            .unless("command -v ollama >/dev/null 2>&1"),
        );

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

        // Caddy systemd drop-in for Cloudflare API token
        manifest.add_step(
            EnsureDirectory::new("/etc/systemd/system/caddy.service.d")
                .with_permissions("0755")
                .with_owner("root:root"),
        );
        manifest.add_step(
            WriteFile::new(
                "/etc/systemd/system/caddy.service.d/cloudflare.conf",
                config.caddy_cloudflare_env(),
            )
            .with_permissions("0644")
            .with_owner("root:root"),
        );

        // Reload systemd after drop-in
        manifest.add_step(RunCommand::new(
            "Reload systemd daemon",
            "systemctl daemon-reload",
        ));

        // fail2ban configuration
        manifest.add_step(
            WriteFile::new("/etc/fail2ban/jail.local", config.fail2ban_config())
                .with_permissions("0644")
                .with_owner("root:root"),
        );

        // =========================================================
        // Phase 9: Firewall Rules (optional)
        // =========================================================
        if config.enable_ufw {
            manifest.add_step(
                EnsureFirewall::new()
                    .allow("22/tcp") // SSH
                    .allow("80/tcp") // HTTP
                    .allow("443/tcp"), // HTTPS
            );
        }

        // =========================================================
        // Phase 10: Enable and Start Services
        // =========================================================
        // Reload systemd to pick up any new/changed unit files from package installs
        manifest.add_step(RunCommand::new(
            "Reload systemd daemon",
            "systemctl daemon-reload",
        ));
        // docker.service requires docker.socket for socket activation
        manifest.add_step(EnsureService::new("docker.socket"));
        manifest.add_step(
            EnsureService::new("docker")
                .with_readiness_check("docker info >/dev/null 2>&1"),
        );
        manifest.add_step(
            EnsureService::new("postgresql")
                .with_readiness_check("pg_isready -q"),
        );
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
        if config.deb_path.is_some() {
            // Local .deb was SCP'd to /tmp/tengu-local.deb before provisioning
            manifest.add_step(RunCommand::new(
                "Install tengu from local .deb",
                "DEBIAN_FRONTEND=noninteractive dpkg -i --force-confold /tmp/tengu-local.deb || apt-get install -f -y",
            ));
        } else {
            let tengu_deb_url =
                "https://github.com/tengu-apps/tengu-deb/releases/download/current/tengu_{arch}.deb";
            manifest.add_step(InstallDebFromUrl::new("tengu", tengu_deb_url));
        }

        // Enable and start tengu service
        manifest.add_step(EnsureService::new("tengu"));

        // Set tengu user shell to /bin/bash — tengu is a normal user and
        // the setup SSH key can log in directly. The command= prefix in
        // authorized_keys handles git key restriction.
        manifest.add_step(
            RunCommand::new(
                "Set tengu user shell to /bin/bash",
                "usermod -s /bin/bash tengu",
            )
            .unless(
                r"getent passwd tengu | grep -q '/bin/bash'",
            ),
        );

        // Place the setup SSH key into tengu's authorized_keys with git-shell restriction.
        // All tengu user access goes through the git shell command — admin uses root SSH.
        // Format: command="/usr/bin/tengu git-shell <username>",restrict <key>
        if !config.ssh_keys.is_empty() {
            let key_cmds: Vec<String> = config.ssh_keys.iter().map(|key| {
                let key_escaped = key.replace('\'', "'\\''");
                // Extract username from key comment (last field, e.g. "chi@junkpile" → "chi")
                let username = key.split_whitespace().last()
                    .and_then(|c| c.split('@').next())
                    .unwrap_or("admin");
                let entry = format!(
                    "command=\"/usr/bin/tengu git-shell {username}\",restrict {key_escaped}"
                );
                // Remove any bare key (Hetzner cloud-init injects it) then add with command= restriction
                format!(
                    "sed -i '\\|^ssh-.*{key_short}|d' /home/tengu/.ssh/authorized_keys 2>/dev/null; \
                     grep -qF 'git-shell' /home/tengu/.ssh/authorized_keys 2>/dev/null && \
                     grep -qF '{key_short}' /home/tengu/.ssh/authorized_keys 2>/dev/null || \
                     echo '{entry}' >> /home/tengu/.ssh/authorized_keys",
                    key_short = key.split_whitespace().nth(1).unwrap_or(""),
                )
            }).collect();

            let mut bash = String::from(
                "mkdir -p /home/tengu/.ssh && chmod 700 /home/tengu/.ssh && "
            );
            bash.push_str(&key_cmds.join(" && "));
            bash.push_str(
                " && chmod 600 /home/tengu/.ssh/authorized_keys && chown -R tengu:tengu /home/tengu/.ssh"
            );

            manifest.add_step(
                RunCommand::new(
                    "Add setup SSH key to tengu authorized_keys",
                    &bash,
                )
            );
        }

        // =========================================================
        // Phase 11a: OpenSSH Configuration for Git Operations
        // =========================================================

        // Write sshd drop-in config for tengu user
        manifest.add_step(
            WriteFile::new(
                "/etc/ssh/sshd_config.d/tengu.conf",
                "Match User tengu\n    \
                 AuthorizedKeysCommand /usr/bin/tengu auth-keys %t %k\n    \
                 AuthorizedKeysCommandUser root\n",
            )
            .with_permissions("0644")
            .with_owner("root:root"),
        );

        // Restart sshd to pick up the new configuration
        // Ubuntu 24.04 uses ssh.service, older versions use sshd.service
        manifest.add_step(RunCommand::new(
            "Restart SSH service for tengu config",
            "systemctl restart ssh 2>/dev/null || systemctl restart sshd 2>/dev/null || true",
        ));

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

        // Create tengu PostgreSQL user (or ensure password is set if user exists)
        manifest.add_step(
            RunCommand::new(
                "Create tengu PostgreSQL user",
                r#"sudo -u postgres psql -c "CREATE USER tengu WITH PASSWORD 'tengu';" 2>/dev/null || sudo -u postgres psql -c "ALTER USER tengu WITH PASSWORD 'tengu';""#,
            )
            .unless(r#"PGPASSWORD=tengu psql -U tengu -h 127.0.0.1 -d tengu -c "SELECT 1" >/dev/null 2>&1"#),
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

        // =========================================================
        // Phase 13: Create Tengu Admin User
        // =========================================================

        // Create admin user with SSH key and save token
        let ssh_key = config.ssh_keys.first().map(|s| s.as_str()).unwrap_or("");
        let create_user_cmd = format!(
            r#"TENGU_USER_JSON=$(tengu user add {user} --key '{ssh_key}' --admin --json 2>/dev/null || echo '{{}}'); \
               TENGU_TOKEN=$(echo "$TENGU_USER_JSON" | jq -r '.token // empty'); \
               if [ -n "$TENGU_TOKEN" ]; then \
                   grep -q "^TENGU_TOKEN=" /etc/tengu/env 2>/dev/null && \
                       sed -i "s/^TENGU_TOKEN=.*/TENGU_TOKEN=$TENGU_TOKEN/" /etc/tengu/env || \
                       echo "TENGU_TOKEN=$TENGU_TOKEN" >> /etc/tengu/env; \
               fi"#,
            user = config.user,
            ssh_key = ssh_key,
        );
        manifest.add_step(
            RunCommand::new("Create Tengu admin user", &create_user_cmd)
                .unless(&format!(r#"tengu user list --json 2>/dev/null | jq -e '.[] | select(.name == "{}")' >/dev/null"#, config.user)),
        );

        manifest
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new("tengu")
    }
}
