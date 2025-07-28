// src/utils.rs
use anyhow::Result;
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Format large numbers in a human-readable way
pub fn format_number(num: f64) -> String {
    if num >= 1_000_000_000.0 {
        format!("{:.2}B", num / 1_000_000_000.0)
    } else if num >= 1_000_000.0 {
        format!("{:.2}M", num / 1_000_000.0)
    } else if num >= 1_000.0 {
        format!("{:.2}K", num / 1_000.0)
    } else {
        format!("{:.2}", num)
    }
}

/// Format a price with appropriate decimal places
pub fn format_price(price: Decimal) -> String {
    let price_f64 = price.to_string().parse::<f64>().unwrap_or(0.0);
    
    if price_f64 >= 1.0 {
        format!("${:.4}", price_f64)
    } else if price_f64 >= 0.01 {
        format!("${:.6}", price_f64)
    } else {
        format!("${:.8}", price_f64)
    }
}

/// Calculate percentage change
pub fn calculate_percentage_change(old_price: Decimal, new_price: Decimal) -> Decimal {
    if old_price == Decimal::ZERO {
        return Decimal::ZERO;
    }
    ((new_price - old_price) / old_price) * Decimal::from(100)
}

/// Validate Solana address format
pub fn is_valid_solana_address(address: &str) -> bool {
    // Basic validation - Solana addresses are base58 encoded and 32-44 characters
    address.len() >= 32 && address.len() <= 44 && address.chars().all(|c| {
        c.is_ascii_alphanumeric() && !"0OIl".contains(c)
    })
}

/// Validate Ethereum address format
pub fn is_valid_ethereum_address(address: &str) -> bool {
    // Ethereum addresses are 42 characters starting with 0x
    address.len() == 42 && address.starts_with("0x") && 
    address[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Rate limiter for API calls
pub struct RateLimiter {
    requests: HashMap<String, Vec<std::time::Instant>>,
    max_requests: usize,
    window_duration: std::time::Duration,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_duration: std::time::Duration) -> Self {
        Self {
            requests: HashMap::new(),
            max_requests,
            window_duration,
        }
    }

    pub async fn check_rate_limit(&mut self, key: &str) -> bool {
        let now = std::time::Instant::now();
        let requests = self.requests.entry(key.to_string()).or_insert_with(Vec::new);
        
        // Remove old requests outside the window
        requests.retain(|&time| now.duration_since(time) < self.window_duration);
        
        // Check if we can make another request
        if requests.len() < self.max_requests {
            requests.push(now);
            true
        } else {
            false
        }
    }
}
