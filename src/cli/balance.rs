use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::common::load_wallet;
use crate::cli::token::fetch_symbols;
use crate::util::amount::lamports_to_sol;

pub async fn run(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    token: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let ctx = load_wallet(wallet_name, cli_network)?;

    let mut sol_lamports: Option<u64> = None;
    let accounts = ctx.client.get_token_accounts(&ctx.address).await?;

    let filtered: Vec<_> = match token {
        Some(mint) => accounts
            .iter()
            .filter(|a| a.mint == mint)
            .cloned()
            .collect(),
        None => {
            sol_lamports = Some(ctx.client.get_balance(&ctx.address).await?);
            accounts
                .iter()
                .filter(|a| a.amount_raw > 0)
                .cloned()
                .collect()
        }
    };

    let mints: Vec<&str> = filtered.iter().map(|a| a.mint.as_str()).collect();
    let symbols = fetch_symbols(&ctx.client, &mints).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "address": ctx.address,
            "network": ctx.network,
            "sol_lamports": sol_lamports,
            "sol": sol_lamports.map(lamports_to_sol),
            "tokens": filtered.iter().zip(symbols.iter()).map(|(a, sym)| serde_json::json!({
                "mint": a.mint,
                "symbol": sym,
                "amount_raw": a.amount_raw,
                "decimals": a.decimals,
                "ui_amount": a.ui_amount,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", "Balance".green().bold());
    println!("  wallet:  {}", ctx.name);
    println!("  address: {}", ctx.address);
    println!("  network: {}", ctx.network);
    if let Some(l) = sol_lamports {
        println!("  SOL:     {:.9} ({} lamports)", lamports_to_sol(l), l);
    }
    if filtered.is_empty() {
        if token.is_some() {
            println!("  (no account for that mint)");
        }
    } else {
        println!();
        println!("{}", "Tokens".green().bold());
        for (a, sym) in filtered.iter().zip(symbols.iter()) {
            let sym_disp = sym.as_deref().unwrap_or("—");
            println!(
                "  {:<10}  {:<44}  {:>20}  decimals={}",
                sym_disp, a.mint, a.ui_amount, a.decimals
            );
        }
    }
    Ok(())
}
