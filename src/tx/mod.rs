use anyhow::{anyhow, Result};
use ed25519_dalek::{Signer, SigningKey};

pub mod token;

pub const SYSTEM_PROGRAM_ID: [u8; 32] = [0u8; 32];

pub fn compact_u16(mut n: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(3);
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            v.push(b);
            return v;
        }
        b |= 0x80;
        v.push(b);
    }
}

/// Inverse of `compact_u16`: parse a 1–3 byte shortvec length prefix.
/// Returns `(value, bytes_consumed)`.
pub fn parse_compact_u16(bytes: &[u8]) -> Result<(u16, usize)> {
    let mut n: u32 = 0;
    for i in 0..3 {
        let b = *bytes
            .get(i)
            .ok_or_else(|| anyhow!("compact-u16 truncated at byte {}", i))?;
        n |= ((b & 0x7f) as u32) << (i * 7);
        if b & 0x80 == 0 {
            if n > u16::MAX as u32 {
                return Err(anyhow!("compact-u16 overflows u16: {}", n));
            }
            return Ok((n as u16, i + 1));
        }
    }
    Err(anyhow!("compact-u16 exceeds 3 bytes"))
}

pub fn decode_base58_pubkey(s: &str) -> Result<[u8; 32]> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("invalid base58 address '{}': {}", s, e))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "invalid address '{}': expected 32 bytes, got {}",
            s,
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn decode_base58_blockhash(s: &str) -> Result<[u8; 32]> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| anyhow!("invalid base58 blockhash '{}': {}", s, e))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "invalid blockhash '{}': expected 32 bytes, got {}",
            s,
            bytes.len()
        ));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn build_transfer_message(
    from_pubkey: &[u8; 32],
    to_pubkey: &[u8; 32],
    lamports: u64,
    recent_blockhash: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(200);
    // Header: 1 required signature, 0 readonly signed, 1 readonly unsigned (system program)
    msg.push(1);
    msg.push(0);
    msg.push(1);
    // Account keys: [from (signer+writable), to (writable), system_program (readonly)]
    msg.extend(compact_u16(3));
    msg.extend_from_slice(from_pubkey);
    msg.extend_from_slice(to_pubkey);
    msg.extend_from_slice(&SYSTEM_PROGRAM_ID);
    // Recent blockhash
    msg.extend_from_slice(recent_blockhash);
    // Instructions: 1
    msg.extend(compact_u16(1));
    // Instruction: program_id_index=2 (system program)
    msg.push(2);
    // Account indices for instruction: [0 (from), 1 (to)]
    msg.extend(compact_u16(2));
    msg.push(0);
    msg.push(1);
    // Data: 4-byte transfer discriminator (2 = Transfer, LE) + 8-byte lamports (LE)
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    msg.extend(compact_u16(data.len() as u16));
    msg.extend_from_slice(&data);
    msg
}

pub fn sign_and_serialize(signing_key: &SigningKey, message: &[u8]) -> Vec<u8> {
    let sig = signing_key.sign(message);
    let mut tx = Vec::with_capacity(1 + 64 + message.len());
    tx.extend(compact_u16(1));
    tx.extend_from_slice(&sig.to_bytes());
    tx.extend_from_slice(message);
    tx
}

