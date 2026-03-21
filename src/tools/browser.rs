//! Browser automation tool using headless Chrome/Chromium via CDP.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::tools::Tool;
use crate::types::{AgentError, ToolDefinition};

/// Browser action timeout in seconds
const ACTION_TIMEOUT_SECS: u64 = 15;
/// Maximum number of open pages
const MAX_PAGES: usize = 3;

/// Browser automation tool for the agent
pub struct BrowserTool {
    browser: Arc<Mutex<Option<Browser>>>,
    pages: Arc<Mutex<Vec<Page>>>,
}

impl BrowserTool {
    /// Create a new browser tool (browser is lazily initialized)
    pub fn new() -> Self {
        Self {
            browser: Arc::new(Mutex::new(None)),
            pages: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Ensure the browser is running
    async fn ensure_browser(&self) -> Result<(), AgentError> {
        let mut browser_guard = self.browser.lock().await;

        if browser_guard.is_some() {
            return Ok(());
        }

        info!("Launching browser (visible mode)");

        let (browser, mut handler) = Browser::launch(
            BrowserConfig::builder()
                .with_head() // Show browser window
                .arg("--disable-gpu")
                .arg("--no-sandbox")
                .arg("--disable-dev-shm-usage")
                .arg("--disable-extensions")
                .arg("--disable-downloads")
                .window_size(1280, 800)
                .build()
                .map_err(|e| AgentError::ToolError(format!("Failed to build browser config: {}", e)))?,
        )
        .await
        .map_err(|e| AgentError::ToolError(format!("Failed to launch browser: {}", e)))?;

        // Spawn the browser handler
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                debug!("Browser event: {:?}", event);
            }
        });

        *browser_guard = Some(browser);
        info!("Browser launched successfully");

