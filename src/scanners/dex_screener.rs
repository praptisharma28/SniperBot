// like a security guard at a nightclub:

// Watches the door (monitors DEX Screener API every 30 seconds)
// Checks IDs (filters tokens by liquidity, volume, etc.)
// Lets good people in (saves quality tokens to database)
// Calls the bouncer (triggers analysis for suspicious activity)

// src/scanners/dex_screener.rs
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
                    } else {
                        info!("🔍 DEX Screener: No new tokens found this scan");
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

        info!("📊 Processing {} tokens from DEX Screener", trending_tokens.len());

        for dex_token in trending_tokens {
            // Check if we already have this token
            if state.db.get_token(&dex_token.base_token.address).await?.is_some() {
                continue; // Skip if we already know about this token
            }

            // Convert DEX Screener data to our Token model
            let token = Token {
                id: None,
                address: dex_token.base_token.address.clone(),
                symbol: dex_token.base_token.symbol.clone(),
                name: dex_token.base_token.name.clone(),
                chain: dex_token.chain_id.clone(),
                source: "dex_screener".to_string(),
                created_at: Utc::now(),
                first_seen: Utc::now(),
                is_active: true,
            };

            // Save the token
            match state.db.save_token(&token).await {
                Ok(_) => {
                    info!("💾 Saved new token: {} ({}) on {}", token.symbol, token.name, token.chain);
                    new_tokens_count += 1;

                    // Create and save metrics
                    let metrics = self.convert_to_metrics(&dex_token).await;
                    if let Err(e) = state.db.save_token_metrics(&metrics).await {
                        warn!("Failed to save metrics for {}: {}", token.symbol, e);
                    }

                    // Analyze the token
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
        // Use only working endpoints based on your tests
        let strategies = vec![
            ("trending", "https://api.dexscreener.com/latest/dex/tokens/trending"),
            ("search_sol", "https://api.dexscreener.com/latest/dex/search?q=SOL"),
            ("search_eth", "https://api.dexscreener.com/latest/dex/search?q=ETH"),
            ("search_bnb", "https://api.dexscreener.com/latest/dex/search?q=BNB"),
            ("search_usdc", "https://api.dexscreener.com/latest/dex/search?q=USDC"),
            ("search_wbtc", "https://api.dexscreener.com/latest/dex/search?q=WBTC"),
            ("search_solana", "https://api.dexscreener.com/latest/dex/search?q=solana"),
            ("search_ethereum", "https://api.dexscreener.com/latest/dex/search?q=ethereum"),
        ];
        
        for (name, url) in strategies.iter() {
            info!("🌐 Trying DEX Screener strategy: {}", name);
            
            // Add delay between requests to avoid rate limiting
            tokio::time::sleep(Duration::from_millis(1000)).await; // Increased delay
            
            match self.try_fetch_from_endpoint_with_retry(url, 2).await { // Reduced retries
                Ok(tokens) if !tokens.is_empty() => {
                    info!("✅ Successfully fetched {} tokens using strategy: {}", tokens.len(), name);
                    // Limit to first 10 tokens to avoid overwhelming the system
                    return Ok(tokens.into_iter().take(10).collect());
                }
                Ok(_) => {
                    warn!("⚠️  Strategy {} returned no tokens, trying next...", name);
                    continue;
                }
                Err(e) => {
                    warn!("❌ Strategy {} failed: {}, trying next...", name, e);
                    continue;
                }
            }
        }
        
        // If all real endpoints fail, don't use test tokens in production
        warn!("⚠️  All DEX Screener strategies failed");
        Ok(vec![])
    }

    async fn try_fetch_from_endpoint_with_retry(&self, url: &str, retries: u32) -> Result<Vec<DexScreenerToken>> {
        let mut last_error = None;
        
        for attempt in 1..=retries {
            match self.try_fetch_from_endpoint(url).await {
                Ok(tokens) => return Ok(tokens),
                Err(e) => {
                    warn!("Attempt {}/{} failed for {}: {}", attempt, retries, url, e);
                    last_error = Some(e);
                    if attempt < retries {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All retries failed")))
    }

    async fn try_fetch_from_endpoint(&self, url: &str) -> Result<Vec<DexScreenerToken>> {
        let response = self.client
            .get(url)
            .header("Accept", "application/json")
            .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .header("Referer", "https://dexscreener.com/")
            .send()
            .await?;

        let status = response.status();
        info!("📡 API Response Status: {}", status);

        if !status.is_success() {
            return Err(anyhow::anyhow!("DEX Screener API error: {} - {}", status, 
                match status.as_u16() {
                    429 => "Rate limited - too many requests",
                    403 => "Forbidden - possible geographic restriction",
                    404 => "Endpoint not found",
                    500..=599 => "Server error",
                    _ => "Unknown error"
                }));
        }

        let response_text = response.text().await?;
        info!("📡 Raw response: {}", response_text.chars().take(200).collect::<String>());

        // Check if response contains the null pairs issue
        if response_text.contains("\"pairs\":null") {
            warn!("⚠️  API returned null pairs - this endpoint has no data");
            return Ok(vec![]);
        }

        let dex_response: DexScreenerResponse = serde_json::from_str(&response_text)
            .map_err(|e| anyhow::anyhow!("Failed to parse JSON: {}. Response preview: {}", e, response_text.chars().take(500).collect::<String>()))?;
        
        // Handle null pairs gracefully
        let pairs = dex_response.pairs.unwrap_or_else(|| {
            warn!("⚠️  DEX Screener returned null pairs, using empty array");
            vec![]
        });

        info!("📊 Raw pairs from API: {}", pairs.len());

        // Apply filtering with more lenient criteria
        let filtered_tokens: Vec<DexScreenerToken> = pairs
            .into_iter()
            .filter(|token| self.should_track_token(token))
            .collect();

        info!("🎯 Filtered {} quality tokens from DEX Screener", filtered_tokens.len());
        
        Ok(filtered_tokens)
    }

    // Create test tokens when API fails completely
    fn create_test_tokens(&self) -> Vec<DexScreenerToken> {
        warn!("🧪 Creating test tokens for demonstration purposes");
        
        vec![
            DexScreenerToken {
                chain_id: "solana".to_string(),
                dex_id: "raydium".to_string(),
                url: "https://dexscreener.com/solana/test1".to_string(),
                base_token: BaseToken {
                    address: "So11111111111111111111111111111111111111112".to_string(),
                    name: "Wrapped SOL".to_string(),
                    symbol: "SOL".to_string(),
                },
                quote_token: QuoteToken {
                    address: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    name: "USD Coin".to_string(),
                    symbol: "USDC".to_string(),
                },
                price_native: Some(1.0),
                price_usd: Some(150.0),
                market_cap: Some(1000000.0),
                liquidity: Some(Liquidity {
                    usd: Some(50000.0),
                    base: Some(333.33),
                    quote: Some(50000.0),
                }),
                volume: Some(Volume {
                    h24: Some(100000.0),
                    h6: Some(25000.0),
                    h1: Some(4166.0),
                    m5: Some(347.0),
                }),
                price_change: Some(PriceChange {
                    m5: Some(0.1),
                    h1: Some(0.5),
                    h6: Some(2.0),
                    h24: Some(5.0),
                }),
            }
        ]
    }

    fn should_track_token(&self, token: &DexScreenerToken) -> bool {
        // More permissive filtering to get real tokens
        
        // Must be on supported chains
        let supported_chains = vec!["solana", "ethereum", "bsc", "polygon", "arbitrum", "avalanche", "pulsechain"];
        if !supported_chains.contains(&token.chain_id.as_str()) {
            return false;
        }

        // Skip obvious scam indicators
        if token.base_token.symbol.len() > 20 || token.base_token.name.len() > 50 {
            return false;
        }

        // Skip if price change is too extreme (likely manipulation)
        if let Some(price_change) = &token.price_change {
            if let Some(h24) = price_change.h24 {
                if h24.abs() > 1000.0 { // More than 10x change in 24h is suspicious
                    return false;
                }
            }
        }

        // Very permissive liquidity requirement
        if let Some(liquidity) = &token.liquidity {
            if let Some(usd) = liquidity.usd {
                if usd < 100.0 { // Minimum $100 liquidity
                    return false;
                }
            }
        }

        // Very permissive volume requirement  
        if let Some(volume) = &token.volume {
            if let Some(h24) = volume.h24 {
                if h24 < 10.0 { // Minimum $10 24h volume
                    return false;
                }
            }
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

// DEX Screener API Response Types - FIXED to handle null pairs
#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    #[serde(rename = "schemaVersion")]
    schema_version: Option<String>,
    pairs: Option<Vec<DexScreenerToken>>, // Changed from Vec to Option<Vec>
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
    price_native: Option<f64>,
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
    // Use our token analyzer
    crate::analyzers::token_analyzer::analyze_token(state, token).await
}
