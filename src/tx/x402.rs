//! x402-path SPL token transfer message builder.
//!
//! Intentionally emits the **plain SPL Token `Transfer`** (discriminator = 3)
//! rather than `TransferChecked` (discriminator = 12), because Woody's
//! reference x402 server at `pay-in-usdc/server.ts` only recognizes type-3
//! transfers:
//!
//! ```ts
//! if (ix.data.length >= 9 && ix.data[0] === 3) {
//!   transferAmount = Number(ix.data.readBigUInt64LE(1));
//!   ...
//! }
//! ```
//!
//! Regular `solw token send` keeps `TransferChecked` — this module is *only*
//! reachable from `solw pay`.
use anyhow::Result;

use crate::pda::{ASSOCIATED_TOKEN_PROGRAM_ID_B58, TOKEN_PROGRAM_ID_B58};
use crate::tx::{compact_u16, decode_base58_pubkey, SYSTEM_PROGRAM_ID};

/// SPL Token plain `Transfer` discriminator.
const TRANSFER_IX: u8 = 3;

/// Associated Token Account `CreateIdempotent` discriminator.
const CREATE_ATA_IDEMPOTENT_IX: u8 = 1;

pub struct X402TransferParams<'a> {
    pub payer: &'a [u8; 32],
    pub source_ata: &'a [u8; 32],
    pub dest_ata: &'a [u8; 32],
    pub dest_owner: &'a [u8; 32],
    pub mint: &'a [u8; 32],
    pub amount_raw: u64,
    pub create_dest_ata: bool,
    pub recent_blockhash: &'a [u8; 32],
}

/// Build the unsigned message bytes for a plain-SPL-Transfer payment, with an
/// optional `CreateAssociatedTokenAccountIdempotent` preamble when the
/// recipient ATA doesn't yet exist.
///
/// Layouts
/// -------
/// Without ATA create (4 accounts, 1 ix, 180 bytes):
///   slots: [payer(signer,w), dest_ata(w), source_ata(w), token_pg(ro)]
///   header: (1, 0, 1)
///   ix: Transfer{ program_idx=3, accounts=[2,1,0], data=[3 | amount_le_u64] }
///
/// With ATA create (8 accounts, 2 ix, 318 bytes):
///   slots: [payer(signer,w), dest_ata(w), source_ata(w), dest_owner(ro),
///           mint(ro), token_pg(ro), system_pg(ro), ata_pg(ro)]
///   header: (1, 0, 5)
///   ix[0]: CreateAtaIdempotent{ program_idx=7, accounts=[0,1,3,4,6,5], data=[1] }
///   ix[1]: Transfer{ program_idx=5, accounts=[2,1,0], data=[3 | amount_le_u64] }
///
/// Instruction account order `[source, dest, owner]` matches Woody's server
/// check `ix.keys[1].pubkey === RECIPIENT_TOKEN_ACCOUNT`.
pub fn build_x402_transfer_message(p: &X402TransferParams) -> Result<Vec<u8>> {
    let token_program = decode_base58_pubkey(TOKEN_PROGRAM_ID_B58)?;
    if p.create_dest_ata {
        let ata_program = decode_base58_pubkey(ASSOCIATED_TOKEN_PROGRAM_ID_B58)?;
        Ok(build_with_create_ata(p, &token_program, &ata_program))
    } else {
        Ok(build_transfer_only(p, &token_program))
    }
}

fn build_transfer_only(p: &X402TransferParams, token_program: &[u8; 32]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(200);

    // Header: 1 signer, 0 readonly-signed, 1 readonly-unsigned (token_program).
    msg.push(1);
    msg.push(0);
    msg.push(1);

    // Account count (4).
    msg.extend(compact_u16(4));
    msg.extend_from_slice(p.payer);
    msg.extend_from_slice(p.dest_ata);
    msg.extend_from_slice(p.source_ata);
    msg.extend_from_slice(token_program);

    // Blockhash.
    msg.extend_from_slice(p.recent_blockhash);

    // 1 instruction.
    msg.extend(compact_u16(1));

    // Transfer: program_id=token_program (idx 3); accounts [source(2), dest(1), owner(0)].
    msg.push(3);
    msg.extend(compact_u16(3));
    msg.extend_from_slice(&[2u8, 1, 0]);
    let mut data = Vec::with_capacity(9);
    data.push(TRANSFER_IX);
    data.extend_from_slice(&p.amount_raw.to_le_bytes());
    msg.extend(compact_u16(data.len() as u16));
    msg.extend_from_slice(&data);

    msg
}

