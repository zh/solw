---
name: solw
description: Solana CLI wallet for balance queries, SOL/SPL/NFT transfers, Jupiter swaps, devnet airdrops, and paying x402 HTTP 402 endpoints with USDC. Use when interacting with the Solana blockchain, swapping tokens on Solana, or paying an x402-protected HTTP URL.
---

# solw — Solana CLI Wallet

Rust CLI wallet for Solana. Supports multi-wallet key management, SOL / SPL token / NFT transfers, Metaplex metadata reads, Jupiter swaps (mainnet-only), devnet/testnet faucet airdrops, and x402 HTTP 402 micropayments. Hand-rolled transaction builder — no `solana-sdk` dependency.

## Commands

### Wallet & Balance

```bash
# Create a new wallet (network is stored per-wallet; mainnet by default)
solw wallet create <name> [--network mainnet|devnet|testnet]

# Import from seed phrase (prompts for the 12 words)
solw wallet import <name> [--network mainnet|devnet|testnet]

# List wallets / show info / switch default
solw wallet list
solw wallet info
solw wallet default <name>

# Export seed phrase (prompts before printing)
solw wallet export

# Delete a wallet
solw wallet delete <name>

# Check SOL balance + all non-empty SPL token accounts
solw balance --json
solw balance --token <mint> --json     # single-token balance

# Receive address + QR code
solw receive [--no-qr]
```

### Send SOL

```bash
# Send SOL (amount in SOL, not lamports)
solw send <recipient> <amount>

# Non-interactive (for AI agents, after user approval)
solw send <recipient> <amount> --confirmed --json

# Drain wallet; reserves ~0.00091 SOL for rent-exemption
solw send-all <recipient> --confirmed --json
```

### SPL Tokens

```bash
# List non-empty token accounts
solw token list --json

# Include zero-balance ATAs (rent-reclaim workflows)
solw token list --all --json

# Mint info (decimals, supply, authorities, Metaplex name/symbol)
solw token info <mint> --json

# Send (amount in UI units; e.g. 1.5 for 1.5 USDC — decimals fetched automatically)
solw token send <mint> <recipient> <amount> --confirmed --json
```

Recipient ATA is auto-created (idempotent) if it does not exist; the sender pays the rent.

### NFTs

```bash
# List NFTs (filters accounts with decimals=0 and amount=1)
solw nft list --json

# Mint info + Metaplex name/symbol + metadata PDA
solw nft info <mint> --json

# Send an NFT (delegates to token send with amount=1)
solw nft send <mint> <recipient> --confirmed --json
```

### Jupiter Swaps (mainnet only)

```bash
# Read-only quote (no wallet required, no funds move)
solw swap quote <input> <output> <amount> [--slippage-bps 50] --json

# Execute on mainnet (wallet required)
solw swap execute <input> <output> <amount> [--slippage-bps 50] --confirmed --json
```

`<amount>` is the **input token in UI units** (e.g. `0.001` for 0.001 SOL, `1.5` for 1.5 USDC). Decimals are fetched from the mint automatically. Pass `--raw` to supply raw base units instead (integer; for agents that have already done the conversion).

Built-in aliases: `SOL` / `WSOL`, `USDC`, `USDT`, `BONK`, `JUP`. Any other string is treated as a base58 mint address.

Examples:

```bash
# Quote 0.001 SOL → USDC
solw swap quote SOL USDC 0.001 --json

# Quote 1 USDC → BONK with 1% slippage
solw swap quote USDC BONK 1 --slippage-bps 100 --json

# Execute 0.002 SOL → USDC on mainnet
solw swap execute SOL USDC 0.002 --confirmed --json

# Raw base units (back-compat path for agents)
solw swap quote SOL USDC 1000000 --raw --json
```

### Airdrop (devnet / testnet only)

```bash
solw airdrop                    # default 1 SOL
solw airdrop 0.5                # custom amount (max 2.0 SOL per call)
solw airdrop 1 --json
solw --network devnet airdrop 2
```

The public faucet is aggressively rate-limited. If it fails, fall back to:
- https://faucet.solana.com (GitHub login raises your daily cap)
- https://faucet.quicknode.com/solana/devnet (1 claim per 12h, no auth)

### Transaction History

