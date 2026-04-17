//! x402 HTTP 402 Payment Required protocol client.
//!
//! Targets the Solana-flavored "exact" scheme as served by Woody's reference
//! server at `pay-in-usdc/server.ts`. Shapes and header name match that
//! dialect; the canonical x402-svm spec (VersionedTransaction v0 + facilitator
//! settlement) is scoped for a future stage.
//!
//! Upstream reference:
//!   https://github.com/coinbase/x402 (canonical)
//!   /Users/stoyan/Work/blockchain/solana/x402-solana-examples/pay-in-usdc/
//!   (Woody's devnet implementation)
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};

/// x402 payment quote parsed from a server's 402 response body.
///
/// Matches Woody's shape `{ payment: { recipientWallet, tokenAccount, mint,
/// amount, amountUSDC, cluster, message } }`. `amount_ui` is re-derived from
/// `amount_raw` once we know the mint decimals; the server-supplied
/// `amountUSDC` field is retained as `amount_ui_hint` for display.
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub recipient_wallet: String,
    pub token_account: String,
    pub mint: String,
    pub amount_raw: u64,
    pub amount_ui_hint: Option<f64>,
    pub cluster: String,
    pub message: Option<String>,
}

impl Quote {
    /// Map the quote's Solana cluster label to the x402 `network` field used
    /// in the X-Payment header (`solana-devnet` / `solana-mainnet`).
    pub fn x402_network(&self) -> Result<&'static str> {
        match self.cluster.as_str() {
            "devnet" => Ok("solana-devnet"),
            "mainnet" | "mainnet-beta" => Ok("solana-mainnet"),
            other => Err(anyhow!("unsupported quote cluster '{}'", other)),
        }
    }
}

/// Parse Woody's 402 body shape into a `Quote`.
///
/// Rejects missing / wrong-typed fields loudly — a malformed 402 means the
/// target endpoint doesn't speak a dialect we understand and we must not
/// silently guess.
pub fn parse_quote_response(body: &Value) -> Result<Quote> {
    let payment = body
        .get("payment")
        .ok_or_else(|| anyhow!("402 body missing 'payment' object"))?;
    let recipient_wallet = payment
        .get("recipientWallet")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("402 payment missing 'recipientWallet'"))?
        .to_string();
    let token_account = payment
        .get("tokenAccount")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("402 payment missing 'tokenAccount'"))?
        .to_string();
    let mint = payment
        .get("mint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("402 payment missing 'mint'"))?
        .to_string();
    let amount_raw = payment
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("402 payment 'amount' missing or not a u64"))?;
    let amount_ui_hint = payment.get("amountUSDC").and_then(|v| v.as_f64());
    let cluster = payment
        .get("cluster")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("402 payment missing 'cluster'"))?
        .to_string();
    let message = payment
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(Quote {
        recipient_wallet,
        token_account,
        mint,
        amount_raw,
        amount_ui_hint,
        cluster,
        message,
    })
}

/// Build the `X-Payment` header value.
///
/// Envelope: `{ x402Version: 1, scheme: "exact", network, payload: {
/// serializedTransaction } }` serialized as JSON then base64-encoded. Header
/// name is `X-Payment` (mixed-case) — matches Woody's server.
pub fn build_x_payment_header(serialized_tx_b64: &str, network: &str) -> String {
    let envelope = json!({
        "x402Version": 1,
        "scheme": "exact",
        "network": network,
        "payload": { "serializedTransaction": serialized_tx_b64 },
    });
    B64.encode(envelope.to_string())
}

/// Decoded `paymentDetails` block from Woody's success response.
#[derive(Debug, Clone, PartialEq)]
pub struct PaymentDetails {
    pub signature: String,
    pub amount_raw: u64,
    pub amount_ui_hint: Option<f64>,
    pub recipient: String,
    pub explorer_url: Option<String>,
}

