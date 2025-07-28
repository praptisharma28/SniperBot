use anyhow::Result;
use rust_decimal::Decimal;
use log::info;
use std::sync::Arc;

use crate::models::SimulatedTrade;
use crate::AppState;

pub struct ProfitTakingStrategy {
    targets: Vec<Decimal>, // Profit targets (2x, 5x, 10x, etc.)
}

impl ProfitTakingStrategy {
    pub fn new(targets: Vec<f64>) -> Self {
        let targets = targets.into_iter()
            .map(|t| Decimal::try_from(t).unwrap_or(Decimal::from(2)))
            .collect();
        
        Self { targets }
    }

    /// Check if any active trades should be closed based on current prices
    pub async fn check_profit_targets(&self, state: &Arc<AppState>) -> Result<()> {
        let active_trades = state.db.get_active_trades().await?;

        for trade in active_trades {
            if let Some(current_metrics) = state.db.get_latest_metrics(&trade.token_address).await? {
                if let Some(current_price) = current_metrics.price_usd {
                    let multiplier = current_price / trade.entry_price;
                    
                    // Check if we hit any profit target
                    for &target in &self.targets {
                        if multiplier >= target {
                            let profit_usd = (current_price - trade.entry_price) * trade.investment_usd / trade.entry_price;
                            
                            // Close the trade
                            if let Some(trade_id) = trade.id {
                                state.db.close_trade(
                                    trade_id,
                                    current_price,
                                    profit_usd,
                                    multiplier,
                                    &format!("{}x target reached", target)
                                ).await?;

                                info!("🎯 Closed trade for {} at {}x profit (${:.2})", 
                                      trade.token_address, multiplier, profit_usd);
                            }
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
