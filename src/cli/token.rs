use anyhow::{bail, Context, Result};
use inquire::Confirm;
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_tx_url, load_wallet};
use crate::metaplex;
use crate::pda::derive_associated_token_account;
use crate::rpc::RpcClient;
use crate::tx::{
    decode_base58_blockhash, decode_base58_pubkey, sign_and_serialize,
    token::{build_token_transfer_message, TokenTransferParams},
};

pub async fn list(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    include_empty: bool,
    json_out: bool,
) -> Result<()> {
    let ctx = load_wallet(wallet_name, cli_network)?;
    let all_accounts = ctx.client.get_token_accounts(&ctx.address).await?;
    let total = all_accounts.len();
    let accounts: Vec<_> = if include_empty {
        all_accounts
    } else {
        all_accounts.into_iter().filter(|a| a.amount_raw > 0).collect()
    };
    let hidden = total - accounts.len();

    let mints: Vec<&str> = accounts.iter().map(|a| a.mint.as_str()).collect();
    let symbols = fetch_symbols(&ctx.client, &mints).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "address": ctx.address,
            "network": ctx.network,
            "include_empty": include_empty,
            "hidden_empty": hidden,
            "tokens": accounts.iter().zip(symbols.iter()).map(|(a, sym)| serde_json::json!({
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

    println!("{}", "Tokens".green().bold());
    println!("  wallet:  {}", ctx.name);
    println!("  address: {}", ctx.address);
    println!("  network: {}", ctx.network);
    println!();
    if accounts.is_empty() {
        if hidden > 0 {
            println!("  (no non-empty token accounts; {} empty hidden — use --all to show)", hidden);
        } else {
            println!("  (no SPL token accounts)");
        }
        return Ok(());
    }
    for (a, sym) in accounts.iter().zip(symbols.iter()) {
        let sym_disp = sym.as_deref().unwrap_or("—");
        println!(
            "  {:<10}  {:<44}  {:>20}  decimals={}",
            sym_disp, a.mint, a.ui_amount, a.decimals
        );
    }
    if hidden > 0 {
        println!();
        println!("  ({} empty token account{} hidden — use --all to show)", hidden, if hidden == 1 { "" } else { "s" });
    }
    Ok(())
}

pub async fn info(
    _wallet_name: Option<&str>,
    cli_network: Option<&str>,
    mint: &str,
    json_out: bool,
) -> Result<()> {
    let network = crate::storage::resolve_network(None, cli_network);
    let client = RpcClient::for_network(&network)?;
    let m = client.get_mint_info(mint).await?;
    let supply_ui = m.supply_raw as f64 / 10f64.powi(m.decimals as i32);

    // Best-effort Metaplex metadata fetch. Never fails the command — missing
    // and malformed metadata are both reported in-line instead.
    let metadata = fetch_metadata(&client, mint).await;

    if json_out {
        let (name, symbol, uri, has_metadata, metadata_error) = match &metadata {
            MetadataLookup::Present(md) => (
                Some(md.name.as_str()),
                Some(md.symbol.as_str()),
                Some(md.uri.as_str()),
                true,
                None,
            ),
            MetadataLookup::Absent => (None, None, None, false, None),
            MetadataLookup::Error(e) => (None, None, None, false, Some(e.as_str())),
        };
        let body = serde_json::json!({
            "mint": mint,
            "network": network,
            "decimals": m.decimals,
            "supply_raw": m.supply_raw,
            "supply_ui": supply_ui,
            "mint_authority": m.mint_authority,
            "freeze_authority": m.freeze_authority,
            "is_initialized": m.is_initialized,
            "name": name,
            "symbol": symbol,
            "uri": uri,
            "has_metadata": has_metadata,
            "metadata_error": metadata_error,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", "Token mint".green().bold());
    println!("  mint:            {}", mint);
    println!("  network:         {}", network);
    match &metadata {
        MetadataLookup::Present(md) => {
            println!("  name:            {}", md.name);
            println!("  symbol:          {}", md.symbol);
        }
        MetadataLookup::Absent => {
            println!("  name:            (no Metaplex metadata)");
        }
        MetadataLookup::Error(e) => {
            println!("  name:            (malformed metadata: {})", e);
        }
    }
    println!("  decimals:        {}", m.decimals);
    println!("  supply:          {} ({} raw)", supply_ui, m.supply_raw);
    println!(
        "  mint authority:  {}",
        m.mint_authority.as_deref().unwrap_or("(disabled / fixed supply)")
    );
    println!(
        "  freeze:          {}",
        m.freeze_authority.as_deref().unwrap_or("(none)")
    );
    Ok(())
}

pub(crate) enum MetadataLookup {
    Present(metaplex::TokenMetadata),
    Absent,
    Error(String),
}

/// Fetch Metaplex symbols for a batch of mints. Returns `None` for missing /
/// malformed metadata so callers can render `—`. Sequential on purpose — the
/// typical wallet has <10 token accounts and a batched RPC isn't worth the
/// complexity; revisit if we grow a heavy-holder use case.
pub(crate) async fn fetch_symbols(client: &RpcClient, mints: &[&str]) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(mints.len());
    for mint in mints {
        let sym = match fetch_metadata(client, mint).await {
            MetadataLookup::Present(md) => {
                let trimmed = md.symbol.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            _ => None,
        };
        out.push(sym);
    }
    out
}

pub(crate) async fn fetch_metadata(client: &RpcClient, mint: &str) -> MetadataLookup {
    let pda = match metaplex::metadata_pda(mint) {
        Ok((p, _)) => p,
        Err(e) => return MetadataLookup::Error(e.to_string()),
    };
    let pda_b58 = bs58::encode(pda).into_string();
    match client.get_account_data_base64(&pda_b58).await {
        Ok(Some(bytes)) => match metaplex::parse_metadata_account(&bytes) {
            Ok(md) => MetadataLookup::Present(md),
            Err(e) => MetadataLookup::Error(e.to_string()),
        },
        Ok(None) => MetadataLookup::Absent,
        Err(e) => MetadataLookup::Error(e.to_string()),
    }
}

pub async fn send(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    mint: &str,
    to: &str,
    amount_ui: f64,
    confirmed: bool,
    json_out: bool,
) -> Result<()> {
    let ctx = load_wallet(wallet_name, cli_network)?;

    let from_bytes = decode_base58_pubkey(&ctx.address)?;
    let to_bytes = decode_base58_pubkey(to).context("invalid recipient address")?;
    let mint_bytes = decode_base58_pubkey(mint).context("invalid mint address")?;

    // Fetch mint decimals (required for TransferChecked) + convert UI→raw.
    let mint_info = ctx.client.get_mint_info(mint).await?;
    let amount_raw = crate::util::amount::ui_to_raw(amount_ui, mint_info.decimals)?;

    let (source_ata, _) = derive_associated_token_account(&from_bytes, &mint_bytes)?;
    let (dest_ata, _) = derive_associated_token_account(&to_bytes, &mint_bytes)?;
    let source_ata_b58 = bs58::encode(source_ata).into_string();
    let dest_ata_b58 = bs58::encode(dest_ata).into_string();

    let source_exists = ctx.client.account_exists(&source_ata_b58).await?;
    if !source_exists {
        bail!(
            "source token account does not exist for mint {}; acquire some tokens first",
            mint
        );
    }
    let dest_exists = ctx.client.account_exists(&dest_ata_b58).await?;

    if !confirmed && !json_out {
        println!("{}", "Confirm token transfer".yellow().bold());
        println!("  wallet:     {} ({})", ctx.name, ctx.address);
        println!("  network:    {}", ctx.network);
        println!("  mint:       {}", mint);
        println!("  decimals:   {}", mint_info.decimals);
        println!("  amount:     {} ({} raw)", amount_ui, amount_raw);
        println!("  to (owner): {}", to);
        println!("  to (ATA):   {}", dest_ata_b58);
        println!(
            "  create recipient ATA: {}",
            if dest_exists { "no" } else { "yes (payer rent)" }
        );
        let ok = Confirm::new("Send this token transfer?")
            .with_default(false)
            .prompt()
            .context("confirmation prompt failed")?;
        if !ok {
            bail!("aborted by user");
        }
    }

    let (blockhash_b58, _) = ctx.client.get_latest_blockhash().await?;
    let blockhash = decode_base58_blockhash(&blockhash_b58)?;

    let msg = build_token_transfer_message(&TokenTransferParams {
        payer: &from_bytes,
        source_ata: &source_ata,
        dest_ata: &dest_ata,
        dest_owner: &to_bytes,
        mint: &mint_bytes,
        amount_raw,
        decimals: mint_info.decimals,
        create_dest_ata: !dest_exists,
        recent_blockhash: &blockhash,
    })?;
    let raw_tx = sign_and_serialize(&ctx.keypair.signing_key, &msg);
    let signature = ctx.client.send_transaction(&raw_tx).await?;
    let confirm_result = ctx.client.confirm_signature(&signature, 20).await;

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "from": ctx.address,
            "to": to,
            "mint": mint,
            "network": ctx.network,
            "amount_ui": amount_ui,
            "amount_raw": amount_raw,
            "decimals": mint_info.decimals,
            "source_ata": source_ata_b58,
            "dest_ata": dest_ata_b58,
            "created_dest_ata": !dest_exists,
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

    println!("{}", "Token transfer submitted".green().bold());
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

