//! Skill factory for generating new skills dynamically
//! Note: Currently a placeholder - skill generation will be added in future

use std::path::PathBuf;
use tracing::{info, warn};

use super::types::SkillTemplate;
use crate::types::AgentError;

/// SkillFactory generates new skills based on task requirements
/// Note: Skill generation is planned for future implementation
pub struct SkillFactory {
    skills_dir: PathBuf,
}

impl SkillFactory {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Save a skill to the skills directory
    pub async fn save_skill(&self, template: &SkillTemplate) -> Result<PathBuf, AgentError> {
        let extension = match template.language.as_str() {
            "python" => "py",
            "javascript" => "js",
            _ => "py",
        };

        let filename = format!("{}.{}", template.name, extension);
        let path = self.skills_dir.join(&filename);

        // Create skills directory if it doesn't exist
        tokio::fs::create_dir_all(&self.skills_dir)
            .await
            .map_err(|e| AgentError::ConfigError(format!("Failed to create skills dir: {}", e)))?;

        // Write the code file
        tokio::fs::write(&path, &template.code)
            .await
            .map_err(|e| AgentError::ConfigError(format!("Failed to write skill file: {}", e)))?;

        // Write the manifest file
        let manifest_path = self.skills_dir.join(format!("{}.json", template.name));
        let manifest = serde_json::json!({
            "name": template.name,
            "description": template.description,
            "language": template.language,
            "entry_point": filename,
            "input_schema": template.input_schema,
            "dependencies": template.dependencies
        });

        tokio::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
            .await
            .map_err(|e| AgentError::ConfigError(format!("Failed to write manifest: {}", e)))?;

        info!("Saved skill {} to {:?}", template.name, path);

        Ok(path)
    }

    /// Install dependencies for a skill
    pub async fn install_dependencies(&self, template: &SkillTemplate) -> Result<(), AgentError> {
        if template.dependencies.is_empty() {
            return Ok(());
        }

        info!("Installing dependencies for skill {}: {:?}", template.name, template.dependencies);

        match template.language.as_str() {
            "python" => {
                let output = tokio::process::Command::new("pip")
                    .args(["install", "--quiet"])
                    .args(&template.dependencies)
                    .output()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Failed to run pip: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!("pip install warning: {}", stderr);
                }
            }
            "javascript" => {
                let output = tokio::process::Command::new("npm")
                    .args(["install", "--save"])
                    .args(&template.dependencies)
                    .current_dir(&self.skills_dir)
                    .output()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Failed to run npm: {}", e)))?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!("npm install warning: {}", stderr);
                }
            }
            _ => {}
        }

        Ok(())
    }
}
