//! `solw swap` — Jupiter v1 (lite-api) swap subcommands.
use anyhow::{bail, Context, Result};
use inquire::Confirm;
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_tx_url, load_wallet};
use crate::jupiter::{JupiterClient, QuoteResponse};
use crate::rpc::RpcClient;
use crate::tx::{decode_base58_pubkey, sign_prebuilt_transaction, verify_swap_transaction};
use crate::util::amount::ui_to_raw;

/// Simple token alias table so users can type `SOL`/`USDC` instead of mint addresses.
pub fn resolve_mint(alias_or_mint: &str) -> String {
    match alias_or_mint.to_ascii_uppercase().as_str() {
        "SOL" | "WSOL" => "So11111111111111111111111111111111111111112".to_string(),
        "USDC" => "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
        "USDT" => "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
        "BONK" => "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        "JUP" => "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN".to_string(),
        _ => alias_or_mint.to_string(),
    }
}

/// Reconcile the two input modes:
///   - `raw == false` (default): `amount` is UI units; convert via `decimals`.
///   - `raw == true`: `amount` must be a non-negative integer-valued f64; we
///     accept it as-is (the user is bypassing decimals and shipping base
///     units directly).
pub fn parse_amount_input(amount: f64, raw: bool, decimals: u8) -> Result<u64> {
    if raw {
        if !amount.is_finite() || amount <= 0.0 {
            bail!("amount must be positive and finite");
        }
        if amount > u64::MAX as f64 {
            bail!("amount exceeds u64");
        }
        if amount.fract() != 0.0 {
            bail!("--raw requires an integer amount (got {})", amount);
        }
        Ok(amount as u64)
    } else {
        ui_to_raw(amount, decimals)
    }
}

pub async fn quote(
    input: &str,
    output: &str,
    amount: f64,
    raw: bool,
    slippage_bps: u16,
    cli_network: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let input_mint = resolve_mint(input);
    let output_mint = resolve_mint(output);

    // Jupiter only routes mainnet liquidity; decimals lookup goes to mainnet
    // RPC regardless of wallet default. `--network` still overrides (honors
    // SOLW_RPC_URL_* env vars transparently via for_network).
    let network = cli_network.unwrap_or("mainnet").to_string();
    let rpc = RpcClient::for_network(&network)?;
    let jup = JupiterClient::new();

    let (input_info, output_info) = tokio::try_join!(
        rpc.get_mint_info(&input_mint),
        rpc.get_mint_info(&output_mint),
    )?;

    let amount_raw = parse_amount_input(amount, raw, input_info.decimals)?;

    let q = jup
        .quote(&input_mint, &output_mint, amount_raw, slippage_bps)
        .await?;
    print_quote(
        &q,
        &input_mint,
        &output_mint,
        input_info.decimals,
        output_info.decimals,
        slippage_bps,
        json_out,
    )
}

pub struct SwapParams<'a> {
    pub input: &'a str,
    pub output: &'a str,
    pub amount: f64,
    pub raw: bool,
    pub slippage_bps: u16,
}

pub async fn execute(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    params: SwapParams<'_>,
    confirmed: bool,
    json_out: bool,
) -> Result<()> {
    let SwapParams { input, output, amount, raw, slippage_bps } = params;
    let input_mint = resolve_mint(input);
    let output_mint = resolve_mint(output);

    // Jupiter only routes mainnet liquidity; force mainnet for execute regardless of wallet default.
    let network = cli_network.unwrap_or("mainnet").to_string();
    if network != "mainnet" {
        bail!("swap execute requires mainnet; got '{}'", network);
    }
    let ctx = load_wallet(wallet_name, Some(&network))?;
    let jup = JupiterClient::new();

    let (input_info, output_info) = tokio::try_join!(
        ctx.client.get_mint_info(&input_mint),
        ctx.client.get_mint_info(&output_mint),
    )?;
    let amount_raw = parse_amount_input(amount, raw, input_info.decimals)?;

    let q = jup
        .quote(&input_mint, &output_mint, amount_raw, slippage_bps)
        .await?;

    if !confirmed && !json_out {
        print_quote(
            &q,
            &input_mint,
            &output_mint,
            input_info.decimals,
            output_info.decimals,
            slippage_bps,
            false,
        )?;
        println!("  wallet:   {} ({})", ctx.name, ctx.address);
        let ok = Confirm::new("Execute this swap on mainnet?")
            .with_default(false)
            .prompt()
            .context("confirmation prompt failed")?;
        if !ok {
            bail!("aborted by user");
        }
    }

    let mut raw_tx = jup
        .swap_transaction(&q, &ctx.address)
        .await
        .context("fetching swap transaction from Jupiter")?;
    let user_pubkey = decode_base58_pubkey(&ctx.address)?;
    let input_mint_bytes = decode_base58_pubkey(&input_mint)?;
    let output_mint_bytes = decode_base58_pubkey(&output_mint)?;
    verify_swap_transaction(&raw_tx, &user_pubkey, &input_mint_bytes, &output_mint_bytes)
        .context("Jupiter swap transaction failed verification")?;
    sign_prebuilt_transaction(&ctx.keypair.signing_key, &mut raw_tx)
        .context("signing Jupiter swap transaction")?;
    let signature = ctx.client.send_transaction(&raw_tx).await?;
    let confirm_result = ctx.client.confirm_signature(&signature, 40).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "address": ctx.address,
            "network": ctx.network,
            "input_mint": input_mint,
            "output_mint": output_mint,
            "input_decimals": input_info.decimals,
            "output_decimals": output_info.decimals,
            "input_amount_raw": q.in_amount(),
            "output_amount_raw": q.out_amount(),
            "in_amount_ui": raw_str_to_ui(q.in_amount(), input_info.decimals),
            "out_amount_ui": raw_str_to_ui(q.out_amount(), output_info.decimals),
            "price_impact_pct": q.price_impact_pct(),
            "route": q.route_labels(),
            "slippage_bps": slippage_bps,
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

    println!("{}", "Swap submitted".green().bold());
    println!("  signature: {}", signature);
    println!("  explorer:  {}", explorer_tx_url(&network, &signature));
    match confirm_result {
        Ok(()) => println!("  status:    {}", "confirmed".green()),
        Err(e) => {
            println!("  status:    {} ({})", "unconfirmed".red(), e);
            std::process::exit(2);
        }
    }
    Ok(())
}

