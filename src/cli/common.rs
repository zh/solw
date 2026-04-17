//! Cross-subcommand helpers used by multiple CLI modules.
//!
//! Lives here rather than in a single owning subcommand so modules don't
//! cross-import for tiny utilities (e.g. `airdrop` → `send::explorer_cluster`).

use anyhow::{anyhow, Result};

use crate::rpc::RpcClient;
use crate::storage;
use crate::wallet;

/// Everything a CLI subcommand typically needs after resolving a wallet:
/// its storage name, derived address, network label, JSON-RPC client, and
/// the signing keypair. Read-only commands ignore `keypair`.
pub struct WalletContext {
    pub name: String,
    pub address: String,
    pub network: String,
    pub client: RpcClient,
    pub keypair: wallet::Keypair,
}

/// Resolve wallet + network from CLI args / storage defaults, then build
/// a configured `RpcClient`. Consolidates the "name → mnemonic → keypair →
/// address → network → client" boilerplate that every subcommand ran inline.
pub fn load_wallet(
    wallet_name: Option<&str>,
    cli_network: Option<&str>,
) -> Result<WalletContext> {
    let name = storage::resolve_wallet_name(wallet_name)?;
    let mnemonic = storage::get_mnemonic(&name)?
        .ok_or_else(|| anyhow!("mnemonic not found for '{}'", name))?;
    let keypair = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let address = keypair.address();
    let network = storage::resolve_network(Some(&name), cli_network);
    let client = RpcClient::for_network(&network)?;
    Ok(WalletContext { name, address, network, client, keypair })
}

/// Map our internal network name to the query-string cluster the Solana
/// Explorer expects (`?cluster=mainnet-beta|devnet|testnet`).
pub fn explorer_cluster(network: &str) -> &'static str {
    match network {
        "mainnet" => "mainnet-beta",
        "devnet" => "devnet",
        "testnet" => "testnet",
        _ => "mainnet-beta",
    }
}

pub fn explorer_tx_url(network: &str, signature: &str) -> String {
    format!(
        "https://explorer.solana.com/tx/{}?cluster={}",
        signature,
        explorer_cluster(network)
    )
}

pub fn explorer_address_url(network: &str, address: &str) -> String {
    format!(
        "https://explorer.solana.com/address/{}?cluster={}",
        address,
        explorer_cluster(network)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_cluster_mapping() {
        assert_eq!(explorer_cluster("mainnet"), "mainnet-beta");
        assert_eq!(explorer_cluster("devnet"), "devnet");
        assert_eq!(explorer_cluster("testnet"), "testnet");
        assert_eq!(explorer_cluster("unknown"), "mainnet-beta");
    }

    #[test]
    fn explorer_tx_url_formats() {
        assert_eq!(
            explorer_tx_url("devnet", "abc123"),
            "https://explorer.solana.com/tx/abc123?cluster=devnet"
        );
        assert_eq!(
            explorer_tx_url("mainnet", "sig"),
            "https://explorer.solana.com/tx/sig?cluster=mainnet-beta"
        );
    }

    #[test]
    fn explorer_address_url_formats() {
        assert_eq!(
            explorer_address_url("mainnet", "Ab12"),
            "https://explorer.solana.com/address/Ab12?cluster=mainnet-beta"
        );
    }
}
