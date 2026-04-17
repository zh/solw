//! `solw pay` — x402 HTTP 402 payment client.
//!
//! Full flow: GET the resource URL → parse the 402 quote → guardrails
//! (max-price, cluster match, source balance) → `--inspect` exits with an
//! unsigned-tx preview, otherwise we confirm with the user, sign the message
//! locally, base64 it into the `X-Payment` header, re-GET, and surface either
//! the premium content (200) or a payment failure (402 / other).
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use inquire::Confirm;
use owo_colors::OwoColorize;

use crate::cli::common::{explorer_tx_url, load_wallet, WalletContext};
use crate::pda::derive_associated_token_account;
use crate::tx::{
    decode_base58_blockhash, decode_base58_pubkey, sign_and_serialize,
    x402::{build_x402_transfer_message, X402TransferParams},
};
use crate::util::amount::{raw_to_ui, ui_to_raw};
use crate::x402::{self, HttpClient, PaymentDetails, Quote};

/// Default spend cap (UI units) for any `solw pay` invocation.
pub const DEFAULT_MAX_PRICE_UI: f64 = 0.01;

pub struct PayParams<'a> {
    pub url: &'a str,
    pub max_price_ui: f64,
    pub inspect: bool,
    pub confirmed: bool,
    pub json_out: bool,
}

pub async fn run(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
    params: PayParams<'_>,
) -> Result<()> {
    let PayParams {
        url,
        max_price_ui,
        inspect,
        confirmed,
        json_out,
    } = params;

    if !(max_price_ui > 0.0 && max_price_ui.is_finite()) {
        bail!("--max-price must be positive and finite (got {})", max_price_ui);
    }

    let ctx = load_wallet(wallet_name, cli_network)?;
    let http = HttpClient::new();

    let (status, body) = http.get_quote(url).await?;

    // If the endpoint returned content directly (no payment required) just
    // surface it and exit — nothing to pay.
    if status == 200 {
        if json_out {
            println!("{}", serde_json::to_string_pretty(&body)?);
        } else {
            println!("{} (HTTP 200, no payment required)", "No payment required".green().bold());
            println!("{}", serde_json::to_string_pretty(&body)?);
        }
        return Ok(());
    }
    if status != 402 {
        bail!(
            "x402 endpoint returned HTTP {} (expected 200 or 402): {}",
            status,
            body
        );
    }

    let quote = x402::parse_quote_response(&body)?;

    // Cluster <-> wallet-network sanity check runs BEFORE any RPC lookup —
    // otherwise a devnet-only mint hit on mainnet surfaces as a cryptic
    // "mint data not jsonParsed" parse error instead of the real mismatch.
    // "mainnet-beta" is the Solana Explorer label; a server using it still
    // means mainnet.
    let quote_norm = if quote.cluster == "mainnet-beta" { "mainnet" } else { quote.cluster.as_str() };
    if quote_norm != ctx.network.as_str() {
        bail!(
            "quote cluster '{}' does not match wallet network '{}'",
            quote.cluster,
            ctx.network
        );
    }

    // Decimals lookup — the server only ships the raw amount. We need
    // decimals to convert the --max-price cap to raw units and to render the
    // quote nicely.
    let mint_info = ctx.client.get_mint_info(&quote.mint).await?;
    let decimals = mint_info.decimals;

    // Compare in raw (integer) units. Float comparison would lose precision
    // past ~2^53 raw units and could let a hostile quote slip past the cap
    // by exploiting float rounding.
    let max_price_raw = ui_to_raw(max_price_ui, decimals)
        .context("invalid --max-price for this mint's decimals")?;
    if quote.amount_raw > max_price_raw {
        bail!(
            "quote exceeds --max-price: {} raw > {} raw ({} UI, mint={})",
            quote.amount_raw,
            max_price_raw,
            max_price_ui,
            quote.mint
        );
    }
    let amount_ui = raw_to_ui(quote.amount_raw, decimals); // display only

    let preview = build_tx_preview(&ctx, &quote, amount_ui).await?;

    if inspect {
        return emit_inspect(&quote, decimals, amount_ui, &ctx.address, &preview, json_out);
    }

    // Interactive confirmation, unless the caller opted out or JSON mode is on
    // (JSON output is the machine-readable contract — no prompts).
    if !confirmed && !json_out {
        print_confirm_summary(&ctx, &quote, decimals, amount_ui, &preview, url);
        let ok = Confirm::new("Sign + submit this x402 payment?")
            .with_default(false)
            .prompt()
            .context("confirmation prompt failed")?;
        if !ok {
            bail!("aborted by user");
        }
    }

    // Sign the unsigned message we already built for the preview. Deterministic:
    // what you inspected is what you sign.
    let raw_tx = sign_and_serialize(&ctx.keypair.signing_key, &preview.message_bytes);
    let tx_b64 = B64.encode(&raw_tx);
    let x402_network = quote.x402_network()?;
    let header = x402::build_x_payment_header(&tx_b64, x402_network);

    let (retry_status, retry_body) = http
        .get_with_payment_header(url, &header)
        .await?;

    match retry_status {
        200 => emit_success(&ctx, &quote, &preview, decimals, amount_ui, &retry_body, json_out),
        402 => emit_payment_failed(&retry_body, json_out),
        other => bail!(
            "x402 retry returned HTTP {} (expected 200 or 402): {}",
            other,
            retry_body
        ),
    }
}

