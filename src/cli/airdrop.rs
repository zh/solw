//! `solw airdrop` — request a SOL airdrop on devnet/testnet.
use anyhow::{bail, Result};
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_tx_url, load_wallet};
use crate::util::amount::{lamports_to_sol, sol_to_lamports};

/// Hard ceiling per airdrop call. The public devnet faucet rejects anything larger.
const MAX_AIRDROP_SOL: f64 = 2.0;
/// Default amount when the user does not pass one.
const DEFAULT_AIRDROP_SOL: f64 = 1.0;

pub async fn run(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    amount_sol: Option<f64>,
    json_out: bool,
) -> Result<()> {
    let amount_sol = amount_sol.unwrap_or(DEFAULT_AIRDROP_SOL);
    let lamports = validate_amount(amount_sol)?;

    let ctx = load_wallet(wallet_name, cli_network)?;
    if ctx.network == "mainnet" {
        bail!("airdrop is unavailable on mainnet (no faucet); use devnet or testnet");
    }

    let signature = match ctx.client.request_airdrop(&ctx.address, lamports).await {
        Ok(s) => s,
        Err(e) => {
            bail!(
                "airdrop request failed (the public faucet is frequently exhausted or rate-limited). Try:\n  \
                 https://faucet.solana.com  (GitHub login raises your daily cap)\n  \
                 https://faucet.quicknode.com/solana/devnet  (1 claim / 12h, no auth)\n\n\
                 Underlying error: {}",
                e
            );
        }
    };

    let confirm_result = ctx.client.confirm_signature(&signature, 20).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "address": ctx.address,
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

    println!("{}", "Airdrop requested".green().bold());
    println!("  wallet:    {} ({})", ctx.name, ctx.address);
    println!("  network:   {}", ctx.network);
    println!(
        "  amount:    {:.9} SOL ({} lamports)",
        lamports_to_sol(lamports),
        lamports
    );
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

fn validate_amount(amount_sol: f64) -> Result<u64> {
    if amount_sol.is_finite() && amount_sol > MAX_AIRDROP_SOL {
        bail!(
            "amount {} SOL exceeds the per-call faucet cap of {} SOL",
            amount_sol,
            MAX_AIRDROP_SOL
        );
    }
    sol_to_lamports(amount_sol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_amount_accepts_defaults_and_bounds() {
        assert_eq!(validate_amount(1.0).unwrap(), 1_000_000_000);
        assert_eq!(validate_amount(0.5).unwrap(), 500_000_000);
        assert_eq!(validate_amount(2.0).unwrap(), 2_000_000_000);
        assert_eq!(validate_amount(0.000000001).unwrap(), 1);
    }

    #[test]
    fn validate_amount_rejects_nonpositive() {
        assert!(validate_amount(0.0).is_err());
        assert!(validate_amount(-0.1).is_err());
        assert!(validate_amount(f64::NAN).is_err());
        assert!(validate_amount(f64::INFINITY).is_err());
    }

    #[test]
    fn validate_amount_rejects_over_cap() {
        assert!(validate_amount(2.0001).is_err());
        assert!(validate_amount(10.0).is_err());
    }
}
