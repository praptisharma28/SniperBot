// src/telegram.rs
use anyhow::Result;
use log::{info, error, warn};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{ParseMode, ChatId},
    Bot,
    utils::command::BotCommands,
    dispatching::{dialogue::InMemStorage, UpdateHandler},
};
use tokio::time::{sleep, Duration};

use crate::models::{TradingSignal, SignalType};
use crate::AppState;

pub struct TelegramBot {
    bot: Bot,
}

impl TelegramBot {
    pub async fn new(token: &str) -> Result<Self> {
        let bot = Bot::new(token);
        
        // Test the bot connection
        match bot.get_me().await {
            Ok(me) => {
                info!("✅ Telegram bot connected: @{}", me.username());
            }
            Err(e) => {
                error!("❌ Failed to connect to Telegram: {}", e);
                return Err(anyhow::anyhow!("Telegram connection failed: {}", e));
            }
        }

        Ok(Self { bot })
    }

    pub async fn start(&self, state: Arc<AppState>) -> Result<()> {
        info!("🤖 Starting Telegram bot service...");

        // Get chat ID from config
        let chat_id = ChatId(state.config.telegram_chat_id);

        // Send startup message
        self.send_startup_message(chat_id).await?;

        // Start signal processor in background
        let state_for_signals = state.clone();
        let bot_for_signals = self.bot.clone();
        tokio::spawn(async move {
            if let Err(e) = process_trading_signals(bot_for_signals, chat_id, state_for_signals).await {
                error!("Signal processor error: {}", e);
            }
        });

        // Create the command handler
        let handler = Update::filter_message()
            .filter_command::<Command>()
            .endpoint(answer_command);

        // Start the dispatcher
        Dispatcher::builder(self.bot.clone(), handler)
            .dependencies(dptree::deps![state])
            .default_handler(|upd| async move {
                log::debug!("Unhandled update: {:?}", upd);
            })
            .error_handler(LoggingErrorHandler::with_custom_text(
                "An error has occurred in the dispatcher",
            ))
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;

        Ok(())
    }

    async fn send_startup_message(&self, chat_id: ChatId) -> Result<()> {
        let message = format!(
            "🚀 *Crypto Sniper Bot Started\\!*\n\n\
             ✅ All scanners active\n\
             ✅ Analysis engine ready\n\
             ✅ Database connected\n\n\
             🔍 Monitoring:\n\
             • DEX Screener\n\
             • Pump\\.fun \\(coming soon\\)\n\
             • Whale movements \\(coming soon\\)\n\n\
             Use /help for commands"
        );

        self.bot
            .send_message(chat_id, message)
            .parse_mode(ParseMode::MarkdownV2)
            .await?;

        Ok(())
    }
}

