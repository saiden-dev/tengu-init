//! Configuration types for Tengu provisioning

/// TLS provisioning mode
#[derive(Debug, Clone)]
pub enum TlsMode {
    /// Cloudflare DNS-01 challenge + optional CF tunnel
    Cloudflare {
        /// Cloudflare API key (global key or scoped token)
        api_key: String,
        /// Cloudflare account email
        email: String,
    },
    /// Direct HTTPS via Let's Encrypt HTTP-01 (Caddy default ACME)
    Direct {
        /// Email for Let's Encrypt ACME registration
        acme_email: String,
    },
}

impl Default for TlsMode {
    fn default() -> Self {
        Self::Direct {
            acme_email: String::new(),
        }
    }
}

/// Configuration for a Tengu installation
#[derive(Debug, Clone, Default)]
pub struct TenguConfig {
    /// System username
    pub user: String,
    /// Platform domain (e.g., "tengu.to")
    pub domain_platform: String,
    /// Apps domain (e.g., "tengu.host")
    pub domain_apps: String,
    /// TLS mode (Cloudflare DNS-01 or Direct HTTP-01)
    pub tls_mode: TlsMode,
    /// Resend API key
    pub resend_api_key: String,
    /// Notification email
    pub notify_email: String,
    /// SSH public keys
    pub ssh_keys: Vec<String>,
    /// Tengu release tag
    pub release: String,
    /// Enable UFW firewall configuration (CF mode only; direct always enables)
    pub enable_ufw: bool,
    /// Path to local .deb package (skips download when set)
    pub deb_path: Option<String>,
}

impl TenguConfig {
    /// Create a new config builder
    pub fn builder() -> TenguConfigBuilder {
        TenguConfigBuilder::default()
    }

    /// Whether this config uses Cloudflare mode
    pub fn is_cloudflare(&self) -> bool {
        matches!(self.tls_mode, TlsMode::Cloudflare { .. })
    }

    /// ACME email address (from CF email or direct `acme_email`)
    pub fn acme_email(&self) -> &str {
        match &self.tls_mode {
            TlsMode::Cloudflare { email, .. } => email,
            TlsMode::Direct { acme_email } => acme_email,
        }
    }

    /// Generate fail2ban configuration
    pub fn fail2ban_config(&self) -> String {
        r"[sshd]
enabled = true
port = ssh
filter = sshd
logpath = /var/log/auth.log
maxretry = 3
bantime = 3600
findtime = 600
"
        .to_string()
    }

    /// Generate Tengu config.toml content
    pub fn tengu_config_toml(&self) -> String {
        match &self.tls_mode {
            TlsMode::Cloudflare { api_key, email } => format!(
                r#"# Tengu PaaS Configuration
domain = "{domain_apps}"

[database]
url = "postgres://tengu:tengu@localhost:5432/tengu"

[cloudflare]
api_key = "{api_key}"
email = "{email}"

[cloudflare.domains]
platform = "{domain_platform}"
apps = "{domain_apps}"

[cloudflare.services]
api = "api.{domain_platform}"
docs = "docs.{domain_platform}"
git = "git.{domain_platform}"
ssh = "ssh.{domain_platform}"
"#,
                domain_apps = self.domain_apps,
                api_key = api_key,
                email = email,
                domain_platform = self.domain_platform,
            ),
            TlsMode::Direct { .. } => format!(
                r#"# Tengu PaaS Configuration
domain = "{domain_apps}"

[database]
url = "postgres://tengu:tengu@localhost:5432/tengu"

[server]
tunnel = false
"#,
                domain_apps = self.domain_apps,
            ),
        }
    }

