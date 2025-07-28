use anyhow::Result;
use chrono::Utc;
use log::{info, warn};
use rust_decimal::Decimal;
use std::sync::Arc;

use crate::models::{Token, TokenMetrics, AnalysisResult, RiskLevel, Recommendation, TradingSignal, SignalType};
use crate::AppState;

pub struct TokenAnalyzer {
    // Configuration thresholds
    min_liquidity: Decimal,
    max_top_holder_pct: Decimal,
    min_holders: u32,
}

impl TokenAnalyzer {
    pub fn new(state: &AppState) -> Self {
        Self {
            min_liquidity: Decimal::try_from(state.config.trading.min_liquidity_usd).unwrap_or(Decimal::from(10000)),
            max_top_holder_pct: Decimal::try_from(state.config.trading.max_top_holder_percentage).unwrap_or(Decimal::from(30)),
            min_holders: state.config.trading.min_holders,
        }
    }

    /// Main analysis function - this is where the magic happens!
    pub async fn analyze_token(&self, state: &Arc<AppState>, token: &Token) -> Result<AnalysisResult> {
        info!("🔬 Starting deep analysis of {} ({})", token.symbol, token.name);

        // Get the latest metrics for this token
        let metrics = match state.db.get_latest_metrics(&token.address).await? {
            Some(metrics) => metrics,
            None => {
                warn!("No metrics found for token {}", token.symbol);
                return Ok(self.create_insufficient_data_result(&token.address));
            }
        };

        // Start with base score
        let mut score = Decimal::from(50); // Start neutral (0-100 scale)
        let mut flags = Vec::new();
        let mut risk_level = RiskLevel::Medium;

        // 1. LIQUIDITY ANALYSIS (25 points max)
        score += self.analyze_liquidity(&metrics, &mut flags);

        // 2. HOLDER DISTRIBUTION ANALYSIS (20 points max)
        score += self.analyze_holder_distribution(&metrics, &mut flags);

        // 3. VOLUME ANALYSIS (15 points max)
        score += self.analyze_volume(&metrics, &mut flags);

        // 4. PRICE STABILITY ANALYSIS (15 points max)
        score += self.analyze_price_stability(state, &token.address, &mut flags).await;

        // 5. CONTRACT SECURITY ANALYSIS (15 points max)
        score += self.analyze_contract_security(&metrics, &mut flags);

        // 6. MARKET TIMING ANALYSIS (10 points max)
        score += self.analyze_market_timing(&token, &mut flags);

        // Determine risk level based on score and flags
        risk_level = self.calculate_risk_level(score, &flags);

        // Determine if it's safe to trade
        let is_safe = score >= Decimal::from(70) && !self.has_critical_flags(&flags);

        // Calculate potential multiplier based on analysis
        let potential_multiplier = self.calculate_potential_multiplier(score, &metrics, &flags);

        // Make recommendation
        let recommendation = self.make_recommendation(score, &risk_level, is_safe);

        let result = AnalysisResult {
            token_address: token.address.clone(),
            score,
            is_safe,
            risk_level,
            flags,
            potential_multiplier,
            recommendation,
        };

        info!("📊 Analysis complete for {}: Score={}, Safe={}, Risk={:?}", 
              token.symbol, result.score, result.is_safe, result.risk_level);

        // Generate trading signal if this looks promising
        if result.is_safe && result.score >= Decimal::from(75) {
            self.generate_trading_signal(state, token, &result).await?;
        }

        Ok(result)
    }

