//! Minimal Solana JSON-RPC client.
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Return the default JSON-RPC endpoint for a network name.
pub fn rpc_url(network: &str) -> Result<&'static str> {
    match network {
        "mainnet" => Ok("https://api.mainnet-beta.solana.com"),
        "devnet" => Ok("https://api.devnet.solana.com"),
        "testnet" => Ok("https://api.testnet.solana.com"),
        other => anyhow::bail!("unknown network '{}'", other),
    }
}

/// Env var overriding the default cluster URL (any network).
pub const RPC_URL_ENV: &str = "SOLW_RPC_URL";

/// Network-specific env var name. E.g. `SOLW_RPC_URL_MAINNET`. Wins over the
/// global `SOLW_RPC_URL` when both are set — lets the user point Alchemy's
/// separate mainnet / devnet URLs at the right network at the same time.
pub fn rpc_url_env_for_network(network: &str) -> String {
    format!("{}_{}", RPC_URL_ENV, network.to_uppercase())
}

/// Resolve the URL to use for `network`. Precedence: `per_network` >
/// `global` > built-in cluster URL. Blank / whitespace-only values are
/// treated as unset. Pure so unit tests don't touch process env.
///
/// Rejects non-HTTPS URLs except for `http://localhost` and `http://127.0.0.1`,
/// which are allowed so users can point at a local validator.
pub fn resolve_rpc_url(
    per_network: Option<&str>,
    global: Option<&str>,
    network: &str,
) -> Result<String> {
    for s in [per_network, global].into_iter().flatten() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            validate_rpc_url(trimmed)?;
            return Ok(trimmed.to_string());
        }
    }
    Ok(rpc_url(network)?.to_string())
}

/// Reject any endpoint that isn't `https://`, except loopback HTTP.
/// A misconfigured env var pointing at `http://` would silently ship
/// the user's pubkey and every query in plaintext; better to fail loud.
fn validate_rpc_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .with_context(|| format!("invalid RPC URL '{}'", url))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            if host == "localhost" || host == "127.0.0.1" {
                Ok(())
            } else {
                anyhow::bail!(
                    "RPC URL must use https:// (got '{}'); http:// is only allowed for localhost",
                    url
                )
            }
        }
        other => anyhow::bail!("unsupported RPC URL scheme '{}' in '{}'", other, url),
    }
}

#[derive(Debug, Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: Value,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcErrorBody>,
}

#[derive(Debug, Deserialize)]
struct RpcErrorBody {
    code: i64,
    message: String,
}

pub struct RpcClient {
    url: String,
    http: reqwest::Client,
}

