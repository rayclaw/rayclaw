use std::sync::OnceLock;

use async_trait::async_trait;
use serde_json::json;

use super::web_html::extract_ddg_results;
use super::{schema_object, Tool, ToolResult};
use crate::llm_types::ToolDefinition;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent("RayClaw/1.0")
            .build()
            .expect("failed to build HTTP client")
    })
}

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".into(),
            description: "Search the web using DuckDuckGo. Returns titles, URLs, and snippets."
                .into(),
            input_schema: schema_object(
                json!({
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                }),
                &["query"],
            ),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::error("Missing required parameter: query".into()),
        };

        match search_ddg(query).await {
            Ok(results) => {
                if results.is_empty() {
                    ToolResult::success("No results found.".into())
                } else {
                    ToolResult::success(results)
                }
            }
            Err(e) => ToolResult::error(format!("Search failed: {e}")),
        }
    }
}

async fn search_ddg(query: &str) -> Result<String, String> {
    let encoded = urlencoding::encode(query);
    let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

    let resp = http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body = resp.text().await.map_err(|e| e.to_string())?;
    let items = extract_ddg_results(&body, 8);

    let mut output = String::new();
    for (i, item) in items.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            item.title,
            item.url,
            item.snippet
        ));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_web_search_definition() {
        let tool = WebSearchTool;
        assert_eq!(tool.name(), "web_search");
        let def = tool.definition();
        assert_eq!(def.name, "web_search");
        assert!(def.description.contains("DuckDuckGo"));
        assert!(def.input_schema["properties"]["query"].is_object());
        let required = def.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[tokio::test]
    async fn test_web_search_missing_query() {
        let tool = WebSearchTool;
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter: query"));
    }

    #[tokio::test]
    async fn test_web_search_null_query() {
        let tool = WebSearchTool;
        let result = tool.execute(json!({"query": null})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter: query"));
    }
}
