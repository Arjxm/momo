//! User-added skills system.
//!
//! Users can add custom skills by dropping files into the skills/ directory.
//! Each skill is a directory containing:
//! - SKILL.toml - Manifest file with metadata (for Executable skills)
//! - main.py / main.js / main.wasm - Implementation file
//! OR:
//! - skill.md - Markdown documentation with YAML frontmatter (for Knowledge skills)
//!
//! Executable skills communicate via stdin/stdout JSON.
//! Knowledge skills are injected into the system prompt when relevant.

pub mod loader;
pub mod matcher;
pub mod registry;
pub mod sandbox;

use serde::{Deserialize, Serialize};

/// Type of skill - executable code or knowledge documentation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SkillType {
    /// Executable skill (Python, JavaScript, WASM)
    Executable,
    /// Knowledge skill (Markdown documentation injected into context)
    Knowledge,
}

use std::sync::Arc;

use tracing::info;

use crate::graph::GraphBrain;
use crate::types::{AgentError, ToolNode};

pub use loader::{SkillLoader, SkillManifest};
pub use matcher::SkillMatcher;
pub use registry::SkillRegistry;
pub use sandbox::SkillSandbox;

/// High-level skill manager that coordinates loading, registration, and execution
pub struct SkillManager {
    registry: SkillRegistry,
}

impl SkillManager {
    /// Create a new skill manager
    pub fn new(brain: Arc<GraphBrain>, skills_dir: &str) -> Self {
        Self {
            registry: SkillRegistry::new(brain, skills_dir),
        }
    }

    /// Initialize the skill manager by loading all skills
    pub fn init(&mut self) -> Result<Vec<ToolNode>, AgentError> {
        info!("Initializing skill manager");
        self.registry.load_all()
    }

    /// Execute a skill by name
    pub async fn execute(&self, skill_name: &str, input: &serde_json::Value) -> Result<String, AgentError> {
        self.registry.execute_skill(skill_name, input).await
    }

    /// Check if a skill exists
    pub fn has_skill(&self, name: &str) -> bool {
        self.registry.has_skill(name)
    }

    /// List all available skills
    pub fn list_skills(&self) -> Vec<&SkillManifest> {
        self.registry.list_skills()
    }

    /// Get a skill manifest
    pub fn get_skill(&self, name: &str) -> Option<&SkillManifest> {
        self.registry.get_manifest(name)
    }

    /// Hot-reload a skill
    pub fn reload(&mut self, manifest: SkillManifest) -> Result<(), AgentError> {
        self.registry.hot_reload(manifest)
    }

    /// Get the underlying registry
    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Get a mutable reference to the registry
    pub fn registry_mut(&mut self) -> &mut SkillRegistry {
        &mut self.registry
    }

    /// Match user query against knowledge skills and return formatted documentation
    pub fn match_knowledge_skills(&self, query: &str) -> String {
        let knowledge_skills = self.registry.knowledge_skills();
        if knowledge_skills.is_empty() {
            return String::new();
        }

        let matched = SkillMatcher::match_skills(query, &knowledge_skills);
        if matched.is_empty() {
            return String::new();
        }

        info!("Matched {} knowledge skill(s) for query", matched.len());
        for skill in &matched {
            info!("  - {}: {}", skill.name,
                if skill.description.len() > 50 {
                    format!("{}...", &skill.description[..50])
                } else {
                    skill.description.clone()
                });
        }

        SkillMatcher::format_for_prompt(&matched)
    }
}
