use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use super::types::{AgentType, Plan, TaskNode, TaskPriority};
use crate::providers::{LLMProvider, Message};
use crate::types::AgentError;

/// The Planner agent decomposes complex tasks into subtasks
pub struct Planner {
    provider: Arc<dyn LLMProvider>,
    available_tools: Vec<String>,
}

impl Planner {
    pub fn new(provider: Arc<dyn LLMProvider>, available_tools: Vec<String>) -> Self {
        Self {
            provider,
            available_tools,
        }
    }

    fn system_prompt() -> String {
        r#"You are a Task Planner AI. Your job is to decompose complex user requests into smaller, executable subtasks.

For each request, analyze what needs to be done and break it into subtasks. Each subtask should:
1. Be assigned to the most appropriate agent type
2. Have clear dependencies on other subtasks
3. Include enough context for the worker to execute

AGENT TYPES:
- research: Web search, document analysis, information gathering
- code: Write code, analyze code, fix bugs, create files
- comms: Send emails, messages, notifications
- data: Database queries, data transformation, analysis
- browser: Web automation, scraping, form filling

OUTPUT FORMAT:
Return a JSON object with this structure:
{
  "analysis": "Brief analysis of the request",
  "tasks": [
    {
      "id": "task_1",
      "description": "What needs to be done",
      "agent_type": "research|code|comms|data|browser",
      "dependencies": [],
      "tool_hint": "optional suggested tool name",
      "priority": "low|normal|high|urgent",
      "context": "Any additional context for the worker"
    }
  ]
}

RULES:
1. Task IDs should be simple: task_1, task_2, etc.
2. Dependencies reference other task IDs
3. Put independent tasks first (no dependencies)
4. Keep tasks atomic - one clear action each
5. Only include tasks that can be executed with available tools
6. If a task needs the result of another, add it to dependencies

EXAMPLE:
Request: "Research the top 3 AI startups and write a summary report"

{
  "analysis": "Need to search for AI startups, gather info, then synthesize into report",
  "tasks": [
    {
      "id": "task_1",
      "description": "Search for top AI startups in 2024",
      "agent_type": "research",
      "dependencies": [],
      "tool_hint": "arxiv_search",
      "priority": "high",
      "context": "Look for recent news and rankings"
    },
    {
      "id": "task_2",
      "description": "Gather details on each startup: funding, product, team",
      "agent_type": "browser",
      "dependencies": ["task_1"],
      "priority": "normal",
      "context": "Visit company websites and LinkedIn"
    },
    {
      "id": "task_3",
      "description": "Write summary report in markdown",
      "agent_type": "code",
      "dependencies": ["task_1", "task_2"],
      "tool_hint": "write_file",
      "priority": "normal",
      "context": "Create workspace/ai_startups_report.md"
    }
  ]
}

Always respond with valid JSON only."#.to_string()
    }

    /// Decompose a user request into a plan
    pub async fn decompose(&self, request: &str) -> Result<Plan, AgentError> {
        info!("Planner decomposing request: {}", request);

        let tools_list = self.available_tools.join(", ");
        let prompt = format!(
            "Available tools: {}\n\nUser request: {}\n\nCreate a task plan:",
            tools_list, request
        );

        let messages = vec![Message::user(&prompt)];

        // Send to provider without tools (pure reasoning)
        let response = self.provider.chat(&messages, &[], Some(&Self::system_prompt())).await?;

        let text = response.text();
        debug!("Planner response: {}", text);

        // Parse the JSON response
        let plan = self.parse_plan_response(&text, request)?;

        info!(
            "Created plan with {} tasks: {:?}",
            plan.tasks.len(),
            plan.tasks.iter().map(|t| &t.description).collect::<Vec<_>>()
        );

        Ok(plan)
    }

    /// Parse the planner's JSON response into a Plan
    fn parse_plan_response(&self, response: &str, original_request: &str) -> Result<Plan, AgentError> {
        // Try to extract JSON from the response (may have markdown code blocks)
        let json_str = Self::extract_json(response);

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| AgentError::ParseError(format!("Failed to parse plan JSON: {}", e)))?;

