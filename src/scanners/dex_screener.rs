// like a security guard at a nightclub:

// Watches the door (monitors DEX Screener API every 30 seconds)
// Checks IDs (filters tokens by liquidity, volume, etc.)
// Lets good people in (saves quality tokens to database)
// Calls the bouncer (triggers analysis for suspicious activity)

use anyhow::Result;
use chrono::Utc;
use log::{info, warn, error};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::config::Config;
use crate::models::{Token, TokenMetrics};
use crate::AppState;

pub struct DexScreenerScanner {
    client: Client,
    config: Config,
}

impl DexScreenerScanner {
    pub fn new(config: &Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("CryptoBot/1.0")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            config: config.clone(),
        }
    }

    pub async fn start_scanning(&self, state: Arc<AppState>) -> Result<()> {
        info!("🔍 Starting DEX Screener scanner...");
        
        loop {
            match self.scan_new_tokens(&state).await {
                Ok(count) => {
                    if count > 0 {
                        info!("✅ DEX Screener: Found {} new tokens", count);
                    }
                }
                Err(e) => {
                    error!("❌ DEX Screener scan error: {}", e);
                }
            }

            // Check if we should keep running
            if !*state.running.read().await {
                info!("🛑 DEX Screener scanner stopping...");
                break;
            }

            sleep(Duration::from_secs(self.config.scan_intervals.dex_screener)).await;
        }

        Ok(())
    }

    async fn scan_new_tokens(&self, state: &Arc<AppState>) -> Result<usize> {
        // Get trending tokens from DEX Screener
        let trending_tokens = self.fetch_trending_tokens().await?;
        let mut new_tokens_count = 0;

        for dex_token in trending_tokens {
            // Check if we already have this token
            if state.db.get_token(&dex_token.base_token.address).await?.is_some() {
                continue; // Skip if we already know about this token
            }

            // Convert DEX Screener data to our Token model
            let token = Token {
                id: None,
                address: dex_token.base_token.address.clone(),
                symbol: dex_token.base_token.symbol,
                name: dex_token.base_token.name,
                chain: dex_token.chain_id.clone(),
                source: "dex_screener".to_string(),
                created_at: Utc::now(),
                first_seen: Utc::now(),
                is_active: true,
            };

            // Save the token
            match state.db.save_token(&token).await {
                Ok(_) => {
                    info!("💾 Saved new token: {} ({})", token.symbol, token.name);
                    new_tokens_count += 1;

                    // Create and save metrics
                    let metrics = self.convert_to_metrics(&dex_token).await;
                    if let Err(e) = state.db.save_token_metrics(&metrics).await {
                        warn!("Failed to save metrics for {}: {}", token.symbol, e);
                    }

                    // Analyze the token (we'll implement this next)
                    tokio::spawn({
                        let state = state.clone();
                        let token = token.clone();
                        async move {
                            if let Err(e) = analyze_and_signal(state, token).await {
                                error!("Analysis failed: {}", e);
                            }
                        }
                    });
                }
                Err(e) => {
                    warn!("Failed to save token {}: {}", token.symbol, e);
                }
            }
        }

        Ok(new_tokens_count)
    }

    async fn fetch_trending_tokens(&self) -> Result<Vec<DexScreenerToken>> {
        // DEX Screener API endpoint for trending tokens
        let url = "https://api.dexscreener.com/latest/dex/tokens/trending";
        
        info!("🌐 Fetching trending tokens from DEX Screener...");
        
        let response = self.client
            .get(url)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!("DEX Screener API error: {}", response.status()));
        }

        let dex_response: DexScreenerResponse = response.json().await?;
        
        // Filter for quality tokens
        let filtered_tokens: Vec<DexScreenerToken> = dex_response.pairs
            .into_iter()
            .filter(|token| self.should_track_token(token))
            .collect();

        info!("🎯 Filtered {} quality tokens from DEX Screener", filtered_tokens.len());
        
        Ok(filtered_tokens)
    }

    fn should_track_token(&self, token: &DexScreenerToken) -> bool {
        // Basic filtering criteria
        
        // Must have minimum liquidity
        if let Some(liquidity) = &token.liquidity {
            if let Some(usd) = liquidity.usd {
                if usd < self.config.trading.min_liquidity_usd {
                    return false;
                }
            }
        }

        // Must have volume
        if let Some(volume) = &token.volume {
            if let Some(h24) = volume.h24 {
                if h24 < 1000.0 { // Minimum $1000 24h volume
                    return false;
                }
            }
        }

        // Skip if price change is too extreme (might be manipulation)
        if let Some(price_change) = &token.price_change {
            if let Some(h24) = price_change.h24 {
                if h24.abs() > 10000.0 { // More than 100x change in 24h is suspicious
                    return false;
                }
            }
        }

        // Must be on supported chains
        let supported_chains = vec!["solana", "ethereum", "bsc", "polygon"];
        if !supported_chains.contains(&token.chain_id.as_str()) {
            return false;
        }

        true
    }

    async fn convert_to_metrics(&self, dex_token: &DexScreenerToken) -> TokenMetrics {
        TokenMetrics {
            id: None,
            token_address: dex_token.base_token.address.clone(),
            timestamp: Utc::now(),
            price_usd: dex_token.price_usd.map(|p| Decimal::try_from(p).unwrap_or(Decimal::ZERO)),
            market_cap_usd: dex_token.market_cap.map(|mc| Decimal::try_from(mc).unwrap_or(Decimal::ZERO)),
            liquidity_usd: dex_token.liquidity.as_ref()
                .and_then(|l| l.usd)
                .map(|l| Decimal::try_from(l).unwrap_or(Decimal::ZERO)),
            volume_24h_usd: dex_token.volume.as_ref()
                .and_then(|v| v.h24)
                .map(|v| Decimal::try_from(v).unwrap_or(Decimal::ZERO)),
            total_supply: None, // DEX Screener doesn't provide this
            circulating_supply: None,
            holder_count: None,
            top_10_holders_percentage: None,
            is_honeypot: None, // We'll check this with other tools
            is_mintable: None,
            has_proxy: None,
            contract_verified: None,
        }
    }
}