/// Parse Woody's success body shape:
/// `{ data, paymentDetails: { signature, amount, amountUSDC, recipient,
/// explorerUrl } }`. Returns `(data_string, payment_details)` where `data` is
/// the premium content (usually a string but may be any JSON value).
pub fn parse_success_response(body: &Value) -> Result<(Value, PaymentDetails)> {
    let data = body
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("success body missing 'data'"))?;
    let details = body
        .get("paymentDetails")
        .ok_or_else(|| anyhow!("success body missing 'paymentDetails'"))?;
    let signature = details
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("paymentDetails missing 'signature'"))?
        .to_string();
    let amount_raw = details
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("paymentDetails 'amount' missing or not a u64"))?;
    let amount_ui_hint = details.get("amountUSDC").and_then(|v| v.as_f64());
    let recipient = details
        .get("recipient")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("paymentDetails missing 'recipient'"))?
        .to_string();
    let explorer_url = details
        .get("explorerUrl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok((
        data,
        PaymentDetails {
            signature,
            amount_raw,
            amount_ui_hint,
            recipient,
            explorer_url,
        },
    ))
}

/// Thin HTTP client over `reqwest` with the two x402 operations we need:
///   - GET a resource URL and return the (status, body-json) pair so the
///     caller can decide whether it's a 402 quote or a direct 200 response.
///   - GET the same URL with an `X-Payment` header carrying the payment proof.
pub struct HttpClient {
    http: reqwest::Client,
}

impl HttpClient {
    pub fn new() -> Self {
        Self { http: reqwest::Client::new() }
    }

