//! BIP39 mnemonic -> seed -> ed25519 keypair derivation at m/44'/501'/0'/0'.
//!
//! This is the path used by Phantom, Solflare, and most Solana web wallets.
//! The official `solana-cli` also supports this path via `--derivation-path`.
use anyhow::{Context, Result};
use bip39::Mnemonic;
use ed25519_dalek::{SigningKey, VerifyingKey};
use ed25519_dalek_bip32::{DerivationPath, ExtendedSigningKey};
use std::str::FromStr;
use zeroize::Zeroizing;

/// Default Solana BIP44 derivation path used by Phantom/Solflare.
pub const DEFAULT_DERIVATION_PATH: &str = "m/44'/501'/0'/0'";

/// Solana ed25519 keypair derived from a BIP39 mnemonic.
pub struct Keypair {
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
}

impl Keypair {
    /// Derive a keypair from a mnemonic phrase using the default Solana path.
    pub fn from_mnemonic(mnemonic: &str) -> Result<Self> {
        Self::from_mnemonic_with_path(mnemonic, DEFAULT_DERIVATION_PATH)
    }

    pub fn from_mnemonic_with_path(mnemonic: &str, path: &str) -> Result<Self> {
        let parsed = Mnemonic::parse_normalized(mnemonic.trim())
            .context("invalid BIP39 mnemonic phrase")?;
        // Zeroize the 64-byte seed on drop. SigningKey is also zero-on-drop
        // via the `zeroize` feature of ed25519-dalek.
        let seed = Zeroizing::new(parsed.to_seed(""));
        let derivation_path = DerivationPath::from_str(path)
            .context("invalid derivation path")?;
        let extended = ExtendedSigningKey::from_seed(&*seed)
            .context("failed to build extended signing key from seed")?
            .derive(&derivation_path)
            .context("failed to derive child key")?;
        let signing_key = extended.signing_key;
        let verifying_key = signing_key.verifying_key();
        Ok(Self { signing_key, verifying_key })
    }

    /// Solana address: base58 of the 32-byte ed25519 public key.
    pub fn address(&self) -> String {
        bs58::encode(self.verifying_key.to_bytes()).into_string()
    }
}

/// Generate a fresh 12-word English BIP39 mnemonic.
pub fn generate_mnemonic_12() -> Result<String> {
    let mut entropy = [0u8; 16];
    use bip39::rand::RngCore;
    bip39::rand::thread_rng().fill_bytes(&mut entropy);
    let m = Mnemonic::from_entropy(&entropy).context("failed to build mnemonic from entropy")?;
    Ok(m.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known test vector: Phantom/Solflare compatible derivation.
    /// Mnemonic from BIP39 spec tests; address verified against multiple implementations.
    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// Regression-pinned output for our SLIP-0010 ed25519 derivation at m/44'/501'/0'/0'
    /// from the canonical BIP39 all-zeros-entropy test mnemonic.
    ///
    /// Cross-validated 2026-04-17 against `ed25519-hd-key` + `@solana/web3.js`:
    /// both produce `HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk`, confirming
    /// our derivation is compatible with Phantom / Solflare.
    #[test]
    fn test_derivation_vector_pinned() {
        let kp = Keypair::from_mnemonic(TEST_MNEMONIC).unwrap();
        assert_eq!(
            kp.address(),
            "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk"
        );
    }

    #[test]
    fn test_address_is_base58_and_32_bytes() {
        let mnemonic = generate_mnemonic_12().unwrap();
        let kp = Keypair::from_mnemonic(&mnemonic).unwrap();
        let decoded = bs58::decode(&kp.address()).into_vec().unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn test_deterministic() {
        let kp1 = Keypair::from_mnemonic(TEST_MNEMONIC).unwrap();
        let kp2 = Keypair::from_mnemonic(TEST_MNEMONIC).unwrap();
        assert_eq!(kp1.address(), kp2.address());
        assert_eq!(
            kp1.signing_key.to_bytes(),
            kp2.signing_key.to_bytes()
        );
    }

    #[test]
    fn test_generated_mnemonic_has_12_words() {
        let m = generate_mnemonic_12().unwrap();
        assert_eq!(m.split_whitespace().count(), 12);
    }

    #[test]
    fn test_rejects_invalid_mnemonic() {
        assert!(Keypair::from_mnemonic("not a real phrase").is_err());
    }
}
