//! Skill registry - registers loaded skills as Tool nodes in the graph.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::graph::GraphBrain;
use crate::skills::loader::{SkillLoader, SkillManifest};
use crate::skills::sandbox::SkillSandbox;
use crate::types::{AgentError, ToolNode, ToolType};

/// Registry that manages user skills and their integration with the graph
pub struct SkillRegistry {
    brain: Arc<GraphBrain>,
    loader: SkillLoader,
    sandbox: SkillSandbox,
    /// Cache of loaded manifests by skill name
    manifests: HashMap<String, SkillManifest>,
}

impl SkillRegistry {
    /// Create a new skill registry
    pub fn new(brain: Arc<GraphBrain>, skills_dir: &str) -> Self {
        Self {
            brain,
            loader: SkillLoader::new(skills_dir),
            sandbox: SkillSandbox::new(),
            manifests: HashMap::new(),
        }
    }

    /// Load all skills from the skills directory and register them in the graph
    pub fn load_all(&mut self) -> Result<Vec<ToolNode>, AgentError> {
        info!("Loading all skills from {:?}", self.loader.skills_dir());

        let manifests = self.loader.scan()?;

        let mut registered_tools = Vec::new();
        for manifest in manifests {
            match self.register_skill(&manifest) {
                Ok(tool) => {
                    self.manifests.insert(manifest.name.clone(), manifest);
                    registered_tools.push(tool);
                }
                Err(e) => {
                    warn!("Failed to register skill {}: {}", manifest.name, e);
                }
            }
        }

        info!("Registered {} skills", registered_tools.len());
        Ok(registered_tools)
    }

    /// Register a single skill in the graph
    fn register_skill(&self, manifest: &SkillManifest) -> Result<ToolNode, AgentError> {
        // Create the tool node
        let tool_node = ToolNode::new(
            manifest.name.clone(),
            manifest.description.clone(),
            ToolType::Skill,
            manifest.input_schema.clone(),
            manifest.entrypoint.to_string_lossy().to_string(),
        );

        // Register in graph
        self.brain.register_tool(&tool_node)?;

        // Link to topics
        for topic in &manifest.topics {
            self.brain.ensure_topic(topic)?;
            self.brain.link_tool_topic(&manifest.name, topic)?;
        }

        // Record declared compositions
        for other_tool in &manifest.composes_with {
            self.brain.record_composition(
                &manifest.name,
                other_tool,
                &format!("{} works with {}", manifest.name, other_tool),
            )?;
        }

        debug!(
            "Registered skill: {} (v{}, {})",
            manifest.name, manifest.version, manifest.language
        );

        Ok(tool_node)
    }

    /// Execute a skill by name
    pub async fn execute_skill(
        &self,
        skill_name: &str,
        input: &serde_json::Value,
    ) -> Result<String, AgentError> {
        // Get the manifest for this skill
        let manifest = self.manifests.get(skill_name).ok_or_else(|| {
            AgentError::ToolError(format!("Skill not found: {}", skill_name))
        })?;

        debug!("Executing skill: {} ({})", skill_name, manifest.language);

        // Execute in sandbox
        let result = self
            .sandbox
            .execute(
                &manifest.language,
                &manifest.entrypoint.to_string_lossy(),
                input,
                Some(10), // 10 second timeout
            )
            .await
            .map_err(|e| AgentError::ToolError(format!("Skill execution failed: {}", e)))?;

        // Update tool stats
        self.brain.update_tool_stats(skill_name, true)?;

        Ok(result)
    }

    /// Hot-reload a skill when its files change
    pub fn hot_reload(&mut self, manifest: SkillManifest) -> Result<(), AgentError> {
        info!(
            "Hot-reloading skill: {} (v{})",
            manifest.name, manifest.version
        );

        // Re-register the skill
        self.register_skill(&manifest)?;

        // Update the cached manifest
        self.manifests.insert(manifest.name.clone(), manifest);

        Ok(())
    }

    /// Get a skill manifest by name
    pub fn get_manifest(&self, name: &str) -> Option<&SkillManifest> {
        self.manifests.get(name)
    }

    /// List all loaded skills
    pub fn list_skills(&self) -> Vec<&SkillManifest> {
        self.manifests.values().collect()
    }

    /// Check if a skill exists
    pub fn has_skill(&self, name: &str) -> bool {
        self.manifests.contains_key(name)
    }

    /// Get the loader for external access
    pub fn loader(&self) -> &SkillLoader {
        &self.loader
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(dir: &std::path::Path) {
        let skill_dir = dir.join("word_count");
        fs::create_dir_all(&skill_dir).unwrap();

        let skill_toml = r#"
[skill]
name = "word_count"
description = "Counts words in text"
version = 1
language = "python"
entrypoint = "main.py"

[input]
schema = '{"type": "object", "properties": {"text": {"type": "string"}}}'

[output]
schema = '{"type": "object", "properties": {"count": {"type": "integer"}}}'

[metadata]
topics = ["text", "nlp"]
"#;
        fs::write(skill_dir.join("SKILL.toml"), skill_toml).unwrap();

        let main_py = r#"
import sys, json
data = json.load(sys.stdin)
text = data.get("text", "")
count = len(text.split())
json.dump({"count": count}, sys.stdout)
"#;
        fs::write(skill_dir.join("main.py"), main_py).unwrap();
    }

    // Note: Full integration tests would require a real GraphBrain instance
    // These tests focus on the loader integration

    #[test]
    fn test_skill_manifest_caching() {
        let temp_dir = TempDir::new().unwrap();
        create_test_skill(temp_dir.path());

        let loader = SkillLoader::new(temp_dir.path().to_str().unwrap());
        let manifests = loader.scan().unwrap();

        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name, "word_count");
    }
}