    /// Generate Caddyfile content (mode-aware)
    pub fn caddyfile(&self) -> String {
        match &self.tls_mode {
            TlsMode::Cloudflare { email, .. } => format!(
                r"{{
    email {email}
    # App sites are behind CF tunnel — TLS terminated at Cloudflare edge.
    # Only platform routes (api/docs/git) use Caddy-managed TLS via DNS challenge.
    auto_https disable_redirects
}}

(cf_tls) {{
    tls {{
        dns cloudflare {{env.CF_API_TOKEN}}
    }}
}}

import sites/*.caddy

api.{dp} {{
    import cf_tls
    reverse_proxy localhost:8080
}}

docs.{dp} {{
    import cf_tls
    reverse_proxy localhost:8080
}}

git.{dp} {{
    import cf_tls
    reverse_proxy localhost:8080
}}
",
                email = email,
                dp = self.domain_platform,
            ),
            TlsMode::Direct { acme_email } => format!(
                r"{{
    email {acme_email}
}}

import sites/*.caddy

api.{dp} {{
    reverse_proxy localhost:8080
}}

docs.{dp} {{
    reverse_proxy localhost:8080
}}
",
                acme_email = acme_email,
                dp = self.domain_platform,
            ),
        }
    }

    /// Generate Caddy systemd drop-in for Cloudflare API credentials
    ///
    /// Sets `CF_API_TOKEN`, `CF_API_KEY`, and `CF_API_EMAIL` so the Caddy Cloudflare
    /// DNS module works with both scoped API tokens and Global API Keys.
    ///
    /// Only meaningful in Cloudflare mode — returns empty string in Direct mode.
    pub fn caddy_cloudflare_env(&self) -> String {
        match &self.tls_mode {
            TlsMode::Cloudflare { api_key, email } => format!(
                "[Service]\nEnvironment=\"CF_API_TOKEN={api_key}\"\nEnvironment=\"CF_API_KEY={api_key}\"\nEnvironment=\"CF_API_EMAIL={email}\"\n",
            ),
            TlsMode::Direct { .. } => String::new(),
        }
    }

    /// Create a test configuration (Cloudflare mode) for unit tests
    #[cfg(test)]
    pub fn test_config() -> Self {
        Self::test_config_cloudflare()
    }

    /// Create a Cloudflare-mode test configuration
    #[cfg(test)]
    pub fn test_config_cloudflare() -> Self {
        Self {
            user: "testuser".into(),
            domain_platform: "test.example.com".into(),
            domain_apps: "apps.example.com".into(),
            tls_mode: TlsMode::Cloudflare {
                api_key: "test-api-key".into(),
                email: "test@example.com".into(),
            },
            resend_api_key: "re_test".into(),
            notify_email: "notify@example.com".into(),
            ssh_keys: vec!["ssh-ed25519 AAAA... test@test".into()],
            release: "v0.1.0-test".into(),
            enable_ufw: true,
            deb_path: None,
        }
    }

    /// Create a Direct HTTPS mode test configuration
    #[cfg(test)]
    pub fn test_config_direct() -> Self {
        Self {
            user: "testuser".into(),
            domain_platform: "test.example.com".into(),
            domain_apps: "apps.example.com".into(),
            tls_mode: TlsMode::Direct {
                acme_email: "admin@example.com".into(),
            },
            resend_api_key: "re_test".into(),
            notify_email: "notify@example.com".into(),
            ssh_keys: vec!["ssh-ed25519 AAAA... test@test".into()],
            release: "v0.1.0-test".into(),
            enable_ufw: true,
            deb_path: None,
        }
    }
}

/// Builder for `TenguConfig`
#[derive(Debug, Clone, Default)]
pub struct TenguConfigBuilder {
    config: TenguConfig,
}

impl TenguConfigBuilder {
    /// Set the system username
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.config.user = user.into();
        self
    }

    /// Set the platform domain
    pub fn domain_platform(mut self, domain: impl Into<String>) -> Self {
        self.config.domain_platform = domain.into();
        self
    }

    /// Set the apps domain
    pub fn domain_apps(mut self, domain: impl Into<String>) -> Self {
        self.config.domain_apps = domain.into();
        self
    }

    /// Set the TLS mode (Cloudflare or Direct)
    pub fn tls_mode(mut self, mode: TlsMode) -> Self {
        self.config.tls_mode = mode;
        self
    }

    /// Set the Resend API key
    pub fn resend_api_key(mut self, key: impl Into<String>) -> Self {
        self.config.resend_api_key = key.into();
        self
    }

    /// Set the notification email
    pub fn notify_email(mut self, email: impl Into<String>) -> Self {
        self.config.notify_email = email.into();
        self
    }

    /// Add SSH keys
    pub fn ssh_keys(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config.ssh_keys = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Set the release tag
    pub fn release(mut self, release: impl Into<String>) -> Self {
        self.config.release = release.into();
        self
    }

    /// Enable or disable UFW firewall configuration
    pub fn enable_ufw(mut self, enable: bool) -> Self {
        self.config.enable_ufw = enable;
        self
    }

    /// Set local .deb path
    pub fn deb_path(mut self, path: Option<String>) -> Self {
        self.config.deb_path = path;
        self
    }

    /// Build the configuration
    pub fn build(self) -> TenguConfig {
        self.config
    }
}
