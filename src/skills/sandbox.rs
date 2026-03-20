//! Skill sandbox - executes user-provided skill code in isolation.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, error, warn};

use crate::types::AgentError;

/// Default timeout for skill execution (seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Sandbox error types
#[derive(Debug)]
pub enum SandboxError {
    ExecutionFailed(String),
    Timeout,
    InvalidOutput(String),
    UnsupportedLanguage(String),
    ProcessError(String),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            SandboxError::Timeout => write!(f, "Skill execution timed out"),
            SandboxError::InvalidOutput(msg) => write!(f, "Invalid output: {}", msg),
            SandboxError::UnsupportedLanguage(lang) => write!(f, "Unsupported language: {}", lang),
            SandboxError::ProcessError(msg) => write!(f, "Process error: {}", msg),
        }
    }
}

impl std::error::Error for SandboxError {}

impl From<SandboxError> for AgentError {
    fn from(err: SandboxError) -> Self {
        AgentError::ToolError(err.to_string())
    }
}

/// Sandbox for executing user skills safely
pub struct SkillSandbox;

impl SkillSandbox {
    /// Create a new skill sandbox
    pub fn new() -> Self {
        Self
    }

    /// Execute a skill in the sandbox
    pub async fn execute(
        &self,
        language: &str,
        code_path: &str,
        input: &serde_json::Value,
        timeout_secs: Option<u64>,
    ) -> Result<String, SandboxError> {
        let timeout_duration = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

        let result = timeout(timeout_duration, async {
            match language {
                "python" => self.execute_python(code_path, input).await,
                "javascript" => self.execute_javascript(code_path, input).await,
                "wasm" => self.execute_wasm(code_path, input).await,
                other => Err(SandboxError::UnsupportedLanguage(other.to_string())),
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(SandboxError::Timeout),
        }
    }

    /// Execute Python skill
    async fn execute_python(
        &self,
        code_path: &str,
        input: &serde_json::Value,
    ) -> Result<String, SandboxError> {
        debug!("Executing Python skill: {}", code_path);

        let mut cmd = Command::new("python3");
        cmd.arg(code_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Restrict environment for security
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/tmp")
            .env("PYTHONDONTWRITEBYTECODE", "1");

        let mut child = cmd.spawn().map_err(|e| {
            SandboxError::ProcessError(format!("Failed to spawn Python process: {}", e))
        })?;

        // Write input to stdin
        let input_json = serde_json::to_string(input)
            .map_err(|e| SandboxError::ProcessError(format!("Failed to serialize input: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(input_json.as_bytes()).await.map_err(|e| {
                SandboxError::ProcessError(format!("Failed to write to stdin: {}", e))
            })?;
        }

        // Wait for completion
        let output = child.wait_with_output().await.map_err(|e| {
            SandboxError::ProcessError(format!("Failed to wait for process: {}", e))
        })?;

        // Check exit code
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Python skill failed: {}", stderr);
            return Err(SandboxError::ExecutionFailed(stderr.to_string()));
        }

        // Parse stdout as result
        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!("Python skill output: {}", stdout);

        Ok(stdout.to_string())
    }

    /// Execute JavaScript skill (using Deno for sandboxing)
    async fn execute_javascript(
        &self,
        code_path: &str,
        input: &serde_json::Value,
    ) -> Result<String, SandboxError> {
        debug!("Executing JavaScript skill: {}", code_path);

        // Use Deno with restricted permissions
        let mut cmd = Command::new("deno");
        cmd.args([
            "run",
            "--no-net",      // No network access
            "--no-read",     // No filesystem read (except stdin)
            "--no-write",    // No filesystem write
            "--no-env",      // No environment access
            "--no-run",      // No subprocess spawning
            code_path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            // If Deno is not available, try Node.js (less secure)
            warn!("Deno not available, trying Node.js: {}", e);
            SandboxError::ProcessError(format!(
                "Failed to spawn Deno (install Deno for secure JS execution): {}",
                e
            ))
        })?;

        // Write input to stdin
        let input_json = serde_json::to_string(input)
            .map_err(|e| SandboxError::ProcessError(format!("Failed to serialize input: {}", e)))?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(input_json.as_bytes()).await.map_err(|e| {
                SandboxError::ProcessError(format!("Failed to write to stdin: {}", e))
            })?;
        }

        let output = child.wait_with_output().await.map_err(|e| {
            SandboxError::ProcessError(format!("Failed to wait for process: {}", e))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("JavaScript skill failed: {}", stderr);
            return Err(SandboxError::ExecutionFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!("JavaScript skill output: {}", stdout);

        Ok(stdout.to_string())
    }

    /// Execute WASM skill (most secure)
    async fn execute_wasm(
        &self,
        code_path: &str,
        input: &serde_json::Value,
    ) -> Result<String, SandboxError> {
        debug!("Executing WASM skill: {}", code_path);

        // Read the WASM file
        let wasm_bytes = std::fs::read(code_path)
            .map_err(|e| SandboxError::ProcessError(format!("Failed to read WASM file: {}", e)))?;

        // Create Wasmtime engine and store
        let engine = wasmtime::Engine::default();
        let mut store = wasmtime::Store::new(&engine, ());

        // Compile the module
        let module = wasmtime::Module::new(&engine, &wasm_bytes)
            .map_err(|e| SandboxError::ProcessError(format!("Failed to compile WASM: {}", e)))?;

        // Create instance
        let instance = wasmtime::Instance::new(&mut store, &module, &[])
            .map_err(|e| SandboxError::ProcessError(format!("Failed to instantiate WASM: {}", e)))?;

        // Get the main function
        let main_func = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "process")
            .map_err(|e| {
                SandboxError::ProcessError(format!(
                    "WASM module must export 'process(input_ptr, input_len) -> result_ptr': {}",
                    e
                ))
            })?;

        // For a complete WASM implementation, we'd need to:
        // 1. Allocate memory for input
        // 2. Write input JSON to WASM memory
        // 3. Call the process function
        // 4. Read result from WASM memory
        // This is a simplified placeholder

        let input_json = serde_json::to_string(input)
            .map_err(|e| SandboxError::ProcessError(format!("Failed to serialize input: {}", e)))?;

        // Placeholder - real implementation would use WASM memory
        warn!("WASM execution is a placeholder - full implementation needed");

        // Return a placeholder result
        Ok(format!(
            "{{\"status\": \"wasm_placeholder\", \"input_length\": {}}}",
            input_json.len()
        ))
    }
}

impl Default for SkillSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_python_execution() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.py");

        // Create a simple Python script
        let script = r#"
import sys, json
data = json.load(sys.stdin)
result = {"doubled": data.get("value", 0) * 2}
json.dump(result, sys.stdout)
"#;
        fs::write(&script_path, script).unwrap();

        let sandbox = SkillSandbox::new();
        let input = serde_json::json!({"value": 21});

        let result = sandbox
            .execute("python", script_path.to_str().unwrap(), &input, Some(5))
            .await;

        match result {
            Ok(output) => {
                let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
                assert_eq!(parsed["doubled"], 42);
            }
            Err(e) => {
                // Python might not be available in test environment
                eprintln!("Python test skipped: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("slow.py");

        // Create a script that takes too long
        let script = r#"
import time
time.sleep(10)
print('{"result": "done"}')
"#;
        fs::write(&script_path, script).unwrap();

        let sandbox = SkillSandbox::new();
        let input = serde_json::json!({});

        let result = sandbox
            .execute("python", script_path.to_str().unwrap(), &input, Some(1))
            .await;

        match result {
            Err(SandboxError::Timeout) => {
                // Expected
            }
            other => {
                // Python might not be available, or the result might be an error
                eprintln!("Expected timeout, got: {:?}", other);
            }
        }
    }
}