fn print_confirm_summary(
    ctx: &WalletContext,
    quote: &Quote,
    decimals: u8,
    amount_ui: f64,
    preview: &TxPreview,
    url: &str,
) {
    println!("{}", "Confirm x402 payment".yellow().bold());
    println!("  wallet:   {} ({})", ctx.name, ctx.address);
    println!("  network:  {}", ctx.network);
    println!("  url:      {}", url);
    println!("  amount:   {} ({} raw, decimals={})", amount_ui, quote.amount_raw, decimals);
    println!("  mint:     {}", quote.mint);
    println!("  to owner: {}", quote.recipient_wallet);
    println!("  to ATA:   {}", quote.token_account);
    if let Some(msg) = &quote.message {
        println!("  server:   {}", msg);
    }
    println!(
        "  create dest ATA: {}",
        if preview.create_dest_ata {
            "yes (payer rent)"
        } else {
            "no"
        }
    );
}

fn emit_success(
    ctx: &WalletContext,
    quote: &Quote,
    preview: &TxPreview,
    decimals: u8,
    amount_ui: f64,
    body: &serde_json::Value,
    json_out: bool,
) -> Result<()> {
    // Try to parse Woody's documented success envelope. If the server just
    // returned 200 with a custom shape, don't fail — surface the raw body.
    let parsed: Option<(serde_json::Value, PaymentDetails)> =
        x402::parse_success_response(body).ok();
    let details = parsed.as_ref().map(|(_, d)| d);

    // Prefer the server-supplied explorer URL; fall back to the canonical one
    // if the server didn't include one (signature is still authoritative).
    let signature = details.map(|d| d.signature.clone());
    let explorer = signature.as_ref().map(|sig| {
        details
            .and_then(|d| d.explorer_url.clone())
            .unwrap_or_else(|| explorer_tx_url(&ctx.network, sig))
    });

    if json_out {
        let envelope = serde_json::json!({
            "confirmed": true,
            "signature": signature,
            "explorer_url": explorer,
            "amount_raw": quote.amount_raw,
            "amount_ui": amount_ui,
            "decimals": decimals,
            "mint": quote.mint,
            "recipient_wallet": quote.recipient_wallet,
            "dest_ata": quote.token_account,
            "source_ata": preview.source_ata_b58,
            "created_dest_ata": preview.create_dest_ata,
            "network": ctx.network,
            "x402_network": quote.x402_network()?,
            "data": parsed.as_ref().map(|(d, _)| d.clone()).unwrap_or_else(|| body.clone()),
        });
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }

    println!("{}", "x402 payment accepted".green().bold());
    println!("  amount:    {} ({} raw)", amount_ui, quote.amount_raw);
    println!("  recipient: {}", quote.recipient_wallet);
    if let Some(sig) = &signature {
        println!("  signature: {}", sig);
    }
    if let Some(exp) = &explorer {
        println!("  explorer:  {}", exp);
    }
    println!();
    match parsed.as_ref().map(|(d, _)| d) {
        Some(data) => match data.as_str() {
            Some(s) => println!("{}", s),
            None => println!("{}", serde_json::to_string_pretty(data)?),
        },
        None => println!("{}", serde_json::to_string_pretty(body)?),
    }
    Ok(())
}