/// Safety-check a Jupiter-returned legacy transaction before we sign it.
///
/// Confirms the payload matches the swap we asked for: single signer, user is
/// the fee payer, both mints appear in the account keys, not a versioned
/// transaction. A malicious Jupiter response that drained a different wallet
/// or swapped a different mint would fail one of these checks.
///
/// Scope: legacy v0 transactions only. Versioned transactions (top bit of
/// msg[0] set) are rejected — we don't follow ALT references, so we can't
/// prove the full account set.
pub fn verify_swap_transaction(
    raw_tx: &[u8],
    user_pubkey: &[u8; 32],
    input_mint: &[u8; 32],
    output_mint: &[u8; 32],
) -> Result<()> {
    let (num_sigs, consumed) = parse_compact_u16(raw_tx)?;
    if num_sigs != 1 {
        return Err(anyhow!("expected 1 signature slot, got {}", num_sigs));
    }
    let sig_region_end = consumed
        .checked_add(64)
        .ok_or_else(|| anyhow!("signature region overflow"))?;
    let msg = raw_tx
        .get(sig_region_end..)
        .ok_or_else(|| anyhow!("prebuilt transaction missing message"))?;
    let header_byte = *msg
        .first()
        .ok_or_else(|| anyhow!("message is empty"))?;
    if header_byte & 0x80 != 0 {
        return Err(anyhow!(
            "versioned transactions not supported; expected legacy v0"
        ));
    }
    let num_required_sigs = header_byte;
    if num_required_sigs != 1 {
        return Err(anyhow!(
            "expected 1 required signature, got {}",
            num_required_sigs
        ));
    }
    if msg.len() < 3 {
        return Err(anyhow!("message truncated in header"));
    }
    let mut cur = 3usize;
    let (num_keys, consumed) = parse_compact_u16(&msg[cur..])?;
    cur = cur
        .checked_add(consumed)
        .ok_or_else(|| anyhow!("offset overflow after key count"))?;
    let keys_end = cur
        .checked_add((num_keys as usize) * 32)
        .ok_or_else(|| anyhow!("keys region overflow"))?;
    if keys_end > msg.len() {
        return Err(anyhow!(
            "message truncated in account keys ({} < {})",
            msg.len(),
            keys_end
        ));
    }
    let first_key = msg
        .get(cur..cur + 32)
        .ok_or_else(|| anyhow!("missing fee-payer key"))?;
    if first_key != user_pubkey {
        return Err(anyhow!(
            "fee payer mismatch: tx fee-payer is not the wallet pubkey (tampered swap response?)"
        ));
    }
    let mut has_input = false;
    let mut has_output = false;
    for i in 0..(num_keys as usize) {
        let start = cur + i * 32;
        let key: &[u8] = &msg[start..start + 32];
        if key == input_mint {
            has_input = true;
        }
        if key == output_mint {
            has_output = true;
        }
    }
    if !has_input {
        return Err(anyhow!(
            "swap tx does not reference the input mint we asked for"
        ));
    }
    if !has_output {
        return Err(anyhow!(
            "swap tx does not reference the output mint we asked for"
        ));
    }
    Ok(())
}

