use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

use crate::tools::Tool;
use crate::types::{AgentError, ToolDefinition};

/// Web fetch tool that retrieves and extracts text from web pages
pub struct WebFetch {
    client: reqwest::Client,
}

impl WebFetch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }
}

impl Default for WebFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetch {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Fetches a web page and extracts its text content. Returns clean text without HTML tags.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(
        &self,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        let url = input
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ToolError("Missing 'url' parameter".to_string()))?;

        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(AgentError::ToolError(
                "URL must start with http:// or https://".to_string(),
            ));
        }

        let response = self
            .client
            .get(url)
            .header("User-Agent", "agent-brain/0.1.0 (https://github.com/example/agent-brain)")
            .send()
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to fetch URL: {}", e)))?;

        if !response.status().is_success() {
            return Err(AgentError::ToolError(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to read response: {}", e)))?;

        let text = strip_html(&html);
        let truncated = truncate_text(&text, 5000);

        Ok(truncated)
    }
}

/// Strip HTML tags using a simple state machine
fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_name = String::new();
    let mut collecting_tag_name = false;

    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                collecting_tag_name = true;
                tag_name.clear();
            }
            '>' => {
                in_tag = false;
                collecting_tag_name = false;

                let tag_lower = tag_name.to_lowercase();
                if tag_lower == "script" {
                    in_script = true;
                } else if tag_lower == "/script" {
                    in_script = false;
                } else if tag_lower == "style" {
                    in_style = true;
                } else if tag_lower == "/style" {
                    in_style = false;
                } else if tag_lower == "br"
                    || tag_lower == "br/"
                    || tag_lower == "p"
                    || tag_lower == "/p"
                    || tag_lower == "div"
                    || tag_lower == "/div"
                    || tag_lower == "li"
                    || tag_lower == "/li"
                    || tag_lower.starts_with("h")
                {
                    // Add newline for block elements
                    if !result.ends_with('\n') && !result.is_empty() {
                        result.push('\n');
                    }
                }
            }
            _ if in_tag => {
                if collecting_tag_name {
                    if c.is_whitespace() {
                        collecting_tag_name = false;
                    } else if c == '/' {
                        // Include '/' at the start for closing tags like </script>
                        // but stop if we already have content (self-closing like <br/>)
                        if tag_name.is_empty() {
                            tag_name.push(c);
                        } else {
                            collecting_tag_name = false;
                        }
                    } else {
                        tag_name.push(c);
                    }
                }
            }
            _ if in_script || in_style => {
                // Skip script and style content
            }
            _ => {
                result.push(c);
            }
        }
    }

    // Decode common HTML entities
    let result = result
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");

    // Clean up whitespace
    let lines: Vec<&str> = result
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();

    lines.join("\n")
}

/// Truncate text to a maximum length
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}...\n\n[Content truncated at {} characters]", &text[..max_len], max_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_html_simple() {
        let html = "<p>Hello <b>World</b></p>";
        let text = strip_html(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_strip_html_with_script() {
        let html = "<p>Before</p><script>alert('hi');</script><p>After</p>";
        let text = strip_html(html);
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "<p>&lt;hello&gt; &amp; &quot;world&quot;</p>";
        let text = strip_html(html);
        assert!(text.contains("<hello>"));
        assert!(text.contains("&"));
        assert!(text.contains("\"world\""));
    }

    #[test]
    fn test_truncate() {
        let text = "a".repeat(6000);
        let truncated = truncate_text(&text, 5000);
        assert!(truncated.len() < 5100);
        assert!(truncated.contains("[Content truncated"));
    }
}
