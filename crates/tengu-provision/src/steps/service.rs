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
    /// Optional readiness check command (polled after start succeeds)
    /// Falls back to `systemctl is-active <name>` if not set.
    readiness_check: Option<String>,
    /// Max seconds to wait for readiness (default: 30)
    readiness_timeout: u32,
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
            readiness_check: None,
            readiness_timeout: 30,
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

    /// Set a custom readiness check command.
    /// This command is polled after the service starts until it returns 0.
    /// Example: `pg_isready` for `PostgreSQL`, `docker info` for Docker.
    pub fn with_readiness_check(mut self, cmd: impl Into<String>) -> Self {
        self.readiness_check = Some(cmd.into());
        self
    }

    /// Set the maximum time in seconds to wait for readiness (default: 30)
    pub fn with_readiness_timeout(mut self, seconds: u32) -> Self {
        self.readiness_timeout = seconds;
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
                 {{ for i in 1 2 3 4 5; do \
                     sleep 3; \
                     systemctl start {name} && break; \
                 done; \
                 systemctl is-active {name} >/dev/null 2>&1 || \
                     echo \"WARNING: {name} failed to start after 5 attempts — provisioning is idempotent, you can safely re-run tengu-init to retry\"; }}",
                name = self.name
            ));

            // Poll readiness after start — wait until the service is truly ready
            let check = self
                .readiness_check
                .clone()
                .unwrap_or_else(|| format!("systemctl is-active --quiet {}", self.name));
            let timeout = self.readiness_timeout;
            cmds.push(format!(
                "READY_ELAPSED=0; \
                 while ! ({check}); do \
                     sleep 2; \
                     READY_ELAPSED=$((READY_ELAPSED + 2)); \
                     if [ \"$READY_ELAPSED\" -ge {timeout} ]; then \
                         echo \"WARNING: {name} not ready after {timeout}s — provisioning is idempotent, you can safely re-run tengu-init to retry\"; \
                         break; \
                     fi; \
                 done",
                check = check,
                timeout = timeout,
                name = self.name,
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