    fn analyze_liquidity(&self, metrics: &TokenMetrics, flags: &mut Vec<String>) -> Decimal {
        let mut score = Decimal::ZERO;

        if let Some(liquidity) = metrics.liquidity_usd {
            if liquidity >= Decimal::from(100000) {
                score += Decimal::from(25); // Excellent liquidity
            } else if liquidity >= Decimal::from(50000) {
                score += Decimal::from(20); // Good liquidity
            } else if liquidity >= Decimal::from(20000) {
                score += Decimal::from(15); // Decent liquidity
            } else if liquidity >= self.min_liquidity {
                score += Decimal::from(10); // Minimum acceptable
            } else {
                flags.push("🚨 LOW_LIQUIDITY: May be hard to sell".to_string());
                score -= Decimal::from(10); // Penalty for low liquidity
            }

            info!("💧 Liquidity analysis: ${} = +{} points", liquidity, score);
        } else {
            flags.push("❓ UNKNOWN_LIQUIDITY: Could not determine liquidity".to_string());
        }

        score
    }

    fn analyze_holder_distribution(&self, metrics: &TokenMetrics, flags: &mut Vec<String>) -> Decimal {
        let mut score = Decimal::ZERO;

        // Check holder count
        if let Some(holders) = metrics.holder_count {
            if holders >= 10000 {
                score += Decimal::from(10); // Excellent distribution
            } else if holders >= 5000 {
                score += Decimal::from(8); // Good distribution
            } else if holders >= 1000 {
                score += Decimal::from(6); // Decent distribution
            } else if holders >= self.min_holders {
                score += Decimal::from(4); // Minimum acceptable
            } else {
                flags.push(format!("👥 FEW_HOLDERS: Only {} holders (risky)", holders));
                score -= Decimal::from(5);
            }
        }

        // Check top holder concentration
        if let Some(top_holder_pct) = metrics.top_10_holders_percentage {
            if top_holder_pct <= Decimal::from(20) {
                score += Decimal::from(10); // Great distribution
            } else if top_holder_pct <= Decimal::from(40) {
                score += Decimal::from(7); // Good distribution
            } else if top_holder_pct <= Decimal::from(60) {
                score += Decimal::from(4); // Concerning but acceptable
            } else {
                flags.push(format!("🐋 WHALE_DOMINATED: Top 10 holders own {}%", top_holder_pct));
                score -= Decimal::from(10); // Heavy penalty
            }
        }

        info!("👥 Holder distribution analysis: +{} points", score);
        score
    }

    fn analyze_volume(&self, metrics: &TokenMetrics, flags: &mut Vec<String>) -> Decimal {
        let mut score = Decimal::ZERO;

        if let Some(volume_24h) = metrics.volume_24h_usd {
            if let Some(liquidity) = metrics.liquidity_usd {
                // Volume to liquidity ratio is important
                let volume_ratio = volume_24h / liquidity;

                if volume_ratio >= Decimal::from(2) {
                    score += Decimal::from(15); // Excellent trading activity
                } else if volume_ratio >= Decimal::from(1) {
                    score += Decimal::from(12); // Good activity
                } else if volume_ratio >= Decimal::try_from(0.5).unwrap() {
                    score += Decimal::from(8); // Decent activity
                } else if volume_ratio >= Decimal::try_from(0.1).unwrap() {
                    score += Decimal::from(5); // Low activity
                } else {
                    flags.push("📈 LOW_VOLUME: Very little trading activity".to_string());
                    score -= Decimal::from(5);
                }

                info!("📊 Volume analysis: 24h=${}, Ratio={}, Score=+{}", volume_24h, volume_ratio, score);
            }
        }

        score
    }

    async fn analyze_price_stability(&self, state: &Arc<AppState>, token_address: &str, flags: &mut Vec<String>) -> Decimal {
        // For now, we'll implement basic price stability analysis
        // In a full implementation, we'd look at historical price data
        
        // TODO: Implement price history analysis
        // For now, give neutral score
        let score = Decimal::from(7); // Neutral score
        
        info!("💹 Price stability analysis: +{} points", score);
        score
    }

