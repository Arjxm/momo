//! Free REST API tools - zero API key required
//!
//! These tools call public APIs that don't require authentication:
//! - Hacker News (Firebase + Algolia)
//! - Open-Meteo (weather)
//! - Exchange rates
//! - Wikipedia

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

use crate::tools::Tool;
use crate::types::{AgentError, ToolDefinition};

// ============================================================================
// Hacker News Tool
// ============================================================================

pub struct HackerNews {
    client: reqwest::Client,
}

impl HackerNews {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HNItem {
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    score: Option<i32>,
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    descendants: Option<i32>,
    #[serde(rename = "type")]
    #[serde(default)]
    item_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl Tool for HackerNews {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "hacker_news".to_string(),
            description: "Get top stories, new stories, or search Hacker News. Returns titles, URLs, scores, and comments.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["top", "new", "best", "search"],
                        "description": "Action: top/new/best stories, or search"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query (only for action=search)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of stories to return (default: 10, max: 30)"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, input: HashMap<String, serde_json::Value>) -> Result<String, AgentError> {
        let action = input.get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("top");

        let limit = input.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(30) as usize;

        debug!("HackerNews: action={}, limit={}", action, limit);

        match action {
            "search" => {
                let query = input.get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let url = format!(
                    "https://hn.algolia.com/api/v1/search?query={}&hitsPerPage={}",
                    urlencoding::encode(query),
                    limit
                );

                let resp: serde_json::Value = self.client.get(&url)
                    .send()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("HN search failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("HN parse failed: {}", e)))?;

                let hits = resp.get("hits").and_then(|h| h.as_array());

                let mut results = Vec::new();
                if let Some(hits) = hits {
                    for hit in hits.iter().take(limit) {
                        let title = hit.get("title").and_then(|t| t.as_str()).unwrap_or("No title");
                        let url = hit.get("url").and_then(|u| u.as_str()).unwrap_or("");
                        let points = hit.get("points").and_then(|p| p.as_i64()).unwrap_or(0);
                        let author = hit.get("author").and_then(|a| a.as_str()).unwrap_or("unknown");
                        let object_id = hit.get("objectID").and_then(|o| o.as_str()).unwrap_or("");

                        results.push(format!(
                            "- {} ({} points by {})\n  URL: {}\n  HN: https://news.ycombinator.com/item?id={}",
                            title, points, author,
                            if url.is_empty() { "N/A" } else { url },
                            object_id
                        ));
                    }
                }

                Ok(format!("## Hacker News Search: \"{}\"\n\n{}", query, results.join("\n\n")))
            }
            _ => {
                // top, new, or best stories
                let endpoint = match action {
                    "new" => "newstories",
                    "best" => "beststories",
                    _ => "topstories",
                };

                let ids_url = format!("https://hacker-news.firebaseio.com/v0/{}.json", endpoint);
                let ids: Vec<u64> = self.client.get(&ids_url)
                    .send()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("HN fetch failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("HN parse failed: {}", e)))?;

                let mut results = Vec::new();
                for id in ids.iter().take(limit) {
                    let item_url = format!("https://hacker-news.firebaseio.com/v0/item/{}.json", id);
                    if let Ok(resp) = self.client.get(&item_url).send().await {
                        if let Ok(item) = resp.json::<HNItem>().await {
                            let title = item.title.unwrap_or_else(|| "No title".to_string());
                            let url = item.url.unwrap_or_default();
                            let score = item.score.unwrap_or(0);
                            let by = item.by.unwrap_or_else(|| "unknown".to_string());
                            let comments = item.descendants.unwrap_or(0);

                            results.push(format!(
                                "- {} ({} points, {} comments by {})\n  URL: {}\n  HN: https://news.ycombinator.com/item?id={}",
                                title, score, comments, by,
                                if url.is_empty() { "N/A" } else { &url },
                                id
                            ));
                        }
                    }
                }

                Ok(format!("## Hacker News {} Stories\n\n{}",
                    action.to_uppercase(),
                    results.join("\n\n")
                ))
            }
        }
    }
}

// ============================================================================
// Weather Tool (Open-Meteo)
// ============================================================================

pub struct Weather {
    client: reqwest::Client,
}

impl Weather {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for Weather {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "weather".to_string(),
            description: "Get current weather and forecast for any location. Uses Open-Meteo API (free, no API key).".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "latitude": {
                        "type": "number",
                        "description": "Latitude of the location"
                    },
                    "longitude": {
                        "type": "number",
                        "description": "Longitude of the location"
                    },
                    "location": {
                        "type": "string",
                        "description": "Location name (will geocode if lat/lon not provided)"
                    }
                }
            }),
        }
    }

    async fn execute(&self, input: HashMap<String, serde_json::Value>) -> Result<String, AgentError> {
        let (lat, lon) = if let (Some(lat), Some(lon)) = (
            input.get("latitude").and_then(|v| v.as_f64()),
            input.get("longitude").and_then(|v| v.as_f64()),
        ) {
            (lat, lon)
        } else if let Some(location) = input.get("location").and_then(|v| v.as_str()) {
            // Geocode the location
            let geo_url = format!(
                "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1",
                urlencoding::encode(location)
            );

            let geo_resp: serde_json::Value = self.client.get(&geo_url)
                .send()
                .await
                .map_err(|e| AgentError::ToolError(format!("Geocoding failed: {}", e)))?
                .json()
                .await
                .map_err(|e| AgentError::ToolError(format!("Geocoding parse failed: {}", e)))?;

            let results = geo_resp.get("results").and_then(|r| r.as_array());
            if let Some(results) = results {
                if let Some(first) = results.first() {
                    let lat = first.get("latitude").and_then(|l| l.as_f64()).unwrap_or(0.0);
                    let lon = first.get("longitude").and_then(|l| l.as_f64()).unwrap_or(0.0);
                    (lat, lon)
                } else {
                    return Err(AgentError::ToolError(format!("Location not found: {}", location)));
                }
            } else {
                return Err(AgentError::ToolError(format!("Location not found: {}", location)));
            }
        } else {
            return Err(AgentError::ToolError("Provide latitude/longitude or location name".to_string()));
        };

        debug!("Weather: lat={}, lon={}", lat, lon);

        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,apparent_temperature,precipitation,weather_code,wind_speed_10m&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum&timezone=auto",
            lat, lon
        );

        let resp: serde_json::Value = self.client.get(&url)
            .send()
            .await
            .map_err(|e| AgentError::ToolError(format!("Weather fetch failed: {}", e)))?
            .json()
            .await
            .map_err(|e| AgentError::ToolError(format!("Weather parse failed: {}", e)))?;

        let current = resp.get("current");
        let daily = resp.get("daily");
        let timezone = resp.get("timezone").and_then(|t| t.as_str()).unwrap_or("UTC");

        let mut output = format!("## Weather for ({:.2}, {:.2})\nTimezone: {}\n\n", lat, lon, timezone);

        if let Some(current) = current {
            let temp = current.get("temperature_2m").and_then(|t| t.as_f64()).unwrap_or(0.0);
            let feels_like = current.get("apparent_temperature").and_then(|t| t.as_f64()).unwrap_or(0.0);
            let humidity = current.get("relative_humidity_2m").and_then(|h| h.as_f64()).unwrap_or(0.0);
            let precip = current.get("precipitation").and_then(|p| p.as_f64()).unwrap_or(0.0);
            let wind = current.get("wind_speed_10m").and_then(|w| w.as_f64()).unwrap_or(0.0);
            let code = current.get("weather_code").and_then(|c| c.as_i64()).unwrap_or(0);

            output.push_str(&format!(
                "### Current Conditions\n- Temperature: {:.1}°C (feels like {:.1}°C)\n- Humidity: {:.0}%\n- Precipitation: {:.1}mm\n- Wind: {:.1} km/h\n- Conditions: {}\n\n",
                temp, feels_like, humidity, precip, wind, weather_code_to_text(code as i32)
            ));
        }

        if let Some(daily) = daily {
            let dates = daily.get("time").and_then(|t| t.as_array());
            let max_temps = daily.get("temperature_2m_max").and_then(|t| t.as_array());
            let min_temps = daily.get("temperature_2m_min").and_then(|t| t.as_array());
            let codes = daily.get("weather_code").and_then(|c| c.as_array());

            if let (Some(dates), Some(max_temps), Some(min_temps), Some(codes)) = (dates, max_temps, min_temps, codes) {
                output.push_str("### 7-Day Forecast\n");
                for i in 0..dates.len().min(7) {
                    let date = dates.get(i).and_then(|d| d.as_str()).unwrap_or("?");
                    let max = max_temps.get(i).and_then(|t| t.as_f64()).unwrap_or(0.0);
                    let min = min_temps.get(i).and_then(|t| t.as_f64()).unwrap_or(0.0);
                    let code = codes.get(i).and_then(|c| c.as_i64()).unwrap_or(0);

                    output.push_str(&format!(
                        "- {}: {:.0}°C / {:.0}°C - {}\n",
                        date, max, min, weather_code_to_text(code as i32)
                    ));
                }
            }
        }

        Ok(output)
    }
}

