use anyhow::Result;
use owo_colors::OwoColorize;

use crate::storage;
use crate::wallet;

const DEFAULT_NETWORK: &str = "mainnet";

fn validate_network(net: &str) -> Result<()> {
    match net {
        "mainnet" | "devnet" | "testnet" => Ok(()),
        other => anyhow::bail!(
            "unknown network '{}': expected mainnet, devnet, or testnet",
            other
        ),
    }
}

pub fn create(name: &str, cli_network: Option<&str>) -> Result<()> {
    storage::validate_wallet_name(name)?;
    if storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' already exists", name);
    }
    let network = cli_network.unwrap_or(DEFAULT_NETWORK);
    validate_network(network)?;

    let mnemonic = wallet::generate_mnemonic_12()?;
    let kp = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let address = kp.address();

    storage::store_mnemonic(&mnemonic, name)?;
    storage::store_network(name, network)?;
    storage::store_address(name, &address)?;

    if storage::get_default_wallet()?.is_none() {
        storage::set_default_wallet(name)?;
    }

    println!("{}", "Wallet created".green().bold());
    println!("  name:    {}", name);
    println!("  address: {}", address);
    println!("  network: {}", network);
    println!();
    println!("{}", "Seed phrase (SAVE THIS):".yellow().bold());
    println!("  {}", mnemonic);
    println!();
    println!(
        "{}",
        "Anyone with this phrase can spend your funds. Store it offline.".dimmed()
    );
    Ok(())
}

pub fn import(name: &str, cli_network: Option<&str>) -> Result<()> {
    storage::validate_wallet_name(name)?;
    if storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' already exists", name);
    }
    let network = cli_network.unwrap_or(DEFAULT_NETWORK);
    validate_network(network)?;

    let mnemonic = inquire::Password::new("Seed phrase:")
        .without_confirmation()
        .with_display_mode(inquire::PasswordDisplayMode::Masked)
        .prompt()?;

    let kp = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let address = kp.address();

    storage::store_mnemonic(&mnemonic, name)?;
    storage::store_network(name, network)?;
    storage::store_address(name, &address)?;

    if storage::get_default_wallet()?.is_none() {
        storage::set_default_wallet(name)?;
    }

    println!("{}", "Wallet imported".green().bold());
    println!("  name:    {}", name);
    println!("  address: {}", address);
    println!("  network: {}", network);
    Ok(())
}

pub fn info(wallet_name: Option<&str>, cli_network: Option<&str>) -> Result<()> {
    let name = storage::resolve_wallet_name(wallet_name)?;
    let mnemonic = storage::get_mnemonic(&name)?
        .ok_or_else(|| anyhow::anyhow!("mnemonic not found for wallet '{}'", name))?;
    let kp = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let network = storage::resolve_network(Some(&name), cli_network);
    let default = storage::get_default_wallet()?;
    let is_default = default.as_deref() == Some(name.as_str());

    println!("{}", "Wallet".green().bold());
    println!("  name:    {}{}", name, if is_default { " (default)" } else { "" });
    println!("  address: {}", kp.address());
    println!("  network: {}", network);
    println!("  path:    {}", wallet::DEFAULT_DERIVATION_PATH);
    Ok(())
}

pub fn export(wallet_name: Option<&str>) -> Result<()> {
    let name = storage::resolve_wallet_name(wallet_name)?;
    let confirm = inquire::Confirm::new(&format!(
        "Print the seed phrase for '{}' to stdout?",
        name
    ))
    .with_default(false)
    .with_help_message("Anyone who sees this phrase can spend your funds.")
    .prompt()?;
    if !confirm {
        println!("aborted");
        return Ok(());
    }
    let mnemonic = storage::get_mnemonic(&name)?
        .ok_or_else(|| anyhow::anyhow!("mnemonic not found for wallet '{}'", name))?;
    println!("{}", mnemonic);
    Ok(())
}

pub fn delete(name: &str) -> Result<()> {
    storage::validate_wallet_name(name)?;
    if !storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' not found", name);
    }
    let confirm = inquire::Confirm::new(&format!("Really delete wallet '{}'?", name))
        .with_default(false)
        .with_help_message("You will lose access unless you have the seed phrase saved.")
        .prompt()?;
    if !confirm {
        println!("aborted");
        return Ok(());
    }
    storage::delete_wallet(name)?;
    println!("deleted wallet '{}'", name);
    Ok(())
}

pub fn set_default(name: &str) -> Result<()> {
    storage::validate_wallet_name(name)?;
    if !storage::wallet_exists(name)? {
        anyhow::bail!("wallet '{}' not found", name);
    }
    storage::set_default_wallet(name)?;
    println!("default wallet set to '{}'", name);
    Ok(())
}

pub fn list() -> Result<()> {
    let names = storage::list_wallets()?;
    if names.is_empty() {
        println!("no wallets -- create one with: solw wallet create <name>");
        return Ok(());
    }
    let default = storage::get_default_wallet()?;
    for n in &names {
        let address = resolve_address_for_listing(n);
        let net = storage::get_network(n)?.unwrap_or_else(|| DEFAULT_NETWORK.to_string());
        let marker = if default.as_deref() == Some(n.as_str()) { "*" } else { " " };
        println!("{} {:20} {:8} {}", marker, n, net, address);
    }
    Ok(())
}

/// Read the address from the `.pub` sidecar (cheap). Fall back to deriving
/// from the mnemonic for wallets created before the sidecar existed, and
/// opportunistically cache the result so next `list` is fast.
fn resolve_address_for_listing(name: &str) -> String {
    if let Ok(Some(addr)) = storage::get_address(name) {
        return addr;
    }
    let mnemonic = match storage::get_mnemonic(name) {
        Ok(Some(m)) => m,
        _ => return "<missing>".to_string(),
    };
    match wallet::Keypair::from_mnemonic(&mnemonic) {
        Ok(kp) => {
            let addr = kp.address();
            let _ = storage::store_address(name, &addr);
            addr
        }
        Err(_) => "<invalid mnemonic>".to_string(),
    }
}
