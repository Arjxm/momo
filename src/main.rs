mod agent;
mod claude;
mod config;
mod debug_server;
mod graph;
mod orchestrator;
mod providers;
mod skills;
mod tools;
mod tui;
mod types;

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind};

use crate::tui::{App, Tui};

use crate::agent::Agent;
use crate::config::AppConfig;
use crate::graph::GraphBrain;
use crate::orchestrator::Orchestrator;
use crate::providers::{create_provider, ProviderConfig, ProviderType};
use crate::skills::SkillManager;
use crate::tools::mcp_bridge::MCPBridge;
use crate::tools::{
    ArxivSearch, Calculator, ExchangeRates, HackerNews, Tool, ToolRegistry, Weather,
    WebFetch, Wikipedia,
};
use crate::types::{AgentConfig, ToolNode, ToolType};

// Cost per million tokens (approximate for Claude Sonnet)
const INPUT_COST_PER_MILLION: f64 = 3.0;
const OUTPUT_COST_PER_MILLION: f64 = 15.0;

fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    let input_cost = (input_tokens as f64 / 1_000_000.0) * INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

#[tokio::main]
async fn main() {
    // Initialize file-based tracing for debug logs
    // Tail logs in another terminal: tail -f /tmp/momo.log
    let log_file = std::fs::File::create("/tmp/momo.log").ok();
    if let Some(file) = log_file {
        use tracing_subscriber::{fmt, EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
        let file_layer = fmt::layer()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_target(true)
            .with_level(true);

        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,momo=debug"));

        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .init();
    }

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

    // Collect startup messages for TUI
    let mut startup_logs: Vec<String> = Vec::new();

    // Initialize the graph brain
    startup_logs.push(format!("Initializing graph brain at: {}", app_config.db_path));
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
    startup_logs.push(format!("Registered {} native tools", native_tools.len()));

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
    startup_logs.push(format!("Loaded {} user skills", skill_tools.len()));

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
        startup_logs.push(format!("Connected to {} MCP servers", bridge.connected_servers().len()));
    }

    // Create provider configuration
    let provider_config = ProviderConfig {
        provider_type: match app_config.provider.provider_type.as_str() {
            "anthropic" => ProviderType::Anthropic,
            "openai" => ProviderType::OpenAI,
            "gemini" => ProviderType::Gemini,
            "deepseek" => ProviderType::DeepSeek,
            "ollama" => ProviderType::Ollama,
            "openrouter" => ProviderType::OpenRouter,
            "groq" => ProviderType::Groq,
            "together" => ProviderType::Together,
            "lmstudio" => ProviderType::LMStudio,
            _ => ProviderType::Custom,
        },
        api_key: app_config.provider.api_key.clone(),
        base_url: app_config.provider.base_url.clone(),
        model: app_config.provider.model.clone(),
        max_tokens: app_config.provider.max_tokens,
        temperature: app_config.provider.temperature,
        extra: std::collections::HashMap::new(),
    };

    // Create the LLM provider
    let llm_provider: Arc<dyn providers::LLMProvider> = match create_provider(provider_config.clone()) {
        Ok(p) => Arc::from(p),
        Err(e) => {
            eprintln!("Error creating LLM provider: {}", e);
            std::process::exit(1);
        }
    };

    let provider_name = llm_provider.name().to_string();
    let model_name = llm_provider.model().to_string();

    // Create agent configuration
    let agent_config = AgentConfig {
        model: app_config.provider.model.clone(),
        max_tokens: app_config.provider.max_tokens,
        ..AgentConfig::default()
    };

    // Create a second registry for orchestrator
    let mut orchestrator_registry = ToolRegistry::new();

    // Re-register native tools for orchestrator
    orchestrator_registry.register(Calculator::new());
    orchestrator_registry.register(ArxivSearch::new());
    orchestrator_registry.register(WebFetch::new());
    orchestrator_registry.register(HackerNews::new());
    orchestrator_registry.register(Weather::new());
    orchestrator_registry.register(ExchangeRates::new());
    orchestrator_registry.register(Wikipedia::new());

    // Register MCP tools for orchestrator
    {
        let mcp_tools = brain.get_tools_by_type("mcp").unwrap_or_default();
        for tool in &mcp_tools {
            orchestrator_registry.register_with_type(
                MCPToolWrapper {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                },
                ToolType::Mcp,
            );
        }
    }

    // Create the agent
    let agent = Agent::new(
        llm_provider.clone(),
        registry,
        agent_config,
        brain.clone(),
        app_config.default_user_id.clone(),
    )
    .with_memory_extractor()
    .with_skill_manager(skill_manager.clone())
    .with_mcp_bridge(mcp_bridge.clone());

    // Create the orchestrator for complex multi-task operations
    let orchestrator = Orchestrator::new(
        llm_provider.clone(),
        orchestrator_registry,
        brain.clone(),
        app_config.skills_dir.clone().into(),
    )
    .with_mcp_bridge(mcp_bridge)
    .with_skill_manager(skill_manager);

    // Wrap agent and orchestrator in Arc for sharing across tasks
    let agent = Arc::new(agent);
    let orchestrator = Arc::new(orchestrator);

    // Create TUI app state
    let app = Arc::new(std::sync::Mutex::new(App::new()));

    // Add startup info to logs
    {
        let mut app_guard = app.lock().unwrap();
        app_guard.add_log("INFO", "Agent Brain v0.3 started");
        app_guard.add_log("INFO", &format!("Provider: {} | Model: {}", provider_name, model_name));
        // Add collected startup logs
        for msg in &startup_logs {
            app_guard.add_log("INFO", msg);
        }
        if let Ok(stats) = brain.stats() {
            app_guard.add_log("INFO", &stats.to_string());
        }
        app_guard.status = format!("Ready | {} | {}", provider_name, model_name);
    }

    // Create TUI
    let mut tui = match Tui::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to initialize TUI: {}", e);
            std::process::exit(1);
        }
    };

    // Main TUI event loop
    loop {
        // Draw the UI
        if let Err(e) = tui.draw(&app.lock().unwrap()) {
            eprintln!("Draw error: {}", e);
            break;
        }

        // Check if we should quit
        if app.lock().unwrap().should_quit {
            break;
        }

        // Check for pending input to process
        let pending = app.lock().unwrap().pending_input.take();
        if let Some(input) = pending {
            // Clone what we need for the async task
            let agent_clone = agent.clone();
            let orchestrator_clone = orchestrator.clone();
            let app_clone = app.clone();

            // Check for mode prefixes
            let (use_orchestrator, use_learning, task) = if input.starts_with("/multi ") {
                (true, false, input.strip_prefix("/multi ").unwrap().to_string())
            } else if input.starts_with("/learn ") {
                (true, true, input.strip_prefix("/learn ").unwrap().to_string())
            } else {
                (false, false, input.clone())
            };

            // Process special commands
            match input.to_lowercase().as_str() {
                "stats" => {
                    let mut app_guard = app_clone.lock().unwrap();
                    if let Ok(stats) = brain.stats() {
                        app_guard.add_assistant_message(&stats.to_string(), (0, 0), vec![]);
                    }
                    app_guard.is_thinking = false;
                    app_guard.status = "Ready".to_string();
                }
                "debug" => {
                    let debug_brain = brain.clone();
                    tokio::spawn(async move {
                        debug_server::start_debug_server(debug_brain, 3030).await;
                    });
                    let mut app_guard = app_clone.lock().unwrap();
                    app_guard.add_assistant_message(
                        "Debug server started at http://localhost:3030",
                        (0, 0),
                        vec![],
                    );
                    app_guard.is_thinking = false;
                    app_guard.status = "Debug server running".to_string();
                }
                "mistakes" => {
                    let mut app_guard = app_clone.lock().unwrap();
                    match brain.get_all_mistakes() {
                        Ok(mistakes) => {
                            if mistakes.is_empty() {
                                app_guard.add_assistant_message(
                                    "No mistakes recorded yet. Use `/learn <task>` to execute with validation.",
                                    (0, 0),
                                    vec![],
                                );
                            } else {
                                let mut output = format!("📚 {} Recorded Mistakes:\n\n", mistakes.len());
                                for (i, m) in mistakes.iter().enumerate() {
                                    let icon = match m.severity {
                                        crate::types::Severity::Critical => "🚨",
                                        crate::types::Severity::Major => "⚠️",
                                        crate::types::Severity::Minor => "💡",
                                    };
                                    output.push_str(&format!(
                                        "{} {}. [{}] {}\n   Type: {:?}\n   Prevention: {}\n   Corrected: {}\n\n",
                                        icon, i + 1, m.severity, m.description,
                                        m.mistake_type, m.prevention_strategy,
                                        if m.was_corrected { "Yes" } else { "No" }
                                    ));
                                }
                                app_guard.add_assistant_message(&output, (0, 0), vec![]);
                            }
                        }
                        Err(e) => {
                            app_guard.add_assistant_message(&format!("Error fetching mistakes: {}", e), (0, 0), vec![]);
                        }
                    }
                    app_guard.is_thinking = false;
                    app_guard.status = "Ready".to_string();
                }
                _ => {
                    // Run agent or orchestrator in a background task
                    tokio::spawn(async move {
                        {
                            let mut app_guard = app_clone.lock().unwrap();
                            let mode = if use_orchestrator { "multi-agent" } else { "single-agent" };
                            app_guard.add_log("INFO", &format!("[{}] Processing: {}", mode, task));
                        }

                        if use_learning {
                            // Learning mode: execute with validation and mistake recording
                            {
                                let mut app_guard = app_clone.lock().unwrap();
                                app_guard.add_log("INFO", "🧠 [LEARNING] Executing with validation...");
                            }
                            match orchestrator_clone.execute_with_learning(&task, None, 3).await {
                                Ok(result) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    let status = if result.validation_passed {
                                        "✅ Validation passed"
                                    } else {
                                        "❌ Validation failed - mistakes recorded"
                                    };
                                    let output = format!(
                                        "{}\n\n---\n{}\nMistakes recorded: {}\nRetries: {}",
                                        result.response, status, result.mistakes_recorded, result.attempts
                                    );
                                    app_guard.add_assistant_message(
                                        &output,
                                        (result.total_tokens, 0),
                                        result.tools_used,
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = format!("Ready | {}", status);
                                    app_guard.add_log(
                                        "INFO",
                                        &format!(
                                            "Learning complete: {} mistakes, {} retries",
                                            result.mistakes_recorded, result.attempts
                                        ),
                                    );
                                }
                                Err(e) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    app_guard.add_assistant_message(
                                        &format!("Learning error: {}", e),
                                        (0, 0),
                                        vec![],
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = "Error occurred".to_string();
                                    app_guard.add_log("ERROR", &format!("Learning error: {}", e));
                                }
                            }
                        } else if use_orchestrator {
                            // Multi-agent mode via orchestrator
                            match orchestrator_clone.execute(&task).await {
                                Ok(result) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    app_guard.add_assistant_message(
                                        &result.response,
                                        (result.total_tokens, 0),
                                        result.tools_used,
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = format!("Ready | {} tasks completed", result.tasks_completed);
                                    app_guard.add_log(
                                        "INFO",
                                        &format!(
                                            "Orchestrator complete: {} tasks, {} tokens",
                                            result.tasks_completed, result.total_tokens
                                        ),
                                    );
                                }
                                Err(e) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    app_guard.add_assistant_message(
                                        &format!("Orchestrator error: {}", e),
                                        (0, 0),
                                        vec![],
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = "Error occurred".to_string();
                                    app_guard.add_log("ERROR", &format!("Orchestrator error: {}", e));
                                }
                            }
                        } else {
                            // Single-agent mode
                            match agent_clone.run(&task).await {
                                Ok(result) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    app_guard.add_assistant_message(
                                        &result.response,
                                        (result.usage.input_tokens, result.usage.output_tokens),
                                        result.tools_used,
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = "Ready".to_string();
                                    app_guard.add_log(
                                        "INFO",
                                        &format!(
                                            "Response complete: {} in, {} out tokens",
                                            result.usage.input_tokens, result.usage.output_tokens
                                        ),
                                    );
                                }
                                Err(e) => {
                                    let mut app_guard = app_clone.lock().unwrap();
                                    app_guard.add_assistant_message(
                                        &format!("Error: {}", e),
                                        (0, 0),
                                        vec![],
                                    );
                                    app_guard.is_thinking = false;
                                    app_guard.status = "Error occurred".to_string();
                                    app_guard.add_log("ERROR", &format!("Agent error: {}", e));
                                }
                            }
                        }
                    });
                }
            }
        }

        // Poll for events
        if let Ok(Some(event)) = tui.poll_event(Duration::from_millis(50)) {
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Press {
                    let mut app_guard = app.lock().unwrap();
                    app_guard.handle_key(key.code, key.modifiers);
                }
            }
        }
    }

    // Cleanup
    tui.restore().ok();

    // Print session summary
    let app_guard = app.lock().unwrap();
    println!("\nSession summary:");
    println!(
        "  Total tokens: {} input, {} output",
        app_guard.total_tokens.0, app_guard.total_tokens.1
    );
    println!(
        "  Estimated cost: ${:.4}",
        estimate_cost(app_guard.total_tokens.0, app_guard.total_tokens.1)
    );
    println!("\nGoodbye!");
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

    // === Free REST API Tools (zero API key required) ===

    // Hacker News
    let hn = HackerNews::new();
    let hn_def = hn.definition();
    let hn_node = ToolNode::new(
        hn_def.name.clone(),
        hn_def.description.clone(),
        ToolType::Native,
        hn_def.input_schema.clone(),
        "free_api".to_string(),
    );
    brain.register_tool(&hn_node).ok();
    brain.link_tool_topic(&hn_node.name, "news").ok();
    brain.link_tool_topic(&hn_node.name, "tech").ok();
    registry.register(hn);
    tools.push(hn_node);

    // Weather (Open-Meteo)
    let weather = Weather::new();
    let weather_def = weather.definition();
    let weather_node = ToolNode::new(
        weather_def.name.clone(),
        weather_def.description.clone(),
        ToolType::Native,
        weather_def.input_schema.clone(),
        "free_api".to_string(),
    );
    brain.register_tool(&weather_node).ok();
    brain.link_tool_topic(&weather_node.name, "weather").ok();
    registry.register(weather);
    tools.push(weather_node);

    // Exchange Rates
    let exchange = ExchangeRates::new();
    let exchange_def = exchange.definition();
    let exchange_node = ToolNode::new(
        exchange_def.name.clone(),
        exchange_def.description.clone(),
        ToolType::Native,
        exchange_def.input_schema.clone(),
        "free_api".to_string(),
    );
    brain.register_tool(&exchange_node).ok();
    brain.link_tool_topic(&exchange_node.name, "finance").ok();
    brain.link_tool_topic(&exchange_node.name, "currency").ok();
    registry.register(exchange);
    tools.push(exchange_node);

    // Wikipedia
    let wiki = Wikipedia::new();
    let wiki_def = wiki.definition();
    let wiki_node = ToolNode::new(
        wiki_def.name.clone(),
        wiki_def.description.clone(),
        ToolType::Native,
        wiki_def.input_schema.clone(),
        "free_api".to_string(),
    );
    brain.register_tool(&wiki_node).ok();
    brain.link_tool_topic(&wiki_node.name, "knowledge").ok();
    brain.link_tool_topic(&wiki_node.name, "reference").ok();
    registry.register(wiki);
    tools.push(wiki_node);

    tools
}


// ═══════════════════════════════════════════════════════════════════
// TOOL WRAPPERS
// ═══════════════════════════════════════════════════════════════════

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