// DEX Screener API Response Types
#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    pairs: Vec<DexScreenerToken>,
}

#[derive(Debug, Deserialize)]
struct DexScreenerToken {
    #[serde(rename = "chainId")]
    chain_id: String,
    #[serde(rename = "dexId")]
    dex_id: String,
    url: String,
    #[serde(rename = "baseToken")]
    base_token: BaseToken,
    #[serde(rename = "quoteToken")]
    quote_token: QuoteToken,
    #[serde(rename = "priceNative")]
    price_native: Option<String>,
    #[serde(rename = "priceUsd")]
    price_usd: Option<f64>,
    #[serde(rename = "marketCap")]
    market_cap: Option<f64>,
    liquidity: Option<Liquidity>,
    volume: Option<Volume>,
    #[serde(rename = "priceChange")]
    price_change: Option<PriceChange>,
}

#[derive(Debug, Deserialize)]
struct BaseToken {
    address: String,
    name: String,
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct QuoteToken {
    address: String,
    name: String,
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct Liquidity {
    usd: Option<f64>,
    base: Option<f64>,
    quote: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Volume {
    h24: Option<f64>,
    h6: Option<f64>,
    h1: Option<f64>,
    m5: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct PriceChange {
    m5: Option<f64>,
    h1: Option<f64>,
    h6: Option<f64>,
    h24: Option<f64>,
}

// This function will analyze tokens and generate signals
async fn analyze_and_signal(state: Arc<AppState>, token: Token) -> Result<()> {
    // We'll implement the full analysis logic next
    // For now, just log that we're analyzing
    info!("🧠 Analyzing token: {} ({})", token.symbol, token.name);
    
    // TODO: Implement full analysis using our analyzers module
    
    Ok(())
}