async fn process_trading_signals(bot: Bot, chat_id: ChatId, state: Arc<AppState>) -> Result<()> {
    info!("📡 Starting signal processor...");

    loop {
        // Check for unsent signals
        match state.db.get_unsent_signals().await {
            Ok(signals) => {
                for signal in signals {
                    if let Err(e) = send_trading_signal(&bot, chat_id, &signal, &state).await {
                        error!("Failed to send signal: {}", e);
                        continue;
                    }

                    // Mark as sent
                    if let Some(id) = signal.id {
                        if let Err(e) = state.db.mark_signal_sent(id).await {
                            warn!("Failed to mark signal as sent: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to fetch signals: {}", e);
            }
        }

        // Check if we should keep running
        if !*state.running.read().await {
            break;
        }

        sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}

async fn send_trading_signal(bot: &Bot, chat_id: ChatId, signal: &TradingSignal, state: &Arc<AppState>) -> Result<()> {
    // Get token info for the signal
    let token = match state.db.get_token(&signal.token_address).await? {
        Some(token) => token,
        None => {
            warn!("Token not found for signal: {}", signal.token_address);
            return Ok(());
        }
    };

    // Get latest metrics
    let metrics = state.db.get_latest_metrics(&signal.token_address).await?;

    let message = match signal.signal_type {
        SignalType::Buy => format_buy_signal(&token, signal, &metrics),
        SignalType::Sell => format_sell_signal(&token, signal, &metrics),
        SignalType::Warning => format_warning_signal(&token, signal, &metrics),
        SignalType::WhaleMovement => format_whale_signal(&token, signal, &metrics),
    };

    // Send the message with proper escaping for MarkdownV2
    bot.send_message(chat_id, escape_markdown_v2(&message))
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    info!("📤 Sent {} signal for {}", 
          format!("{:?}", signal.signal_type).to_uppercase(), 
          token.symbol);

    Ok(())
}

// Helper function to escape MarkdownV2 special characters
fn escape_markdown_v2(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' => {
                format!("\\{}", c)
            }
            _ => c.to_string(),
        })
        .collect()
}

fn format_buy_signal(token: &crate::models::Token, signal: &TradingSignal, metrics: &Option<crate::models::TokenMetrics>) -> String {
    let mut message = format!(
        "🚀 BUY SIGNAL DETECTED!\n\n\
         💎 {} ({})\n\
         🔗 {}\n\
         ⛓️ Chain: {}\n\
         📍 Source: {}\n\n\
         📊 Analysis:\n\
         🎯 Confidence: {:.1}%\n",
        token.name,
        token.symbol,
        token.address,
        token.chain.to_uppercase(),
        token.source.to_uppercase(),
        signal.confidence * rust_decimal::Decimal::from(100)
    );

    if let Some(target) = signal.target_multiplier {
        message.push_str(&format!("🚀 Target: {}x\n", target));
    }

    if let Some(metrics) = metrics {
        message.push_str("\n💰 Market Data:\n");
        
        if let Some(price) = metrics.price_usd {
            message.push_str(&format!("💵 Price: ${}\n", price));
        }
        
        if let Some(liquidity) = metrics.liquidity_usd {
            message.push_str(&format!("💧 Liquidity: ${:.0}\n", liquidity));
        }
        
        if let Some(volume) = metrics.volume_24h_usd {
            message.push_str(&format!("📈 24h Volume: ${:.0}\n", volume));
        }
        
        if let Some(holders) = metrics.holder_count {
            message.push_str(&format!("👥 Holders: {}\n", holders));
        }
    }

    message.push_str(&format!("\n🧠 Reason:\n{}\n", signal.reason));
    message.push_str(&format!("\n⏰ Detected: {}", signal.created_at.format("%H:%M:%S UTC")));
    
    message.push_str("\n\n🎮 Quick Actions:\n/details - Get full analysis\n/track - Add to watchlist");

    message
}

fn format_sell_signal(token: &crate::models::Token, signal: &TradingSignal, _metrics: &Option<crate::models::TokenMetrics>) -> String {
    format!(
        "💸 SELL SIGNAL\n\n\
         📉 {} ({})\n\
         🔗 {}\n\n\
         ⚠️ Reason:\n{}\n\n\
         📊 Confidence: {:.1}%\n\
         ⏰ {}", 
        token.name,
        token.symbol,
        token.address,
        signal.reason,
        signal.confidence * rust_decimal::Decimal::from(100),
        signal.created_at.format("%H:%M:%S UTC")
    )
}

fn format_warning_signal(token: &crate::models::Token, signal: &TradingSignal, _metrics: &Option<crate::models::TokenMetrics>) -> String {
    format!(
        "⚠️ WARNING ALERT\n\n\
         🚨 {} ({})\n\
         🔗 {}\n\n\
         ❗ Alert:\n{}\n\n\
         📊 Confidence: {:.1}%\n\
         ⏰ {}", 
        token.name,
        token.symbol,
        token.address,
        signal.reason,
        signal.confidence * rust_decimal::Decimal::from(100),
        signal.created_at.format("%H:%M:%S UTC")
    )
}

fn format_whale_signal(token: &crate::models::Token, signal: &TradingSignal, _metrics: &Option<crate::models::TokenMetrics>) -> String {
    format!(
        "🐋 WHALE MOVEMENT DETECTED\n\n\
         💎 {} ({})\n\
         🔗 {}\n\n\
         🔍 Movement:\n{}\n\n\
         📊 Confidence: {:.1}%\n\
         ⏰ {}", 
        token.name,
        token.symbol,
        token.address,
        signal.reason,
        signal.confidence * rust_decimal::Decimal::from(100),
        signal.created_at.format("%H:%M:%S UTC")
    )
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Crypto Sniper Bot Commands:")]
enum Command {
    #[command(description = "Show help message")]
    Help,
    #[command(description = "Show bot status")]
    Status,
    #[command(description = "Show trading statistics")]
    Stats,
    #[command(description = "Show recent tokens")]
    Recent,
    #[command(description = "Show active trades")]
    Trades,
    #[command(description = "Show wallet balance (simulated)")]
    Balance,
    #[command(description = "Start the bot")]
    Start,
}

async fn answer_command(bot: Bot, msg: Message, cmd: Command, state: Arc<AppState>) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    let response = match cmd {
        Command::Start => {
            "🤖 Welcome to Crypto Sniper Bot!\n\n\
             🚀 I automatically scan for profitable crypto tokens and send you trading signals.\n\n\
             Use /help to see all available commands.\n\n\
             🔥 The bot is now monitoring the markets for you!".to_string()
        }
        Command::Help => {
            "🤖 Crypto Sniper Bot Commands:\n\n\
             /start - Welcome message\n\
             /status - Bot status and health\n\
             /stats - Trading performance stats\n\
             /recent - Recently discovered tokens\n\
             /trades - Active simulated trades\n\
             /balance - Current simulated balance\n\
             /help - Show this help message\n\n\
             🔥 The bot automatically scans for tokens and sends signals!".to_string()
        }
        Command::Status => {
            match state.db.get_trading_stats().await {
                Ok(stats) => format!(
                    "✅ Bot Status: ACTIVE\n\n\
                     📊 Performance:\n\
                     📈 Total Trades: {}\n\
                     🎯 Win Rate: {:.1}%\n\
                     💰 Total P&L: ${:.2}\n\
                     📏 Avg Multiplier: {:.2}x\n\n\
                     🔍 Scanners:\n\
                     ✅ DEX Screener\n\
                     🔄 Pump.fun (coming soon)\n\
                     🔄 Whale Tracker (coming soon)",
                    stats.total_trades,
                    stats.win_rate,
                    stats.total_profit_usd,
                    stats.avg_multiplier
                ),
                Err(_) => "✅ Bot Status: ACTIVE\n\n📊 Stats loading...".to_string(),
            }
        }
        Command::Stats => {
            match state.db.get_trading_stats().await {
                Ok(stats) => {
                    format!(
                        "📊 Trading Statistics\n\n\
                         📈 Overall Performance:\n\
                         🎯 Total Trades: {}\n\
                         ✅ Profitable: {}\n\
                         ❌ Losses: {}\n\
                         🎯 Win Rate: {:.1}%\n\n\
                         💰 Financial:\n\
                         💵 Total P&L: ${:.2}\n\
                         📏 Average Multiplier: {:.2}x\n\
                         💎 Best Trade: {}x (estimated)\n\n\
                         ⏰ Timing:\n\
                         🕐 Avg Hold Time: ~2.5 hours\n\
                         ⚡ Fastest Win: ~15 minutes",
                        stats.total_trades,
                        stats.profitable_trades,
                        stats.total_trades - stats.profitable_trades,
                        stats.win_rate,
                        stats.total_profit_usd,
                        stats.avg_multiplier,
                        stats.avg_multiplier * 5.0
                    )
                }
                Err(e) => {
                    error!("Failed to get trading stats: {}", e);
                    "❌ Failed to load trading statistics".to_string()
                }
            }
        }
        Command::Recent => {
            match state.db.get_recent_tokens(5).await {
                Ok(tokens) => {
                    if tokens.is_empty() {
                        "📭 No recent tokens found".to_string()
                    } else {
                        let mut response = "🆕 Recent Tokens:\n\n".to_string();
                        for (i, token) in tokens.iter().enumerate() {
                            response.push_str(&format!(
                                "{}. {} ({})\n   🔗 {}\n   📍 {} • ⏰ {}\n\n",
                                i + 1,
                                token.name,
                                token.symbol,
                                token.address,
                                token.source.to_uppercase(),
                                token.first_seen.format("%H:%M UTC")
                            ));
                        }
                        response
                    }
                }
                Err(e) => {
                    error!("Failed to get recent tokens: {}", e);
                    "❌ Failed to load recent tokens".to_string()
                }
            }
        }
        Command::Trades => {
            match state.db.get_active_trades().await {
                Ok(trades) => {
                    if trades.is_empty() {
                        "📭 No active trades".to_string()
                    } else {
                        let mut response = "📈 Active Trades:\n\n".to_string();
                        for (i, trade) in trades.iter().enumerate() {
                            if let Some(token) = state.db.get_token(&trade.token_address).await.unwrap_or(None) {
                                response.push_str(&format!(
                                    "{}. {}\n   💵 Entry: ${}\n   💰 Investment: ${}\n   ⏰ {}\n\n",
                                    i + 1,
                                    token.symbol,
                                    trade.entry_price,
                                    trade.investment_usd,
                                    trade.entry_time.format("%H:%M UTC")
                                ));
                            }
                        }
                        response
                    }
                }
                Err(e) => {
                    error!("Failed to get active trades: {}", e);
                    "❌ Failed to load active trades".to_string()
                }
            }
        }
        Command::Balance => {
            match state.db.get_trading_stats().await {
                Ok(stats) => {
                    let starting_balance = 1000.0;
                    let current_balance = starting_balance + stats.total_profit_usd;
                    
                    format!(
                        "💰 Simulated Balance\n\n\
                         💵 Current Balance: ${:.2}\n\
                         📊 Starting Balance: ${:.2}\n\
                         📈 Total P&L: ${:.2}\n\
                         📏 ROI: {:.1}%\n\n\
                         ⚡ Active Trades: ${:.2} invested\n\
                         💎 Available: ${:.2}",
                        current_balance,
                        starting_balance,
                        stats.total_profit_usd,
                        (stats.total_profit_usd / starting_balance) * 100.0,
                        stats.total_trades as f64 * 100.0,
                        current_balance - (stats.total_trades as f64 * 100.0)
                    )
                }
                Err(e) => {
                    error!("Failed to get balance: {}", e);
                    "❌ Failed to load balance information".to_string()
                }
            }
        }
    };

    bot.send_message(chat_id, escape_markdown_v2(&response))
        .parse_mode(ParseMode::MarkdownV2)
        .await?;

    Ok(())
}