fn weather_code_to_text(code: i32) -> &'static str {
    match code {
        0 => "Clear sky",
        1 | 2 | 3 => "Partly cloudy",
        45 | 48 => "Foggy",
        51 | 53 | 55 => "Drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 => "Snow",
        77 => "Snow grains",
        80 | 81 | 82 => "Rain showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

// ============================================================================
// Exchange Rates Tool
// ============================================================================

pub struct ExchangeRates {
    client: reqwest::Client,
}

impl ExchangeRates {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for ExchangeRates {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "exchange_rates".to_string(),
            description: "Get current currency exchange rates. Convert between 160+ currencies.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Base currency code (e.g., USD, EUR, GBP)"
                    },
                    "to": {
                        "type": "string",
                        "description": "Target currency code (optional, shows all if not specified)"
                    },
                    "amount": {
                        "type": "number",
                        "description": "Amount to convert (default: 1)"
                    }
                },
                "required": ["from"]
            }),
        }
    }

    async fn execute(&self, input: HashMap<String, serde_json::Value>) -> Result<String, AgentError> {
        let from = input.get("from")
            .and_then(|v| v.as_str())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| "USD".to_string());

        let to = input.get("to")
            .and_then(|v| v.as_str())
            .map(|s| s.to_uppercase());

        let amount = input.get("amount")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);

        debug!("ExchangeRates: from={}, to={:?}, amount={}", from, to, amount);

        let url = format!("https://open.er-api.com/v6/latest/{}", from);

        let resp: serde_json::Value = self.client.get(&url)
            .send()
            .await
            .map_err(|e| AgentError::ToolError(format!("Exchange rate fetch failed: {}", e)))?
            .json()
            .await
            .map_err(|e| AgentError::ToolError(format!("Exchange rate parse failed: {}", e)))?;

        let rates = resp.get("rates").and_then(|r| r.as_object());

        if let Some(rates) = rates {
            if let Some(target) = to {
                // Single conversion
                if let Some(rate) = rates.get(&target).and_then(|r| r.as_f64()) {
                    let converted = amount * rate;
                    Ok(format!(
                        "{:.2} {} = {:.2} {} (rate: {:.6})",
                        amount, from, converted, target, rate
                    ))
                } else {
                    Err(AgentError::ToolError(format!("Currency not found: {}", target)))
                }
            } else {
                // Show common currencies
                let common = ["EUR", "GBP", "JPY", "CNY", "INR", "CAD", "AUD", "CHF", "KRW", "BRL"];
                let mut output = format!("## Exchange Rates from {}\n\n", from);

                for currency in common {
                    if let Some(rate) = rates.get(currency).and_then(|r| r.as_f64()) {
                        let converted = amount * rate;
                        output.push_str(&format!("- {} {}: {:.2} {}\n", amount, from, converted, currency));
                    }
                }

                Ok(output)
            }
        } else {
            Err(AgentError::ToolError(format!("Invalid base currency: {}", from)))
        }
    }
}

