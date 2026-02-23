//! File management steps

use super::{CloudInitFile, CloudInitFragment, Step};
use sha2::{Digest, Sha256};

/// Write a file with specified content
#[derive(Debug, Clone)]
pub struct WriteFile {
    /// File path
    pub path: String,
    /// File content
    pub content: String,
    /// File permissions (e.g., "0644")
    pub permissions: Option<String>,
    /// File owner (e.g., "root:root")
    pub owner: Option<String>,
    /// Description
    description: String,
}

impl WriteFile {
    /// Create a new file write step
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        let path = path.into();
        let description = format!("Write {path}");
        Self {
            path,
            content: content.into(),
            permissions: None,
            owner: None,
            description,
        }
    }

    /// Set file permissions
    pub fn with_permissions(mut self, perms: impl Into<String>) -> Self {
        self.permissions = Some(perms.into());
        self
    }

    /// Set file owner
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Compute SHA256 hash of content (hex-encoded)
    fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.content.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

}

impl Step for WriteFile {
    fn description(&self) -> &str {
        &self.description
    }

    fn to_cloud_init(&self) -> CloudInitFragment {
        CloudInitFragment {
            write_files: vec![CloudInitFile {
                path: self.path.clone(),
                content: self.content.clone(),
                permissions: self.permissions.clone(),
                owner: self.owner.clone(),
            }],
            ..Default::default()
        }
    }

    fn to_bash(&self) -> Vec<String> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut cmds = vec![];

        // Create parent directory
        cmds.push(format!("mkdir -p \"$(dirname '{}')\"", self.path));

        // Pre-compute expected hash at generation time
        let expected_hash = self.content_hash();

        // Use base64 encoding to avoid heredoc indentation issues
        let encoded = STANDARD.encode(&self.content);

        // Compare hash and write only if different
        cmds.push(format!(
            r#"CURRENT=$(sha256sum '{}' 2>/dev/null | cut -d' ' -f1 || echo 'none')
if [ "$CURRENT" != "{}" ]; then
echo '{}' | base64 -d > '{}'
fi"#,
            self.path, expected_hash, encoded, self.path
        ));

        if let Some(perms) = &self.permissions {
            cmds.push(format!("chmod {} '{}'", perms, self.path));
        }

        if let Some(owner) = &self.owner {
            cmds.push(format!("chown {} '{}'", owner, self.path));
        }

        cmds
    }

    fn check_command(&self) -> Option<String> {
        // Check if file exists with expected content hash
        let expected_hash = self.content_hash();
        Some(format!(
            "[ -f '{}' ] && [ \"$(sha256sum '{}' | cut -d' ' -f1)\" = \"{}\" ]",
            self.path, self.path, expected_hash
        ))
    }
}
