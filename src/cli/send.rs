use anyhow::{bail, Context, Result};
use inquire::Confirm;
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_tx_url, load_wallet};
use crate::tx::{build_transfer_message, decode_base58_blockhash, decode_base58_pubkey, sign_and_serialize};
use crate::util::amount::{lamports_to_sol, sol_to_lamports};

/// Minimum lamports reserved when sending all: roughly rent-exempt minimum for a
/// system account (~890,880 lamports) plus two 5000-lamport signatures of headroom.
/// Conservative; the exact value changes per cluster but 900,880 lamports is a
/// safe floor at the time of writing.
const SEND_ALL_RESERVE_LAMPORTS: u64 = 900_880 + 10_000;

pub async fn run(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    to: &str,
    amount_sol: f64,
    confirmed: bool,
    json_out: bool,
) -> Result<()> {
    let lamports = sol_to_lamports(amount_sol)?;
    send_inner(wallet_name, cli_network, to, lamports, confirmed, json_out, false).await
}

pub async fn run_all(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    to: &str,
    confirmed: bool,
    json_out: bool,
) -> Result<()> {
    send_inner(wallet_name, cli_network, to, 0, confirmed, json_out, true).await
}

async fn send_inner(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    to: &str,
    lamports_hint: u64,
    confirmed: bool,
    json_out: bool,
    send_all: bool,
) -> Result<()> {
    let ctx = load_wallet(wallet_name, cli_network)?;

    let from_bytes = decode_base58_pubkey(&ctx.address)?;
    let to_bytes = decode_base58_pubkey(to).context("invalid recipient address")?;

    let lamports = if send_all {
        let current = ctx.client.get_balance(&ctx.address).await?;
        if current <= SEND_ALL_RESERVE_LAMPORTS {
            bail!(
                "balance {} lamports is below minimum reserve {}",
                current,
                SEND_ALL_RESERVE_LAMPORTS
            );
        }
        current - SEND_ALL_RESERVE_LAMPORTS
    } else {
        lamports_hint
    };

    if lamports == 0 {
        bail!("amount must be greater than zero");
    }

    if !confirmed && !json_out {
        println!("{}", "Confirm transfer".yellow().bold());
        println!("  wallet:  {} ({})", ctx.name, ctx.address);
        println!("  network: {}", ctx.network);
        println!("  to:      {}", to);
        println!(
            "  amount:  {:.9} SOL ({} lamports){}",
            lamports_to_sol(lamports),
            lamports,
            if send_all { " [send-all, reserve held back for rent]" } else { "" }
        );
        let ok = Confirm::new("Send this transaction?")
            .with_default(false)
            .prompt()
            .context("confirmation prompt failed")?;
        if !ok {
            bail!("aborted by user");
        }
    }

    let (blockhash_b58, _last_valid) = ctx.client.get_latest_blockhash().await?;
    let blockhash = decode_base58_blockhash(&blockhash_b58)?;

    let msg = build_transfer_message(&from_bytes, &to_bytes, lamports, &blockhash);
    let raw_tx = sign_and_serialize(&ctx.keypair.signing_key, &msg);
    let signature = ctx.client.send_transaction(&raw_tx).await?;

    // Confirm (~30s max)
    let confirm_result = ctx.client.confirm_signature(&signature, 20).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "from": ctx.address,
            "to": to,
            "network": ctx.network,
            "lamports": lamports,
            "sol": lamports_to_sol(lamports),
            "signature": signature,
            "confirmed": confirm_result.is_ok(),
            "confirm_error": confirm_result.as_ref().err().map(|e| e.to_string()),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        if confirm_result.is_err() {
            std::process::exit(2);
        }
        return Ok(());
    }

    println!("{}", "Transfer submitted".green().bold());
    println!("  signature: {}", signature);
    println!("  explorer:  {}", explorer_tx_url(&ctx.network, &signature));
    match confirm_result {
        Ok(()) => println!("  status:    {}", "confirmed".green()),
        Err(e) => {
            println!("  status:    {} ({})", "unconfirmed".red(), e);
            std::process::exit(2);
        }
    }
    Ok(())
}

