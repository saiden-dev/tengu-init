//! Package installation steps

use super::{CloudInitFragment, Step};

/// Repository configuration for adding external apt sources
#[derive(Debug, Clone)]
pub struct Repository {
    /// URL to the GPG key
    pub key_url: String,
    /// APT repository line (e.g., "deb [arch=amd64] https://... focal main")
    pub repo_line: String,
    /// Path to store the keyring (e.g., "/usr/share/keyrings/docker.gpg")
    pub keyring_path: String,
}

impl Repository {
    /// Docker official repository
    pub fn docker() -> Self {
        Self {
            key_url: "https://download.docker.com/linux/ubuntu/gpg".into(),
            repo_line: "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable".into(),
            keyring_path: "/usr/share/keyrings/docker-archive-keyring.gpg".into(),
        }
    }

    /// `PostgreSQL` official repository
    pub fn postgresql() -> Self {
        Self {
            key_url: "https://www.postgresql.org/media/keys/ACCC4CF8.asc".into(),
            repo_line: "deb [signed-by=/usr/share/keyrings/postgresql-archive-keyring.gpg] https://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main".into(),
            keyring_path: "/usr/share/keyrings/postgresql-archive-keyring.gpg".into(),
        }
    }
}

/// Install an apt package, optionally from an external repository
#[derive(Debug, Clone)]
pub struct InstallPackage {
    /// Package name
    pub name: String,
    /// External repository to add (if any)
    pub repository: Option<Repository>,
    /// Description override
    description: String,
}

impl InstallPackage {
    /// Create a new package installation step
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let description = format!("Install {name}");
        Self {
            name,
            repository: None,
            description,
        }
    }

    /// Add an external repository for this package
    pub fn with_repository(mut self, repo: Repository) -> Self {
        self.repository = Some(repo);
        self
    }
}

impl Step for InstallPackage {
    fn description(&self) -> &str {
        &self.description
    }

    fn to_cloud_init(&self) -> CloudInitFragment {
        let mut fragment = CloudInitFragment::default();

        // Add repository setup commands if needed
        if let Some(repo) = &self.repository {
            fragment.runcmd.push(format!(
                "curl -fsSL {} | gpg --dearmor -o {}",
                repo.key_url, repo.keyring_path
            ));
            fragment.runcmd.push(format!(
                "echo '{}' > /etc/apt/sources.list.d/{}.list",
                repo.repo_line, self.name
            ));
            fragment.runcmd.push("apt-get update".into());
        }

        fragment.packages.push(self.name.clone());
        fragment
    }

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

/// Install a .deb package from a URL
#[derive(Debug, Clone)]
pub struct InstallDebFromUrl {
    /// Package name (for dpkg -s check)
    pub name: String,
    /// URL template (can contain `{arch}` placeholder)
    pub url_template: String,
    /// Custom check command (optional, defaults to dpkg -s)
    pub custom_check: Option<String>,
    /// Description
    description: String,
}

impl InstallDebFromUrl {
    /// Create a new deb installation step
    pub fn new(name: impl Into<String>, url_template: impl Into<String>) -> Self {
        let name = name.into();
        let description = format!("Install {name} from URL");
        Self {
            name,
            url_template: url_template.into(),
            custom_check: None,
            description,
        }
    }

    /// Add a custom check command
    pub fn with_check(mut self, check: impl Into<String>) -> Self {
        self.custom_check = Some(check.into());
        self
    }

    /// Ollama from the official installer
    pub fn ollama() -> Self {
        // Ollama provides a .deb in their releases
        Self::new(
            "ollama",
            "https://github.com/ollama/ollama/releases/latest/download/ollama-linux-{arch}.deb",
        )
        .with_check("command -v ollama >/dev/null 2>&1")
    }

    /// Tengu Caddy (custom Caddy build with Cloudflare DNS)
    pub fn tengu_caddy() -> Self {
        Self::new(
            "tengu-caddy",
            "https://github.com/tengu-apps/tengu-caddy/releases/latest/download/tengu-caddy_2.10.2-1_{arch}.deb",
        )
    }
}

impl Step for InstallDebFromUrl {
    fn description(&self) -> &str {
        &self.description
    }

    fn to_cloud_init(&self) -> CloudInitFragment {
        let mut fragment = CloudInitFragment::default();

        // Cloud-init doesn't have built-in idempotency for runcmd,
        // so we include the check inline
        let check = self
            .custom_check
            .clone()
            .unwrap_or_else(|| format!("dpkg -s {} >/dev/null 2>&1", self.name));

        let cmd = format!(
            r#"if ! {check}; then
    ARCH=$(dpkg --print-architecture)
    URL=$(echo '{url}' | sed "s/{{arch}}/$ARCH/g")
    wget -q "$URL" -O /tmp/{name}.deb
    dpkg -i /tmp/{name}.deb || apt-get install -f -y
    rm -f /tmp/{name}.deb
fi"#,
            check = check,
            url = self.url_template,
            name = self.name
        );

        fragment.runcmd.push(cmd);
        fragment
    }

    fn to_bash(&self) -> Vec<String> {
        // The idempotency check will be wrapped by the renderer using check_command()
        // So to_bash() just returns the actual installation commands
        vec![format!(
            r#"ARCH=$(dpkg --print-architecture)
URL=$(echo '{url}' | sed "s/{{arch}}/$ARCH/g")
wget -q "$URL" -O /tmp/{name}.deb
dpkg -i /tmp/{name}.deb || apt-get install -f -y
rm -f /tmp/{name}.deb"#,
            url = self.url_template,
            name = self.name
        )]
    }

    fn check_command(&self) -> Option<String> {
        self.custom_check
            .clone()
            .or_else(|| Some(format!("dpkg -s {} >/dev/null 2>&1", self.name)))
    }
}
