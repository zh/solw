# solw

Agent-friendly Solana CLI wallet: SOL / SPL / NFT transfers, Jupiter swaps, Metaplex metadata, x402 HTTP-402 pay — JSON output, no `solana-sdk`.

```bash
$ solw balance
  SOL:     1.234567890
  USDC:    100.5
  JUP:     42.0

$ solw swap quote SOL USDC 0.01
  in:       0.01 (1000000 raw)
  out:      2.341 (2341000 raw)
  impact:   0.01%
  route:    Raydium > Orca

$ solw swap execute SOL USDC 0.01 --confirmed --json

$ solw pay http://localhost:3001/premium --inspect           # dry-run: fetch quote, build unsigned tx, exit
$ solw pay http://localhost:3001/premium --confirmed --json  # fetch, sign, submit, print premium payload
```

## Highlights

- **Multi-wallet** — create, import, and switch between named wallets with BIP39 seed phrases (SLIP-0010 derivation `m/44'/501'/0'/0'`, compatible with Phantom / Solflare).
- **SOL / SPL tokens / NFTs** — send with UI-unit amounts, auto-create recipient ATAs, decode Metaplex name + symbol.
- **Jupiter swaps** — quote (safe, no wallet needed) and execute (mainnet-only) via `lite-api.jup.ag/swap/v1`.
- **x402 HTTP-402 pay** — `solw pay <URL>` fetches a USDC quote from any x402-speaking server, verifies the recipient ATA is canonical, enforces a spend cap, signs locally, re-submits with `X-Payment`, and surfaces the paid response. `--inspect` prints the quote + unsigned tx without signing.
- **Devnet / testnet airdrop** — request faucet SOL straight from the CLI.
- **Networks** — mainnet, devnet, testnet; network stored per wallet, overridable per-call with `--network`.
- **Custom RPCs** — per-network env-var overrides (Alchemy, Helius, QuickNode, …).
- **Agent-friendly** — `--json` on every read command, `--confirmed` on every value-moving command, stable exit codes (`0` ok, `1` pre-submit error, `2` unconfirmed submission).
- **No `solana-sdk`** — wire format (compact-u16 shortvec, System Program `Transfer`, SPL `TransferChecked`, `CreateAssociatedTokenAccountIdempotent`) and PDA / ATA derivation are hand-rolled and cross-validated against `@solana/spl-token` and `@solana/web3.js` via pinned test vectors.
- **Local key storage** — plaintext seed phrases with `0600` file permissions; private keys never leave disk.

## Install

```bash
cargo install --path .
```

Requires Rust 1.70+.

## Quick Start

```bash
# Create a wallet (devnet is safe for trying things out)
solw wallet create mywallet --network devnet

# Fund it from the faucet
solw airdrop 1

# Check balance
solw balance

# Send some SOL
solw send <recipient> 0.01

# List SPL tokens you hold
solw token list

# Quote a Jupiter swap (read-only)
solw swap quote SOL USDC 0.001
```

## Command Reference

See [USAGE.md](USAGE.md) for the full command catalog, flag reference, custom-RPC setup, JSON contracts, security notes, and storage layout.

## Using solw with AI Agents

`solw` is designed to be driven by AI agents — every read command has `--json`, every value-moving command has `--confirmed` and emits structured JSON with a stable `signature` / `confirmed` / `confirm_error` contract.

A ready-to-install agent skill lives at [`skills/solw/SKILL.md`](skills/solw/SKILL.md). Point your agent framework at it (Claude Code, Cursor, etc.) and the agent will know the command surface, the approval-gate policy for fund-spending operations, and the JSON fields it can rely on.

## Security

- Seed phrases are stored plaintext with `0600` perms at `~/.solw/wallets/<name>` (override with `SOLW_HOME`). Same model as the Solana CLI's file-system wallets.
- All value-moving commands (`send`, `send-all`, `token send`, `nft send`, `swap execute`, `pay`) require interactive confirmation unless `--confirmed` is passed.
- `swap execute` is hard-restricted to mainnet — Jupiter only routes mainnet liquidity.
- All built-in cluster endpoints are HTTPS-only.
- Swap transactions returned by Jupiter are verified locally (single required signer = the user's pubkey, transfers touch both input and output mints, not a versioned-v0 transaction) before being signed and submitted.
- `solw pay` never trusts the x402 server's advertised token account: the destination ATA is re-derived from `(recipientWallet, mint)` and a mismatch aborts before signing. A raw-unit `--max-price` cap (default `0.01` USDC) and a cluster-vs-wallet-network check run before any RPC lookup.

## Storage Layout

```
~/.solw/                       # 0700
├── default                    # name of default wallet
└── wallets/                   # 0700
    ├── <name>                 # BIP39 mnemonic (0600)
    ├── <name>.pub             # base58 pubkey cache
    └── <name>.net             # network: mainnet|devnet|testnet
```

## Dependencies

Deliberately avoids the heavyweight `solana-sdk` crate. Core crates: `clap`, `tokio`, `reqwest`, `ed25519-dalek` (+ `ed25519-dalek-bip32`), `bip39`, `bs58`, `sha2`, `curve25519-dalek`, `zeroize`, `inquire`, `qrcode`, `owo-colors`.

Jupiter swaps go through the v1 lite-api at https://lite-api.jup.ag/swap/v1 — `/quote` for route pricing, `/swap` for the unsigned legacy transaction, which `solw` then signs locally with the wallet's ed25519 key and submits via standard JSON-RPC `sendTransaction`.

## License

MIT