```bash
solw history --json
solw history --limit 50 --json
```

### x402 HTTP 402 Payments

```bash
# Fetch the quote, show it, and build the unsigned tx — NO signing, NO funds move
solw pay <url> --inspect --json

# Sign + submit + retry with X-Payment header (requires --confirmed or interactive approval)
solw pay <url> --confirmed --json

# Cap the price (UI units; default 0.01)
solw pay <url> --max-price 0.001 --confirmed --json
```

`solw pay <URL>` implements the x402 "exact" scheme on Solana: GET the URL, expect a 402 with a `payment` quote, pay with USDC on-chain, then re-GET with an `X-Payment` header carrying the base64-encoded signed transaction. The server verifies the transfer and returns the premium content (HTTP 200 with `data` + `paymentDetails`).

Options:

- `--max-price <ui>` — reject the quote if it exceeds this UI-unit amount (default **0.01** USDC).
- `--inspect` — fetch the quote, build the unsigned tx, print everything, and exit **without signing or submitting**.
- `--confirmed` — skip the interactive confirm prompt (standard solw contract; only after explicit user approval).
- `--json` — machine-readable output (success envelope or error envelope).

Exit codes:

- `0` — payment accepted, content returned.
- `1` — pre-submit error (quote rejected, insufficient balance, cluster mismatch, `--max-price` exceeded, malformed 402 body).
- `2` — transaction submitted but server returned 402 on retry (on-chain failure or verification rejected).

Today verified against Woody's reference Solana x402 server (devnet USDC). Canonical x402-svm spec compatibility (VersionedTransaction v0, facilitator settlement) is a future stage.

## Decision Flow

When an agent needs to interact with Solana:

1. **Read-only query** (balance, receive, history, token info, nft info, swap quote):
   - Execute directly — no approval needed, no funds at risk
   - Use `--json` for machine-readable output

2. **Token swap**:
   - First: `solw swap quote <from> <to> <amount> --json` to check the rate
   - Inform user of the expected swap (amount in, expected out, route, price impact)
   - **Wait for explicit user approval**
   - Then: `solw swap execute <from> <to> <amount> --confirmed --json`

3. **Send SOL, SPL tokens, or NFTs**:
   - Inform user of the amount and recipient
   - **Wait for explicit user approval**
   - Then: `solw send <recipient> <amount> --confirmed --json`
         / `solw token send <mint> <recipient> <amount> --confirmed --json`
         / `solw nft send <mint> <recipient> --confirmed --json`

4. **x402 paid HTTP request**:
   - First: `solw pay <url> --inspect --json` to fetch + display the quote without paying
   - Show the user the amount, recipient, and network
   - **Wait for explicit user approval**
   - Then: `solw pay <url> --confirmed --json` (honors the `--max-price` cap)

5. **Devnet airdrop**:
   - Safe on devnet/testnet; refused on mainnet (no faucet exists)
   - No user approval needed — no real funds at stake

## User Approval Required Before Any Transaction

**CRITICAL**: The agent MUST NOT execute fund-spending commands without explicit user approval. These commands spend real SOL / tokens / NFTs from the user's wallet:

- `solw send` / `solw send-all`
- `solw token send`
- `solw nft send`
- `solw swap execute`
- `solw pay` (without `--inspect`)

Always:
1. Show the user what will happen (amount, recipient, network, swap details)
2. Wait for explicit confirmation ("yes", "go ahead", "do it")
3. Only then execute with `--confirmed --json`

Safe commands that need no approval:
- `solw balance`, `solw receive`, `solw history`
- `solw token list`, `solw token info`
- `solw nft list`, `solw nft info`
- `solw swap quote`
- `solw pay --inspect` (quote only, no signing)
- `solw airdrop` (devnet / testnet only)
- `solw wallet list`, `solw wallet info`

## AI Agent Workflow

### Example: Swap SOL for USDC

```
Agent: solw swap quote SOL USDC 0.01 --json
  -> {"in_amount_ui": 0.01, "out_amount_ui": 2.341, "price_impact_pct": "0.01",
      "route": ["Raydium", "Orca"], ...}

Agent: "Swapping 0.01 SOL for ~2.34 USDC via Raydium > Orca (0.01% impact). Approve?"
User: "yes"

Agent: solw swap execute SOL USDC 0.01 --confirmed --json
  -> {"signature": "...", "confirmed": true, "out_amount_ui": 2.341, ...}
```

