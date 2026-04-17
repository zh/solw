//! Filesystem wallet storage at ~/.solw/.
//!
//! Layout:
//!   ~/.solw/
//!   ├── default                 # name of default wallet (0600)
//!   └── wallets/                # 0700
//!       ├── <name>              # BIP39 mnemonic (0600)
//!       ├── <name>.net          # network: mainnet|devnet|testnet (0600)
//!       └── <name>.pub          # cached base58 pubkey (0600, avoids re-deriving for list)
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::path::PathBuf;

const RESERVED_NAMES: &[&str] = &["default", "config"];

#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("invalid wallet name '{name}': must be alphanumeric, hyphens, or underscores (max 64 chars)")]
    InvalidWalletName { name: String },
    #[error("wallet '{name}' already exists")]
    WalletExists { name: String },
    #[error("wallet '{name}' not found")]
    WalletNotFound { name: String },
    #[error("no default wallet set -- use --name or create a wallet first")]
    NoDefaultWallet,
}

thread_local! {
    static BASE_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub fn set_base_dir_override(path: Option<PathBuf>) {
    BASE_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = path);
}

fn base_dir() -> Result<PathBuf> {
    let override_path = BASE_DIR_OVERRIDE.with(|cell| cell.borrow().clone());
    if let Some(p) = override_path {
        return Ok(p);
    }
    if let Ok(env) = std::env::var("SOLW_HOME") {
        return Ok(PathBuf::from(env));
    }
    dirs::home_dir()
        .map(|h| h.join(".solw"))
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))
}

#[cfg(unix)]
fn ensure_dir_permissions(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode != 0o700 {
        eprintln!(
            "WARNING: fixing permissions on {} (was {:o}, setting to 0700)",
            path.display(),
            mode
        );
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).context("failed to fix directory permissions")?;
    }
    Ok(())
}

fn write_secret_file(path: &std::path::Path, content: &str) -> Result<()> {
    std::fs::write(path, content).context("failed to write secret file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).context("failed to set file permissions")?;
    }
    Ok(())
}

fn solw_dir() -> Result<PathBuf> {
    let dir = base_dir()?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir).context("failed to create solw directory")?;
    }
    #[cfg(unix)]
    ensure_dir_permissions(&dir)?;
    Ok(dir)
}

pub(crate) fn wallets_dir() -> Result<PathBuf> {
    let dir = solw_dir()?.join("wallets");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).context("failed to create wallets directory")?;
    }
    #[cfg(unix)]
    ensure_dir_permissions(&dir)?;
    Ok(dir)
}

pub fn validate_wallet_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(StorageError::InvalidWalletName { name: name.to_string() }.into());
    }
    if name.starts_with('.') {
        return Err(StorageError::InvalidWalletName { name: name.to_string() }.into());
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(StorageError::InvalidWalletName { name: name.to_string() }.into());
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(StorageError::InvalidWalletName { name: name.to_string() }.into());
    }
    Ok(())
}

pub fn store_mnemonic(mnemonic: &str, name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    if path.exists() {
        return Err(StorageError::WalletExists { name: name.to_string() }.into());
    }
    let content = format!("{}\n", mnemonic.trim());
    write_secret_file(&path, &content)?;
    Ok(())
}

pub fn get_mnemonic(name: &str) -> Result<Option<String>> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read mnemonic file")),
    }
}

pub fn store_network(name: &str, network: &str) -> Result<()> {
    let path = wallets_dir()?.join(format!("{}.net", name));
    write_secret_file(&path, network)
}

pub fn store_address(name: &str, address: &str) -> Result<()> {
    let path = wallets_dir()?.join(format!("{}.pub", name));
    write_secret_file(&path, &format!("{}\n", address))
}

pub fn get_address(name: &str) -> Result<Option<String>> {
    let path = wallets_dir()?.join(format!("{}.pub", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read address file")),
    }
}

pub fn get_network(name: &str) -> Result<Option<String>> {
    let path = wallets_dir()?.join(format!("{}.net", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read network file")),
    }
}

pub fn resolve_network(wallet_name: Option<&str>, cli_network: Option<&str>) -> String {
    if let Some(net) = cli_network {
        return net.to_string();
    }
    let name = wallet_name
        .map(|n| n.to_string())
        .or_else(|| get_default_wallet().ok().flatten())
        .unwrap_or_default();
    if name.is_empty() {
        return "mainnet".to_string();
    }
    get_network(&name)
        .unwrap_or(None)
        .unwrap_or_else(|| "mainnet".to_string())
}

pub fn delete_wallet(name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let dir = wallets_dir()?;
    let path = dir.join(name);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::Error::new(e).context("failed to delete wallet file")),
    }
    let _ = std::fs::remove_file(dir.join(format!("{}.net", name)));
    let _ = std::fs::remove_file(dir.join(format!("{}.pub", name)));
    if let Ok(Some(default_name)) = get_default_wallet() {
        if default_name == name {
            clear_default_wallet()?;
        }
    }
    Ok(())
}

pub fn set_default_wallet(name: &str) -> Result<()> {
    validate_wallet_name(name)?;
    let path = solw_dir()?.join("default");
    write_secret_file(&path, &format!("{}\n", name))
}

pub fn get_default_wallet() -> Result<Option<String>> {
    let path = solw_dir()?.join("default");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim().to_string();
            if trimmed.is_empty() { Ok(None) } else { Ok(Some(trimmed)) }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context("failed to read default wallet file")),
    }
}

