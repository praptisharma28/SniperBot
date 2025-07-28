use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use log::{info, warn};

pub struct HoneypotChecker {
    client: Client,
}

impl HoneypotChecker {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Check if a token is a honeypot using external API
    pub async fn check_honeypot(&self, chain: &str, address: &str) -> Result<bool> {
        // Using honeypot.is API (free tier)
        let url = format!("https://api.honeypot.is/v2/IsHoneypot?address={}&chainID={}", 
                         address, self.get_chain_id(chain));

        info!("🍯 Checking honeypot status for {} on {}", address, chain);

        match self.client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    let result: HoneypotResponse = response.json().await?;
                    Ok(result.honeypot_result.is_honeypot)
                } else {
                    warn!("Honeypot API returned error: {}", response.status());
                    Ok(false) // Default to safe if API fails
                }
            }
            Err(e) => {
                warn!("Failed to check honeypot: {}", e);
                Ok(false) // Default to safe if network fails
            }
        }
    }

    fn get_chain_id(&self, chain: &str) -> u32 {
        match chain.to_lowercase().as_str() {
            "ethereum" => 1,
            "bsc" => 56,
            "polygon" => 137,
            "solana" => 101, // Custom ID for Solana
            _ => 1, // Default to Ethereum
        }
    }
}

#[derive(Debug, Deserialize)]
struct HoneypotResponse {
    #[serde(rename = "honeypotResult")]
    honeypot_result: HoneypotResult,
}

#[derive(Debug, Deserialize)]
struct HoneypotResult {
    #[serde(rename = "isHoneypot")]
    is_honeypot: bool,
}