    pub async fn get_quote(&self, url: &str) -> Result<(u16, Value)> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("x402 GET {} failed", url))?;
        let status = resp.status().as_u16();
        let body = response_to_json(resp).await?;
        Ok((status, body))
    }

    pub async fn get_with_payment_header(
        &self,
        url: &str,
        header_value: &str,
    ) -> Result<(u16, Value)> {
        let resp = self
            .http
            .get(url)
            .header("X-Payment", header_value)
            .send()
            .await
            .with_context(|| format!("x402 GET {} (with X-Payment) failed", url))?;
        let status = resp.status().as_u16();
        let body = response_to_json(resp).await?;
        Ok((status, body))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a response body as JSON, tolerating non-JSON replies by wrapping
/// them in `{"raw": "..."}` so the caller can still display them. x402
/// endpoints should always return JSON, but a broken endpoint shouldn't
/// crash solw before we can emit a useful diagnostic.
async fn response_to_json(resp: reqwest::Response) -> Result<Value> {
    let bytes = resp
        .bytes()
        .await
        .context("reading x402 response body failed")?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => Ok(v),
        Err(_) => {
            let s = String::from_utf8_lossy(&bytes).into_owned();
            Ok(json!({ "raw": s }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Woody's literal 402 body for /premium on devnet — copied from
    /// `pay-in-usdc/server.ts`.
    fn woody_quote_body() -> Value {
        json!({
            "payment": {
                "recipientWallet": "seFkxFkXEY9JGEpCyPfCWTuPZG9WK6ucf95zvKCfsRX",
                "tokenAccount": "Apedt5CQepXxLptAHFSVBHwNrhFZA6yhQG1WyjBJTxyPb",
                "mint": "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU",
                "amount": 100,
                "amountUSDC": 0.0001,
                "cluster": "devnet",
                "message": "Send USDC to the token account"
            }
        })
    }

    #[test]
    fn parse_quote_response_accepts_woody_shape() {
        let q = parse_quote_response(&woody_quote_body()).unwrap();
        assert_eq!(q.recipient_wallet, "seFkxFkXEY9JGEpCyPfCWTuPZG9WK6ucf95zvKCfsRX");
        assert_eq!(q.token_account, "Apedt5CQepXxLptAHFSVBHwNrhFZA6yhQG1WyjBJTxyPb");
        assert_eq!(q.mint, "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU");
        assert_eq!(q.amount_raw, 100);
        assert_eq!(q.amount_ui_hint, Some(0.0001));
        assert_eq!(q.cluster, "devnet");
        assert_eq!(q.message.as_deref(), Some("Send USDC to the token account"));
    }

    #[test]
    fn parse_quote_response_tolerates_missing_optional_fields() {
        let body = json!({
            "payment": {
                "recipientWallet": "A".repeat(32),
                "tokenAccount": "B".repeat(32),
                "mint": "C".repeat(32),
                "amount": 42,
                "cluster": "mainnet"
            }
        });
        let q = parse_quote_response(&body).unwrap();
        assert_eq!(q.amount_ui_hint, None);
        assert_eq!(q.message, None);
        assert_eq!(q.cluster, "mainnet");
    }

    #[test]
    fn parse_quote_response_rejects_missing_required_fields() {
        // No top-level "payment"
        assert!(parse_quote_response(&json!({})).is_err());
        // Missing amount
        assert!(parse_quote_response(&json!({
            "payment": {
                "recipientWallet": "a", "tokenAccount": "b", "mint": "c",
                "cluster": "devnet"
            }
        }))
        .is_err());
        // Missing cluster
        assert!(parse_quote_response(&json!({
            "payment": {
                "recipientWallet": "a", "tokenAccount": "b", "mint": "c",
                "amount": 1
            }
        }))
        .is_err());
    }

    #[test]
    fn x402_network_mapping() {
        let mut q = parse_quote_response(&woody_quote_body()).unwrap();
        assert_eq!(q.x402_network().unwrap(), "solana-devnet");
        q.cluster = "mainnet".to_string();
        assert_eq!(q.x402_network().unwrap(), "solana-mainnet");
        q.cluster = "mainnet-beta".to_string();
        assert_eq!(q.x402_network().unwrap(), "solana-mainnet");
        q.cluster = "testnet".to_string();
        assert!(q.x402_network().is_err());
    }

    #[test]
    fn build_x_payment_header_roundtrips_to_envelope() {
        let tx_b64 = "AQID";
        let header = build_x_payment_header(tx_b64, "solana-devnet");
        let decoded = B64.decode(&header).unwrap();
        let env: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(env["x402Version"], 1);
        assert_eq!(env["scheme"], "exact");
        assert_eq!(env["network"], "solana-devnet");
        assert_eq!(env["payload"]["serializedTransaction"], tx_b64);
    }

    #[test]
    fn build_x_payment_header_is_stable_base64() {
        // Same inputs produce identical header bytes — callers depend on this
        // for caching / replay detection.
        let a = build_x_payment_header("hello", "solana-devnet");
        let b = build_x_payment_header("hello", "solana-devnet");
        assert_eq!(a, b);
    }

    #[test]
    fn parse_success_response_accepts_woody_shape() {
        let body = json!({
            "data": "Premium content - USDC payment verified!",
            "paymentDetails": {
                "signature": "5sigAbCDeFg",
                "amount": 100,
                "amountUSDC": 0.0001,
                "recipient": "Apedt5CQepXxLptAHFSVBHwNrhFZA6yhQG1WyjBJTxyPb",
                "explorerUrl": "https://explorer.solana.com/tx/5sigAbCDeFg?cluster=devnet"
            }
        });
        let (data, details) = parse_success_response(&body).unwrap();
        assert_eq!(data.as_str(), Some("Premium content - USDC payment verified!"));
        assert_eq!(details.signature, "5sigAbCDeFg");
        assert_eq!(details.amount_raw, 100);
        assert_eq!(details.amount_ui_hint, Some(0.0001));
        assert_eq!(details.recipient, "Apedt5CQepXxLptAHFSVBHwNrhFZA6yhQG1WyjBJTxyPb");
        assert_eq!(
            details.explorer_url.as_deref(),
            Some("https://explorer.solana.com/tx/5sigAbCDeFg?cluster=devnet")
        );
    }

    #[test]
    fn parse_success_response_rejects_missing_data() {
        let body = json!({
            "paymentDetails": {
                "signature": "s", "amount": 1, "recipient": "r"
            }
        });
        assert!(parse_success_response(&body).is_err());
    }
}
