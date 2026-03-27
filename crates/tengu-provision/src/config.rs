//! Configuration types for Tengu provisioning

/// Configuration for a Tengu installation
#[derive(Debug, Clone, Default)]
pub struct TenguConfig {
    /// System username
    pub user: String,
    /// Platform domain (e.g., "tengu.to")
    pub domain_platform: String,
    /// Apps domain (e.g., "tengu.host")
    pub domain_apps: String,
    /// Cloudflare API key
    pub cf_api_key: String,
    /// Cloudflare email
    pub cf_email: String,
    /// Resend API key
    pub resend_api_key: String,
    /// Notification email
    pub notify_email: String,
    /// SSH public keys
    pub ssh_keys: Vec<String>,
    /// Tengu release tag
    pub release: String,
}

impl TenguConfig {
    /// Create a new config builder
    pub fn builder() -> TenguConfigBuilder {
        TenguConfigBuilder::default()
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
        format!(
            r#"# Tengu PaaS Configuration
domain = "{}"

[database]
url = "postgres://tengu:tengu@localhost:5432/tengu"

[cloudflare]
api_key = "{}"
email = "{}"

[cloudflare.domains]
platform = "{}"
apps = "{}"

[cloudflare.services]
api = "api.{}"
docs = "docs.{}"
git = "git.{}"
ssh = "ssh.{}"
"#,
            self.domain_apps,
            self.cf_api_key,
            self.cf_email,
            self.domain_platform,
            self.domain_apps,
            self.domain_platform,
            self.domain_platform,
            self.domain_platform,
            self.domain_platform,
        )
    }

    /// Generate Caddyfile content (uses Cloudflare DNS challenge for TLS)
    pub fn caddyfile(&self) -> String {
        format!(
            r"{{
    email {}
}}

(cf_tls) {{
    tls {{
        dns cloudflare {{env.CF_API_TOKEN}}
    }}
}}

import sites/*.caddy

api.{} {{
    import cf_tls
    reverse_proxy localhost:8080
}}

docs.{} {{
    import cf_tls
    reverse_proxy localhost:8080
}}

git.{} {{
    import cf_tls
    reverse_proxy localhost:8080
}}
",
            self.cf_email, self.domain_platform, self.domain_platform, self.domain_platform,
        )
    }

    /// Generate Caddy systemd drop-in for Cloudflare API token
    pub fn caddy_cloudflare_env(&self) -> String {
        format!(
            "[Service]\nEnvironment=\"CF_API_TOKEN={}\"\n",
            self.cf_api_key
        )
    }

    /// Create a test configuration for unit tests
    #[cfg(test)]
    pub fn test_config() -> Self {
        Self {
            user: "testuser".into(),
            domain_platform: "test.example.com".into(),
            domain_apps: "apps.example.com".into(),
            cf_api_key: "test-api-key".into(),
            cf_email: "test@example.com".into(),
            resend_api_key: "re_test".into(),
            notify_email: "notify@example.com".into(),
            ssh_keys: vec!["ssh-ed25519 AAAA... test@test".into()],
            release: "v0.1.0-test".into(),
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

    /// Set the Cloudflare API key
    pub fn cf_api_key(mut self, key: impl Into<String>) -> Self {
        self.config.cf_api_key = key.into();
        self
    }

    /// Set the Cloudflare email
    pub fn cf_email(mut self, email: impl Into<String>) -> Self {
        self.config.cf_email = email.into();
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

    /// Build the configuration
    pub fn build(self) -> TenguConfig {
        self.config
    }
}
