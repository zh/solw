//! NFT commands are a thin layer over SPL tokens: an NFT is a mint with
//! decimals = 0 and supply = 1. Metadata (name/symbol/URI) lives at the
//! Metaplex metadata PDA.
use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_address_url, load_wallet};
use crate::metaplex;
use crate::rpc::RpcClient;
use crate::storage;

pub async fn list(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let ctx = load_wallet(wallet_name, cli_network)?;
    let accounts = ctx.client.get_token_accounts(&ctx.address).await?;
    let nfts: Vec<_> = accounts
        .iter()
        .filter(|a| a.decimals == 0 && a.amount_raw == 1)
        .cloned()
        .collect();

    if json_out {
        let body = serde_json::json!({
            "wallet": ctx.name,
            "address": ctx.address,
            "network": ctx.network,
            "nfts": nfts.iter().map(|a| serde_json::json!({
                "mint": a.mint,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", "NFTs".green().bold());
    println!("  wallet:  {}", ctx.name);
    println!("  address: {}", ctx.address);
    println!("  network: {}", ctx.network);
    println!();
    if nfts.is_empty() {
        println!("  (no NFTs found)");
        return Ok(());
    }
    for a in nfts {
        println!("  {}", a.mint);
    }
    Ok(())
}

pub async fn info(
    _wallet_name: Option<&str>,
    cli_network: Option<&str>,
    mint: &str,
    json_out: bool,
) -> Result<()> {
    let network = storage::resolve_network(None, cli_network);
    let client = RpcClient::for_network(&network)?;
    let mint_info = client.get_mint_info(mint).await?;
    let (metadata_pda, _bump) = metaplex::metadata_pda(mint)?;
    let metadata_pda_b58 = bs58::encode(metadata_pda).into_string();
    let metadata = crate::cli::token::fetch_metadata(&client, mint).await;

    if json_out {
        let (name, symbol, uri, has_metadata, metadata_error) = match &metadata {
            crate::cli::token::MetadataLookup::Present(md) => (
                Some(md.name.as_str()),
                Some(md.symbol.as_str()),
                Some(md.uri.as_str()),
                true,
                None,
            ),
            crate::cli::token::MetadataLookup::Absent => (None, None, None, false, None),
            crate::cli::token::MetadataLookup::Error(e) => {
                (None, None, None, false, Some(e.as_str()))
            }
        };
        let body = serde_json::json!({
            "mint": mint,
            "network": network,
            "decimals": mint_info.decimals,
            "supply_raw": mint_info.supply_raw,
            "metadata_pda": metadata_pda_b58,
            "is_nft": mint_info.decimals == 0 && mint_info.supply_raw == 1,
            "name": name,
            "symbol": symbol,
            "uri": uri,
            "has_metadata": has_metadata,
            "metadata_error": metadata_error,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", "NFT mint".green().bold());
    println!("  mint:           {}", mint);
    println!("  network:        {}", network);
    match &metadata {
        crate::cli::token::MetadataLookup::Present(md) => {
            println!("  name:           {}", md.name);
            println!("  symbol:         {}", md.symbol);
        }
        crate::cli::token::MetadataLookup::Absent => {
            println!("  name:           (no Metaplex metadata)");
        }
        crate::cli::token::MetadataLookup::Error(e) => {
            println!("  name:           (malformed metadata: {})", e);
        }
    }
    println!("  decimals:       {}", mint_info.decimals);
    println!("  supply:         {}", mint_info.supply_raw);
    println!("  metadata PDA:   {}", metadata_pda_b58);
    println!("  explorer:       {}", explorer_address_url(&network, mint));
    Ok(())
}

pub async fn send(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    mint: &str,
    to: &str,
    confirmed: bool,
    json_out: bool,
) -> Result<()> {
    crate::cli::token::send(wallet_name, cli_network, mint, to, 1.0, confirmed, json_out).await
}


