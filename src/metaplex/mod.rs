//! Metaplex Token Metadata: PDA derivation + minimal binary decoder.
//!
//! Only the three human-readable fields (`name`, `symbol`, `uri`) are decoded;
//! the rest of the account (seller_fee_bps, creators, flags, collection, etc.)
//! is ignored. We avoid the `borsh` crate on purpose — the project hand-rolls
//! the rest of the Solana wire format, and three length-prefixed strings do
//! not justify a new dependency.
use anyhow::{anyhow, bail, Result};

use crate::pda::find_program_address;
use crate::tx::decode_base58_pubkey;

pub const METAPLEX_METADATA_PROGRAM_ID_B58: &str = "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s";

/// Discriminator byte for a v1 Metadata account.
const METADATA_KEY_V1: u8 = 4;

/// Derive the Metaplex Token Metadata PDA for a mint.
pub fn metadata_pda(mint_b58: &str) -> Result<([u8; 32], u8)> {
    let mint = decode_base58_pubkey(mint_b58)?;
    let program = decode_base58_pubkey(METAPLEX_METADATA_PROGRAM_ID_B58)?;
    find_program_address(&[b"metadata", &program, &mint], &program)
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenMetadata {
    pub name: String,
    pub symbol: String,
    pub uri: String,
}

/// Decode the name/symbol/uri from a raw Metaplex Metadata account.
///
/// Layout (only the prefix we care about):
///   0      key: u8                          (must be 4)
///   1..33  update_authority: [u8; 32]
///   33..65 mint: [u8; 32]
///   65..   name:   u32 LE length + bytes
///   ..     symbol: u32 LE length + bytes
///   ..     uri:    u32 LE length + bytes
///
/// Metaplex writes fixed-length reserved regions (name=32, symbol=10, uri=200
/// on most tokens) and pads the unused suffix with `\0`. We trim trailing nulls
/// from each decoded string.
pub fn parse_metadata_account(raw: &[u8]) -> Result<TokenMetadata> {
    let mut cur = 0usize;
    let key = *raw
        .first()
        .ok_or_else(|| anyhow!("metadata account is empty"))?;
    if key != METADATA_KEY_V1 {
        bail!(
            "not a Metaplex v1 Metadata account (discriminator byte = {}, expected {})",
            key,
            METADATA_KEY_V1
        );
    }
    cur += 1;
    // skip update_authority (32) + mint (32)
    cur = cur
        .checked_add(64)
        .ok_or_else(|| anyhow!("offset overflow"))?;
    if cur > raw.len() {
        bail!("metadata account truncated before name");
    }

    let (name, cur) = read_lp_string(raw, cur, "name")?;
    let (symbol, cur) = read_lp_string(raw, cur, "symbol")?;
    let (uri, _) = read_lp_string(raw, cur, "uri")?;

    Ok(TokenMetadata { name, symbol, uri })
}

fn read_lp_string(raw: &[u8], cur: usize, field: &str) -> Result<(String, usize)> {
    let end = cur
        .checked_add(4)
        .ok_or_else(|| anyhow!("offset overflow reading '{}' length", field))?;
    if end > raw.len() {
        bail!("metadata account truncated reading '{}' length", field);
    }
    let len_bytes: [u8; 4] = raw[cur..end]
        .try_into()
        .map_err(|_| anyhow!("internal: failed to take 4 length bytes for '{}'", field))?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    let data_end = end
        .checked_add(len)
        .ok_or_else(|| anyhow!("offset overflow reading '{}' bytes", field))?;
    if data_end > raw.len() {
        bail!(
            "metadata account truncated reading '{}' (len={}, remaining={})",
            field,
            len,
            raw.len().saturating_sub(end)
        );
    }
    let bytes = &raw[end..data_end];
    let trimmed = trim_trailing_nuls(bytes);
    let s = std::str::from_utf8(trimmed)
        .map_err(|e| anyhow!("'{}' is not valid UTF-8: {}", field, e))?
        .to_string();
    Ok((s, data_end))
}

fn trim_trailing_nuls(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lp_string_fixed(text: &str, reserved: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + reserved);
        out.extend_from_slice(&(reserved as u32).to_le_bytes());
        let bytes = text.as_bytes();
        assert!(bytes.len() <= reserved, "test fixture too long");
        out.extend_from_slice(bytes);
        out.extend(std::iter::repeat_n(0u8, reserved - bytes.len()));
        out
    }

    fn build_fixture(name: &str, symbol: &str, uri: &str) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.push(METADATA_KEY_V1);
        raw.extend(std::iter::repeat_n(0u8, 32)); // update_authority
        raw.extend(std::iter::repeat_n(0u8, 32)); // mint
        raw.extend(lp_string_fixed(name, 32));
        raw.extend(lp_string_fixed(symbol, 10));
        raw.extend(lp_string_fixed(uri, 200));
        // trailing fields (seller_fee_bps etc.) — not decoded; can omit
        raw
    }

    #[test]
    fn parse_metadata_round_trip() {
        let raw = build_fixture("USD Coin", "USDC", "https://example.com/usdc.json");
        let m = parse_metadata_account(&raw).unwrap();
        assert_eq!(m.name, "USD Coin");
        assert_eq!(m.symbol, "USDC");
        assert_eq!(m.uri, "https://example.com/usdc.json");
    }

    #[test]
    fn parse_metadata_strips_trailing_nuls_even_if_len_is_reserved_capacity() {
        // Some tokens encode length = reserved (32/10/200) rather than actual text len;
        // the padding null bytes then leak into the string unless we trim them.
        let raw = build_fixture("FLUF", "FLUF", "x");
        let m = parse_metadata_account(&raw).unwrap();
        assert_eq!(m.name, "FLUF");
        assert_eq!(m.symbol, "FLUF");
        assert_eq!(m.uri, "x");
    }

    #[test]
    fn parse_metadata_wrong_key_rejected() {
        let mut raw = build_fixture("x", "y", "z");
        raw[0] = 0; // not a Metadata account
        let err = parse_metadata_account(&raw).unwrap_err();
        assert!(err.to_string().contains("Metaplex v1"), "got: {}", err);
    }

    #[test]
    fn parse_metadata_empty_rejected() {
        assert!(parse_metadata_account(&[]).is_err());
    }

    #[test]
    fn parse_metadata_truncated_before_name_rejected() {
        // key + update_authority + mint, then nothing.
        let mut raw = Vec::new();
        raw.push(METADATA_KEY_V1);
        raw.extend(std::iter::repeat_n(0u8, 64));
        let err = parse_metadata_account(&raw).unwrap_err();
        assert!(err.to_string().contains("truncated"), "got: {}", err);
    }

    #[test]
    fn parse_metadata_truncated_mid_string_rejected() {
        // Declare a 100-byte name length but only provide 10 bytes of data.
        let mut raw = Vec::new();
        raw.push(METADATA_KEY_V1);
        raw.extend(std::iter::repeat_n(0u8, 64));
        raw.extend(&(100u32).to_le_bytes());
        raw.extend(std::iter::repeat_n(b'A', 10));
        let err = parse_metadata_account(&raw).unwrap_err();
        assert!(err.to_string().contains("truncated"), "got: {}", err);
    }

    #[test]
    fn metadata_pda_deterministic() {
        let mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mainnet
        let (pda1, _) = metadata_pda(mint).unwrap();
        let (pda2, _) = metadata_pda(mint).unwrap();
        assert_eq!(pda1, pda2);
        assert!(!crate::pda::is_on_curve(&pda1));
    }

    #[test]
    fn trim_trailing_nuls_behaviour() {
        assert_eq!(trim_trailing_nuls(b"abc\0\0\0"), b"abc");
        assert_eq!(trim_trailing_nuls(b"abc"), b"abc");
        assert_eq!(trim_trailing_nuls(b"\0\0"), b"");
        assert_eq!(trim_trailing_nuls(b""), b"");
    }
}
