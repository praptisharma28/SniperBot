// src/main.rs
use anyhow::Result;
use log::{info, error};
use std::sync::Arc;
use tokio::sync::RwLock;

mod config;
mod models;
mod scanners;
mod analyzers;
mod database;
mod telegram;
mod strategies;
mod utils;

use config::Config;
use database::Database;
use telegram::TelegramBot;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::init();
    info!("🚀 Starting Crypto Research Bot");

    // Load configuration
    let config = Config::load()?;
    info!("✅ Configuration loaded");

    // Initialize database
    let db = Database::new(&config.database_url).await?;
    db.migrate().await?;
    info!("✅ Database initialized");

    // Initialize Telegram bot
    let telegram = TelegramBot::new(&config.telegram_token).await?;
    info!("✅ Telegram bot initialized");

    // Create shared state
    let app_state = Arc::new(AppState {
        config,
        db,
        telegram,
        running: RwLock::new(true),
    });

    // Start all the scanning services
    let mut handles = vec![];

    // Start DEX Screener scanner
    handles.push(tokio::spawn(start_dex_screener_scanner(app_state.clone())));

    // Start Pump.fun scanner (when we implement it)
    // handles.push(tokio::spawn(start_pumpfun_scanner(app_state.clone())));

    // Start whale tracking
    // handles.push(tokio::spawn(start_whale_tracker(app_state.clone())));

    // Start Telegram bot
    handles.push(tokio::spawn(start_telegram_bot(app_state.clone())));

    info!("🔥 All services started! Bot is now running...");

    // Wait for all services to complete
    for handle in handles {
        if let Err(e) = handle.await {
            error!("Service error: {}", e);
        }
    }

    Ok(())
}

/// Shared application state
pub struct AppState {
    pub config: Config,
    pub db: Database,
    pub telegram: TelegramBot,
    pub running: RwLock<bool>,
}

async fn start_dex_screener_scanner(state: Arc<AppState>) -> Result<()> {
    use scanners::dex_screener::DexScreenerScanner;
    
    let scanner = DexScreenerScanner::new(&state.config);
    scanner.start_scanning(state).await
}

async fn start_telegram_bot(state: Arc<AppState>) -> Result<()> {
    state.telegram.start(state).await
}
