//! Validator - Validates task outputs against specifications.
//!
//! Performs both:
//! - Fast checks (file existence, count verification) without LLM
//! - LLM-based qualitative validation when needed

use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::providers::{LLMProvider, Message};
use crate::types::AgentError;
use super::types::{
    TaskSpecification, ValidationResult, RequirementResult, OutputResult,
    MissingElement, NumericRequirement, ExpectedOutput, OutputType, ComparisonOp,
};

/// Validates task outputs against specifications
#[derive(Clone)]
pub struct Validator {
    provider: Arc<dyn LLMProvider>,
}

impl Validator {
    pub fn new(provider: Arc<dyn LLMProvider>) -> Self {
        Self { provider }
    }

    /// Validate task output against specification
    pub async fn validate(
        &self,
        spec: &TaskSpecification,
        task_output: &str,
        working_dir: Option<&str>,
    ) -> Result<ValidationResult, AgentError> {
        info!("[VALIDATE] Validating output against {} requirements",
            spec.numeric_requirements.len() + spec.expected_outputs.len() + spec.qualitative_requirements.len());

        let mut result = ValidationResult::success();
        let mut all_passed = true;

        // Phase 1: Fast checks (no LLM)
        // Check numeric requirements by scanning output
        for req in &spec.numeric_requirements {
            let req_result = self.check_numeric_requirement(req, task_output);
            if !req_result.passed {
                all_passed = false;
                result.missing_elements.push(MissingElement {
                    element: format!("{} {}", req.expected_count, req.entity),
                    category: "numeric".to_string(),
                    details: req_result.explanation.clone(),
                });
            }
            result.requirement_results.push(req_result);
        }

        // Check expected outputs (file existence)
        if let Some(dir) = working_dir {
            for output in &spec.expected_outputs {
                let out_result = self.check_output(output, dir);
                if !out_result.found && output.required {
                    all_passed = false;
                    result.missing_elements.push(MissingElement {
                        element: output.name.clone(),
                        category: "output".to_string(),
                        details: format!("Expected {} not found", output.name),
                    });
                }
                result.output_results.push(out_result);
            }
        }

        // Phase 2: LLM-based validation for qualitative requirements
        if !spec.qualitative_requirements.is_empty() {
            let qual_result = self.validate_qualitative(&spec.qualitative_requirements, task_output).await?;
            for req_result in qual_result {
                if !req_result.passed {
                    all_passed = false;
                    result.missing_elements.push(MissingElement {
                        element: req_result.requirement.clone(),
                        category: "qualitative".to_string(),
                        details: req_result.explanation.clone(),
                    });
                }
                result.requirement_results.push(req_result);
            }
        }

        // Calculate confidence
        let total_reqs = result.requirement_results.len() + result.output_results.len();
        let passed_reqs = result.requirement_results.iter().filter(|r| r.passed).count()
            + result.output_results.iter().filter(|r| r.found || !r.expected.required).count();

        result.confidence = if total_reqs > 0 {
            passed_reqs as f64 / total_reqs as f64
        } else {
            1.0
        };

        result.overall_success = all_passed;
        result.summary = if all_passed {
            "All requirements satisfied".to_string()
        } else {
            format!(
                "Failed {} of {} requirements: {}",
                result.missing_elements.len(),
                total_reqs,
                result.missing_elements.iter()
                    .map(|m| m.element.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        info!("[VALIDATE] Result: {} (confidence: {:.2})",
            if all_passed { "PASS" } else { "FAIL" }, result.confidence);

        Ok(result)
    }

    /// Check a numeric requirement against the output
    fn check_numeric_requirement(&self, req: &NumericRequirement, output: &str) -> RequirementResult {
        let output_lower = output.to_lowercase();
        let entity = req.entity.to_lowercase();

        // Try to find numbers associated with the entity
        // Look for patterns like "3 sites", "found 10 products", etc.
        let patterns = vec![
            format!(r"(\d+)\s*{}", regex::escape(&entity)),
            format!(r"{}\s*[:\-=]\s*(\d+)", regex::escape(&entity)),
            format!(r"found\s+(\d+)\s*{}", regex::escape(&entity)),
            format!(r"extracted\s+(\d+)\s*{}", regex::escape(&entity)),
            format!(r"searched\s+(\d+)\s*{}", regex::escape(&entity)),
        ];

        let mut found_count: Option<u32> = None;

        for pattern in patterns {
            if let Ok(re) = regex::Regex::new(&pattern) {
                if let Some(cap) = re.captures(&output_lower) {
                    if let Some(num_match) = cap.get(1) {
                        if let Ok(n) = num_match.as_str().parse::<u32>() {
                            found_count = Some(n);
                            break;
                        }
                    }
                }
            }
        }

        // Also count occurrences of entity-related terms
        let entity_singular = entity.trim_end_matches('s');
        let entity_count = output_lower.matches(&entity).count()
            + output_lower.matches(entity_singular).count();

        let actual_count = found_count.unwrap_or(entity_count as u32);

        let passed = match req.comparison {
            ComparisonOp::Exactly => actual_count == req.expected_count,
            ComparisonOp::AtLeast => actual_count >= req.expected_count,
            ComparisonOp::AtMost => actual_count <= req.expected_count,
        };

        let comparison_str = match req.comparison {
            ComparisonOp::Exactly => "exactly",
            ComparisonOp::AtLeast => "at least",
            ComparisonOp::AtMost => "at most",
        };

        RequirementResult {
            requirement: format!("{} {} {}", comparison_str, req.expected_count, req.entity),
            passed,
            actual_value: Some(actual_count.to_string()),
            expected_value: Some(req.expected_count.to_string()),
            explanation: if passed {
                format!("Found {} {} (expected {} {})", actual_count, req.entity, comparison_str, req.expected_count)
            } else {
                format!("Found only {} {} but expected {} {}", actual_count, req.entity, comparison_str, req.expected_count)
            },
        }
    }

    /// Check if an expected output exists
    fn check_output(&self, expected: &ExpectedOutput, working_dir: &str) -> OutputResult {
        let found = match expected.output_type {
            OutputType::File => {
                // Check for file existence
                let name = &expected.name;

                // Handle glob patterns
                if name.contains('*') || name.contains('?') {
                    let pattern = format!("{}/{}", working_dir, name);
                    glob::glob(&pattern)
                        .map(|paths| paths.filter_map(Result::ok).next().is_some())
                        .unwrap_or(false)
                } else {
                    let path = Path::new(working_dir).join(name);
                    path.exists()
                }
            }
            OutputType::Data | OutputType::Message | OutputType::Artifact => {
                // For non-file outputs, we can't easily check existence
                // Return true and let qualitative validation handle it
                true
            }
        };

        OutputResult {
            expected: expected.clone(),
            found,
            location: if found {
                Some(format!("{}/{}", working_dir, expected.name))
            } else {
                None
            },
        }
    }

    /// Validate qualitative requirements using LLM
    async fn validate_qualitative(
        &self,
        requirements: &[String],
        output: &str,
    ) -> Result<Vec<RequirementResult>, AgentError> {
        if requirements.is_empty() {
            return Ok(Vec::new());
        }

        let prompt = self.build_validation_prompt(requirements, output);
        let messages = vec![Message::user(&prompt)];

        let response = self.provider.chat(&messages, &[], Some(VALIDATION_SYSTEM_PROMPT)).await?;
        let response_text = response.text();

        debug!("[VALIDATE] LLM response: {}", &response_text);

        self.parse_validation_response(&response_text, requirements)
    }

    /// Build the validation prompt
    fn build_validation_prompt(&self, requirements: &[String], output: &str) -> String {
        let reqs_numbered: Vec<String> = requirements
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{}. {}", i + 1, r))
            .collect();

        format!(
            r#"Validate if this output satisfies these requirements:

REQUIREMENTS:
{}

OUTPUT:
{}

For each requirement, respond with JSON:
{{
  "results": [
    {{"requirement": "requirement text", "passed": true|false, "explanation": "why it passed or failed"}}
  ]
}}"#,
            reqs_numbered.join("\n"),
            if output.len() > 4000 { &output[..4000] } else { output }
        )
    }

    /// Parse the LLM validation response
    fn parse_validation_response(
        &self,
        response: &str,
        requirements: &[String],
    ) -> Result<Vec<RequirementResult>, AgentError> {
        // Extract JSON
        let json_str = extract_json(response);

        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| {
                warn!("[VALIDATE] Failed to parse JSON: {}", e);
                AgentError::ParseError(format!("Failed to parse validation JSON: {}", e))
            })?;

        let mut results = Vec::new();

        if let Some(arr) = parsed.get("results").and_then(|v| v.as_array()) {
            for item in arr {
                let requirement = item.get("requirement")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown requirement")
                    .to_string();

                let passed = item.get("passed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let explanation = item.get("explanation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("No explanation provided")
                    .to_string();

                results.push(RequirementResult {
                    requirement,
                    passed,
                    actual_value: None,
                    expected_value: None,
                    explanation,
                });
            }
        }

        // Ensure we have a result for each requirement
        while results.len() < requirements.len() {
            let i = results.len();
            results.push(RequirementResult {
                requirement: requirements[i].clone(),
                passed: false,
                actual_value: None,
                expected_value: None,
                explanation: "Could not validate this requirement".to_string(),
            });
        }

        Ok(results)
    }

    /// Quick validation without LLM (for simple numeric/output checks only)
    pub fn validate_quick(
        &self,
        spec: &TaskSpecification,
        task_output: &str,
        working_dir: Option<&str>,
    ) -> ValidationResult {
        let mut result = ValidationResult::success();
        let mut all_passed = true;

        // Check numeric requirements
        for req in &spec.numeric_requirements {
            let req_result = self.check_numeric_requirement(req, task_output);
            if !req_result.passed {
                all_passed = false;
            }
            result.requirement_results.push(req_result);
        }

        // Check outputs
        if let Some(dir) = working_dir {
            for output in &spec.expected_outputs {
                let out_result = self.check_output(output, dir);
                if !out_result.found && output.required {
                    all_passed = false;
                }
                result.output_results.push(out_result);
            }
        }

        result.overall_success = all_passed;
        result.summary = if all_passed {
            "Quick validation passed".to_string()
        } else {
            "Quick validation failed".to_string()
        };

        result
    }
}

/// System prompt for validation
const VALIDATION_SYSTEM_PROMPT: &str = r#"You are a task output validator. Your job is to check if a task's output satisfies given requirements.

Be strict but fair:
- A requirement is satisfied only if the output clearly demonstrates it was met
- Partial completion counts as failure
- Be specific about what's missing or incorrect

Respond with valid JSON only, no explanations outside the JSON."#;

/// Extract JSON from a response
fn extract_json(response: &str) -> String {
    if let Some(start) = response.find("```json") {
        if let Some(end) = response[start + 7..].find("```") {
            return response[start + 7..start + 7 + end].trim().to_string();
        }
    }

    if let Some(start) = response.find("```") {
        let after_start = &response[start + 3..];
        if let Some(end) = after_start.find("```") {
            let content = after_start[..end].trim();
            if content.starts_with('{') {
                return content.to_string();
            }
        }
    }

    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            return response[start..=end].to_string();
        }
    }

    response.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_numeric_requirement() {
        let validator = Validator {
            provider: Arc::new(MockProvider),
        };

        let req = NumericRequirement {
            entity: "sites".to_string(),
            expected_count: 3,
            comparison: ComparisonOp::AtLeast,
        };

        let output = "I searched 3 sites and found several products.";
        let result = validator.check_numeric_requirement(&req, output);
        assert!(result.passed);

        let output_fail = "I searched 2 sites.";
        let result_fail = validator.check_numeric_requirement(&req, output_fail);
        assert!(!result_fail.passed);
    }

    struct MockProvider;

    #[async_trait::async_trait]
    impl LLMProvider for MockProvider {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[crate::types::ToolDefinition],
            _system: Option<&str>,
        ) -> Result<crate::providers::LLMResponse, AgentError> {
            unimplemented!()
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn model(&self) -> &str {
            "mock-model"
        }
    }
}