pub fn clear_default_wallet() -> Result<()> {
    let path = solw_dir()?.join("default");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context("failed to clear default wallet")),
    }
}

pub fn list_wallets() -> Result<Vec<String>> {
    let dir = wallets_dir()?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).context("failed to read wallets directory")? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                // Valid wallet names never contain '.'; skip every sidecar
                // (.net, .pub, or anything else we add later) in one rule.
                if name.contains('.') {
                    continue;
                }
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

pub fn wallet_exists(name: &str) -> Result<bool> {
    validate_wallet_name(name)?;
    let path = wallets_dir()?.join(name);
    Ok(path.exists())
}

pub fn resolve_wallet_name(name: Option<&str>) -> Result<String> {
    if let Some(n) = name {
        validate_wallet_name(n)?;
        if !wallet_exists(n)? {
            return Err(StorageError::WalletNotFound { name: n.to_string() }.into());
        }
        return Ok(n.to_string());
    }
    match get_default_wallet()? {
        Some(default_name) => {
            if !wallet_exists(&default_name)? {
                return Err(StorageError::WalletNotFound { name: default_name }.into());
            }
            Ok(default_name)
        }
        None => Err(StorageError::NoDefaultWallet.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_temp_home() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        set_base_dir_override(Some(tmp.path().to_path_buf()));
        tmp
    }

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_wallet_name("my-wallet").is_ok());
        assert!(validate_wallet_name("test_1").is_ok());
        assert!(validate_wallet_name("ABC123").is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        assert!(validate_wallet_name("").is_err());
        assert!(validate_wallet_name("a b").is_err());
        assert!(validate_wallet_name("a/b").is_err());
        assert!(validate_wallet_name("a.b").is_err());
        assert!(validate_wallet_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn test_validate_name_rejects_reserved() {
        assert!(validate_wallet_name("default").is_err());
        assert!(validate_wallet_name("config").is_err());
    }

    #[test]
    fn test_validate_name_rejects_dot_prefix() {
        assert!(validate_wallet_name(".hidden").is_err());
    }

    #[test]
    fn test_store_and_get_mnemonic() {
        let _tmp = setup_temp_home();
        store_mnemonic("seed phrase here", "w1").unwrap();
        assert_eq!(get_mnemonic("w1").unwrap(), Some("seed phrase here".to_string()));
    }

    #[test]
    fn test_store_mnemonic_duplicate_rejected() {
        let _tmp = setup_temp_home();
        store_mnemonic("first", "dup").unwrap();
        assert!(store_mnemonic("second", "dup").is_err());
        assert_eq!(get_mnemonic("dup").unwrap(), Some("first".to_string()));
    }

    #[test]
    fn test_delete_wallet() {
        let _tmp = setup_temp_home();
        store_mnemonic("x", "gone").unwrap();
        delete_wallet("gone").unwrap();
        assert!(!wallet_exists("gone").unwrap());
    }

    #[test]
    fn test_delete_wallet_clears_default() {
        let _tmp = setup_temp_home();
        store_mnemonic("x", "w").unwrap();
        set_default_wallet("w").unwrap();
        delete_wallet("w").unwrap();
        assert_eq!(get_default_wallet().unwrap(), None);
    }

    #[test]
    fn test_default_roundtrip() {
        let _tmp = setup_temp_home();
        store_mnemonic("x", "d").unwrap();
        set_default_wallet("d").unwrap();
        assert_eq!(get_default_wallet().unwrap(), Some("d".to_string()));
        clear_default_wallet().unwrap();
        assert_eq!(get_default_wallet().unwrap(), None);
    }

    #[test]
    fn test_list_wallets_skips_sidecars() {
        let _tmp = setup_temp_home();
        store_mnemonic("a", "alpha").unwrap();
        store_network("alpha", "mainnet").unwrap();
        store_address("alpha", "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk").unwrap();
        assert_eq!(list_wallets().unwrap(), vec!["alpha".to_string()]);
    }

    #[test]
    fn test_address_sidecar_roundtrip() {
        let _tmp = setup_temp_home();
        store_mnemonic("m", "w").unwrap();
        assert_eq!(get_address("w").unwrap(), None);
        store_address("w", "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk").unwrap();
        assert_eq!(
            get_address("w").unwrap().as_deref(),
            Some("HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk")
        );
    }

    #[test]
    fn test_delete_wallet_removes_sidecars() {
        let _tmp = setup_temp_home();
        store_mnemonic("m", "gone").unwrap();
        store_network("gone", "devnet").unwrap();
        store_address("gone", "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk").unwrap();
        delete_wallet("gone").unwrap();
        assert!(get_network("gone").unwrap().is_none());
        assert!(get_address("gone").unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_network_sidecar_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _tmp = setup_temp_home();
        store_network("perm", "mainnet").unwrap();
        let p = wallets_dir().unwrap().join("perm.net");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn test_resolve_network_cli_override() {
        let _tmp = setup_temp_home();
        store_mnemonic("x", "nw").unwrap();
        store_network("nw", "mainnet").unwrap();
        assert_eq!(resolve_network(Some("nw"), Some("devnet")), "devnet");
        assert_eq!(resolve_network(Some("nw"), None), "mainnet");
    }

    #[cfg(unix)]
    #[test]
    fn test_file_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;
        let _tmp = setup_temp_home();
        store_mnemonic("s", "perm").unwrap();
        let p = wallets_dir().unwrap().join("perm");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn test_resolve_wallet_name_no_default() {
        let _tmp = setup_temp_home();
        assert!(resolve_wallet_name(None).is_err());
    }
}
