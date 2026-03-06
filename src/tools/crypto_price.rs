use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{schema_object, Tool, ToolResult};
use crate::llm_types::ToolDefinition;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("RayClaw/1.0")
            .build()
            .expect("failed to build HTTP client")
    })
}

#[derive(Debug, Deserialize)]
struct CoinGeckoPrice {
    id: String,
    symbol: String,
    name: String,
    #[serde(rename = "current_price")]
    current_price: f64,
    #[serde(rename = "market_cap")]
    market_cap: u64,
    #[serde(rename = "total_volume")]
    total_volume: u64,
    #[serde(rename = "price_change_24h")]
    price_change_24h: f64,
    #[serde(rename = "price_change_percentage_24h")]
    price_change_percentage_24h: f64,
}

pub struct CryptoPriceTool;

#[async_trait]
impl Tool for CryptoPriceTool {
    fn name(&self) -> &str {
        "crypto_price"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "crypto_price".into(),
            description: "Get real-time cryptocurrency prices from CoinGecko. Supports Bitcoin, Ethereum, and thousands of other cryptocurrencies.".into(),
            input_schema: schema_object(
                json!({
                    "coin": {
                        "type": "string",
                        "description": "The cryptocurrency coin ID (e.g., 'bitcoin', 'ethereum', 'solana'). Use 'list' to see top 10 coins."
                    }
                }),
                &["coin"],
            ),
        }
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let coin = match input.get("coin").and_then(|v| v.as_str()) {
            Some(c) => c.to_lowercase(),
            None => return ToolResult::error("Missing required parameter: coin".into()),
        };

        if coin == "list" {
            return self.get_top_coins().await;
        }

        match self.get_coin_price(&coin).await {
            Ok(price) => {
                let emoji = if price.price_change_percentage_24h >= 0.0 { "📈" } else { "📉" };
                let result = format!(
                    "{} {} (${})\n💰 Price: ${:.2}\n📊 24h Change: {:.2}%\n🏦 Market Cap: ${:.2}B\n📈 Volume: ${:.2}B",
                    emoji,
                    price.name,
                    price.symbol.to_uppercase(),
                    price.current_price,
                    price.price_change_percentage_24h,
                    price.market_cap as f64 / 1e9,
                    price.total_volume as f64 / 1e9
                );
                ToolResult::success(result)
            }
            Err(e) => ToolResult::error(format!("Failed to fetch price: {}", e)),
        }
    }
}

impl CryptoPriceTool {
    async fn get_coin_price(&self, coin_id: &str) -> anyhow::Result<CoinGeckoPrice> {
        let url = format!(
            "https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&ids={}",
            coin_id
        );

        let response = http_client().get(&url).send().await?;
        
        if !response.status().is_success() {
            return Err(anyhow::anyhow!("API request failed: {}", response.status()));
        }

        let prices: Vec<CoinGeckoPrice> = response.json().await?;
        
        prices.into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Coin '{}' not found. Try 'bitcoin', 'ethereum', or 'list' for top coins.", coin_id))
    }

    async fn get_top_coins(&self) -> ToolResult {
        let url = "https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&order=market_cap_desc&per_page=10&page=1";

        match http_client().get(url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    return ToolResult::error(format!("API request failed: {}", response.status()));
                }

                match response.json::<Vec<CoinGeckoPrice>>().await {
                    Ok(coins) => {
                        let mut result = "Top 10 Cryptocurrencies by Market Cap:\n\n".to_string();
                        for (i, coin) in coins.iter().enumerate() {
                            let emoji = if coin.price_change_percentage_24h >= 0.0 { "🟢" } else { "🔴" };
                            result.push_str(&format!(
                                "{}. {} (${}) - ${:.2} ({} {:.2}%)\n",
                                i + 1,
                                coin.name,
                                coin.symbol.to_uppercase(),
                                coin.current_price,
                                emoji,
                                coin.price_change_percentage_24h
                            ));
                        }
                        result.push_str("\nUse the coin ID (e.g., 'bitcoin', 'ethereum') to get detailed info.");
                        ToolResult::success(result)
                    }
                    Err(e) => ToolResult::error(format!("Failed to parse response: {}", e)),
                }
            }
            Err(e) => ToolResult::error(format!("Request failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_crypto_price_tool() {
        let tool = CryptoPriceTool;
        let input = json!({"coin": "bitcoin"});
        let result = tool.execute(input).await;
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_crypto_price_list() {
        let tool = CryptoPriceTool;
        let input = json!({"coin": "list"});
        let result = tool.execute(input).await;
        assert!(result.is_success());
    }
}