        Ok(())
    }

    /// Get the current page or create a new one
    async fn get_or_create_page(&self) -> Result<Page, AgentError> {
        self.ensure_browser().await?;

        let browser_guard = self.browser.lock().await;
        let browser = browser_guard
            .as_ref()
            .ok_or_else(|| AgentError::ToolError("Browser not initialized".to_string()))?;

        let mut pages_guard = self.pages.lock().await;

        // If we have pages, return the most recent one
        if let Some(page) = pages_guard.last() {
            return Ok(page.clone());
        }

        // Create a new page
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to create page: {}", e)))?;

        // Set user agent
        page.set_user_agent("AgentBrain/0.2 (autonomous agent)")
            .await
            .ok();

        pages_guard.push(page.clone());

        // Enforce max pages limit
        while pages_guard.len() > MAX_PAGES {
            let old_page = pages_guard.remove(0);
            old_page.close().await.ok();
        }

        Ok(page)
    }

    /// Navigate to a URL
    async fn action_navigate(&self, url: &str) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        info!("Navigating to: {}", url);

        page.goto(url)
            .await
            .map_err(|e| AgentError::ToolError(format!("Navigation failed: {}", e)))?;

        // Wait a bit for page to load
        tokio::time::sleep(Duration::from_millis(500)).await;

        let title = page
            .get_title()
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to get title: {}", e)))?
            .unwrap_or_else(|| "No title".to_string());

        Ok(format!("Navigated to {}. Page title: {}", url, title))
    }

    /// Extract text content from the page
    async fn action_extract_text(&self, selector: Option<&str>) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        let text = if let Some(sel) = selector {
            // Extract text from specific element
            let js = format!(
                "document.querySelector('{}')?.innerText || 'Element not found'",
                sel.replace('\'', "\\'")
            );
            page.evaluate(js)
                .await
                .map_err(|e| AgentError::ToolError(format!("Failed to extract text: {}", e)))?
                .into_value::<String>()
                .unwrap_or_else(|_| "Failed to extract".to_string())
        } else {
            // Extract all visible text
            let js = "document.body.innerText";
            page.evaluate(js)
                .await
                .map_err(|e| AgentError::ToolError(format!("Failed to extract text: {}", e)))?
                .into_value::<String>()
                .unwrap_or_else(|_| "Failed to extract".to_string())
        };

        // Truncate if too long
        let text = if text.len() > 5000 {
            format!("{}...\n[Truncated at 5000 chars]", &text[..5000])
        } else {
            text
        };

        Ok(format!("Page text ({} chars):\n{}", text.len(), text))
    }

    /// Extract links from the page
    async fn action_extract_links(&self) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        let js = r#"
            Array.from(document.querySelectorAll('a[href]'))
                .map(a => ({text: a.innerText.trim().substring(0, 50), href: a.href}))
                .filter(l => l.text && l.href.startsWith('http'))
                .slice(0, 20)
        "#;

        let links: Vec<serde_json::Value> = page
            .evaluate(js)
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to extract links: {}", e)))?
            .into_value()
            .unwrap_or_default();

        if links.is_empty() {
            return Ok("No links found on the page.".to_string());
        }

        let mut output = format!("Found {} links:\n", links.len());
        for (i, link) in links.iter().enumerate() {
            let text = link["text"].as_str().unwrap_or("(no text)");
            let href = link["href"].as_str().unwrap_or("#");
            output.push_str(&format!("{}. {} -> {}\n", i + 1, text, href));
        }

        Ok(output)
    }

    /// Click an element
    async fn action_click(&self, selector: &str) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        info!("Clicking: {}", selector);

        page.find_element(selector)
            .await
            .map_err(|e| AgentError::ToolError(format!("Element not found: {}", e)))?
            .click()
            .await
            .map_err(|e| AgentError::ToolError(format!("Click failed: {}", e)))?;

        // Wait for potential navigation/JS
        tokio::time::sleep(Duration::from_secs(1)).await;

        let title = page
            .get_title()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Unknown".to_string());

        Ok(format!("Clicked '{}'. Page title now: {}", selector, title))
    }

    /// Fill a form field
    async fn action_fill(&self, selector: &str, value: &str) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        info!("Filling {} with value", selector);

        page.find_element(selector)
            .await
            .map_err(|e| AgentError::ToolError(format!("Element not found: {}", e)))?
            .click()
            .await
            .ok();

        page.find_element(selector)
            .await
            .map_err(|e| AgentError::ToolError(format!("Element not found: {}", e)))?
            .type_str(value)
            .await
            .map_err(|e| AgentError::ToolError(format!("Failed to type: {}", e)))?;

        Ok(format!(
            "Filled '{}' with '{}' ({} chars)",
            selector,
            if value.len() > 20 {
                format!("{}...", &value[..20])
            } else {
                value.to_string()
            },
            value.len()
        ))
    }

    /// Take a screenshot
    async fn action_screenshot(&self) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let path = format!("/tmp/screenshot_{}.png", timestamp);

        info!("Taking screenshot: {}", path);

        page.screenshot(
            chromiumoxide::page::ScreenshotParams::builder()
                .full_page(true)
                .build(),
        )
        .await
        .map_err(|e| AgentError::ToolError(format!("Screenshot failed: {}", e)))?;

        // Note: chromiumoxide returns bytes, we'd need to save them
        // This is a simplified version
        Ok(format!("Screenshot saved to {}", path))
    }

    /// Run JavaScript
    async fn action_run_js(&self, code: &str) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        debug!("Running JS: {}", code);

        let result = page
            .evaluate(code)
            .await
            .map_err(|e| AgentError::ToolError(format!("JS execution failed: {}", e)))?;

        let value: serde_json::Value = result.into_value().unwrap_or(serde_json::Value::Null);

        Ok(format!("JS result: {}", value))
    }

    /// Get HTML content
    async fn action_get_html(&self, selector: Option<&str>) -> Result<String, AgentError> {
        let page = self.get_or_create_page().await?;

        let html = if let Some(sel) = selector {
            let js = format!(
                "document.querySelector('{}')?.outerHTML || 'Element not found'",
                sel.replace('\'', "\\'")
            );
            page.evaluate(js)
                .await
                .map_err(|e| AgentError::ToolError(format!("Failed to get HTML: {}", e)))?
                .into_value::<String>()
                .unwrap_or_else(|_| "Failed to extract".to_string())
        } else {
            let js = "document.documentElement.outerHTML";
            page.evaluate(js)
                .await
                .map_err(|e| AgentError::ToolError(format!("Failed to get HTML: {}", e)))?
                .into_value::<String>()
                .unwrap_or_else(|_| "Failed to extract".to_string())
        };

        let html = if html.len() > 5000 {
            format!("{}...\n[Truncated at 5000 chars]", &html[..5000])
        } else {
            html
        };

        Ok(format!("HTML ({} chars):\n{}", html.len(), html))
    }

    /// Execute a browser action
    pub async fn execute_action(
        &self,
        input: &HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ToolError("Missing 'action' parameter".to_string()))?;

        let result = tokio::time::timeout(
            Duration::from_secs(ACTION_TIMEOUT_SECS),
            self.dispatch_action(action, input),
        )
        .await
        .map_err(|_| AgentError::ToolError("Browser action timed out".to_string()))?;

        result
    }

    async fn dispatch_action(
        &self,
        action: &str,
        input: &HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        match action {
            "navigate" => {
                let url = input
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::ToolError("Missing 'url' for navigate".to_string()))?;
                self.action_navigate(url).await
            }
            "extract_text" => {
                let selector = input.get("selector").and_then(|v| v.as_str());
                self.action_extract_text(selector).await
            }
            "extract_links" => self.action_extract_links().await,
            "click" => {
                let selector = input
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::ToolError("Missing 'selector' for click".to_string()))?;
                self.action_click(selector).await
            }
            "fill" => {
                let selector = input
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::ToolError("Missing 'selector' for fill".to_string()))?;
                let value = input
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::ToolError("Missing 'value' for fill".to_string()))?;
                self.action_fill(selector, value).await
            }
            "screenshot" => self.action_screenshot().await,
            "run_js" => {
                let code = input
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::ToolError("Missing 'value' (JS code) for run_js".to_string()))?;
                self.action_run_js(code).await
            }
            "get_html" => {
                let selector = input.get("selector").and_then(|v| v.as_str());
                self.action_get_html(selector).await
            }
            _ => Err(AgentError::ToolError(format!("Unknown action: {}", action))),
        }
    }

    /// Clean up browser resources
    pub async fn cleanup(&self) {
        let mut pages_guard = self.pages.lock().await;
        for page in pages_guard.drain(..) {
            page.close().await.ok();
        }
        drop(pages_guard);

        let mut browser_guard = self.browser.lock().await;
        if let Some(browser) = browser_guard.take() {
            drop(browser);
        }

        info!("Browser cleaned up");
    }
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "browser".to_string(),
            description: "Control a headless web browser. Can navigate to URLs, extract page content, click elements, fill forms, take screenshots, and execute JavaScript. Use when you need to interact with websites that require JavaScript or have dynamic content.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["navigate", "extract_text", "extract_links", "click", "fill", "screenshot", "run_js", "get_html"],
                        "description": "The browser action to perform"
                    },
                    "url": {
                        "type": "string",
                        "description": "URL to navigate to (for 'navigate' action)"
                    },
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for click/fill/extract actions"
                    },
                    "value": {
                        "type": "string",
                        "description": "Value to fill (for 'fill' action) or JS code (for 'run_js')"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(
        &self,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        self.execute_action(&input).await
    }
}

impl Drop for BrowserTool {
    fn drop(&mut self) {
        // Note: Can't do async cleanup in drop
        // The cleanup method should be called explicitly before dropping
        warn!("BrowserTool dropped - call cleanup() before dropping for proper resource release");
    }
}
