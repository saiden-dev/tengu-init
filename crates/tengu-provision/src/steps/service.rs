//! Systemd service management steps

use super::{CloudInitFragment, Step};

/// Ensure a systemd service is enabled and/or started
#[derive(Debug, Clone)]
pub struct EnsureService {
    /// Service name
    pub name: String,
    /// Whether to enable the service
    pub enabled: bool,
    /// Whether to start the service
    pub started: bool,
    /// Description
    description: String,
}

impl EnsureService {
    /// Create a new service step (enabled and started by default)
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let description = format!("Ensure service {name}");
        Self {
            name,
            enabled: true,
            started: true,
            description,
        }
    }

    /// Set whether the service should be enabled
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set whether the service should be started
    pub fn started(mut self, started: bool) -> Self {
        self.started = started;
        self
    }
}

impl Step for EnsureService {
    fn description(&self) -> &str {
        &self.description
    }

    fn to_cloud_init(&self) -> CloudInitFragment {
        CloudInitFragment {
            runcmd: self.to_bash(),
            ..Default::default()
        }
    }

    fn to_bash(&self) -> Vec<String> {
        let mut cmds = vec![];

        if self.enabled {
            cmds.push(format!(
                "systemctl is-enabled {} >/dev/null 2>&1 || systemctl enable {}",
                self.name, self.name
            ));
        }

        if self.started {
            // Retry up to 5 times with 3s sleep if start fails (services may need time after install)
            cmds.push(format!(
                "systemctl is-active {name} >/dev/null 2>&1 || \
                 systemctl start {name} || \
                 {{ for i in 1 2 3 4 5; do sleep 3; systemctl start {name} && break; done; }}",
                name = self.name
            ));
        }

        cmds
    }

    fn check_command(&self) -> Option<String> {
        if self.started {
            Some(format!("systemctl is-active {} >/dev/null 2>&1", self.name))
        } else if self.enabled {
            Some(format!(
                "systemctl is-enabled {} >/dev/null 2>&1",
                self.name
            ))
        } else {
            None
        }
    }
}