fn build_with_create_ata(
    p: &X402TransferParams,
    token_program: &[u8; 32],
    ata_program: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(400);

    // Header: 1 signer, 0 readonly-signed, 5 readonly-unsigned
    // (dest_owner, mint, token_pg, sys_pg, ata_pg).
    msg.push(1);
    msg.push(0);
    msg.push(5);

    // Account count (8).
    msg.extend(compact_u16(8));
    msg.extend_from_slice(p.payer);
    msg.extend_from_slice(p.dest_ata);
    msg.extend_from_slice(p.source_ata);
    msg.extend_from_slice(p.dest_owner);
    msg.extend_from_slice(p.mint);
    msg.extend_from_slice(token_program);
    msg.extend_from_slice(&SYSTEM_PROGRAM_ID);
    msg.extend_from_slice(ata_program);

    // Blockhash.
    msg.extend_from_slice(p.recent_blockhash);

    // 2 instructions.
    msg.extend(compact_u16(2));

    // CreateAtaIdempotent: program_id=ata_program (idx 7);
    // accounts [payer(0), dest_ata(1), dest_owner(3), mint(4), system_pg(6), token_pg(5)].
    msg.push(7);
    msg.extend(compact_u16(6));
    msg.extend_from_slice(&[0u8, 1, 3, 4, 6, 5]);
    msg.extend(compact_u16(1));
    msg.push(CREATE_ATA_IDEMPOTENT_IX);

    // Transfer: program_id=token_program (idx 5); accounts [source(2), dest(1), owner(0)].
    msg.push(5);
    msg.extend(compact_u16(3));
    msg.extend_from_slice(&[2u8, 1, 0]);
    let mut data = Vec::with_capacity(9);
    data.push(TRANSFER_IX);
    data.extend_from_slice(&p.amount_raw.to_le_bytes());
    msg.extend(compact_u16(data.len() as u16));
    msg.extend_from_slice(&data);

    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_params() -> X402TransferParams<'static> {
        static PAYER: [u8; 32] = [1u8; 32];
        static SRC: [u8; 32] = [2u8; 32];
        static DST: [u8; 32] = [3u8; 32];
        static OWN: [u8; 32] = [4u8; 32];
        static MINT: [u8; 32] = [5u8; 32];
        static BH: [u8; 32] = [6u8; 32];
        X402TransferParams {
            payer: &PAYER,
            source_ata: &SRC,
            dest_ata: &DST,
            dest_owner: &OWN,
            mint: &MINT,
            amount_raw: 100,
            create_dest_ata: false,
            recent_blockhash: &BH,
        }
    }

    #[test]
    fn transfer_only_header_and_key_count() {
        let p = dummy_params();
        let msg = build_x402_transfer_message(&p).unwrap();
        assert_eq!(&msg[0..3], &[1, 0, 1]);
        assert_eq!(msg[3], 4, "account count");
    }

    #[test]
    fn transfer_only_key_layout() {
        let p = dummy_params();
        let msg = build_x402_transfer_message(&p).unwrap();
        // Accounts are 32-byte each starting at offset 4.
        assert_eq!(&msg[4..36], p.payer);
        assert_eq!(&msg[36..68], p.dest_ata);
        assert_eq!(&msg[68..100], p.source_ata);
        // Token program at slot 3.
        let token_program = decode_base58_pubkey(TOKEN_PROGRAM_ID_B58).unwrap();
        assert_eq!(&msg[100..132], &token_program);
        // Blockhash follows immediately.
        assert_eq!(&msg[132..164], p.recent_blockhash);
    }

    #[test]
    fn transfer_only_instruction_encoding() {
        let p = dummy_params();
        let msg = build_x402_transfer_message(&p).unwrap();
        // After 3 (header) + 1 (acct count) + 4*32 (accounts) + 32 (blockhash) = 164
        // msg[164] = instruction count (1)
        assert_eq!(msg[164], 1);
        assert_eq!(msg[165], 3, "program idx = token_program (slot 3)");
        assert_eq!(msg[166], 3, "ix account count");
        assert_eq!(&msg[167..170], &[2u8, 1, 0]);
        assert_eq!(msg[170], 9, "data length");
        assert_eq!(msg[171], TRANSFER_IX);
        let amount = u64::from_le_bytes(msg[172..180].try_into().unwrap());
        assert_eq!(amount, 100);
        // Total length is fixed — any future drift in layout trips this check.
        assert_eq!(msg.len(), 180);
    }

    #[test]
    fn create_ata_path_header_and_key_count() {
        let mut p = dummy_params();
        p.create_dest_ata = true;
        let msg = build_x402_transfer_message(&p).unwrap();
        assert_eq!(&msg[0..3], &[1, 0, 5]);
        assert_eq!(msg[3], 8, "account count with ATA create");
    }

    #[test]
    fn create_ata_path_has_two_instructions_transfer_last() {
        let mut p = dummy_params();
        p.create_dest_ata = true;
        let msg = build_x402_transfer_message(&p).unwrap();
        // 3 (header) + 1 (acct count) + 8*32 (accounts) + 32 (blockhash) = 292
        assert_eq!(msg[292], 2, "two instructions");
        // Last 10 bytes: [data_len=9, disc=3, amount_le_u64]
        let tail = &msg[msg.len() - 10..];
        assert_eq!(tail[0], 9);
        assert_eq!(tail[1], TRANSFER_IX);
        let amount = u64::from_le_bytes(tail[2..10].try_into().unwrap());
        assert_eq!(amount, 100);
        // Overall length is also pinned.
        // 3 (header) + 1 (acct count) + 8*32 (accounts) + 32 (blockhash) +
        // 1 (ix count) + 10 (CreateAta ix: 1+1+6+1+1) + 15 (Transfer ix:
        // 1+1+3+1+9) = 318.
        assert_eq!(msg.len(), 318);
    }

    #[test]
    fn create_ata_path_ata_ix_accounts() {
        let mut p = dummy_params();
        p.create_dest_ata = true;
        let msg = build_x402_transfer_message(&p).unwrap();
        // First ix starts at msg[293]:
        //   [program_idx=7][acct_count=6][0,1,3,4,6,5][data_len=1][disc=1]
        assert_eq!(msg[293], 7, "CreateATA program_idx = ata_program (slot 7)");
        assert_eq!(msg[294], 6);
        assert_eq!(&msg[295..301], &[0u8, 1, 3, 4, 6, 5]);
        assert_eq!(msg[301], 1, "data length");
        assert_eq!(msg[302], CREATE_ATA_IDEMPOTENT_IX);
    }

    #[test]
    fn transfer_amount_round_trip_u64_max() {
        let mut p = dummy_params();
        p.amount_raw = u64::MAX;
        let msg = build_x402_transfer_message(&p).unwrap();
        let amount = u64::from_le_bytes(msg[172..180].try_into().unwrap());
        assert_eq!(amount, u64::MAX);
    }

    /// Pinned-vector sanity: the full 180-byte transfer-only message is
    /// deterministic for fixed inputs. Any silent change to the wire format
    /// fails this check. Computed from the documented layout.
    #[test]
    fn transfer_only_pinned_bytes() {
        let p = dummy_params();
        let msg = build_x402_transfer_message(&p).unwrap();
        let token_program = decode_base58_pubkey(TOKEN_PROGRAM_ID_B58).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(&[1u8, 0, 1]); // header
        expected.push(4); // account count
        expected.extend_from_slice(p.payer);
        expected.extend_from_slice(p.dest_ata);
        expected.extend_from_slice(p.source_ata);
        expected.extend_from_slice(&token_program);
        expected.extend_from_slice(p.recent_blockhash);
        expected.push(1); // instruction count
        expected.push(3); // program_idx
        expected.push(3); // ix account count
        expected.extend_from_slice(&[2u8, 1, 0]);
        expected.push(9); // data length
        expected.push(TRANSFER_IX);
        expected.extend_from_slice(&100u64.to_le_bytes());
        assert_eq!(msg, expected);
    }
}
