//! SPL Token transfer message builder.
//!
//! Supports both a plain `TransferChecked` and an optional
//! `CreateAssociatedTokenAccountIdempotent` preamble when the recipient's ATA
//! does not yet exist.
use anyhow::Result;

use crate::pda::{ASSOCIATED_TOKEN_PROGRAM_ID_B58, TOKEN_PROGRAM_ID_B58};
use crate::tx::{compact_u16, decode_base58_pubkey, SYSTEM_PROGRAM_ID};

/// SPL Token TransferChecked discriminator (u8 = 12).
const TRANSFER_CHECKED_IX: u8 = 12;

/// Associated Token Account Create Idempotent discriminator (u8 = 1).
const CREATE_ATA_IDEMPOTENT_IX: u8 = 1;

pub struct TokenTransferParams<'a> {
    pub payer: &'a [u8; 32],
    pub source_ata: &'a [u8; 32],
    pub dest_ata: &'a [u8; 32],
    pub dest_owner: &'a [u8; 32],
    pub mint: &'a [u8; 32],
    pub amount_raw: u64,
    pub decimals: u8,
    pub create_dest_ata: bool,
    pub recent_blockhash: &'a [u8; 32],
}

/// Build the unsigned message bytes for a token transfer.
///
/// Accounts layout (in order):
///   0: payer (signer, writable)            — pays fee + rent if creating ATA
///   1: dest_ata (writable)                 — recipient associated token account
///   2: source_ata (writable)               — sender's ATA
///   3: dest_owner (readonly)               — only needed if creating ATA
///   4: mint (readonly)
///   5: token_program (readonly)            — TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
///   6: system_program (readonly)           — 11111...
///   7: associated_token_program (readonly) — ATokenGPvbdGVx...
///
/// Header = (1 signer, 0 readonly signed, 5 readonly unsigned).
pub fn build_token_transfer_message(p: &TokenTransferParams) -> Result<Vec<u8>> {
    let token_program = decode_base58_pubkey(TOKEN_PROGRAM_ID_B58)?;
    let ata_program = decode_base58_pubkey(ASSOCIATED_TOKEN_PROGRAM_ID_B58)?;

    let mut msg = Vec::with_capacity(300);
    // Header
    msg.push(1); // 1 required signature
    msg.push(0); // 0 readonly signed
    msg.push(5); // 5 readonly unsigned (dest_owner, mint, token_pg, sys_pg, ata_pg)

    // Account count (8)
    msg.extend(compact_u16(8));
    msg.extend_from_slice(p.payer);
    msg.extend_from_slice(p.dest_ata);
    msg.extend_from_slice(p.source_ata);
    msg.extend_from_slice(p.dest_owner);
    msg.extend_from_slice(p.mint);
    msg.extend_from_slice(&token_program);
    msg.extend_from_slice(&SYSTEM_PROGRAM_ID);
    msg.extend_from_slice(&ata_program);

    // Blockhash
    msg.extend_from_slice(p.recent_blockhash);

    // Instructions
    let ix_count = if p.create_dest_ata { 2u16 } else { 1u16 };
    msg.extend(compact_u16(ix_count));

    // --- Optional: CreateAssociatedTokenAccountIdempotent ---
    // Accounts for CreateATA (per ATA program spec, idempotent variant):
    //   0: funding account (signer, writable) = payer (idx 0)
    //   1: associated token account (writable) = dest_ata (idx 1)
    //   2: wallet owner (readonly) = dest_owner (idx 3)
    //   3: mint (readonly) = mint (idx 4)
    //   4: system program (readonly) = sys_pg (idx 6)
    //   5: token program (readonly) = token_pg (idx 5)
    if p.create_dest_ata {
        msg.push(7); // program_id_index = associated_token_program (idx 7)
        msg.extend(compact_u16(6));
        msg.extend_from_slice(&[0u8, 1, 3, 4, 6, 5]);
        // Data: single-byte discriminator
        msg.extend(compact_u16(1));
        msg.push(CREATE_ATA_IDEMPOTENT_IX);
    }

    // --- TransferChecked ---
    // Accounts for TransferChecked (per SPL Token program):
    //   0: source (writable)   = source_ata (idx 2)
    //   1: mint (readonly)     = mint (idx 4)
    //   2: dest (writable)     = dest_ata (idx 1)
    //   3: authority (signer)  = payer (idx 0)
    msg.push(5); // program_id_index = token_program (idx 5)
    msg.extend(compact_u16(4));
    msg.extend_from_slice(&[2u8, 4, 1, 0]);
    // Data: disc (1) + amount (8 LE) + decimals (1)
    let mut data = Vec::with_capacity(10);
    data.push(TRANSFER_CHECKED_IX);
    data.extend_from_slice(&p.amount_raw.to_le_bytes());
    data.push(p.decimals);
    msg.extend(compact_u16(data.len() as u16));
    msg.extend_from_slice(&data);

    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_params() -> TokenTransferParams<'static> {
        static PAYER: [u8; 32] = [1u8; 32];
        static SRC: [u8; 32] = [2u8; 32];
        static DST: [u8; 32] = [3u8; 32];
        static OWN: [u8; 32] = [4u8; 32];
        static MINT: [u8; 32] = [5u8; 32];
        static BH: [u8; 32] = [6u8; 32];
        TokenTransferParams {
            payer: &PAYER,
            source_ata: &SRC,
            dest_ata: &DST,
            dest_owner: &OWN,
            mint: &MINT,
            amount_raw: 1_000_000,
            decimals: 6,
            create_dest_ata: false,
            recent_blockhash: &BH,
        }
    }

    #[test]
    fn transfer_only_structure() {
        let p = dummy_params();
        let msg = build_token_transfer_message(&p).unwrap();
        // Header
        assert_eq!(&msg[0..3], &[1, 0, 5]);
        // 8 accounts
        assert_eq!(msg[3], 8);
        // 1 instruction
        assert_eq!(msg[3 + 1 + 8 * 32 + 32], 1);
    }

    #[test]
    fn transfer_with_ata_create_has_two_instructions() {
        let mut p = dummy_params();
        p.create_dest_ata = true;
        let msg = build_token_transfer_message(&p).unwrap();
        assert_eq!(msg[3 + 1 + 8 * 32 + 32], 2);
    }

    #[test]
    fn transfer_data_encodes_amount_and_decimals() {
        let p = dummy_params();
        let msg = build_token_transfer_message(&p).unwrap();
        // Find TransferChecked data: it's the last 10 bytes of the message.
        let tail = &msg[msg.len() - 10..];
        assert_eq!(tail[0], TRANSFER_CHECKED_IX);
        let amount = u64::from_le_bytes(tail[1..9].try_into().unwrap());
        assert_eq!(amount, 1_000_000);
        assert_eq!(tail[9], 6);
    }

    #[test]
    fn create_ata_then_transfer_data_layout() {
        let mut p = dummy_params();
        p.create_dest_ata = true;
        let msg = build_token_transfer_message(&p).unwrap();
        // Last 10 bytes should still be TransferChecked.
        let tail = &msg[msg.len() - 10..];
        assert_eq!(tail[0], TRANSFER_CHECKED_IX);
        let amount = u64::from_le_bytes(tail[1..9].try_into().unwrap());
        assert_eq!(amount, 1_000_000);
    }
}
