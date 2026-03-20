//! Skill loader - scans and loads user skill packages from the skills/ directory.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::types::AgentError;

/// A loaded skill manifest
#[derive(Debug, Clone)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: u32,
    pub language: String,
    pub entrypoint: PathBuf,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub topics: Vec<String>,
    pub composes_with: Vec<String>,
}

/// Raw TOML structure for SKILL.toml
#[derive(Debug, Deserialize)]
struct SkillToml {
    skill: SkillSection,
    input: InputSection,
    output: OutputSection,
    #[serde(default)]
    metadata: MetadataSection,
}

#[derive(Debug, Deserialize)]
struct SkillSection {
    name: String,
    description: String,
    #[serde(default = "default_version")]
    version: u32,
    language: String,
    entrypoint: String,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct InputSection {
    schema: String,
}

#[derive(Debug, Deserialize)]
struct OutputSection {
    schema: String,
}

#[derive(Debug, Deserialize, Default)]
struct MetadataSection {
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    composes_with: Vec<String>,
}

/// Skill loader that scans the skills directory
pub struct SkillLoader {
    skills_dir: PathBuf,
}

impl SkillLoader {
    /// Create a new skill loader for the given directory
    pub fn new(skills_dir: &str) -> Self {
        Self {
            skills_dir: PathBuf::from(skills_dir),
        }
    }

    /// Scan the skills directory for valid skill packages
    pub fn scan(&self) -> Result<Vec<SkillManifest>, AgentError> {
        info!("Scanning skills directory: {:?}", self.skills_dir);

        if !self.skills_dir.exists() {
            warn!("Skills directory does not exist: {:?}", self.skills_dir);
            return Ok(Vec::new());
        }

        let mut manifests = Vec::new();

        // Iterate through subdirectories
        let entries = std::fs::read_dir(&self.skills_dir).map_err(|e| {
            AgentError::ConfigError(format!("Failed to read skills directory: {}", e))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Look for SKILL.toml in each subdirectory
                let skill_toml = path.join("SKILL.toml");
                if skill_toml.exists() {
                    match self.load_manifest(&path, &skill_toml) {
                        Ok(manifest) => {
                            info!("Loaded skill: {} (v{})", manifest.name, manifest.version);
                            manifests.push(manifest);
                        }
                        Err(e) => {
                            warn!("Failed to load skill from {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        info!("Found {} valid skills", manifests.len());
        Ok(manifests)
    }

    /// Load a single skill manifest
    fn load_manifest(&self, skill_dir: &Path, toml_path: &Path) -> Result<SkillManifest, AgentError> {
        let content = std::fs::read_to_string(toml_path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read SKILL.toml: {}", e)))?;

        let toml: SkillToml = toml::from_str(&content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to parse SKILL.toml: {}", e)))?;

        // Validate language
        match toml.skill.language.as_str() {
            "python" | "javascript" | "wasm" => {}
            other => {
                return Err(AgentError::ConfigError(format!(
                    "Unsupported language: {}",
                    other
                )));
            }
        }

        // Build entrypoint path
        let entrypoint = skill_dir.join(&toml.skill.entrypoint);
        if !entrypoint.exists() {
            return Err(AgentError::ConfigError(format!(
                "Entrypoint not found: {:?}",
                entrypoint
            )));
        }

        // Parse JSON schemas
        let input_schema: serde_json::Value = serde_json::from_str(&toml.input.schema)
            .map_err(|e| AgentError::ConfigError(format!("Invalid input schema: {}", e)))?;

        let output_schema: serde_json::Value = serde_json::from_str(&toml.output.schema)
            .map_err(|e| AgentError::ConfigError(format!("Invalid output schema: {}", e)))?;

        Ok(SkillManifest {
            name: toml.skill.name,
            description: toml.skill.description,
            version: toml.skill.version,
            language: toml.skill.language,
            entrypoint,
            input_schema,
            output_schema,
            topics: toml.metadata.topics,
            composes_with: toml.metadata.composes_with,
        })
    }

    /// Read the code content of a skill
    pub fn load_code(&self, manifest: &SkillManifest) -> Result<String, AgentError> {
        std::fs::read_to_string(&manifest.entrypoint)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read skill code: {}", e)))
    }

    /// Get the skills directory path
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }
}

/// File watcher for hot-reloading skills (simplified version)
pub struct SkillWatcher {
    loader: SkillLoader,
}

impl SkillWatcher {
    pub fn new(skills_dir: &str) -> Self {
        Self {
            loader: SkillLoader::new(skills_dir),
        }
    }

    /// Watch for changes and call the callback when skills are updated
    /// This is a simplified polling-based implementation
    pub async fn watch<F>(&self, mut callback: F) -> Result<(), AgentError>
    where
        F: FnMut(SkillManifest) + Send + 'static,
    {
        use std::collections::HashMap;
        use std::time::SystemTime;

        let mut last_modified: HashMap<PathBuf, SystemTime> = HashMap::new();

        loop {
            // Scan for changes
            let manifests = self.loader.scan()?;

            for manifest in manifests {
                let modified = std::fs::metadata(&manifest.entrypoint)
                    .and_then(|m| m.modified())
                    .ok();

                if let Some(mod_time) = modified {
                    let is_new_or_modified = last_modified
                        .get(&manifest.entrypoint)
                        .map(|&last| mod_time > last)
                        .unwrap_or(true);

                    if is_new_or_modified {
                        debug!("Skill changed: {}", manifest.name);
                        last_modified.insert(manifest.entrypoint.clone(), mod_time);
                        callback(manifest);
                    }
                }
            }

            // Poll every 5 seconds
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("test_skill");
        fs::create_dir(&skill_dir).unwrap();

        // Create SKILL.toml
        let skill_toml = r#"
[skill]
name = "test_skill"
description = "A test skill"
version = 1
language = "python"
entrypoint = "main.py"

[input]
schema = '''
{
  "type": "object",
  "properties": {
    "text": { "type": "string" }
  }
}
'''

[output]
schema = '''
{
  "type": "object",
  "properties": {
    "result": { "type": "string" }
  }
}
'''

[metadata]
topics = ["test", "demo"]
composes_with = ["web_fetch"]
"#;
        fs::write(skill_dir.join("SKILL.toml"), skill_toml).unwrap();

        // Create main.py
        fs::write(skill_dir.join("main.py"), "# Test skill").unwrap();

        // Load the manifest
        let loader = SkillLoader::new(temp_dir.path().to_str().unwrap());
        let manifests = loader.scan().unwrap();

        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "test_skill");
        assert_eq!(manifests[0].language, "python");
        assert_eq!(manifests[0].topics, vec!["test", "demo"]);
    }
}