impl RpcClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            http: reqwest::Client::new(),
        }
    }

    pub fn for_network(network: &str) -> Result<Self> {
        let per_net = std::env::var(rpc_url_env_for_network(network)).ok();
        let global = std::env::var(RPC_URL_ENV).ok();
        let url = resolve_rpc_url(per_net.as_deref(), global.as_deref(), network)?;
        Ok(Self::new(url))
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = RpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };
        let resp: RpcResponse = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("RPC request '{}' failed", method))?
            .error_for_status()
            .with_context(|| format!("RPC HTTP status error for '{}'", method))?
            .json()
            .await
            .with_context(|| format!("failed to parse RPC response for '{}'", method))?;
        if let Some(err) = resp.error {
            anyhow::bail!("RPC error {}: {}", err.code, err.message);
        }
        resp.result.ok_or_else(|| anyhow::anyhow!("RPC response missing 'result' field"))
    }

    /// Get the balance in lamports for a base58 address.
    pub async fn get_balance(&self, address: &str) -> Result<u64> {
        let v = self.call("getBalance", json!([address])).await?;
        parse_get_balance(&v)
    }

    /// Get SPL token accounts owned by a wallet.
    /// Returns a list of (mint, amount_ui_string, decimals) parsed from jsonParsed encoding.
    pub async fn get_token_accounts(&self, owner: &str) -> Result<Vec<TokenAccount>> {
        let params = json!([
            owner,
            { "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" },
            { "encoding": "jsonParsed" }
        ]);
        let v = self.call("getTokenAccountsByOwner", params).await?;
        parse_token_accounts(&v)
    }

    /// Return the SPL mint info (decimals, supply, authority) for a mint address.
    pub async fn get_mint_info(&self, mint: &str) -> Result<MintInfo> {
        let params = json!([mint, { "encoding": "jsonParsed" }]);
        let v = self.call("getAccountInfo", params).await?;
        parse_mint_info(&v)
    }

    /// Fetch an account's raw data bytes (base64-decoded). Returns `None` if the
    /// account does not exist. Used for reading binary account formats (e.g.
    /// Metaplex Token Metadata) that `jsonParsed` encoding does not understand.
    pub async fn get_account_data_base64(&self, address: &str) -> Result<Option<Vec<u8>>> {
        let params = json!([address, { "encoding": "base64" }]);
        let v = self.call("getAccountInfo", params).await?;
        parse_account_data_base64(&v)
    }

    /// Return true if an account exists (has nonzero lamports).
    pub async fn account_exists(&self, address: &str) -> Result<bool> {
        let params = json!([address, { "encoding": "base64" }]);
        let v = self.call("getAccountInfo", params).await?;
        let val = v.get("value");
        Ok(val.map(|x| !x.is_null()).unwrap_or(false))
    }

    /// Get the latest blockhash and return it as base58.
    pub async fn get_latest_blockhash(&self) -> Result<(String, u64)> {
        let v = self
            .call("getLatestBlockhash", json!([{ "commitment": "finalized" }]))
            .await?;
        parse_latest_blockhash(&v)
    }

    /// Submit a signed transaction (raw bytes, base64-encoded) and return its signature (base58).
    pub async fn send_transaction(&self, raw_tx: &[u8]) -> Result<String> {
        let encoded = B64.encode(raw_tx);
        let params = json!([
            encoded,
            {
                "encoding": "base64",
                "skipPreflight": false,
                "preflightCommitment": "processed",
                "maxRetries": null
            }
        ]);
        let v = self.call("sendTransaction", params).await?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("sendTransaction returned non-string: {}", v))
    }

    /// Poll until `signature` reaches `confirmed` or finalized commitment, or timeout.
    pub async fn confirm_signature(&self, signature: &str, max_attempts: u32) -> Result<()> {
        for _ in 0..max_attempts {
            let v = self
                .call(
                    "getSignatureStatuses",
                    json!([[signature], { "searchTransactionHistory": true }]),
                )
                .await?;
            if let Some(arr) = v.get("value").and_then(|x| x.as_array()) {
                if let Some(entry) = arr.first() {
                    if !entry.is_null() {
                        if let Some(err) = entry.get("err").cloned().filter(|v| !v.is_null()) {
                            anyhow::bail!("transaction failed: {}", err);
                        }
                        let commitment = entry
                            .get("confirmationStatus")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        if matches!(commitment, "confirmed" | "finalized") {
                            return Ok(());
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        anyhow::bail!("timed out waiting for confirmation of {}", signature)
    }

    /// Request a devnet/testnet faucet airdrop. Returns the signature (base58).
    pub async fn request_airdrop(&self, address: &str, lamports: u64) -> Result<String> {
        let v = self
            .call("requestAirdrop", json!([address, lamports]))
            .await?;
        v.as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("requestAirdrop returned non-string: {}", v))
    }

    /// Get recent signatures for an address (most-recent first).
    pub async fn get_signatures_for_address(
        &self,
        address: &str,
        limit: u32,
    ) -> Result<Vec<SignatureInfo>> {
        let params = json!([address, { "limit": limit }]);
        let v = self.call("getSignaturesForAddress", params).await?;
        parse_signatures(&v)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenAccount {
    pub mint: String,
    pub amount_raw: u64,
    pub decimals: u8,
    pub ui_amount: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MintInfo {
    pub decimals: u8,
    pub supply_raw: u64,
    pub mint_authority: Option<String>,
    pub freeze_authority: Option<String>,
    pub is_initialized: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignatureInfo {
    pub signature: String,
    pub slot: u64,
    pub block_time: Option<i64>,
    pub err: Option<Value>,
    pub memo: Option<String>,
}

// ---- Pure parsers (tested offline) ----

pub(crate) fn parse_account_data_base64(v: &Value) -> Result<Option<Vec<u8>>> {
    let value = v
        .get("value")
        .ok_or_else(|| anyhow::anyhow!("getAccountInfo response missing 'value'"))?;
    if value.is_null() {
        return Ok(None);
    }
    let data = value
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("getAccountInfo value missing 'data'"))?;
    // Expected shape: ["<base64>", "base64"]
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("getAccountInfo 'data' is not an array; wrong encoding?"))?;
    let encoded = arr
        .first()
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("getAccountInfo 'data[0]' missing or not a string"))?;
    let bytes = B64
        .decode(encoded)
        .context("decoding account data base64")?;
    Ok(Some(bytes))
}

pub(crate) fn parse_get_balance(v: &Value) -> Result<u64> {
    v.get("value")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow::anyhow!("malformed getBalance response: {}", v))
}

pub(crate) fn parse_token_accounts(v: &Value) -> Result<Vec<TokenAccount>> {
    let arr = v
        .get("value")
        .and_then(|x| x.as_array())
        .ok_or_else(|| anyhow::anyhow!("malformed getTokenAccountsByOwner: missing value[]"))?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let info = entry
            .pointer("/account/data/parsed/info")
            .ok_or_else(|| anyhow::anyhow!("token account missing parsed/info"))?;
        let mint = info
            .get("mint")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow::anyhow!("token account missing mint"))?
            .to_string();
        let amount_info = info
            .get("tokenAmount")
            .ok_or_else(|| anyhow::anyhow!("token account missing tokenAmount"))?;
        let amount_raw: u64 = amount_info
            .get("amount")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("token account amount unparseable"))?;
        let decimals = amount_info
            .get("decimals")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| anyhow::anyhow!("token account decimals missing"))?
            as u8;
        let ui_amount = amount_info
            .get("uiAmount")
            .and_then(|x| x.as_f64())
            .unwrap_or_else(|| amount_raw as f64 / 10f64.powi(decimals as i32));
        out.push(TokenAccount { mint, amount_raw, decimals, ui_amount });
    }
    Ok(out)
}

