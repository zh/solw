use anyhow::Result;
use chrono::{DateTime, Utc};
use owo_colors::OwoColorize;

use crate::rpc::RpcClient;
use crate::storage;
use crate::wallet;

pub async fn run(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    limit: u32,
    json_out: bool,
) -> Result<()> {
    let name = storage::resolve_wallet_name(wallet_name)?;
    let mnemonic = storage::get_mnemonic(&name)?
        .ok_or_else(|| anyhow::anyhow!("mnemonic not found for '{}'", name))?;
    let kp = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let address = kp.address();
    let network = storage::resolve_network(Some(&name), cli_network);
    let client = RpcClient::for_network(&network)?;

    let sigs = client.get_signatures_for_address(&address, limit).await?;

    if json_out {
        let body = serde_json::json!({
            "wallet": name,
            "address": address,
            "network": network,
            "count": sigs.len(),
            "signatures": sigs.iter().map(|s| serde_json::json!({
                "signature": s.signature,
                "slot": s.slot,
                "block_time": s.block_time,
                "err": s.err,
                "memo": s.memo,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", "History".green().bold());
    println!("  wallet:  {}", name);
    println!("  address: {}", address);
    println!("  network: {}", network);
    println!();
    if sigs.is_empty() {
        println!("  (no transactions found)");
        return Ok(());
    }
    for s in sigs {
        let when = s
            .block_time
            .and_then(|t| DateTime::<Utc>::from_timestamp(t, 0))
            .map(|d| d.format("%Y-%m-%d %H:%M:%SZ").to_string())
            .unwrap_or_else(|| "-".to_string());
        let status = if s.err.is_some() { "FAIL" } else { "ok  " };
        println!("  [{}] {}  slot={}  {}", status, when, s.slot, s.signature);
        if let Some(memo) = &s.memo {
            println!("         memo: {}", memo);
        }
    }
    Ok(())
}
