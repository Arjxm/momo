//! Skill loader - scans and loads user skill packages from the skills/ directory.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::skills::SkillType;
use crate::types::AgentError;

/// A loaded skill manifest
#[derive(Debug, Clone)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: u32,
    pub skill_type: SkillType,
    /// Language for Executable skills (python, javascript, wasm)
    pub language: Option<String>,
    /// Entrypoint for Executable skills
    pub entrypoint: Option<PathBuf>,
    /// Full markdown content for Knowledge skills
    pub content: Option<String>,
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

/// YAML frontmatter for markdown knowledge skills
#[derive(Debug, Deserialize, Default)]
struct MarkdownFrontmatter {
    name: Option<String>,
    description: Option<String>,
    version: Option<u32>,
    #[serde(default)]
    topics: Option<Vec<String>>,
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
                // Look for SKILL.toml (executable skill) first
                let skill_toml = path.join("SKILL.toml");
                if skill_toml.exists() {
                    match self.load_manifest(&path, &skill_toml) {
                        Ok(manifest) => {
                            info!("Loaded executable skill: {} (v{})", manifest.name, manifest.version);
                            manifests.push(manifest);
                        }
                        Err(e) => {
                            warn!("Failed to load skill from {:?}: {}", path, e);
                        }
                    }
                    continue; // Don't also check for skill.md if SKILL.toml exists
                }

                // Look for skill.md (knowledge skill)
                let skill_md = path.join("skill.md");
                if skill_md.exists() {
                    match self.load_markdown_skill(&path, &skill_md) {
                        Ok(manifest) => {
                            info!("Loaded knowledge skill: {} (v{})", manifest.name, manifest.version);
                            manifests.push(manifest);
                        }
                        Err(e) => {
                            warn!("Failed to load markdown skill from {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        info!("Found {} valid skills", manifests.len());
        Ok(manifests)
    }

    /// Load an executable skill manifest from SKILL.toml
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
            skill_type: SkillType::Executable,
            language: Some(toml.skill.language),
            entrypoint: Some(entrypoint),
            content: None,
            input_schema,
            output_schema,
            topics: toml.metadata.topics,
            composes_with: toml.metadata.composes_with,
        })
    }

    /// Load a knowledge skill from skill.md markdown file
    fn load_markdown_skill(&self, skill_dir: &Path, md_path: &Path) -> Result<SkillManifest, AgentError> {
        let content = std::fs::read_to_string(md_path)
            .map_err(|e| AgentError::ConfigError(format!("Failed to read skill.md: {}", e)))?;

        // Parse YAML frontmatter
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        let name = frontmatter.name.ok_or_else(|| {
            AgentError::ConfigError(format!("Missing 'name' in frontmatter: {:?}", skill_dir))
        })?;

        let description = frontmatter.description.ok_or_else(|| {
            AgentError::ConfigError(format!("Missing 'description' in frontmatter: {:?}", skill_dir))
        })?;

        info!("Loaded knowledge skill: {} from {:?}", name, md_path);

        Ok(SkillManifest {
            name,
            description,
            version: frontmatter.version.unwrap_or(1),
            skill_type: SkillType::Knowledge,
            language: None,
            entrypoint: None,
            content: Some(body),
            input_schema: serde_json::Value::Null,
            output_schema: serde_json::Value::Null,
            topics: frontmatter.topics.unwrap_or_default(),
            composes_with: Vec::new(),
        })
    }

    /// Parse YAML frontmatter from markdown content
    fn parse_frontmatter(content: &str) -> Result<(MarkdownFrontmatter, String), AgentError> {
        let content = content.trim();

        if !content.starts_with("---") {
            return Err(AgentError::ConfigError(
                "Markdown skill must have YAML frontmatter starting with ---".to_string()
            ));
        }

        // Find the closing ---
        let after_first = &content[3..];
        let end_index = after_first.find("\n---").ok_or_else(|| {
            AgentError::ConfigError("Missing closing --- for frontmatter".to_string())
        })?;

        let yaml_content = &after_first[..end_index].trim();
        let body = after_first[end_index + 4..].trim().to_string();

        let frontmatter: MarkdownFrontmatter = serde_yaml::from_str(yaml_content)
            .map_err(|e| AgentError::ConfigError(format!("Failed to parse frontmatter YAML: {}", e)))?;

        Ok((frontmatter, body))
    }

    /// Read the code content of an executable skill
    pub fn load_code(&self, manifest: &SkillManifest) -> Result<String, AgentError> {
        match &manifest.entrypoint {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|e| AgentError::ConfigError(format!("Failed to read skill code: {}", e))),
            None => Err(AgentError::ConfigError(
                "Cannot load code for knowledge skill (no entrypoint)".to_string()
            )),
        }
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
                // For executable skills, watch the entrypoint
                // For knowledge skills, we could watch the skill.md but for simplicity
                // we just trigger on the manifest.name change (re-scan will pick up changes)
                if let Some(ref entrypoint) = manifest.entrypoint {
                    let modified = std::fs::metadata(entrypoint)
                        .and_then(|m| m.modified())
                        .ok();

                    if let Some(mod_time) = modified {
                        let is_new_or_modified = last_modified
                            .get(entrypoint)
                            .map(|&last| mod_time > last)
                            .unwrap_or(true);

                        if is_new_or_modified {
                            debug!("Skill changed: {}", manifest.name);
                            last_modified.insert(entrypoint.clone(), mod_time);
                            callback(manifest);
                        }
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
        assert_eq!(manifests[0].language, Some("python".to_string()));
        assert_eq!(manifests[0].topics, vec!["test", "demo"]);
    }
}