/// Best-effort raw-string → UI-float for display. Returns None if Jupiter
/// didn't send a parseable amount (never observed, but guarded).
fn raw_str_to_ui(raw: Option<&str>, decimals: u8) -> Option<f64> {
    raw.and_then(|s| s.parse::<u64>().ok())
        .map(|n| n as f64 / 10f64.powi(decimals as i32))
}

fn fmt_raw_ui(raw: Option<&str>, decimals: u8) -> String {
    match (raw, raw_str_to_ui(raw, decimals)) {
        (Some(r), Some(ui)) => format!("{} ({} raw)", ui, r),
        (Some(r), None) => r.to_string(),
        _ => "?".to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn print_quote(
    q: &QuoteResponse,
    input_mint: &str,
    output_mint: &str,
    input_decimals: u8,
    output_decimals: u8,
    slippage_bps: u16,
    json_out: bool,
) -> Result<()> {
    if json_out {
        let body = serde_json::json!({
            "input_mint": input_mint,
            "output_mint": output_mint,
            "input_decimals": input_decimals,
            "output_decimals": output_decimals,
            "in_amount_raw": q.in_amount(),
            "out_amount_raw": q.out_amount(),
            "in_amount_ui": raw_str_to_ui(q.in_amount(), input_decimals),
            "out_amount_ui": raw_str_to_ui(q.out_amount(), output_decimals),
            "price_impact_pct": q.price_impact_pct(),
            "slippage_bps": slippage_bps,
            "route": q.route_labels(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("{}", "Jupiter quote".cyan().bold());
        println!(
            "  in:       {} (mint: {}, decimals={})",
            fmt_raw_ui(q.in_amount(), input_decimals),
            input_mint,
            input_decimals
        );
        println!(
            "  out:      {} (mint: {}, decimals={})",
            fmt_raw_ui(q.out_amount(), output_decimals),
            output_mint,
            output_decimals
        );
        println!("  impact:   {}%", q.price_impact_pct().unwrap_or("?"));
        println!("  slippage: {} bps", slippage_bps);
        let labels = q.route_labels();
        if labels.is_empty() {
            println!("  route:    (none reported)");
        } else {
            println!("  route:    {}", labels.join(" > "));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_aliases() {
        assert_eq!(
            resolve_mint("sol"),
            "So11111111111111111111111111111111111111112"
        );
        assert_eq!(
            resolve_mint("USDC"),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(
            resolve_mint("usdt"),
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
        );
    }

    #[test]
    fn resolve_passthrough_mint() {
        let mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
        assert_eq!(resolve_mint(mint), mint);
    }

    #[test]
    fn parse_amount_input_ui_mode_converts_via_decimals() {
        assert_eq!(parse_amount_input(0.001, false, 9).unwrap(), 1_000_000);
        assert_eq!(parse_amount_input(1.5, false, 6).unwrap(), 1_500_000);
    }

    #[test]
    fn parse_amount_input_raw_mode_passes_through() {
        assert_eq!(parse_amount_input(1_000_000.0, true, 9).unwrap(), 1_000_000);
        assert_eq!(parse_amount_input(1.0, true, 0).unwrap(), 1);
    }

    #[test]
    fn parse_amount_input_raw_rejects_fractional() {
        let err = parse_amount_input(1.5, true, 9).unwrap_err();
        assert!(err.to_string().contains("--raw requires an integer"), "got: {}", err);
    }

    #[test]
    fn parse_amount_input_raw_rejects_non_positive() {
        assert!(parse_amount_input(0.0, true, 9).is_err());
        assert!(parse_amount_input(-1.0, true, 9).is_err());
        assert!(parse_amount_input(f64::NAN, true, 9).is_err());
    }

    #[test]
    fn parse_amount_input_raw_rejects_overflow() {
        let err = parse_amount_input(1e30, true, 9).unwrap_err();
        assert!(err.to_string().contains("exceeds u64"), "got: {}", err);
    }
}
