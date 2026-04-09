//! User management steps

use super::{CloudInitFragment, Step};

/// Ensure a system user exists with specified configuration
#[derive(Debug, Clone)]
pub struct EnsureUser {
    /// Username
    pub name: String,
    /// Groups to add user to
    pub groups: Vec<String>,
    /// Login shell
    pub shell: String,
    /// Sudoers rule (e.g., "ALL=(ALL) NOPASSWD:ALL")
    pub sudo: Option<String>,
    /// SSH authorized keys
    pub ssh_keys: Vec<String>,
    /// Description
    description: String,
}

impl EnsureUser {
    /// Create a new user step
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let description = format!("Ensure user {name} exists");
        Self {
            name,
            groups: vec![],
            shell: "/bin/bash".into(),
            sudo: None,
            ssh_keys: vec![],
            description,
        }
    }

    /// Add groups for the user
    pub fn with_groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// Set the login shell
    pub fn with_shell(mut self, shell: impl Into<String>) -> Self {
        self.shell = shell.into();
        self
    }

    /// Set sudo privileges
    pub fn with_sudo(mut self, sudo: impl Into<String>) -> Self {
        self.sudo = Some(sudo.into());
        self
    }

    /// Add SSH keys
    pub fn with_ssh_keys(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.ssh_keys = keys.into_iter().map(Into::into).collect();
        self
    }
}

impl Step for EnsureUser {
    fn description(&self) -> &str {
        &self.description
    }

    fn to_cloud_init(&self) -> CloudInitFragment {
        // Cloud-init handles users differently - this would be in the users: section
        // For now, we emit runcmd equivalents
        CloudInitFragment {
            runcmd: self.to_bash(),
            ..Default::default()
        }
    }

    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![];

        // Create user if not exists (handle leftover group from prior install)
        cmds.push(format!(
            "id {name} >/dev/null 2>&1 || {{ \
                 getent group {name} >/dev/null 2>&1 && useradd -m -s {shell} -g {name} {name} || \
                 useradd -m -s {shell} {name}; \
             }}",
            name = self.name, shell = self.shell
        ));

        // Add to groups
        if !self.groups.is_empty() {
            cmds.push(format!(
                "for g in {}; do \
                    getent group $g >/dev/null && usermod -aG $g {} 2>/dev/null || true; \
                done",
                self.groups.join(" "),
                self.name
            ));
        }

        // Sudoers
        if let Some(sudo) = &self.sudo {
            cmds.push(format!(
                "echo '{} {}' > /etc/sudoers.d/{} && chmod 440 /etc/sudoers.d/{}",
                self.name, sudo, self.name, self.name
            ));
        }

        // SSH keys
        if !self.ssh_keys.is_empty() {
            cmds.push(format!(
                "mkdir -p /home/{}/.ssh && chmod 700 /home/{}/.ssh",
                self.name, self.name
            ));

            for key in &self.ssh_keys {
                // Escape single quotes in key
                let key_escaped = key.replace('\'', "'\\''");
                cmds.push(format!(
                    "grep -qF '{}' /home/{}/.ssh/authorized_keys 2>/dev/null || \
                     echo '{}' >> /home/{}/.ssh/authorized_keys",
                    key_escaped, self.name, key_escaped, self.name
                ));
            }

            cmds.push(format!(
                "chmod 600 /home/{}/.ssh/authorized_keys && chown -R {}:{} /home/{}/.ssh",
                self.name, self.name, self.name, self.name
            ));
        }

        cmds
    }

    fn check_command(&self) -> Option<String> {
        Some(format!("id {} >/dev/null 2>&1", self.name))
    }
}