pub(crate) fn parse_mint_info(v: &Value) -> Result<MintInfo> {
    let value = v
        .get("value")
        .ok_or_else(|| anyhow::anyhow!("getAccountInfo(mint): missing value"))?;
    if value.is_null() {
        anyhow::bail!("mint account not found");
    }
    let parsed = value
        .pointer("/data/parsed")
        .ok_or_else(|| anyhow::anyhow!("mint data not jsonParsed; is this a mint?"))?;
    let kind = parsed.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if kind != "mint" {
        anyhow::bail!("account is not a token mint (type='{}')", kind);
    }
    let info = parsed
        .get("info")
        .ok_or_else(|| anyhow::anyhow!("mint missing info"))?;
    let decimals = info
        .get("decimals")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow::anyhow!("mint missing decimals"))? as u8;
    let supply_raw: u64 = info
        .get("supply")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("mint supply unparseable"))?;
    let mint_authority = info
        .get("mintAuthority")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let freeze_authority = info
        .get("freezeAuthority")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let is_initialized = info
        .get("isInitialized")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    Ok(MintInfo {
        decimals,
        supply_raw,
        mint_authority,
        freeze_authority,
        is_initialized,
    })
}

pub(crate) fn parse_latest_blockhash(v: &Value) -> Result<(String, u64)> {
    let inner = v
        .get("value")
        .ok_or_else(|| anyhow::anyhow!("getLatestBlockhash missing value"))?;
    let blockhash = inner
        .get("blockhash")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!("getLatestBlockhash missing blockhash"))?
        .to_string();
    let last_valid = inner
        .get("lastValidBlockHeight")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| anyhow::anyhow!("getLatestBlockhash missing lastValidBlockHeight"))?;
    Ok((blockhash, last_valid))
}