    fn analyze_contract_security(&self, metrics: &TokenMetrics, flags: &mut Vec<String>) -> Decimal {
        let mut score = Decimal::ZERO;

        // Check if contract is verified
        if let Some(verified) = metrics.contract_verified {
            if verified {
                score += Decimal::from(8);
            } else {
                flags.push("🔒 UNVERIFIED_CONTRACT: Cannot audit contract code".to_string());
                score -= Decimal::from(10);
            }
        }

        // Check for honeypot
        if let Some(is_honeypot) = metrics.is_honeypot {
            if is_honeypot {
                flags.push("🍯 HONEYPOT_DETECTED: Cannot sell tokens!".to_string());
                score -= Decimal::from(50); // Massive penalty
            } else {
                score += Decimal::from(5);
            }
        }

        // Check if contract is mintable (can create new tokens)
        if let Some(is_mintable) = metrics.is_mintable {
            if is_mintable {
                flags.push("🏭 MINTABLE_TOKEN: Supply can be increased".to_string());
                score -= Decimal::from(5);
            } else {
                score += Decimal::from(2);
            }
        }

        // Check for proxy contract (can be changed)
        if let Some(has_proxy) = metrics.has_proxy {
            if has_proxy {
                flags.push("🔄 PROXY_CONTRACT: Contract can be upgraded/changed".to_string());
                score -= Decimal::from(3);
            } else {
                score += Decimal::from(2);
            }
        }

        info!("🔐 Contract security analysis: +{} points", score);
        score
    }

    fn analyze_market_timing(&self, token: &Token, flags: &mut Vec<String>) -> Decimal {
        let mut score = Decimal::ZERO;
        let now = Utc::now();
        let age = now.signed_duration_since(token.first_seen);

        // Very new tokens are riskier but have higher potential
        if age.num_hours() < 1 {
            score += Decimal::from(8); // High potential but risky
            flags.push("🆕 VERY_NEW: Less than 1 hour old".to_string());
        } else if age.num_hours() < 24 {
            score += Decimal::from(10); // Sweet spot for early entry
        } else if age.num_days() < 7 {
            score += Decimal::from(6); // Still relatively new
        } else {
            score += Decimal::from(3); // Older token, less moonshot potential
        }

        info!("⏰ Market timing analysis: Age={}h, Score=+{}", age.num_hours(), score);
        score
    }