fn emit_payment_failed(body: &serde_json::Value, json_out: bool) -> Result<()> {
    // 402 on retry = server rejected the payment (bad signature, already-used
    // blockhash, on-chain failure, etc.). The tx may or may not have landed.
    let error = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("payment rejected by server");
    let details = body.get("details").and_then(|v| v.as_str());
    let signature = body.get("signature").and_then(|v| v.as_str());

    if json_out {
        let envelope = serde_json::json!({
            "confirmed": false,
            "error": error,
            "details": details,
            "signature": signature,
            "server_body": body,
        });
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        eprintln!("{}: {}", "x402 payment rejected".red().bold(), error);
        if let Some(d) = details {
            eprintln!("  details:   {}", d);
        }
        if let Some(sig) = signature {
            eprintln!("  signature: {} (may or may not have landed — check explorer)", sig);
        }
    }
    std::process::exit(2);
}

/// Everything the pay flow builds up before signing: the serialized message
/// bytes plus human-readable metadata for the inspect/confirm UI. Used by
/// both `--inspect` (prints the preview then exits) and the real submission
/// path (signs `message_bytes` directly, so what you inspected is what you
/// signed).
struct TxPreview {
    source_ata_b58: String,
    source_balance_raw: u64,
    create_dest_ata: bool,
    instruction_count: u8,
    blockhash_b58: String,
    message_b64: String,
    message_len: usize,
    message_bytes: Vec<u8>,
}

async fn build_tx_preview(
    ctx: &WalletContext,
    quote: &Quote,
    amount_ui: f64,
) -> Result<TxPreview> {
    let payer_bytes = decode_base58_pubkey(&ctx.address)?;
    let mint_bytes = decode_base58_pubkey(&quote.mint).context("invalid quote mint address")?;
    let dest_ata_bytes = decode_base58_pubkey(&quote.token_account)
        .context("invalid quote tokenAccount address")?;
    let dest_owner_bytes = decode_base58_pubkey(&quote.recipient_wallet)
        .context("invalid quote recipientWallet address")?;

    // SECURITY: re-derive the canonical ATA for (recipient_wallet, mint) and
    // assert the server-supplied tokenAccount matches. Otherwise a hostile
    // x402 server could publish a legitimate-looking recipientWallet (passing
    // user inspection) but point tokenAccount at an attacker-owned ATA,
    // silently redirecting the debit.
    let (expected_dest_ata, _) =
        derive_associated_token_account(&dest_owner_bytes, &mint_bytes)?;
    if expected_dest_ata != dest_ata_bytes {
        bail!(
            "quote tokenAccount {} is not the canonical ATA for recipient {} + mint {} \
             (expected {}) — refusing to sign",
            quote.token_account,
            quote.recipient_wallet,
            quote.mint,
            bs58::encode(expected_dest_ata).into_string()
        );
    }

    // Source ATA is the payer's own USDC account. We derive it rather than
    // trusting the server, so a malicious quote can't redirect the debit.
    let (source_ata, _) = derive_associated_token_account(&payer_bytes, &mint_bytes)?;
    let source_ata_b58 = bs58::encode(source_ata).into_string();

    // Source balance — fail early if the wallet can't cover the quote, so the
    // user doesn't sign a tx that will immediately bounce on-chain.
    let source_balance_raw = ctx
        .client
        .get_token_accounts(&ctx.address)
        .await?
        .into_iter()
        .find(|t| t.mint == quote.mint)
        .map(|t| t.amount_raw)
        .unwrap_or(0);
    if source_balance_raw < quote.amount_raw {
        bail!(
            "insufficient {} balance: have {} raw, need {} raw ({} UI)",
            quote.mint,
            source_balance_raw,
            quote.amount_raw,
            amount_ui
        );
    }

    let dest_exists = ctx.client.account_exists(&quote.token_account).await?;
    let create_dest_ata = !dest_exists;

    let (blockhash_b58, _) = ctx.client.get_latest_blockhash().await?;
    let blockhash = decode_base58_blockhash(&blockhash_b58)?;

    let msg = build_x402_transfer_message(&X402TransferParams {
        payer: &payer_bytes,
        source_ata: &source_ata,
        dest_ata: &dest_ata_bytes,
        dest_owner: &dest_owner_bytes,
        mint: &mint_bytes,
        amount_raw: quote.amount_raw,
        create_dest_ata,
        recent_blockhash: &blockhash,
    })?;

    let instruction_count: u8 = if create_dest_ata { 2 } else { 1 };
    let message_len = msg.len();
    let message_b64 = B64.encode(&msg);

    Ok(TxPreview {
        source_ata_b58,
        source_balance_raw,
        create_dest_ata,
        instruction_count,
        blockhash_b58,
        message_b64,
        message_len,
        message_bytes: msg,
    })
}

