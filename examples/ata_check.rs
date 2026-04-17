//! Standalone ATA derivation check. Duplicates the logic from src/pda/mod.rs so
//! we can run it as `cargo run --example ata_check` against a known devnet pair.
use curve25519_dalek::edwards::CompressedEdwardsY;
use sha2::{Digest, Sha256};

fn decode(s: &str) -> [u8; 32] {
    let v = bs58::decode(s).into_vec().unwrap();
    assert_eq!(v.len(), 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

fn is_on_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_some()
}

fn find_pda(seeds: &[&[u8]], program_id: &[u8; 32]) -> ([u8; 32], u8) {
    for bump in (0u8..=255).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program_id);
        h.update(b"ProgramDerivedAddress");
        let out: [u8; 32] = h.finalize().into();
        if !is_on_curve(&out) {
            return (out, bump);
        }
    }
    panic!("no bump");
}

fn main() {
    let owner = "E1bQJ8eMMn3zmeSewW3HQ8zmJr7KR75JonbwAtWx2bux";
    let mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
    let expected = "CFUgYpbas5UdJkwwSobYgzhFqFuj6C8MfXwBetE3o4SY";

    let owner_b = decode(owner);
    let mint_b = decode(mint);
    let token_pg = decode("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
    let ata_pg = decode("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

    let (ata, bump) = find_pda(&[&owner_b, &token_pg, &mint_b], &ata_pg);
    let ata_b58 = bs58::encode(ata).into_string();
    println!("owner:    {}", owner);
    println!("mint:     {}", mint);
    println!("expected: {}", expected);
    println!("derived:  {} (bump={})", ata_b58, bump);
    if ata_b58 != expected {
        println!("MISMATCH");
        std::process::exit(1);
    }
    println!("MATCH");
}