    fn calculate_risk_level(&self, score: Decimal, flags: &[String]) -> RiskLevel {
        let critical_flags = flags.iter().any(|f| 
            f.contains("HONEYPOT") || f.contains("WHALE_DOMINATED") || f.contains("LOW_LIQUIDITY")
        );

        if critical_flags || score < Decimal::from(30) {
            RiskLevel::Extreme
        } else if score < Decimal::from(50) {
            RiskLevel::High
        } else if score < Decimal::from(70) {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }

    fn has_critical_flags(&self, flags: &[String]) -> bool {
        flags.iter().any(|f| 
            f.contains("HONEYPOT") || 
            f.contains("UNVERIFIED_CONTRACT") ||
            f.contains("LOW_LIQUIDITY")
        )
    }

    fn calculate_potential_multiplier(&self, score: Decimal, metrics: &TokenMetrics, flags: &[String]) -> Option<Decimal> {
        if score < Decimal::from(60) {
            return None; // Too risky
        }

        let mut base_multiplier = Decimal::from(2); // Minimum 2x expectation

        // Higher score = higher potential
        if score >= Decimal::from(90) {
            base_multiplier = Decimal::from(100); // Moon potential
        } else if score >= Decimal::from(85) {
            base_multiplier = Decimal::from(50);
        } else if score >= Decimal::from(80) {
            base_multiplier = Decimal::from(20);
        } else if score >= Decimal::from(75) {
            base_multiplier = Decimal::from(10);
        } else if score >= Decimal::from(70) {
            base_multiplier = Decimal::from(5);
        }

        // Adjust based on liquidity (lower liquidity = higher potential volatility)
        if let Some(liquidity) = metrics.liquidity_usd {
            if liquidity < Decimal::from(50000) {
                base_multiplier *= Decimal::try_from(1.5).unwrap(); // 50% bonus for low liquidity
            }
        }

        // New tokens have higher potential
        if flags.iter().any(|f| f.contains("VERY_NEW")) {
            base_multiplier *= Decimal::from(2); // Double potential for very new tokens
        }

        Some(base_multiplier)
    }

    fn make_recommendation(&self, score: Decimal, risk_level: &RiskLevel, is_safe: bool) -> Recommendation {
        if !is_safe || matches!(risk_level, RiskLevel::Extreme) {
            Recommendation::Avoid
        } else if score >= Decimal::from(75) && matches!(risk_level, RiskLevel::Low | RiskLevel::Medium) {
            Recommendation::Buy
        } else {
            Recommendation::Watch
        }
    }

    async fn generate_trading_signal(&self, state: &Arc<AppState>, token: &Token, result: &AnalysisResult) -> Result<()> {
        let signal = TradingSignal {
            id: None,
            token_address: token.address.clone(),
            signal_type: SignalType::Buy,
            confidence: result.score / Decimal::from(100), // Convert to 0-1 scale
            reason: format!(
                "🚀 {} ({}) - Score: {}/100, Risk: {:?}\n📊 Flags: {}\n🎯 Target: {}x",
                token.symbol,
                token.name,
                result.score,
                result.risk_level,
                if result.flags.is_empty() { "None".to_string() } else { result.flags.join(", ") },
                result.potential_multiplier.unwrap_or(Decimal::from(2))
            ),
            target_multiplier: result.potential_multiplier,
            created_at: Utc::now(),
            is_sent: false,
        };

        state.db.save_trading_signal(&signal).await?;
        info!("💎 Generated BUY signal for {} with {}x potential", token.symbol, 
              result.potential_multiplier.unwrap_or(Decimal::from(2)));

        Ok(())
    }

    fn create_insufficient_data_result(&self, token_address: &str) -> AnalysisResult {
        AnalysisResult {
            token_address: token_address.to_string(),
            score: Decimal::from(20),
            is_safe: false,
            risk_level: RiskLevel::High,
            flags: vec!["❓ INSUFFICIENT_DATA: Cannot analyze properly".to_string()],
            potential_multiplier: None,
            recommendation: Recommendation::Avoid,
        }
    }
}

// Public function to analyze a token (called from scanners)
pub async fn analyze_token(state: Arc<AppState>, token: Token) -> Result<()> {
    let analyzer = TokenAnalyzer::new(&state);
    
    match analyzer.analyze_token(&state, &token).await {
        Ok(result) => {
            info!("✅ Analysis completed for {}: {:?}", token.symbol, result.recommendation);
            
            // If it's a strong buy signal, also start a simulated trade
            if matches!(result.recommendation, Recommendation::Buy) && result.score >= Decimal::from(80) {
                start_simulated_trade(&state, &token, &result).await?;
            }
        }
        Err(e) => {
            warn!("❌ Analysis failed for {}: {}", token.symbol, e);
        }
    }

    Ok(())
}

async fn start_simulated_trade(state: &Arc<AppState>, token: &Token, result: &AnalysisResult) -> Result<()> {
    use crate::models::SimulatedTrade;

    // Get current metrics to determine entry price
    if let Some(metrics) = state.db.get_latest_metrics(&token.address).await? {
        if let Some(price) = metrics.price_usd {
            let trade = SimulatedTrade {
                id: None,
                token_address: token.address.clone(),
                entry_price: price,
                entry_time: Utc::now(),
                exit_price: None,
                exit_time: None,
                investment_usd: Decimal::try_from(state.config.trading.max_investment_usd).unwrap_or(Decimal::from(100)),
                profit_loss: None,
                multiplier: None,
                exit_reason: None,
                is_active: true,
            };

            state.db.save_simulated_trade(&trade).await?;
            info!("📈 Started simulated trade for {} at ${}", token.symbol, price);
        }
    }

    Ok(())
}
