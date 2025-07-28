use anyhow::Result;
use rust_decimal::Decimal;
use log::{info, warn};
use std::sync::Arc;
use chrono::{Utc, Duration};

use crate::AppState;

pub struct RiskManagement {
    stop_loss_pct: Decimal,
    max_hold_time: Duration,
}

impl RiskManagement {
    pub fn new(stop_loss_pct: f64, max_hold_hours: i64) -> Self {
        Self {
            stop_loss_pct: Decimal::try_from(stop_loss_pct).unwrap_or(Decimal::try_from(0.5).unwrap()),
            max_hold_time: Duration::hours(max_hold_hours),
        }
    }

    /// Check if any trades should be closed due to losses or time limits
    pub async fn check_risk_limits(&self, state: &Arc<AppState>) -> Result<()> {
        let active_trades = state.db.get_active_trades().await?;
        let now = Utc::now();

        for trade in active_trades {
            let mut should_close = false;
            let mut close_reason = String::new();

            // Check stop loss
            if let Some(current_metrics) = state.db.get_latest_metrics(&trade.token_address).await? {
                if let Some(current_price) = current_metrics.price_usd {
                    let loss_pct = (trade.entry_price - current_price) / trade.entry_price;
                    
                    if loss_pct >= self.stop_loss_pct {
                        should_close = true;
                        close_reason = format!("Stop loss triggered ({:.1}% loss)", loss_pct * Decimal::from(100));
                    }
                }
            }

            // Check time limit
            let hold_duration = now.signed_duration_since(trade.entry_time);
            if hold_duration > self.max_hold_time {
                should_close = true;
                close_reason = format!("Max hold time exceeded ({} hours)", hold_duration.num_hours());
            }

            // Close trade if needed
            if should_close {
                if let Some(trade_id) = trade.id {
                    if let Some(current_metrics) = state.db.get_latest_metrics(&trade.token_address).await? {
                        if let Some(current_price) = current_metrics.price_usd {
                            let profit_loss = (current_price - trade.entry_price) * trade.investment_usd / trade.entry_price;
                            let multiplier = current_price / trade.entry_price;

                            state.db.close_trade(
                                trade_id,
                                current_price,
                                profit_loss,
                                multiplier,
                                &close_reason
                            ).await?;

                            warn!("🛑 Closed trade for {}: {}", trade.token_address, close_reason);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