// ============================================================================
// Wikipedia Tool
// ============================================================================

pub struct Wikipedia {
    client: reqwest::Client,
}

impl Wikipedia {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for Wikipedia {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "wikipedia".to_string(),
            description: "Search and read Wikipedia articles. Get summaries or full content.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["search", "summary", "content"],
                        "description": "search: find articles, summary: get article summary, content: get full article"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query or article title"
                    },
                    "language": {
                        "type": "string",
                        "description": "Wikipedia language code (default: en)"
                    }
                },
                "required": ["action", "query"]
            }),
        }
    }

    async fn execute(&self, input: HashMap<String, serde_json::Value>) -> Result<String, AgentError> {
        let action = input.get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("summary");

        let query = input.get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ToolError("Query is required".to_string()))?;

        let lang = input.get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("en");

        debug!("Wikipedia: action={}, query={}, lang={}", action, query, lang);

        match action {
            "search" => {
                let url = format!(
                    "https://{}.wikipedia.org/w/api.php?action=opensearch&search={}&limit=10&format=json",
                    lang,
                    urlencoding::encode(query)
                );

                let resp: serde_json::Value = self.client.get(&url)
                    .send()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Wikipedia search failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Wikipedia parse failed: {}", e)))?;

                let titles = resp.get(1).and_then(|t| t.as_array());
                let urls = resp.get(3).and_then(|u| u.as_array());

                let mut output = format!("## Wikipedia Search: \"{}\"\n\n", query);

                if let (Some(titles), Some(urls)) = (titles, urls) {
                    for (title, url) in titles.iter().zip(urls.iter()) {
                        if let (Some(t), Some(u)) = (title.as_str(), url.as_str()) {
                            output.push_str(&format!("- [{}]({})\n", t, u));
                        }
                    }
                }

                Ok(output)
            }
            "summary" | "content" => {
                let url = format!(
                    "https://{}.wikipedia.org/api/rest_v1/page/summary/{}",
                    lang,
                    urlencoding::encode(query)
                );

                let resp: serde_json::Value = self.client.get(&url)
                    .header("User-Agent", "AgentBrain/1.0")
                    .send()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Wikipedia fetch failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| AgentError::ToolError(format!("Wikipedia parse failed: {}", e)))?;

                let title = resp.get("title").and_then(|t| t.as_str()).unwrap_or(query);
                let extract = resp.get("extract").and_then(|e| e.as_str()).unwrap_or("No content found.");
                let page_url = resp.get("content_urls")
                    .and_then(|c| c.get("desktop"))
                    .and_then(|d| d.get("page"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("");

                let mut output = format!("## {}\n\n{}\n\n", title, extract);

                if !page_url.is_empty() {
                    output.push_str(&format!("Read more: {}", page_url));
                }

                Ok(output)
            }
            _ => Err(AgentError::ToolError(format!("Unknown action: {}", action))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_weather_definition() {
        let tool = Weather::new();
        let def = tool.definition();
        assert_eq!(def.name, "weather");
    }

    #[tokio::test]
    async fn test_hacker_news_definition() {
        let tool = HackerNews::new();
        let def = tool.definition();
        assert_eq!(def.name, "hacker_news");
    }

    #[tokio::test]
    async fn test_exchange_rates_definition() {
        let tool = ExchangeRates::new();
        let def = tool.definition();
        assert_eq!(def.name, "exchange_rates");
    }

    #[tokio::test]
    async fn test_wikipedia_definition() {
        let tool = Wikipedia::new();
        let def = tool.definition();
        assert_eq!(def.name, "wikipedia");
    }
}