pub(crate) fn parse_signatures(v: &Value) -> Result<Vec<SignatureInfo>> {
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("getSignaturesForAddress did not return an array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let signature = e
            .get("signature")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow::anyhow!("signature entry missing signature field"))?
            .to_string();
        let slot = e.get("slot").and_then(|x| x.as_u64()).unwrap_or(0);
        let block_time = e.get("blockTime").and_then(|x| x.as_i64());
        let err = e.get("err").cloned().filter(|v| !v.is_null());
        let memo = e
            .get("memo")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        out.push(SignatureInfo { signature, slot, block_time, err, memo });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_url_known_networks() {
        assert_eq!(rpc_url("mainnet").unwrap(), "https://api.mainnet-beta.solana.com");
        assert_eq!(rpc_url("devnet").unwrap(), "https://api.devnet.solana.com");
        assert_eq!(rpc_url("testnet").unwrap(), "https://api.testnet.solana.com");
    }

    #[test]
    fn rpc_url_unknown_network() {
        assert!(rpc_url("mainnt").is_err());
    }

    #[test]
    fn resolve_rpc_url_falls_back_when_env_absent() {
        let got = resolve_rpc_url(None, None, "mainnet").unwrap();
        assert_eq!(got, "https://api.mainnet-beta.solana.com");
    }

    #[test]
    fn resolve_rpc_url_falls_back_when_env_blank() {
        let got = resolve_rpc_url(Some("   "), Some(""), "devnet").unwrap();
        assert_eq!(got, "https://api.devnet.solana.com");
    }

    #[test]
    fn resolve_rpc_url_uses_global_override() {
        let got = resolve_rpc_url(
            None,
            Some("https://mainnet.g.alchemy.com/v2/xyz"),
            "mainnet",
        )
        .unwrap();
        assert_eq!(got, "https://mainnet.g.alchemy.com/v2/xyz");
    }

    #[test]
    fn resolve_rpc_url_override_wins_over_unknown_network() {
        // If the user sets SOLW_RPC_URL explicitly, we don't care whether
        // the network string is one of the built-ins.
        let got =
            resolve_rpc_url(None, Some("https://custom.example/rpc"), "mainnt").unwrap();
        assert_eq!(got, "https://custom.example/rpc");
    }

    #[test]
    fn resolve_rpc_url_trims_whitespace() {
        let got = resolve_rpc_url(
            None,
            Some("  https://x.example/rpc\n"),
            "mainnet",
        )
        .unwrap();
        assert_eq!(got, "https://x.example/rpc");
    }

    #[test]
    fn resolve_rpc_url_per_network_wins_over_global() {
        // Typical Alchemy setup: SOLW_RPC_URL_MAINNET and _DEVNET point at
        // different endpoints. Per-network must win over the global fallback.
        let got = resolve_rpc_url(
            Some("https://solana-devnet.g.alchemy.com/v2/K"),
            Some("https://solana-mainnet.g.alchemy.com/v2/K"),
            "devnet",
        )
        .unwrap();
        assert_eq!(got, "https://solana-devnet.g.alchemy.com/v2/K");
    }

    #[test]
    fn resolve_rpc_url_blank_per_network_falls_through_to_global() {
        // Per-network present-but-empty must NOT mask the global override.
        let got = resolve_rpc_url(
            Some("   "),
            Some("https://custom.example/rpc"),
            "devnet",
        )
        .unwrap();
        assert_eq!(got, "https://custom.example/rpc");
    }

    #[test]
    fn resolve_rpc_url_rejects_plain_http() {
        let err = resolve_rpc_url(None, Some("http://evil.example/rpc"), "mainnet").unwrap_err();
        assert!(err.to_string().contains("https"), "got: {}", err);
    }

    #[test]
    fn resolve_rpc_url_allows_http_localhost() {
        let got = resolve_rpc_url(None, Some("http://localhost:8899"), "mainnet").unwrap();
        assert_eq!(got, "http://localhost:8899");
        let got = resolve_rpc_url(None, Some("http://127.0.0.1:8899"), "devnet").unwrap();
        assert_eq!(got, "http://127.0.0.1:8899");
    }

    #[test]
    fn resolve_rpc_url_rejects_non_http_scheme() {
        assert!(resolve_rpc_url(None, Some("file:///etc/passwd"), "mainnet").is_err());
        assert!(resolve_rpc_url(None, Some("ws://rpc.example/"), "mainnet").is_err());
    }

    #[test]
    fn resolve_rpc_url_rejects_malformed() {
        assert!(resolve_rpc_url(None, Some("not a url"), "mainnet").is_err());
    }

    #[test]
    fn resolve_rpc_url_per_network_validated_too() {
        let err =
            resolve_rpc_url(Some("http://evil.example/rpc"), None, "mainnet").unwrap_err();
        assert!(err.to_string().contains("https"), "got: {}", err);
    }

    #[test]
    fn rpc_url_env_for_network_names() {
        assert_eq!(rpc_url_env_for_network("mainnet"), "SOLW_RPC_URL_MAINNET");
        assert_eq!(rpc_url_env_for_network("devnet"), "SOLW_RPC_URL_DEVNET");
        assert_eq!(rpc_url_env_for_network("testnet"), "SOLW_RPC_URL_TESTNET");
    }

    #[test]
    fn parse_balance_ok() {
        let v = json!({ "context": { "slot": 1 }, "value": 2_500_000_000u64 });
        assert_eq!(parse_get_balance(&v).unwrap(), 2_500_000_000);
    }

    #[test]
    fn parse_balance_malformed() {
        assert!(parse_get_balance(&json!({ "value": "nope" })).is_err());
        assert!(parse_get_balance(&json!({})).is_err());
    }

    #[test]
    fn parse_token_accounts_ok() {
        // jsonParsed encoding shape from https://solana.com/docs/rpc/http/gettokenaccountsbyowner
        let v = json!({
            "context": { "slot": 1 },
            "value": [{
                "pubkey": "Ata111...",
                "account": {
                    "data": {
                        "parsed": {
                            "info": {
                                "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                                "owner": "Owner...",
                                "tokenAmount": {
                                    "amount": "1500000",
                                    "decimals": 6,
                                    "uiAmount": 1.5,
                                    "uiAmountString": "1.5"
                                }
                            }
                        }
                    }
                }
            }]
        });
        let accts = parse_token_accounts(&v).unwrap();
        assert_eq!(accts.len(), 1);
        assert_eq!(accts[0].mint, "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
        assert_eq!(accts[0].amount_raw, 1_500_000);
        assert_eq!(accts[0].decimals, 6);
        assert!((accts[0].ui_amount - 1.5).abs() < 1e-9);
    }

    #[test]
    fn parse_token_accounts_empty() {
        let v = json!({ "context": { "slot": 1 }, "value": [] });
        assert!(parse_token_accounts(&v).unwrap().is_empty());
    }

    #[test]
    fn parse_token_accounts_malformed() {
        assert!(parse_token_accounts(&json!({})).is_err());
        assert!(parse_token_accounts(&json!({ "value": "nope" })).is_err());
    }

    #[test]
    fn parse_signatures_ok() {
        let v = json!([
            { "signature": "sig1", "slot": 100, "blockTime": 1700000000, "err": null, "memo": null },
            { "signature": "sig2", "slot": 99, "blockTime": 1699999000, "err": { "InstructionError": [0, "Custom"] }, "memo": "hi" }
        ]);
        let sigs = parse_signatures(&v).unwrap();
        assert_eq!(sigs.len(), 2);
        assert_eq!(sigs[0].signature, "sig1");
        assert_eq!(sigs[0].slot, 100);
        assert_eq!(sigs[0].block_time, Some(1700000000));
        assert!(sigs[0].err.is_none());
        assert_eq!(sigs[0].memo, None);
        assert!(sigs[1].err.is_some());
        assert_eq!(sigs[1].memo.as_deref(), Some("hi"));
    }

    #[test]
    fn parse_signatures_not_array() {
        assert!(parse_signatures(&json!({})).is_err());
    }

    #[test]
    fn parse_mint_info_ok() {
        let v = json!({
            "context": { "slot": 1 },
            "value": {
                "data": {
                    "parsed": {
                        "type": "mint",
                        "info": {
                            "decimals": 6,
                            "supply": "1000000000000",
                            "mintAuthority": "Auth1...",
                            "freezeAuthority": null,
                            "isInitialized": true
                        }
                    }
                }
            }
        });
        let m = parse_mint_info(&v).unwrap();
        assert_eq!(m.decimals, 6);
        assert_eq!(m.supply_raw, 1_000_000_000_000);
        assert_eq!(m.mint_authority.as_deref(), Some("Auth1..."));
        assert!(m.freeze_authority.is_none());
        assert!(m.is_initialized);
    }

    #[test]
    fn parse_mint_info_not_a_mint() {
        let v = json!({
            "value": {
                "data": { "parsed": { "type": "account", "info": {} } }
            }
        });
        assert!(parse_mint_info(&v).is_err());
    }

    #[test]
    fn parse_mint_info_missing() {
        let v = json!({ "value": null });
        assert!(parse_mint_info(&v).is_err());
    }

    #[test]
    fn parse_latest_blockhash_ok() {
        let v = json!({
            "context": { "slot": 100 },
            "value": { "blockhash": "Hf6i...", "lastValidBlockHeight": 12345 }
        });
        let (hash, last) = parse_latest_blockhash(&v).unwrap();
        assert_eq!(hash, "Hf6i...");
        assert_eq!(last, 12345);
    }

    #[test]
    fn parse_account_data_base64_ok() {
        let v = json!({
            "context": { "slot": 1 },
            "value": {
                "lamports": 1_461_600u64,
                "owner": "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s",
                "executable": false,
                "rentEpoch": 18446744073709551615u64,
                "data": [B64.encode([0xDE, 0xAD, 0xBE, 0xEF]), "base64"]
            }
        });
        let got = parse_account_data_base64(&v).unwrap();
        assert_eq!(got, Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn parse_account_data_base64_null_account() {
        let v = json!({ "context": { "slot": 1 }, "value": null });
        assert_eq!(parse_account_data_base64(&v).unwrap(), None);
    }

    #[test]
    fn parse_account_data_base64_malformed() {
        assert!(parse_account_data_base64(&json!({})).is_err());
        assert!(parse_account_data_base64(&json!({ "value": {} })).is_err());
        assert!(parse_account_data_base64(&json!({
            "value": { "data": "not-an-array" }
        }))
        .is_err());
        assert!(parse_account_data_base64(&json!({
            "value": { "data": ["not-valid-base64!!!", "base64"] }
        }))
        .is_err());
    }

    #[test]
    fn parse_latest_blockhash_malformed() {
        assert!(parse_latest_blockhash(&json!({})).is_err());
        assert!(parse_latest_blockhash(&json!({ "value": {} })).is_err());
    }
}
