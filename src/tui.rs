//! Terminal User Interface for Agent Brain
//!
//! Provides a clean chat interface with logs hidden in a collapsible panel.

use std::io::{self, Stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};

/// A chat message in the conversation
#[derive(Clone)]
pub struct ChatMessage {
    pub role: String, // "user" or "assistant"
    pub content: String,
    pub tokens: Option<(u32, u32)>, // (input, output)
    pub tools: Vec<String>,
}

/// A log entry
#[derive(Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

/// TUI Application state
pub struct App {
    /// User input buffer
    pub input: String,
    /// Cursor position in input
    pub cursor_pos: usize,
    /// Chat history
    pub messages: Vec<ChatMessage>,
    /// Log buffer (circular)
    pub logs: Vec<LogEntry>,
    /// Maximum logs to keep
    max_logs: usize,
    /// Whether to show logs panel
    pub show_logs: bool,
    /// Scroll position in chat
    pub chat_scroll: usize,
    /// Scroll position in logs
    pub log_scroll: usize,
    /// Whether the agent is thinking
    pub is_thinking: bool,
    /// Status message
    pub status: String,
    /// Total tokens used
    pub total_tokens: (u32, u32),
    /// Whether app should quit
    pub should_quit: bool,
    /// Pending input to send
    pub pending_input: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            messages: Vec::new(),
            logs: Vec::new(),
            max_logs: 500,
            show_logs: false,
            chat_scroll: 0,
            log_scroll: 0,
            is_thinking: false,
            status: "Ready".to_string(),
            total_tokens: (0, 0),
            should_quit: false,
            pending_input: None,
        }
    }

    pub fn add_log(&mut self, level: &str, message: &str) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        self.logs.push(LogEntry {
            timestamp,
            level: level.to_string(),
            message: message.to_string(),
        });
        if self.logs.len() > self.max_logs {
            self.logs.remove(0);
        }
        // Auto-scroll to bottom
        self.log_scroll = self.logs.len().saturating_sub(1);
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            tokens: None,
            tools: Vec::new(),
        });
        self.chat_scroll = self.messages.len().saturating_sub(1);
    }

    pub fn add_assistant_message(&mut self, content: &str, tokens: (u32, u32), tools: Vec<String>) {
        self.messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            tokens: Some(tokens),
            tools,
        });
        self.total_tokens.0 += tokens.0;
        self.total_tokens.1 += tokens.1;
        self.chat_scroll = self.messages.len().saturating_sub(1);
        self.is_thinking = false;
    }

    pub fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match key {
            KeyCode::Enter => {
                if !self.input.is_empty() && !self.is_thinking {
                    let input = self.input.clone();
                    self.input.clear();
                    self.cursor_pos = 0;

                    // Handle special commands
                    match input.to_lowercase().as_str() {
                        "quit" | "exit" => {
                            self.should_quit = true;
                        }
                        "logs" => {
                            self.show_logs = !self.show_logs;
                            self.status = if self.show_logs {
                                "Logs panel shown (press 'logs' to hide)".to_string()
                            } else {
                                "Logs panel hidden".to_string()
                            };
                        }
                        "clear" => {
                            self.messages.clear();
                            self.status = "Chat cleared".to_string();
                        }
                        _ => {
                            self.add_user_message(&input);
                            self.is_thinking = true;
                            self.status = "Thinking...".to_string();
                            self.pending_input = Some(input);
                        }
                    }
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.show_logs = !self.show_logs;
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor_pos = (self.cursor_pos + 1).min(self.input.len());
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Up => {
                self.chat_scroll = self.chat_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.chat_scroll = (self.chat_scroll + 1).min(self.messages.len().saturating_sub(1));
            }
            KeyCode::PageUp => {
                if self.show_logs {
                    self.log_scroll = self.log_scroll.saturating_sub(10);
                } else {
                    self.chat_scroll = self.chat_scroll.saturating_sub(5);
                }
            }
            KeyCode::PageDown => {
                if self.show_logs {
                    self.log_scroll = (self.log_scroll + 10).min(self.logs.len().saturating_sub(1));
                } else {
                    self.chat_scroll = (self.chat_scroll + 5).min(self.messages.len().saturating_sub(1));
                }
            }
            _ => {}
        }
    }
}

/// Terminal wrapper for TUI
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    pub fn new() -> io::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    pub fn restore(&mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        Ok(())
    }

    pub fn draw(&mut self, app: &App) -> io::Result<()> {
        self.terminal.draw(|frame| {
            render_ui(frame, app);
        })?;
        Ok(())
    }

    pub fn poll_event(&self, timeout: Duration) -> io::Result<Option<Event>> {
        if event::poll(timeout)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }
}