fn emit_inspect(
    quote: &Quote,
    decimals: u8,
    amount_ui: f64,
    payer_address: &str,
    preview: &TxPreview,
    json_out: bool,
) -> Result<()> {
    if json_out {
        let body = serde_json::json!({
            "inspect": true,
            "payer": payer_address,
            "quote": {
                "recipient_wallet": quote.recipient_wallet,
                "token_account": quote.token_account,
                "mint": quote.mint,
                "decimals": decimals,
                "amount_raw": quote.amount_raw,
                "amount_ui": amount_ui,
                "amount_ui_server_hint": quote.amount_ui_hint,
                "cluster": quote.cluster,
                "x402_network": quote.x402_network()?,
                "message": quote.message,
            },
            "transaction": {
                "source_ata": preview.source_ata_b58,
                "source_balance_raw": preview.source_balance_raw,
                "dest_ata": quote.token_account,
                "create_dest_ata": preview.create_dest_ata,
                "instruction_count": preview.instruction_count,
                "recent_blockhash": preview.blockhash_b58,
                "message_len_bytes": preview.message_len,
                "message_base64": preview.message_b64,
                "signed": false,
            },
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("{}", "x402 quote".cyan().bold());
        println!("  payer:     {}", payer_address);
        println!("  recipient: {}", quote.recipient_wallet);
        println!("  to ATA:    {}", quote.token_account);
        println!("  mint:      {} (decimals={})", quote.mint, decimals);
        println!("  amount:    {} ({} raw)", amount_ui, quote.amount_raw);
        println!("  cluster:   {}", quote.cluster);
        println!("  x402 net:  {}", quote.x402_network()?);
        if let Some(msg) = &quote.message {
            println!("  message:   {}", msg);
        }
        println!();
        println!("{}", "unsigned transaction preview".cyan().bold());
        println!("  source ATA:    {}", preview.source_ata_b58);
        println!("  source balance:{} raw", preview.source_balance_raw);
        println!("  dest ATA:      {}", quote.token_account);
        println!(
            "  create dest ATA: {}",
            if preview.create_dest_ata {
                "yes (payer rent)"
            } else {
                "no"
            }
        );
        println!("  instructions:  {}", preview.instruction_count);
        println!("  blockhash:     {}", preview.blockhash_b58);
        println!("  message bytes: {}", preview.message_len);
        println!("  message (b64): {}", preview.message_b64);
        println!();
        println!("  (inspect only — no transaction signed, no funds moved)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_price_is_one_cent_usdc() {
        assert!((DEFAULT_MAX_PRICE_UI - 0.01).abs() < 1e-12);
    }
}