        let tasks_array = parsed.get("tasks")
            .and_then(|t| t.as_array())
            .ok_or_else(|| AgentError::ParseError("No tasks array in plan".to_string()))?;

        let mut tasks = Vec::new();
        let mut id_map: HashMap<String, String> = HashMap::new();

        for task_json in tasks_array {
            let old_id = task_json.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("unknown")
                .to_string();

            let description = task_json.get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("No description")
                .to_string();

            let agent_type = Self::parse_agent_type(
                task_json.get("agent_type")
                    .and_then(|a| a.as_str())
                    .unwrap_or("code")
            );

            let deps: Vec<String> = task_json.get("dependencies")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let context = task_json.get("context")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();

            let tool_hint = task_json.get("tool_hint")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());

            let priority = Self::parse_priority(
                task_json.get("priority")
                    .and_then(|p| p.as_str())
                    .unwrap_or("normal")
            );

            let mut task = TaskNode::new(description, agent_type, deps, context);
            task = task.with_priority(priority);
            if let Some(hint) = tool_hint {
                task = task.with_tool_hint(hint);
            }

            // Map old ID to new UUID
            id_map.insert(old_id, task.id.clone());
            tasks.push(task);
        }

        // Update dependencies to use new UUIDs
        for task in &mut tasks {
            task.dependencies = task.dependencies
                .iter()
                .filter_map(|old_id| id_map.get(old_id).cloned())
                .collect();
        }

        Ok(Plan::new(original_request.to_string(), tasks))
    }

    /// Extract JSON from response (handles markdown code blocks)
    fn extract_json(response: &str) -> String {
        // Try to find JSON in code blocks first
        if let Some(start) = response.find("```json") {
            if let Some(end) = response[start + 7..].find("```") {
                return response[start + 7..start + 7 + end].trim().to_string();
            }
        }

        // Try plain code blocks
        if let Some(start) = response.find("```") {
            if let Some(end) = response[start + 3..].find("```") {
                let content = response[start + 3..start + 3 + end].trim();
                // Skip language identifier if present
                if let Some(newline) = content.find('\n') {
                    return content[newline + 1..].trim().to_string();
                }
                return content.to_string();
            }
        }

        // Try to find raw JSON object
        if let Some(start) = response.find('{') {
            if let Some(end) = response.rfind('}') {
                return response[start..=end].to_string();
            }
        }

        response.to_string()
    }

    fn parse_agent_type(s: &str) -> AgentType {
        match s.to_lowercase().as_str() {
            "research" => AgentType::Research,
            "code" => AgentType::Code,
            "comms" => AgentType::Comms,
            "data" => AgentType::Data,
            "browser" => AgentType::Browser,
            _ => AgentType::Code, // Default
        }
    }

    fn parse_priority(s: &str) -> TaskPriority {
        match s.to_lowercase().as_str() {
            "low" => TaskPriority::Low,
            "normal" => TaskPriority::Normal,
            "high" => TaskPriority::High,
            "urgent" => TaskPriority::Urgent,
            _ => TaskPriority::Normal,
        }
    }

    /// Check if a request is simple enough to skip planning
    pub fn should_skip_planning(request: &str) -> bool {
        let simple_patterns = [
            "calculate", "what is", "how much", "convert",
            "hello", "hi", "thanks", "bye",
        ];

        let request_lower = request.to_lowercase();

        // Skip if very short
        if request.split_whitespace().count() < 5 {
            return true;
        }

        // Skip for simple patterns
        for pattern in &simple_patterns {
            if request_lower.starts_with(pattern) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json() {
        let response = r#"Here's the plan:
```json
{"tasks": []}
```"#;
        assert_eq!(Planner::extract_json(response), r#"{"tasks": []}"#);
    }

    #[test]
    fn test_should_skip_planning() {
        assert!(Planner::should_skip_planning("hello"));
        assert!(Planner::should_skip_planning("what is 2+2"));
        assert!(!Planner::should_skip_planning("Research the top AI startups and write a detailed report with funding information"));
    }
}