fn render_ui(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // Main layout: chat + optional logs + input + status
    let main_chunks = if app.show_logs {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),      // Chat
                Constraint::Length(12),   // Logs
                Constraint::Length(3),    // Input
                Constraint::Length(1),    // Status
            ])
            .split(size)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),      // Chat
                Constraint::Length(0),    // No logs
                Constraint::Length(3),    // Input
                Constraint::Length(1),    // Status
            ])
            .split(size)
    };

    // Render chat area
    render_chat(frame, app, main_chunks[0]);

    // Render logs if visible
    if app.show_logs {
        render_logs(frame, app, main_chunks[1]);
    }

    // Render input area
    render_input(frame, app, main_chunks[2]);

    // Render status bar
    render_status(frame, app, main_chunks[3]);
}

fn render_chat(frame: &mut Frame, app: &App, area: Rect) {
    let messages: Vec<ListItem> = app
        .messages
        .iter()
        .map(|msg| {
            let style = if msg.role == "user" {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Green)
            };

            let prefix = if msg.role == "user" { "You: " } else { "Agent: " };
            let mut lines = vec![Line::from(vec![
                Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
            ])];

            // Wrap content
            for line in msg.content.lines() {
                lines.push(Line::from(Span::raw(format!("  {}", line))));
            }

            // Add token info for assistant messages
            if let Some((input, output)) = msg.tokens {
                let tools_str = if msg.tools.is_empty() {
                    String::new()
                } else {
                    format!(" | Tools: {}", msg.tools.join(", "))
                };
                lines.push(Line::from(Span::styled(
                    format!("  [{} in, {} out{}]", input, output, tools_str),
                    Style::default().fg(Color::DarkGray),
                )));
            }

            lines.push(Line::from("")); // Empty line between messages
            ListItem::new(lines)
        })
        .collect();

    let title = if app.is_thinking {
        " Chat (thinking...) "
    } else {
        " Chat "
    };

    let chat = List::new(messages)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(chat, area);
}

fn render_logs(frame: &mut Frame, app: &App, area: Rect) {
    let logs: Vec<ListItem> = app
        .logs
        .iter()
        .rev()
        .take(50)
        .rev()
        .map(|log| {
            let level_color = match log.level.as_str() {
                "ERROR" => Color::Red,
                "WARN" => Color::Yellow,
                "INFO" => Color::Blue,
                "DEBUG" => Color::DarkGray,
                _ => Color::White,
            };

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", log.timestamp),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:5} ", log.level),
                    Style::default().fg(level_color),
                ),
                Span::raw(&log.message),
            ]))
        })
        .collect();

    let logs_widget = List::new(logs).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Logs (Ctrl+L to toggle) ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(logs_widget, area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_style = if app.is_thinking {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let input = Paragraph::new(app.input.as_str())
        .style(input_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(if app.is_thinking { " Input (waiting...) " } else { " Input " }),
        );

    frame.render_widget(input, area);

    // Show cursor
    if !app.is_thinking {
        frame.set_cursor_position((area.x + 1 + app.cursor_pos as u16, area.y + 1));
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let cost = estimate_cost(app.total_tokens.0, app.total_tokens.1);

    let status_text = format!(
        " {} | Tokens: {} in, {} out | Cost: ${:.4} | Ctrl+L: logs | quit: exit ",
        app.status,
        app.total_tokens.0,
        app.total_tokens.1,
        cost
    );

    let status = Paragraph::new(status_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(status, area);
}

fn estimate_cost(input_tokens: u32, output_tokens: u32) -> f64 {
    const INPUT_COST_PER_MILLION: f64 = 3.0;
    const OUTPUT_COST_PER_MILLION: f64 = 15.0;
    let input_cost = (input_tokens as f64 / 1_000_000.0) * INPUT_COST_PER_MILLION;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * OUTPUT_COST_PER_MILLION;
    input_cost + output_cost
}

/// Log writer that captures logs for the TUI
pub struct TuiLogWriter {
    app: Arc<Mutex<App>>,
}

impl TuiLogWriter {
    pub fn new(app: Arc<Mutex<App>>) -> Self {
        Self { app }
    }
}

impl std::io::Write for TuiLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(s) = std::str::from_utf8(buf) {
            // Parse the log line (simplified)
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                // Try to extract level from ANSI-colored output
                let level = if trimmed.contains("INFO") {
                    "INFO"
                } else if trimmed.contains("WARN") {
                    "WARN"
                } else if trimmed.contains("ERROR") {
                    "ERROR"
                } else if trimmed.contains("DEBUG") {
                    "DEBUG"
                } else {
                    "INFO"
                };

                // Strip ANSI codes (simple approach)
                let clean: String = trimmed
                    .chars()
                    .filter(|c| !matches!(*c, '\x1b' | '[' | '0'..='9' | 'm'))
                    .collect();

                if let Ok(mut app) = self.app.lock() {
                    app.add_log(level, &clean);
                }
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