/// Sign a pre-built serialized transaction (as returned by Jupiter /swap) in place.
///
/// Layout: `[compact_u16(num_sigs) | 64 * num_sigs signature bytes | message bytes]`.
/// We sign the message slice with `signing_key` and write the 64-byte signature
/// into slot 0, overwriting the zero-filled placeholder Jupiter supplies.
///
/// This assumes the fee payer is signer index 0 (Jupiter's default when
/// `userPublicKey` is the single signer). Multi-sig payloads are out of scope.
pub fn sign_prebuilt_transaction(signing_key: &SigningKey, raw_tx: &mut [u8]) -> Result<()> {
    let (num_sigs, consumed) = parse_compact_u16(raw_tx)?;
    if num_sigs == 0 {
        return Err(anyhow!("prebuilt transaction has zero signature slots"));
    }
    let sig_region_end = consumed
        .checked_add((num_sigs as usize) * 64)
        .ok_or_else(|| anyhow!("signature region overflow"))?;
    if raw_tx.len() < sig_region_end {
        return Err(anyhow!(
            "prebuilt transaction shorter than signature region ({}<{})",
            raw_tx.len(),
            sig_region_end
        ));
    }
    let sig = signing_key.sign(&raw_tx[sig_region_end..]);
    raw_tx[consumed..consumed + 64].copy_from_slice(&sig.to_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_u16_small() {
        assert_eq!(compact_u16(0), vec![0]);
        assert_eq!(compact_u16(1), vec![1]);
        assert_eq!(compact_u16(127), vec![127]);
    }

    #[test]
    fn compact_u16_medium() {
        assert_eq!(compact_u16(128), vec![0x80, 0x01]);
        assert_eq!(compact_u16(255), vec![0xff, 0x01]);
        assert_eq!(compact_u16(16383), vec![0xff, 0x7f]);
    }

    #[test]
    fn compact_u16_large() {
        assert_eq!(compact_u16(16384), vec![0x80, 0x80, 0x01]);
        assert_eq!(compact_u16(65535), vec![0xff, 0xff, 0x03]);
    }

    #[test]
    fn decode_valid_base58_address() {
        let addr = "11111111111111111111111111111111";
        let bytes = decode_base58_pubkey(addr).unwrap();
        assert_eq!(bytes, [0u8; 32]);
    }

    #[test]
    fn decode_invalid_base58_address() {
        assert!(decode_base58_pubkey("not-base58-0OIl").is_err());
    }

    #[test]
    fn transfer_message_structure() {
        let from = [1u8; 32];
        let to = [2u8; 32];
        let blockhash = [3u8; 32];
        let msg = build_transfer_message(&from, &to, 1_000_000_000, &blockhash);

        // Header (3 bytes)
        assert_eq!(&msg[0..3], &[1, 0, 1]);
        // Account count (1 byte: 3)
        assert_eq!(msg[3], 3);
        // From pubkey (32 bytes)
        assert_eq!(&msg[4..36], &from);
        // To pubkey (32 bytes)
        assert_eq!(&msg[36..68], &to);
        // System program (32 bytes, all zero)
        assert_eq!(&msg[68..100], &[0u8; 32]);
        // Blockhash (32 bytes)
        assert_eq!(&msg[100..132], &blockhash);
        // Instruction count (1 byte: 1)
        assert_eq!(msg[132], 1);
        // Instruction: program_id_index=2
        assert_eq!(msg[133], 2);
        // Account count for instruction (2)
        assert_eq!(msg[134], 2);
        // Account indices [0, 1]
        assert_eq!(&msg[135..137], &[0, 1]);
        // Data length (12)
        assert_eq!(msg[137], 12);
        // Data: transfer discriminator (2u32 LE)
        assert_eq!(&msg[138..142], &[2, 0, 0, 0]);
        // Data: lamports (1_000_000_000 LE)
        assert_eq!(&msg[142..150], &1_000_000_000u64.to_le_bytes());
        assert_eq!(msg.len(), 150);
    }

    #[test]
    fn parse_compact_u16_roundtrips() {
        for v in [0u16, 1, 127, 128, 255, 16383, 16384, 65535] {
            let enc = compact_u16(v);
            let (parsed, consumed) = parse_compact_u16(&enc).unwrap();
            assert_eq!(parsed, v);
            assert_eq!(consumed, enc.len());
        }
    }

    #[test]
    fn parse_compact_u16_truncated() {
        assert!(parse_compact_u16(&[]).is_err());
        // 0x80 sets continuation but there's no next byte.
        assert!(parse_compact_u16(&[0x80]).is_err());
        // Three continuation bytes = overflow.
        assert!(parse_compact_u16(&[0x80, 0x80, 0x80]).is_err());
    }

    #[test]
    fn sign_prebuilt_overwrites_signature_slot() {
        use ed25519_dalek::Verifier;
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let vk = sk.verifying_key();
        // Single-signer layout: [1, 0x00*64, ...message]
        let message = vec![0xcdu8; 80];
        let mut tx = Vec::with_capacity(1 + 64 + message.len());
        tx.extend(compact_u16(1));
        tx.extend_from_slice(&[0u8; 64]);
        tx.extend_from_slice(&message);
        sign_prebuilt_transaction(&sk, &mut tx).unwrap();
        let sig_bytes: [u8; 64] = tx[1..65].try_into().unwrap();
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        assert!(vk.verify(&message, &sig).is_ok());
        assert_eq!(&tx[65..], message.as_slice(), "message must not change");
    }

    #[test]
    fn sign_prebuilt_rejects_truncated() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let mut tx = vec![1u8, 0, 0];
        assert!(sign_prebuilt_transaction(&sk, &mut tx).is_err());
    }

    #[test]
    fn sign_prebuilt_rejects_zero_sigs() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let mut tx = vec![0u8];
        tx.extend_from_slice(&[0xaa; 20]);
        assert!(sign_prebuilt_transaction(&sk, &mut tx).is_err());
    }

    /// Build a minimal legacy swap-shaped transaction for the verifier tests.
    /// Optional tweaks let a single builder produce all of the failure cases.
    fn build_swap_like_tx(
        user: &[u8; 32],
        input_mint: &[u8; 32],
        output_mint: &[u8; 32],
        num_sigs: u16,
        header_byte: u8,
        include_input: bool,
        include_output: bool,
    ) -> Vec<u8> {
        let system_program = [0u8; 32];
        let mut keys: Vec<[u8; 32]> = vec![*user];
        if include_input {
            keys.push(*input_mint);
        }
        if include_output {
            keys.push(*output_mint);
        }
        keys.push(system_program);

        let mut msg = Vec::new();
        msg.push(header_byte); // num_required_sigs
        msg.push(0); // num_readonly_signed
        msg.push(1); // num_readonly_unsigned
        msg.extend(compact_u16(keys.len() as u16));
        for k in &keys {
            msg.extend_from_slice(k);
        }
        msg.extend_from_slice(&[9u8; 32]); // blockhash
        msg.extend(compact_u16(0)); // instructions

        let mut tx = Vec::new();
        tx.extend(compact_u16(num_sigs));
        for _ in 0..num_sigs {
            tx.extend_from_slice(&[0u8; 64]);
        }
        tx.extend_from_slice(&msg);
        tx
    }

    #[test]
    fn verify_swap_tx_accepts_valid() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&user, &input, &output, 1, 1, true, true);
        assert!(verify_swap_transaction(&tx, &user, &input, &output).is_ok());
    }

    #[test]
    fn verify_swap_tx_rejects_wrong_fee_payer() {
        let user = [1u8; 32];
        let attacker = [9u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&attacker, &input, &output, 1, 1, true, true);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("fee payer mismatch"), "got: {}", err);
    }

    #[test]
    fn verify_swap_tx_rejects_missing_input_mint() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&user, &input, &output, 1, 1, false, true);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("input mint"), "got: {}", err);
    }

    #[test]
    fn verify_swap_tx_rejects_missing_output_mint() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&user, &input, &output, 1, 1, true, false);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("output mint"), "got: {}", err);
    }

    #[test]
    fn verify_swap_tx_rejects_versioned() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        // top bit set == versioned v0 prefix
        let tx = build_swap_like_tx(&user, &input, &output, 1, 0x80, true, true);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("versioned"), "got: {}", err);
    }

    #[test]
    fn verify_swap_tx_rejects_multiple_sigs() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&user, &input, &output, 2, 1, true, true);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("1 signature"), "got: {}", err);
    }

    #[test]
    fn verify_swap_tx_rejects_multiple_required_sigs() {
        let user = [1u8; 32];
        let input = [2u8; 32];
        let output = [3u8; 32];
        let tx = build_swap_like_tx(&user, &input, &output, 1, 2, true, true);
        let err = verify_swap_transaction(&tx, &user, &input, &output).unwrap_err();
        assert!(err.to_string().contains("required signature"), "got: {}", err);
    }

    #[test]
    fn sign_and_serialize_shape() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let msg = vec![0xaau8; 100];
        let tx = sign_and_serialize(&sk, &msg);
        // compact_u16(1) = [1]; signature = 64 bytes; message = 100 bytes => 165 total
        assert_eq!(tx.len(), 1 + 64 + 100);
        assert_eq!(tx[0], 1);
        // Message appended after sig
        assert_eq!(&tx[65..], msg.as_slice());
    }
}