### Example: Check balance and send SPL tokens

```
Agent: solw balance --json
  -> {"address": "...", "sol": 1.23,
      "tokens": [{"symbol": "USDC", "mint": "...", "ui_amount": 100.5, ...}]}

Agent: "You have 1.23 SOL and 100.5 USDC. Send 50 USDC to <addr>?"
User: "yes"

Agent: solw token send EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v <addr> 50 --confirmed --json
  -> {"signature": "...", "confirmed": true, "created_dest_ata": false, ...}
```

### Example: Fresh devnet wallet for testing

```
Agent: solw wallet create demo --network devnet
Agent: solw -n demo airdrop 1 --json
  -> {"signature": "...", "confirmed": true, "sol": 1.0, ...}
Agent: solw -n demo balance --json
  -> {"sol": 1.0, "tokens": []}
```

## Key Options

| Option | Description |
|--------|-------------|
| `--json` | Machine-readable output (recommended for AI agents) |
| `--confirmed` | Skip confirmation prompt (only after user approval) |
| `--network mainnet\|devnet\|testnet` | Override the wallet's stored network for this call |
| `-n <wallet>` | Target a specific wallet by name (overrides the default) |
| `--raw` | `swap quote` / `swap execute` only — treat amount as raw base units |
| `--slippage-bps <n>` | Swap slippage tolerance (default 50 = 0.5%) |
| `--all` | `token list` only — include zero-balance ATAs |
| `--token <mint>` | `balance` only — show a single token's balance |

## JSON Output Contract

Every value-moving command emits a structured JSON object when `--json` is set. Stable fields agents can rely on:

- **`signature`** — base58 transaction signature (present on `send`, `send-all`, `token send`, `nft send`, `swap execute`, `airdrop`)
- **`confirmed`** — boolean; `true` only when the RPC confirmed the signature
- **`confirm_error`** — string or null; populated iff `confirmed=false`
- **Exit code `2`** when a transaction was submitted but did not confirm in time (client should poll or retry)
- **Exit code `1`** on any pre-submit error (bad address, network failure, user abort, etc.)

Swap output adds `in_amount_ui`, `out_amount_ui`, `input_decimals`, `output_decimals`, `price_impact_pct`, `route`, and the legacy `in_amount` / `out_amount` raw strings for backward compatibility.

## Token Aliases (for swaps and convenience)

| Alias | Mint | Decimals |
|-------|------|----------|
| `SOL` / `WSOL` | `So11111111111111111111111111111111111111112` | 9 |
| `USDC` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | 6 |
| `USDT` | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` | 6 |
| `BONK` | `DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263` | 5 |
| `JUP`  | `JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN` | 6 |

For any other mint, pass the full base58 address. Decimals are fetched from the on-chain mint account, so UI-unit amounts work for any SPL token without pre-configuration.

## Custom RPC Endpoints

Public Solana endpoints are rate-limited and frequently return `429` on `getTokenAccountsByOwner` (used by `balance`, `token list`, `nft list`). To use Alchemy / Helius / QuickNode:

```bash
# Per-network (preferred for providers with separate keys per cluster)
export SOLW_RPC_URL_MAINNET="https://solana-mainnet.g.alchemy.com/v2/<KEY>"
export SOLW_RPC_URL_DEVNET="https://solana-devnet.g.alchemy.com/v2/<KEY>"

# Global fallback (single-network setup)
export SOLW_RPC_URL="https://..."
```

Precedence: per-network > global > built-in public endpoint.

## Notes

- Wallet stored locally at `~/.solw/` with `0700`/`0600` perms; seeds never leave the machine
- Mainnet by default; per-wallet network stored at creation, override per-call with `--network`
- **Swaps are mainnet only** — Jupiter only routes mainnet liquidity (`swap execute` bails on any other network)
- Native SOL swaps are handled by Jupiter (no manual wrap/unwrap needed)
- Metaplex metadata is decoded best-effort — missing or malformed metadata is reported inline, never fails the command
- `send-all` withholds ~910,880 lamports so the source account stays rent-exempt
- Override storage root with `SOLW_HOME=/path/to/dir`
