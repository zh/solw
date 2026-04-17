//! Thin client for Jupiter's v1 (lite-api) swap aggregator.
//!
//! Upstream: https://dev.jup.ag/docs/swap/
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BASE_URL: &str = "https://lite-api.jup.ag/swap/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteResponse(pub Value);

impl QuoteResponse {
    pub fn in_amount(&self) -> Option<&str> {
        self.0.get("inAmount").and_then(|x| x.as_str())
    }
    pub fn out_amount(&self) -> Option<&str> {
        self.0.get("outAmount").and_then(|x| x.as_str())
    }
    pub fn price_impact_pct(&self) -> Option<&str> {
        self.0.get("priceImpactPct").and_then(|x| x.as_str())
    }
    pub fn route_labels(&self) -> Vec<String> {
        self.0
            .get("routePlan")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.pointer("/swapInfo/label").and_then(|x| x.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub struct JupiterClient {
    http: reqwest::Client,
    base: String,
}

impl JupiterClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            base: BASE_URL.to_string(),
        }
    }

    pub async fn quote(
        &self,
        input_mint: &str,
        output_mint: &str,
        amount_raw: u64,
        slippage_bps: u16,
    ) -> Result<QuoteResponse> {
        let url = format!(
            "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
            self.base, input_mint, output_mint, amount_raw, slippage_bps
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("Jupiter /quote request failed")?;
        let status = resp.status();
        let body: Value = resp.json().await.context("parsing Jupiter /quote JSON")?;
        if !status.is_success() {
            return Err(anyhow!(
                "Jupiter /quote HTTP {}: {}",
                status,
                body.get("error").and_then(|x| x.as_str()).unwrap_or("")
            ));
        }
        Ok(QuoteResponse(body))
    }

    /// Returns the unsigned transaction as raw bytes.
    pub async fn swap_transaction(
        &self,
        quote: &QuoteResponse,
        user_pubkey: &str,
    ) -> Result<Vec<u8>> {
        let body = serde_json::json!({
            "quoteResponse": quote.0,
            "userPublicKey": user_pubkey,
            "asLegacyTransaction": true,
            "wrapAndUnwrapSol": true,
            "prioritizationFeeLamports": "auto",
        });
        let url = format!("{}/swap", self.base);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Jupiter /swap request failed")?;
        let status = resp.status();
        let val: Value = resp.json().await.context("parsing Jupiter /swap JSON")?;
        if !status.is_success() {
            return Err(anyhow!(
                "Jupiter /swap HTTP {}: {}",
                status,
                val.get("error")
                    .and_then(|x| x.as_str())
                    .unwrap_or(&val.to_string())
            ));
        }
        let tx_b64 = val
            .get("swapTransaction")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("Jupiter /swap response missing swapTransaction"))?;
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        B64.decode(tx_b64).context("decoding swapTransaction base64")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quote_response_helpers() {
        let q = QuoteResponse(json!({
            "inAmount": "1000000",
            "outAmount": "88084",
            "priceImpactPct": "0",
            "routePlan": [{
                "swapInfo": { "label": "Meteora DLMM" },
                "percent": 100
            }]
        }));
        assert_eq!(q.in_amount(), Some("1000000"));
        assert_eq!(q.out_amount(), Some("88084"));
        assert_eq!(q.price_impact_pct(), Some("0"));
        assert_eq!(q.route_labels(), vec!["Meteora DLMM".to_string()]);
    }

    #[test]
    fn quote_response_empty_route_plan() {
        let q = QuoteResponse(json!({ "inAmount": "1", "outAmount": "2" }));
        assert_eq!(q.route_labels(), Vec::<String>::new());
    }
}
