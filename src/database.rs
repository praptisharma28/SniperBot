// src/database.rs
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, Row};
use log::{info, error};

use crate::models::{Token, TokenMetrics, TradingSignal, SimulatedTrade, WhaleWallet, WhaleTransaction};

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        info!("Connecting to database: {}", database_url);
        let pool = SqlitePool::connect(database_url).await?;
        Ok(Database { pool })
    }

    /// Run database migrations to create tables
    pub async fn migrate(&self) -> Result<()> {
        info!("Running database migrations...");
        
        // Create tokens table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                address TEXT UNIQUE NOT NULL,
                symbol TEXT NOT NULL,
                name TEXT NOT NULL,
                chain TEXT NOT NULL,
                source TEXT NOT NULL,
                created_at TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                is_active BOOLEAN NOT NULL DEFAULT TRUE
            )
        "#).execute(&self.pool).await?;

        // Create token_metrics table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS token_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                price_usd TEXT,
                market_cap_usd TEXT,
                liquidity_usd TEXT,
                volume_24h_usd TEXT,
                total_supply TEXT,
                circulating_supply TEXT,
                holder_count INTEGER,
                top_10_holders_percentage TEXT,
                is_honeypot BOOLEAN,
                is_mintable BOOLEAN,
                has_proxy BOOLEAN,
                contract_verified BOOLEAN,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create trading_signals table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS trading_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                signal_type TEXT NOT NULL,
                confidence TEXT NOT NULL,
                reason TEXT NOT NULL,
                target_multiplier TEXT,
                created_at TEXT NOT NULL,
                is_sent BOOLEAN NOT NULL DEFAULT FALSE,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create simulated_trades table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS simulated_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                entry_time TEXT NOT NULL,
                exit_price TEXT,
                exit_time TEXT,
                investment_usd TEXT NOT NULL,
                profit_loss TEXT,
                multiplier TEXT,
                exit_reason TEXT,
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create whale_wallets table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS whale_wallets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                address TEXT UNIQUE NOT NULL,
                chain TEXT NOT NULL,
                label TEXT,
                balance_usd TEXT,
                success_rate TEXT,
                avg_multiplier TEXT,
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                created_at TEXT NOT NULL
            )
        "#).execute(&self.pool).await?;

        // Create whale_transactions table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS whale_transactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                whale_address TEXT NOT NULL,
                token_address TEXT NOT NULL,
                transaction_hash TEXT UNIQUE NOT NULL,
                action TEXT NOT NULL,
                amount_tokens TEXT NOT NULL,
                amount_usd TEXT,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (whale_address) REFERENCES whale_wallets (address),
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        info!("✅ Database migrations completed");
        Ok(())
    }

    // TOKEN OPERATIONS
    pub async fn save_token(&self, token: &Token) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT OR REPLACE INTO tokens 
            (address, symbol, name, chain, source, created_at, first_seen, is_active)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&token.address)
        .bind(&token.symbol)
        .bind(&token.name)
        .bind(&token.chain)
        .bind(&token.source)
        .bind(token.created_at.to_rfc3339())
        .bind(token.first_seen.to_rfc3339())
        .bind(token.is_active)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_token(&self, address: &str) -> Result<Option<Token>> {
        let row = sqlx::query(r#"
            SELECT * FROM tokens WHERE address = ?
        "#)
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(Token {
                id: Some(row.get("id")),
                address: row.get("address"),
                symbol: row.get("symbol"),
                name: row.get("name"),
                chain: row.get("chain"),
                source: row.get("source"),
                created_at: row.get::<String, _>("created_at").parse()?,
                first_seen: row.get::<String, _>("first_seen").parse()?,
                is_active: row.get("is_active"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_recent_tokens(&self, limit: i64) -> Result<Vec<Token>> {
        let rows = sqlx::query(r#"
            SELECT * FROM tokens 
            WHERE is_active = TRUE 
            ORDER BY first_seen DESC 
            LIMIT ?
        "#)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut signals = Vec::new();
        for row in rows {
            signals.push(TradingSignal {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                signal_type: match row.get::<String, _>("signal_type").as_str() {
                    "buy" => crate::models::SignalType::Buy,
                    "sell" => crate::models::SignalType::Sell,
                    "warning" => crate::models::SignalType::Warning,
                    "whalemovement" => crate::models::SignalType::WhaleMovement,
                    _ => crate::models::SignalType::Buy,
                },
                confidence: row.get::<String, _>("confidence").parse()?,
                reason: row.get("reason"),
                target_multiplier: row.get::<Option<String>, _>("target_multiplier").map(|s| s.parse()).transpose()?,
                created_at: row.get::<String, _>("created_at").parse()?,
                is_sent: row.get("is_sent"),
            });
        }

        Ok(signals)
    }

    pub async fn mark_signal_sent(&self, signal_id: i64) -> Result<()> {
        sqlx::query(r#"
            UPDATE trading_signals 
            SET is_sent = TRUE 
            WHERE id = ?
        "#)
        .bind(signal_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // SIMULATED TRADES OPERATIONS
    pub async fn save_simulated_trade(&self, trade: &SimulatedTrade) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO simulated_trades 
            (token_address, entry_price, entry_time, exit_price, exit_time, 
             investment_usd, profit_loss, multiplier, exit_reason, is_active)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&trade.token_address)
        .bind(trade.entry_price.to_string())
        .bind(trade.entry_time.to_rfc3339())
        .bind(trade.exit_price.map(|d| d.to_string()))
        .bind(trade.exit_time.map(|dt| dt.to_rfc3339()))
        .bind(trade.investment_usd.to_string())
        .bind(trade.profit_loss.map(|d| d.to_string()))
        .bind(trade.multiplier.map(|d| d.to_string()))
        .bind(&trade.exit_reason)
        .bind(trade.is_active)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_active_trades(&self) -> Result<Vec<SimulatedTrade>> {
        let rows = sqlx::query(r#"
            SELECT * FROM simulated_trades 
            WHERE is_active = TRUE 
            ORDER BY entry_time DESC
        "#)
        .fetch_all(&self.pool)
        .await?;

        let mut trades = Vec::new();
        for row in rows {
            trades.push(SimulatedTrade {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                entry_price: row.get::<String, _>("entry_price").parse()?,
                entry_time: row.get::<String, _>("entry_time").parse()?,
                exit_price: row.get::<Option<String>, _>("exit_price").map(|s| s.parse()).transpose()?,
                exit_time: row.get::<Option<String>, _>("exit_time").map(|s| s.parse()).transpose()?,
                investment_usd: row.get::<String, _>("investment_usd").parse()?,
                profit_loss: row.get::<Option<String>, _>("profit_loss").map(|s| s.parse()).transpose()?,
                multiplier: row.get::<Option<String>, _>("multiplier").map(|s| s.parse()).transpose()?,
                exit_reason: row.get("exit_reason"),
                is_active: row.get("is_active"),
            });
        }

        Ok(trades)
    }

    pub async fn close_trade(&self, trade_id: i64, exit_price: rust_decimal::Decimal, 
                            profit_loss: rust_decimal::Decimal, multiplier: rust_decimal::Decimal, 
                            exit_reason: &str) -> Result<()> {
        let now = Utc::now();
        
        sqlx::query(r#"
            UPDATE simulated_trades 
            SET exit_price = ?, exit_time = ?, profit_loss = ?, 
                multiplier = ?, exit_reason = ?, is_active = FALSE
            WHERE id = ?
        "#)
        .bind(exit_price.to_string())
        .bind(now.to_rfc3339())
        .bind(profit_loss.to_string())
        .bind(multiplier.to_string())
        .bind(exit_reason)
        .bind(trade_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // WHALE OPERATIONS
    pub async fn save_whale_wallet(&self, whale: &WhaleWallet) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT OR REPLACE INTO whale_wallets 
            (address, chain, label, balance_usd, success_rate, avg_multiplier, is_active, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&whale.address)
        .bind(&whale.chain)
        .bind(&whale.label)
        .bind(whale.balance_usd.map(|d| d.to_string()))
        .bind(whale.success_rate.map(|d| d.to_string()))
        .bind(whale.avg_multiplier.map(|d| d.to_string()))
        .bind(whale.is_active)
        .bind(whale.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_active_whales(&self) -> Result<Vec<WhaleWallet>> {
        let rows = sqlx::query(r#"
            SELECT * FROM whale_wallets 
            WHERE is_active = TRUE 
            ORDER BY success_rate DESC
        "#)
        .fetch_all(&self.pool)
        .await?;

        let mut whales = Vec::new();
        for row in rows {
            whales.push(WhaleWallet {
                id: Some(row.get("id")),
                address: row.get("address"),
                chain: row.get("chain"),
                label: row.get("label"),
                balance_usd: row.get::<Option<String>, _>("balance_usd").map(|s| s.parse()).transpose()?,
                success_rate: row.get::<Option<String>, _>("success_rate").map(|s| s.parse()).transpose()?,
                avg_multiplier: row.get::<Option<String>, _>("avg_multiplier").map(|s| s.parse()).transpose()?,
                is_active: row.get("is_active"),
                created_at: row.get::<String, _>("created_at").parse()?,
            });
        }

        Ok(whales)
    }

    // STATISTICS
    pub async fn get_trading_stats(&self) -> Result<TradingStats> {
        let total_trades = sqlx::query_scalar::<_, i64>(r#"
            SELECT COUNT(*) FROM simulated_trades WHERE is_active = FALSE
        "#)
        .fetch_one(&self.pool)
        .await?;

        let profitable_trades = sqlx::query_scalar::<_, i64>(r#"
            SELECT COUNT(*) FROM simulated_trades 
            WHERE is_active = FALSE AND profit_loss > '0'
        "#)
        .fetch_one(&self.pool)
        .await?;

        let total_profit = sqlx::query_scalar::<_, Option<String>>(r#"
            SELECT SUM(CAST(profit_loss AS REAL)) FROM simulated_trades 
            WHERE is_active = FALSE
        "#)
        .fetch_one(&self.pool)
        .await?;

        let avg_multiplier = sqlx::query_scalar::<_, Option<String>>(r#"
            SELECT AVG(CAST(multiplier AS REAL)) FROM simulated_trades 
            WHERE is_active = FALSE AND multiplier IS NOT NULL
        "#)
        .fetch_one(&self.pool)
        .await?;

        Ok(TradingStats {
            total_trades,
            profitable_trades,
            win_rate: if total_trades > 0 { 
                (profitable_trades as f64 / total_trades as f64) * 100.0 
            } else { 
                0.0 
            },
            total_profit_usd: total_profit
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0),
            avg_multiplier: avg_multiplier
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(1.0),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TradingStats {
    pub total_trades: i64,
    pub profitable_trades: i64,
    pub win_rate: f64,
    pub total_profit_usd: f64,
    pub avg_multiplier: f64,
}(&self.pool)
        .await?;

        let mut tokens = Vec::new();
        for row in rows {
            tokens.push(Token {
                id: Some(row.get("id")),
                address: row.get("address"),
                symbol: row.get("symbol"),
                name: row.get("name"),
                chain: row.get("chain"),
                source: row.get("source"),
                created_at: row.get::<String, _>("created_at").parse()?,
                first_seen: row.get::<String, _>("first_seen").parse()?,
                is_active: row.get("is_active"),
            });
        }

        Ok(tokens)
    }

    // TOKEN METRICS OPERATIONS
    pub async fn save_token_metrics(&self, metrics: &TokenMetrics) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO token_metrics 
            (token_address, timestamp, price_usd, market_cap_usd, liquidity_usd, 
             volume_24h_usd, total_supply, circulating_supply, holder_count, 
             top_10_holders_percentage, is_honeypot, is_mintable, has_proxy, contract_verified)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&metrics.token_address)
        .bind(metrics.timestamp.to_rfc3339())
        .bind(metrics.price_usd.map(|d| d.to_string()))
        .bind(metrics.market_cap_usd.map(|d| d.to_string()))
        .bind(metrics.liquidity_usd.map(|d| d.to_string()))
        .bind(metrics.volume_24h_usd.map(|d| d.to_string()))
        .bind(metrics.total_supply.map(|d| d.to_string()))
        .bind(metrics.circulating_supply.map(|d| d.to_string()))
        .bind(metrics.holder_count)
        .bind(metrics.top_10_holders_percentage.map(|d| d.to_string()))
        .bind(metrics.is_honeypot)
        .bind(metrics.is_mintable)
        .bind(metrics.has_proxy)
        .bind(metrics.contract_verified)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_latest_metrics(&self, token_address: &str) -> Result<Option<TokenMetrics>> {
        let row = sqlx::query(r#"
            SELECT * FROM token_metrics 
            WHERE token_address = ? 
            ORDER BY timestamp DESC 
            LIMIT 1
        "#)
        .bind(token_address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(TokenMetrics {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                timestamp: row.get::<String, _>("timestamp").parse()?,
                price_usd: row.get::<Option<String>, _>("price_usd").map(|s| s.parse()).transpose()?,
                market_cap_usd: row.get::<Option<String>, _>("market_cap_usd").map(|s| s.parse()).transpose()?,
                liquidity_usd: row.get::<Option<String>, _>("liquidity_usd").map(|s| s.parse()).transpose()?,
                volume_24h_usd: row.get::<Option<String>, _>("volume_24h_usd").map(|s| s.parse()).transpose()?,
                total_supply: row.get::<Option<String>, _>("total_supply").map(|s| s.parse()).transpose()?,
                circulating_supply: row.get::<Option<String>, _>("circulating_supply").map(|s| s.parse()).transpose()?,
                holder_count: row.get("holder_count"),
                top_10_holders_percentage: row.get::<Option<String>, _>("top_10_holders_percentage").map(|s| s.parse()).transpose()?,
                is_honeypot: row.get("is_honeypot"),
                is_mintable: row.get("is_mintable"),
                has_proxy: row.get("has_proxy"),
                contract_verified: row.get("contract_verified"),
            }))
        } else {
            Ok(None)
        }
    }

    // TRADING SIGNALS OPERATIONS
    pub async fn save_trading_signal(&self, signal: &TradingSignal) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO trading_signals 
            (token_address, signal_type, confidence, reason, target_multiplier, created_at, is_sent)
            VALUES (?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&signal.token_address)
        .bind(format!("{:?}", signal.signal_type).to_lowercase())
        .bind(signal.confidence.to_string())
        .bind(&signal.reason)
        .bind(signal.target_multiplier.map(|d| d.to_string()))
        .bind(signal.created_at.to_rfc3339())
        .bind(signal.is_sent)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_unsent_signals(&self) -> Result<Vec<TradingSignal>> {
        let rows = sqlx::query(r#"
            SELECT * FROM trading_signals 
            WHERE is_sent = FALSE 
            ORDER BY created_at ASC
        "#)
        .fetch_all// src/database.rs
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, Row};
use log::{info, error};

use crate::models::{Token, TokenMetrics, TradingSignal, SimulatedTrade, WhaleWallet, WhaleTransaction};

pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        info!("Connecting to database: {}", database_url);
        let pool = SqlitePool::connect(database_url).await?;
        Ok(Database { pool })
    }

    /// Run database migrations to create tables
    pub async fn migrate(&self) -> Result<()> {
        info!("Running database migrations...");
        
        // Create tokens table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                address TEXT UNIQUE NOT NULL,
                symbol TEXT NOT NULL,
                name TEXT NOT NULL,
                chain TEXT NOT NULL,
                source TEXT NOT NULL,
                created_at TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                is_active BOOLEAN NOT NULL DEFAULT TRUE
            )
        "#).execute(&self.pool).await?;

        // Create token_metrics table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS token_metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                price_usd TEXT,
                market_cap_usd TEXT,
                liquidity_usd TEXT,
                volume_24h_usd TEXT,
                total_supply TEXT,
                circulating_supply TEXT,
                holder_count INTEGER,
                top_10_holders_percentage TEXT,
                is_honeypot BOOLEAN,
                is_mintable BOOLEAN,
                has_proxy BOOLEAN,
                contract_verified BOOLEAN,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create trading_signals table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS trading_signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                signal_type TEXT NOT NULL,
                confidence TEXT NOT NULL,
                reason TEXT NOT NULL,
                target_multiplier TEXT,
                created_at TEXT NOT NULL,
                is_sent BOOLEAN NOT NULL DEFAULT FALSE,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create simulated_trades table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS simulated_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                token_address TEXT NOT NULL,
                entry_price TEXT NOT NULL,
                entry_time TEXT NOT NULL,
                exit_price TEXT,
                exit_time TEXT,
                investment_usd TEXT NOT NULL,
                profit_loss TEXT,
                multiplier TEXT,
                exit_reason TEXT,
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        // Create whale_wallets table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS whale_wallets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                address TEXT UNIQUE NOT NULL,
                chain TEXT NOT NULL,
                label TEXT,
                balance_usd TEXT,
                success_rate TEXT,
                avg_multiplier TEXT,
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                created_at TEXT NOT NULL
            )
        "#).execute(&self.pool).await?;

        // Create whale_transactions table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS whale_transactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                whale_address TEXT NOT NULL,
                token_address TEXT NOT NULL,
                transaction_hash TEXT UNIQUE NOT NULL,
                action TEXT NOT NULL,
                amount_tokens TEXT NOT NULL,
                amount_usd TEXT,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (whale_address) REFERENCES whale_wallets (address),
                FOREIGN KEY (token_address) REFERENCES tokens (address)
            )
        "#).execute(&self.pool).await?;

        info!("✅ Database migrations completed");
        Ok(())
    }

    // TOKEN OPERATIONS
    pub async fn save_token(&self, token: &Token) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT OR REPLACE INTO tokens 
            (address, symbol, name, chain, source, created_at, first_seen, is_active)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&token.address)
        .bind(&token.symbol)
        .bind(&token.name)
        .bind(&token.chain)
        .bind(&token.source)
        .bind(token.created_at.to_rfc3339())
        .bind(token.first_seen.to_rfc3339())
        .bind(token.is_active)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_token(&self, address: &str) -> Result<Option<Token>> {
        let row = sqlx::query(r#"
            SELECT * FROM tokens WHERE address = ?
        "#)
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(Token {
                id: Some(row.get("id")),
                address: row.get("address"),
                symbol: row.get("symbol"),
                name: row.get("name"),
                chain: row.get("chain"),
                source: row.get("source"),
                created_at: row.get::<String, _>("created_at").parse()?,
                first_seen: row.get::<String, _>("first_seen").parse()?,
                is_active: row.get("is_active"),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_recent_tokens(&self, limit: i64) -> Result<Vec<Token>> {
        let rows = sqlx::query(r#"
            SELECT * FROM tokens 
            WHERE is_active = TRUE 
            ORDER BY first_seen DESC 
            LIMIT ?
        "#)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut signals = Vec::new();
        for row in rows {
            signals.push(TradingSignal {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                signal_type: match row.get::<String, _>("signal_type").as_str() {
                    "buy" => crate::models::SignalType::Buy,
                    "sell" => crate::models::SignalType::Sell,
                    "warning" => crate::models::SignalType::Warning,
                    "whalemovement" => crate::models::SignalType::WhaleMovement,
                    _ => crate::models::SignalType::Buy,
                },
                confidence: row.get::<String, _>("confidence").parse()?,
                reason: row.get("reason"),
                target_multiplier: row.get::<Option<String>, _>("target_multiplier").map(|s| s.parse()).transpose()?,
                created_at: row.get::<String, _>("created_at").parse()?,
                is_sent: row.get("is_sent"),
            });
        }

        Ok(signals)
    }

    pub async fn mark_signal_sent(&self, signal_id: i64) -> Result<()> {
        sqlx::query(r#"
            UPDATE trading_signals 
            SET is_sent = TRUE 
            WHERE id = ?
        "#)
        .bind(signal_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // SIMULATED TRADES OPERATIONS
    pub async fn save_simulated_trade(&self, trade: &SimulatedTrade) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO simulated_trades 
            (token_address, entry_price, entry_time, exit_price, exit_time, 
             investment_usd, profit_loss, multiplier, exit_reason, is_active)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&trade.token_address)
        .bind(trade.entry_price.to_string())
        .bind(trade.entry_time.to_rfc3339())
        .bind(trade.exit_price.map(|d| d.to_string()))
        .bind(trade.exit_time.map(|dt| dt.to_rfc3339()))
        .bind(trade.investment_usd.to_string())
        .bind(trade.profit_loss.map(|d| d.to_string()))
        .bind(trade.multiplier.map(|d| d.to_string()))
        .bind(&trade.exit_reason)
        .bind(trade.is_active)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_active_trades(&self) -> Result<Vec<SimulatedTrade>> {
        let rows = sqlx::query(r#"
            SELECT * FROM simulated_trades 
            WHERE is_active = TRUE 
            ORDER BY entry_time DESC
        "#)
        .fetch_all(&self.pool)
        .await?;

        let mut trades = Vec::new();
        for row in rows {
            trades.push(SimulatedTrade {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                entry_price: row.get::<String, _>("entry_price").parse()?,
                entry_time: row.get::<String, _>("entry_time").parse()?,
                exit_price: row.get::<Option<String>, _>("exit_price").map(|s| s.parse()).transpose()?,
                exit_time: row.get::<Option<String>, _>("exit_time").map(|s| s.parse()).transpose()?,
                investment_usd: row.get::<String, _>("investment_usd").parse()?,
                profit_loss: row.get::<Option<String>, _>("profit_loss").map(|s| s.parse()).transpose()?,
                multiplier: row.get::<Option<String>, _>("multiplier").map(|s| s.parse()).transpose()?,
                exit_reason: row.get("exit_reason"),
                is_active: row.get("is_active"),
            });
        }

        Ok(trades)
    }

    pub async fn close_trade(&self, trade_id: i64, exit_price: rust_decimal::Decimal, 
                            profit_loss: rust_decimal::Decimal, multiplier: rust_decimal::Decimal, 
                            exit_reason: &str) -> Result<()> {
        let now = Utc::now();
        
        sqlx::query(r#"
            UPDATE simulated_trades 
            SET exit_price = ?, exit_time = ?, profit_loss = ?, 
                multiplier = ?, exit_reason = ?, is_active = FALSE
            WHERE id = ?
        "#)
        .bind(exit_price.to_string())
        .bind(now.to_rfc3339())
        .bind(profit_loss.to_string())
        .bind(multiplier.to_string())
        .bind(exit_reason)
        .bind(trade_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // WHALE OPERATIONS
    pub async fn save_whale_wallet(&self, whale: &WhaleWallet) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT OR REPLACE INTO whale_wallets 
            (address, chain, label, balance_usd, success_rate, avg_multiplier, is_active, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&whale.address)
        .bind(&whale.chain)
        .bind(&whale.label)
        .bind(whale.balance_usd.map(|d| d.to_string()))
        .bind(whale.success_rate.map(|d| d.to_string()))
        .bind(whale.avg_multiplier.map(|d| d.to_string()))
        .bind(whale.is_active)
        .bind(whale.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_active_whales(&self) -> Result<Vec<WhaleWallet>> {
        let rows = sqlx::query(r#"
            SELECT * FROM whale_wallets 
            WHERE is_active = TRUE 
            ORDER BY success_rate DESC
        "#)
        .fetch_all(&self.pool)
        .await?;

        let mut whales = Vec::new();
        for row in rows {
            whales.push(WhaleWallet {
                id: Some(row.get("id")),
                address: row.get("address"),
                chain: row.get("chain"),
                label: row.get("label"),
                balance_usd: row.get::<Option<String>, _>("balance_usd").map(|s| s.parse()).transpose()?,
                success_rate: row.get::<Option<String>, _>("success_rate").map(|s| s.parse()).transpose()?,
                avg_multiplier: row.get::<Option<String>, _>("avg_multiplier").map(|s| s.parse()).transpose()?,
                is_active: row.get("is_active"),
                created_at: row.get::<String, _>("created_at").parse()?,
            });
        }

        Ok(whales)
    }

    // STATISTICS
    pub async fn get_trading_stats(&self) -> Result<TradingStats> {
        let total_trades = sqlx::query_scalar::<_, i64>(r#"
            SELECT COUNT(*) FROM simulated_trades WHERE is_active = FALSE
        "#)
        .fetch_one(&self.pool)
        .await?;

        let profitable_trades = sqlx::query_scalar::<_, i64>(r#"
            SELECT COUNT(*) FROM simulated_trades 
            WHERE is_active = FALSE AND profit_loss > '0'
        "#)
        .fetch_one(&self.pool)
        .await?;

        let total_profit = sqlx::query_scalar::<_, Option<String>>(r#"
            SELECT SUM(CAST(profit_loss AS REAL)) FROM simulated_trades 
            WHERE is_active = FALSE
        "#)
        .fetch_one(&self.pool)
        .await?;

        let avg_multiplier = sqlx::query_scalar::<_, Option<String>>(r#"
            SELECT AVG(CAST(multiplier AS REAL)) FROM simulated_trades 
            WHERE is_active = FALSE AND multiplier IS NOT NULL
        "#)
        .fetch_one(&self.pool)
        .await?;

        Ok(TradingStats {
            total_trades,
            profitable_trades,
            win_rate: if total_trades > 0 { 
                (profitable_trades as f64 / total_trades as f64) * 100.0 
            } else { 
                0.0 
            },
            total_profit_usd: total_profit
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0),
            avg_multiplier: avg_multiplier
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(1.0),
        })
    }
}

#[derive(Debug, Clone)]
pub struct TradingStats {
    pub total_trades: i64,
    pub profitable_trades: i64,
    pub win_rate: f64,
    pub total_profit_usd: f64,
    pub avg_multiplier: f64,
}(&self.pool)
        .await?;

        let mut tokens = Vec::new();
        for row in rows {
            tokens.push(Token {
                id: Some(row.get("id")),
                address: row.get("address"),
                symbol: row.get("symbol"),
                name: row.get("name"),
                chain: row.get("chain"),
                source: row.get("source"),
                created_at: row.get::<String, _>("created_at").parse()?,
                first_seen: row.get::<String, _>("first_seen").parse()?,
                is_active: row.get("is_active"),
            });
        }

        Ok(tokens)
    }

    // TOKEN METRICS OPERATIONS
    pub async fn save_token_metrics(&self, metrics: &TokenMetrics) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO token_metrics 
            (token_address, timestamp, price_usd, market_cap_usd, liquidity_usd, 
             volume_24h_usd, total_supply, circulating_supply, holder_count, 
             top_10_holders_percentage, is_honeypot, is_mintable, has_proxy, contract_verified)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&metrics.token_address)
        .bind(metrics.timestamp.to_rfc3339())
        .bind(metrics.price_usd.map(|d| d.to_string()))
        .bind(metrics.market_cap_usd.map(|d| d.to_string()))
        .bind(metrics.liquidity_usd.map(|d| d.to_string()))
        .bind(metrics.volume_24h_usd.map(|d| d.to_string()))
        .bind(metrics.total_supply.map(|d| d.to_string()))
        .bind(metrics.circulating_supply.map(|d| d.to_string()))
        .bind(metrics.holder_count)
        .bind(metrics.top_10_holders_percentage.map(|d| d.to_string()))
        .bind(metrics.is_honeypot)
        .bind(metrics.is_mintable)
        .bind(metrics.has_proxy)
        .bind(metrics.contract_verified)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_latest_metrics(&self, token_address: &str) -> Result<Option<TokenMetrics>> {
        let row = sqlx::query(r#"
            SELECT * FROM token_metrics 
            WHERE token_address = ? 
            ORDER BY timestamp DESC 
            LIMIT 1
        "#)
        .bind(token_address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = row {
            Ok(Some(TokenMetrics {
                id: Some(row.get("id")),
                token_address: row.get("token_address"),
                timestamp: row.get::<String, _>("timestamp").parse()?,
                price_usd: row.get::<Option<String>, _>("price_usd").map(|s| s.parse()).transpose()?,
                market_cap_usd: row.get::<Option<String>, _>("market_cap_usd").map(|s| s.parse()).transpose()?,
                liquidity_usd: row.get::<Option<String>, _>("liquidity_usd").map(|s| s.parse()).transpose()?,
                volume_24h_usd: row.get::<Option<String>, _>("volume_24h_usd").map(|s| s.parse()).transpose()?,
                total_supply: row.get::<Option<String>, _>("total_supply").map(|s| s.parse()).transpose()?,
                circulating_supply: row.get::<Option<String>, _>("circulating_supply").map(|s| s.parse()).transpose()?,
                holder_count: row.get("holder_count"),
                top_10_holders_percentage: row.get::<Option<String>, _>("top_10_holders_percentage").map(|s| s.parse()).transpose()?,
                is_honeypot: row.get("is_honeypot"),
                is_mintable: row.get("is_mintable"),
                has_proxy: row.get("has_proxy"),
                contract_verified: row.get("contract_verified"),
            }))
        } else {
            Ok(None)
        }
    }

    // TRADING SIGNALS OPERATIONS
    pub async fn save_trading_signal(&self, signal: &TradingSignal) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO trading_signals 
            (token_address, signal_type, confidence, reason, target_multiplier, created_at, is_sent)
            VALUES (?, ?, ?, ?, ?, ?, ?)
        "#)
        .bind(&signal.token_address)
        .bind(format!("{:?}", signal.signal_type).to_lowercase())
        .bind(signal.confidence.to_string())
        .bind(&signal.reason)
        .bind(signal.target_multiplier.map(|d| d.to_string()))
        .bind(signal.created_at.to_rfc3339())
        .bind(signal.is_sent)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn get_unsent_signals(&self) -> Result<Vec<TradingSignal>> {
        let rows = sqlx::query(r#"
            SELECT * FROM trading_signals 
            WHERE is_sent = FALSE 
            ORDER BY created_at ASC
        "#)
        .fetch_all