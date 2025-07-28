// src/config.rs
//its like bots personality settings
//How much money should it risk per trade?
//How often should it check for new coins?
//What makes a "good" coin vs a "scam" coin?

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Database
    pub database_url: String,
    
    // Telegram
    pub telegram_token: String,
    pub telegram_chat_id: i64,
    
    // API Keys (some are optional)
    pub dex_screener_api_key: Option<String>,
    pub birdeye_api_key: Option<String>,
    pub twitter_bearer_token: Option<String>,
    
    // Trading parameters
    pub trading: TradingConfig,
    
    // Scanning intervals (in seconds)
    pub scan_intervals: ScanIntervals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    /// Minimum liquidity required (in USD)
    pub min_liquidity_usd: f64,
    
    /// Maximum percentage of supply held by top holders
    pub max_top_holder_percentage: f64,
    
    /// Minimum number of holders
    pub min_holders: u32,
    
    /// Profit targets (multipliers)
    pub profit_targets: Vec<f64>, // [2.0, 5.0, 10.0, 50.0, 100.0]
    
    /// Stop loss percentage (0.5 = 50% loss)
    pub stop_loss: f64,
    
    /// Maximum investment per token (in USD)
    pub max_investment_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanIntervals {
    pub dex_screener: u64,
    pub pump_fun: u64,
    pub whale_tracking: u64,
    pub twitter_monitoring: u64,
}

impl Config {
    pub fn load() -> Result<Self> {
        // Try to load from environment variables first
        let config = Config {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:crypto_bot.db".to_string()),
            
            telegram_token: env::var("TELEGRAM_TOKEN")
                .expect("TELEGRAM_TOKEN environment variable is required"),
            
            telegram_chat_id: env::var("TELEGRAM_CHAT_ID")
                .expect("TELEGRAM_CHAT_ID environment variable is required")
                .parse()
                .expect("TELEGRAM_CHAT_ID must be a valid integer"),
            
            dex_screener_api_key: env::var("DEX_SCREENER_API_KEY").ok(),
            birdeye_api_key: env::var("BIRDEYE_API_KEY").ok(),
            twitter_bearer_token: env::var("TWITTER_BEARER_TOKEN").ok(),
            
            trading: TradingConfig {
                min_liquidity_usd: env::var("MIN_LIQUIDITY_USD")
                    .unwrap_or_else(|_| "10000.0".to_string())
                    .parse()
                    .unwrap_or(10000.0),
                
                max_top_holder_percentage: env::var("MAX_TOP_HOLDER_PCT")
                    .unwrap_or_else(|_| "30.0".to_string())
                    .parse()
                    .unwrap_or(30.0),
                
                min_holders: env::var("MIN_HOLDERS")
                    .unwrap_or_else(|_| "100".to_string())
                    .parse()
                    .unwrap_or(100),
                
                profit_targets: vec![2.0, 5.0, 10.0, 50.0, 100.0, 500.0],
                
                stop_loss: env::var("STOP_LOSS")
                    .unwrap_or_else(|_| "0.5".to_string())
                    .parse()
                    .unwrap_or(0.5),
                
                max_investment_usd: env::var("MAX_INVESTMENT_USD")
                    .unwrap_or_else(|_| "100.0".to_string())
                    .parse()
                    .unwrap_or(100.0),
            },
            
            scan_intervals: ScanIntervals {
                dex_screener: env::var("DEX_SCREENER_INTERVAL")
                    .unwrap_or_else(|_| "30".to_string())
                    .parse()
                    .unwrap_or(30),
                
                pump_fun: env::var("PUMP_FUN_INTERVAL")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()
                    .unwrap_or(10),
                
                whale_tracking: env::var("WHALE_TRACKING_INTERVAL")
                    .unwrap_or_else(|_| "60".to_string())
                    .parse()
                    .unwrap_or(60),
                
                twitter_monitoring: env::var("TWITTER_MONITORING_INTERVAL")
                    .unwrap_or_else(|_| "120".to_string())
                    .parse()
                    .unwrap_or(120),
            },
        };
        
        Ok(config)
    }
}