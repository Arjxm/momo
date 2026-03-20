mod agent;
mod claude;
mod config;
mod graph;
mod skills;
mod tools;
mod types;

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::agent::Agent;
use crate::claude::ClaudeClient;
use crate::config::AppConfig;
use crate::graph::GraphBrain;
use crate::skills::SkillManager;
use crate::tools::mcp_bridge::MCPBridge;
use crate::tools::{ArxivSearch, BrowserTool, Calculator, Tool, ToolRegistry, WebFetch};
use crate::types::{AgentConfig, ToolNode, ToolType};

// Cost per million tokens (approximate for Claude Sonnet)
const INPUT_COST_PER_MILLION: f64 = 3.0;
const OUTPUT_COST_PER_MILLION: f64 = 15.0;

fn print_banner() {
    println!();
    println!("  ___                  _     ___          _      ");
    println!(" / _ \\                | |   | _ )_ _ __ _(_)_ _  ");
    println!("| |_| |__ _ ___ _ _  _| |_  | _ \\ '_/ _` | | ' \\ ");
    println!(" \\___/___| '_  | '_||_____|_|___/_| \\__,_|_|_||_|");
    println!("         |_| |_|_|                               ");
    println!();
    println!("Welcome to Agent Brain v0.2 - AI assistant with persistent memory");
    println!("Type your questions or commands. Type 'quit', 'exit', or 'stats' for info.");
    println!();
}

fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("agent_brain=info".parse().unwrap())
                .add_directive("reqwest=warn".parse().unwrap())
                .add_directive("chromiumoxide=warn".parse().unwrap())
                .add_directive("ladybugdb=warn".parse().unwrap()),
        )
        .init();

    // Load environment variables from .env file
    if let Err(e) = dotenvy::dotenv() {
        if !e.to_string().contains("not found") {
            eprintln!("Warning: Error loading .env file: {}", e);
        }
    }

    // Load application configuration
    let app_config = match AppConfig::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("Please set ANTHROPIC_API_KEY in your environment or .env file.");
            std::process::exit(1);
        }
    };

    // Initialize the graph brain
    info!("Initializing graph brain at: {}", app_config.db_path);
    let brain = match GraphBrain::open(&app_config.db_path) {
        Ok(brain) => Arc::new(brain),
        Err(e) => {
            eprintln!("Error: Failed to initialize graph brain: {}", e);
            std::process::exit(1);
        }
    };

    // Create tool registry
    let mut registry = ToolRegistry::new();

    // Register native tools
    let native_tools = register_native_tools(&mut registry, &brain);
    info!("Registered {} native tools", native_tools.len());

    // Initialize skill manager
    let skill_manager = Arc::new(tokio::sync::Mutex::new(SkillManager::new(
        brain.clone(),
        &app_config.skills_dir,
    )));

    // Load user skills
    let skill_tools = {
        let mut manager = skill_manager.lock().await;
        match manager.init() {
            Ok(tools) => {
                for tool in &tools {
                    registry.register_with_type(
                        SkillToolWrapper {
                            name: tool.name.clone(),
                            description: tool.description.clone(),
                            input_schema: tool.input_schema.clone(),
                        },
                        ToolType::Skill,
                    );
                }
                tools
            }
            Err(e) => {
                eprintln!("Warning: Failed to load skills: {}", e);
                Vec::new()
            }
        }
    };
    info!("Loaded {} user skills", skill_tools.len());

    // Initialize MCP bridge
    let mcp_bridge = Arc::new(tokio::sync::Mutex::new(MCPBridge::new(brain.clone())));

    // Connect to auto-connect MCP servers
    {
        let mut bridge = mcp_bridge.lock().await;
        if let Err(e) = bridge.connect_all_from_config(&app_config.mcp_config_path).await {
            eprintln!("Warning: Failed to connect to some MCP servers: {}", e);
        }

        // Register MCP tools
        let mcp_tools = brain.get_tools_by_type("mcp").unwrap_or_default();
        for tool in &mcp_tools {
            registry.register_with_type(
                MCPToolWrapper {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                },
                ToolType::Mcp,
            );
        }
        info!("Connected to {} MCP servers", bridge.connected_servers().len());
    }

    // Register browser tool
    let browser = Arc::new(BrowserTool::new());
    registry.register_with_type(browser.as_ref().clone(), ToolType::Browser);
    info!("Browser tool registered");

    // Create agent configuration
    let agent_config = AgentConfig::default();

    // Create Claude client
    let client = ClaudeClient::new(app_config.anthropic_api_key.clone(), agent_config.clone());

    // Create the agent
    let agent = Agent::new(
        client,
        registry,
        agent_config,
        brain.clone(),
        app_config.default_user_id.clone(),
    )
    .with_memory_extractor(app_config.anthropic_api_key.clone())
    .with_skill_manager(skill_manager)
    .with_mcp_bridge(mcp_bridge)
    .with_browser(browser.clone());

    // Print startup summary
    print_banner();
    if let Ok(stats) = brain.stats() {
        println!("{}", stats);
    }
    println!();

    // Interactive REPL
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut total_input_tokens = 0u32;
    let mut total_output_tokens = 0u32;

    loop {
        print!("> ");
        stdout.flush().unwrap();

        let mut input = String::new();
        match stdin.lock().read_line(&mut input) {
            Ok(0) => {
                println!("\nGoodbye!");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                error!("Error reading input: {}", e);
                continue;
            }
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Handle special commands
        if input.eq_ignore_ascii_case("quit") || input.eq_ignore_ascii_case("exit") {
            print_session_summary(total_input_tokens, total_output_tokens, &brain);
            break;
        }

        if input.eq_ignore_ascii_case("stats") {
            print_stats(&brain);
            continue;
        }

        // Run the agent
        println!();
        match agent.run(input).await {
            Ok(result) => {
                println!("{}", result.response);
                println!();

                let tools_str = if result.tools_used.is_empty() {
                    String::new()
                } else {
                    format!(" | Tools: {}", result.tools_used.join(", "))
                };

                println!(
                    "[Tokens: {} in, {} out | Cost: ${:.4}{}]",
                    result.usage.input_tokens,
                    result.usage.output_tokens,
                    estimate_cost(result.usage.input_tokens, result.usage.output_tokens),
                    tools_str
                );

                total_input_tokens += result.usage.input_tokens;
                total_output_tokens += result.usage.output_tokens;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
        println!();
    }

    // Cleanup browser
    browser.cleanup().await;
}

/// Register native tools and add them to the graph
fn register_native_tools(registry: &mut ToolRegistry, brain: &Arc<GraphBrain>) -> Vec<ToolNode> {
    let mut tools = Vec::new();

    // Calculator
    let calc = Calculator::new();
    let calc_def = calc.definition();
    let calc_node = ToolNode::new(
        calc_def.name.clone(),
        calc_def.description.clone(),
        ToolType::Native,
        calc_def.input_schema.clone(),
        "builtin".to_string(),
    );
    brain.register_tool(&calc_node).ok();
    brain.link_tool_topic(&calc_node.name, "math").ok();
    registry.register(calc);
    tools.push(calc_node);

    // ArXiv search
    let arxiv = ArxivSearch::new();
    let arxiv_def = arxiv.definition();
    let arxiv_node = ToolNode::new(
        arxiv_def.name.clone(),
        arxiv_def.description.clone(),
        ToolType::Native,
        arxiv_def.input_schema.clone(),
        "builtin".to_string(),
    );
    brain.register_tool(&arxiv_node).ok();
    brain.link_tool_topic(&arxiv_node.name, "research").ok();
    brain.link_tool_topic(&arxiv_node.name, "papers").ok();
    registry.register(arxiv);
    tools.push(arxiv_node);

    // Web fetch
    let web = WebFetch::new();
    let web_def = web.definition();
    let web_node = ToolNode::new(
        web_def.name.clone(),
        web_def.description.clone(),
        ToolType::Native,
        web_def.input_schema.clone(),
        "builtin".to_string(),
    );
    brain.register_tool(&web_node).ok();
    brain.link_tool_topic(&web_node.name, "web").ok();
    registry.register(web);
    tools.push(web_node);

    tools
}

fn print_stats(brain: &Arc<GraphBrain>) {
    if let Ok(stats) = brain.stats() {
        println!();
        println!("{}", stats);
        println!();
    } else {
        println!("Failed to get graph stats.");
    }
}

fn print_session_summary(input_tokens: u32, output_tokens: u32, brain: &Arc<GraphBrain>) {
    println!("\nSession summary:");
    println!(
        "  Total tokens: {} input, {} output",
        input_tokens, output_tokens
    );
    println!(
        "  Estimated cost: ${:.4}",
        estimate_cost(input_tokens, output_tokens)
    );

    if let Ok(stats) = brain.stats() {
        println!("  {}", stats);
    }

    println!("\nGoodbye!");
}

// ═══════════════════════════════════════════════════════════════════
// TOOL WRAPPERS
// ═══════════════════════════════════════════════════════════════════

/// Wrapper to make BrowserTool clonable for registration
impl Clone for BrowserTool {
    fn clone(&self) -> Self {
        BrowserTool::new()
    }
}

/// Wrapper for skill tools (execute via skill manager)
struct SkillToolWrapper {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[async_trait::async_trait]
impl crate::tools::Tool for SkillToolWrapper {
    fn definition(&self) -> crate::types::ToolDefinition {
        crate::types::ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    async fn execute(
        &self,
        _input: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<String, crate::types::AgentError> {
        // Actual execution happens via skill manager in agent.rs
        Ok("Skill tool placeholder - execution handled by agent".to_string())
    }
}

/// Wrapper for MCP tools (execute via MCP bridge)
struct MCPToolWrapper {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[async_trait::async_trait]
impl crate::tools::Tool for MCPToolWrapper {
    fn definition(&self) -> crate::types::ToolDefinition {
        crate::types::ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }

    async fn execute(
        &self,
        _input: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<String, crate::types::AgentError> {
        // Actual execution happens via MCP bridge in agent.rs
        Ok("MCP tool placeholder - execution handled by agent".to_string())
    }
}
