use async_trait::async_trait;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

use crate::tools::Tool;
use crate::types::{AgentError, ToolDefinition};

/// arXiv search tool that queries the arXiv API
pub struct ArxivSearch {
    client: reqwest::Client,
}

impl ArxivSearch {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ArxivSearch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ArxivSearch {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "arxiv_search".to_string(),
            description: "Search for academic papers on arXiv. Returns titles, authors, abstracts, and links.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query for arXiv papers"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5, max: 10)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(
        &self,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ToolError("Missing 'query' parameter".to_string()))?;

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_i64())
            .unwrap_or(5)
            .min(10) as u32;

        let papers = search_arxiv(&self.client, query, max_results).await?;

        if papers.is_empty() {
            return Ok("No papers found matching your query.".to_string());
        }

        let mut output = format!("Found {} papers:\n\n", papers.len());
        for (i, paper) in papers.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", i + 1, paper.title));
            output.push_str(&format!("   Authors: {}\n", paper.authors.join(", ")));
            output.push_str(&format!("   Published: {}\n", paper.published));
            output.push_str(&format!("   Link: {}\n", paper.link));
            output.push_str(&format!("   Abstract: {}\n\n", truncate_abstract(&paper.summary)));
        }

        Ok(output)
    }
}

#[derive(Debug)]
struct Paper {
    title: String,
    authors: Vec<String>,
    summary: String,
    link: String,
    published: String,
}

async fn search_arxiv(
    client: &reqwest::Client,
    query: &str,
    max_results: u32,
) -> Result<Vec<Paper>, AgentError> {
    let url = format!(
        "http://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results={}",
        urlencoding::encode(query),
        max_results
    );

    let response = client
        .get(&url)
        .header("User-Agent", "agent-brain/0.1.0")
        .send()
        .await?
        .text()
        .await?;

    parse_arxiv_response(&response)
}

fn parse_arxiv_response(xml: &str) -> Result<Vec<Paper>, AgentError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut papers = Vec::new();
    let mut current_paper: Option<PaperBuilder> = None;
    let mut current_element = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                current_element = name.clone();

                if name == "entry" {
                    current_paper = Some(PaperBuilder::default());
                } else if name == "link" && current_paper.is_some() {
                    // Extract href attribute from link element
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" {
                            let href = String::from_utf8_lossy(&attr.value).to_string();
                            if !href.contains("pdf") {
                                if let Some(ref mut paper) = current_paper {
                                    paper.link = Some(href);
                                }
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "entry" {
                    if let Some(builder) = current_paper.take() {
                        if let Some(paper) = builder.build() {
                            papers.push(paper);
                        }
                    }
                }
                current_element.clear();
            }
            Ok(Event::Text(e)) => {
                if let Some(ref mut paper) = current_paper {
                    let text = e.unescape().unwrap_or_default().to_string();
                    match current_element.as_str() {
                        "title" => paper.title = Some(clean_text(&text)),
                        "summary" => paper.summary = Some(clean_text(&text)),
                        "published" => paper.published = Some(text.chars().take(10).collect()),
                        "name" => paper.authors.push(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(AgentError::ParseError(format!(
                    "Error parsing arXiv XML: {}",
                    e
                )))
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(papers)
}

#[derive(Default)]
struct PaperBuilder {
    title: Option<String>,
    authors: Vec<String>,
    summary: Option<String>,
    link: Option<String>,
    published: Option<String>,
}

impl PaperBuilder {
    fn build(self) -> Option<Paper> {
        Some(Paper {
            title: self.title?,
            authors: self.authors,
            summary: self.summary.unwrap_or_default(),
            link: self.link.unwrap_or_default(),
            published: self.published.unwrap_or_default(),
        })
    }
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_abstract(text: &str) -> String {
    if text.len() <= 300 {
        text.to_string()
    } else {
        format!("{}...", &text[..300])
    }
}

// Simple URL encoding for the query
mod urlencoding {
    pub fn encode(input: &str) -> String {
        let mut result = String::new();
        for c in input.chars() {
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    result.push(c);
                }
                ' ' => result.push('+'),
                _ => {
                    for b in c.to_string().bytes() {
                        result.push_str(&format!("%{:02X}", b));
                    }
                }
            }
        }
        result
    }
}
