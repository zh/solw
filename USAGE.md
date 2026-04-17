# solw

CLI wallet for Solana. Local key storage, SPL tokens, NFTs, Jupiter swap.

## Features

- **Multi-wallet** — create, import, and switch between named wallets
- **BIP39 seed** — 12-word mnemonic, SLIP-0010 derivation at `m/44'/501'/0'/0'` (Phantom/Solflare compatible)
- **SOL send/receive** — with QR codes, `send-all` drain (keeps rent-exempt reserve)
- **SPL tokens** — list, info, send; associated token accounts auto-created for recipients
- **NFTs (Metaplex)** — list, info (name, symbol, metadata PDA), send
- **Jupiter swap** — `quote` (safe) and `execute` (mainnet-only) via https://lite-api.jup.ag/swap/v1
- **Devnet / testnet airdrop** — request faucet SOL (capped at 2 SOL per call)
- **Transaction history** — recent signatures with block time and error status
- **Mainnet / devnet / testnet** — network auto-detected per wallet
- **Local key management** — private keys never leave your machine
- **Agent-friendly** — `--json` output and `--confirmed` flags on every value-moving command

## Install

```bash
cargo install --path .
```

## Quick Start

```bash
# Create a wallet (devnet is safe for trying things out)
solw wallet create mywallet --network devnet

# Check balance
solw balance

# Receive address + QR
solw receive

# Send SOL
solw send <recipient> 0.01

# List SPL tokens
solw token list

# Quote a Jupiter swap (no funds move)
solw swap quote SOL USDC 0.001

# Fund the wallet from the devnet faucet
solw airdrop 1

# Transaction history
solw history
```

## Wallet Management

```bash
# Create a new wallet with a 12-word seed phrase
solw wallet create <name> [--network <mainnet|devnet|testnet>]

# Import an existing wallet (prompts for seed phrase)
solw wallet import <name> [--network <mainnet|devnet|testnet>]

# List all wallets
solw wallet list

# Show the selected wallet's info
solw wallet info

# Switch default wallet
solw wallet default <name>

# Export seed phrase
solw wallet export

# Delete a wallet
solw wallet delete <name>
```

Use `-n <name>` with any command to target a specific wallet without changing the default:

```bash
solw -n mywallet balance
solw -n devtest token list
```

## Networks

`solw` supports three clusters:

- **mainnet** — https://api.mainnet-beta.solana.com
- **devnet** — https://api.devnet.solana.com
- **testnet** — https://api.testnet.solana.com

The network is stored per wallet in `~/.solw/wallets/<name>.net` at creation and auto-used thereafter. Override on a single command with `--network <name>`:

```bash
solw wallet create prod                         # defaults to mainnet
solw wallet create devtest --network devnet
solw -n devtest balance                         # auto-detects devnet
solw --network testnet balance                  # one-off override
```

`swap execute` is hard-restricted to mainnet (Jupiter only routes mainnet liquidity).

### Custom RPC endpoint (env vars)

The public Solana endpoints are aggressively rate-limited and frequently return `429 Too Many Requests` on token- and NFT-listing calls (`getTokenAccountsByOwner`). To use a private RPC (Alchemy, Helius, QuickNode, etc.) set one of these env vars:

| Env var | Applies to | When to use |
|---|---|---|
| `SOLW_RPC_URL_MAINNET` | mainnet only | Provider exposes separate per-network URLs (Alchemy) |
| `SOLW_RPC_URL_DEVNET`  | devnet only  | Same |
| `SOLW_RPC_URL_TESTNET` | testnet only | Same |
| `SOLW_RPC_URL`         | any network  | Single-network setup, or a provider that routes all networks through one URL |

**Precedence:** per-network > global > built-in.

Typical Alchemy setup — separate mainnet / devnet keys that would break if you used a single global var:

```bash
export SOLW_RPC_URL_MAINNET="https://solana-mainnet.g.alchemy.com/v2/<KEY>"
export SOLW_RPC_URL_DEVNET="https://solana-devnet.g.alchemy.com/v2/<KEY>"

solw -n mainwallet nft list    # → Alchemy mainnet URL
solw -n devwallet  balance     # → Alchemy devnet URL (auto-routed)
```

`solw` picks the URL based on the wallet's stored network (or the `--network` override). Unset or blank vars fall through to the next precedence step. The wallet's network determines which chain you're interacting with — the env vars only change **which endpoint** serves that chain.

## Balance & Addresses

```bash
# SOL balance
solw balance
solw balance --json

# Single-token balance
solw balance --token <mint-address>

# Receive address + QR code
solw receive
solw receive --no-qr
```

## Sending SOL

```bash
# Send SOL (amount in SOL, not lamports)
solw send <recipient> <amount>

# Drain wallet to an address (keeps ~0.00091 SOL rent-exempt reserve)
solw send-all <recipient>

# Skip interactive confirmation (for automation)
solw send <recipient> 0.01 --confirmed

# Machine-readable output
solw send <recipient> 0.01 --confirmed --json
```

`send-all` withholds ~910,880 lamports so the source account stays rent-exempt and has a lamport of headroom for the fee.

## SPL Tokens

```bash
# List non-empty SPL token accounts (auto-discovered)
solw token list

# Include zero-balance ATAs (left behind after a full transfer-out; useful if
# you want to close them and reclaim their ~0.002 SOL of rent)
solw token list --all

# Show mint info + Metaplex name/symbol/uri (decimals, supply, authorities)
solw token info <mint>

# Send tokens (amount in UI units; e.g. 1.5 for 1.5 USDC)
solw token send <mint> <recipient> <amount>

# Automation + JSON
solw token send <mint> <recipient> 1.5 --confirmed --json
```

`token send` automatically includes a `CreateAssociatedTokenAccountIdempotent` instruction when the recipient's associated token account (ATA) for that mint does not yet exist, so the recipient does not have to pre-create one.

`token info` fetches the Metaplex Token Metadata account (PDA of the mint under program `metaqbxx...x1s`) and decodes the on-chain `name` and `symbol` into the pretty output. Tokens without Metaplex metadata still show the SPL mint info and print `name: (no Metaplex metadata)`. The off-chain metadata `uri` is included in `--json` output but suppressed from the terminal view to keep it compact.

## NFTs

```bash
# List NFTs (filters token accounts with decimals=0 and amount=1)
solw nft list

# Show mint info + Metaplex name/symbol + metadata PDA
solw nft info <mint>

# Send an NFT (delegates to `token send` with amount=1)
solw nft send <mint> <recipient>
solw nft send <mint> <recipient> --confirmed --json
```

`nft info` decodes the same Metaplex `name` and `symbol` as `token info` — same module, same best-effort behavior (missing or malformed metadata never fails the command). The `uri` appears in `--json` output only, suppressed from the terminal view.

## Airdrop (devnet / testnet)

```bash
# Request 1 SOL from the faucet (default)
solw airdrop

# Custom amount (max 2.0 SOL per call)
solw airdrop 0.5

# JSON output
solw airdrop 1 --json

# Force a network override (skips the wallet's stored network)
solw --network devnet airdrop 2
```

The public faucet (`https://api.devnet.solana.com`) is aggressively rate-limited and is frequently exhausted. If `solw airdrop` returns an error, try these web fallbacks:

- https://faucet.solana.com — canonical Solana faucet; sign in with GitHub to raise your daily cap
- https://faucet.quicknode.com/solana/devnet — QuickNode devnet faucet, 1 claim per 12 hours, no auth

`solw airdrop` refuses to run on mainnet (there is no mainnet faucet).

## Transaction History

```bash
# Recent signatures (default limit 20)
solw history

# Custom limit
solw history --limit 50

# JSON output
solw history --json
```

Each entry shows the signature, slot, block time, and whether the transaction failed.

## Jupiter Swap

The hero feature. `swap quote` is read-only and safe. `swap execute` moves real funds on mainnet and requires explicit approval.

```bash
# Read-only quote (no wallet needed)
solw swap quote <input> <output> <amount> [--raw] [--slippage-bps <n>] [--json]

# Execute on mainnet (wallet required)
solw swap execute <input> <output> <amount> [--raw] [--slippage-bps <n>] [--confirmed] [--json]
```

`<amount>` is the amount of the **input token in UI units** (e.g. `0.001` for 0.001 SOL, `1.5` for 1.5 USDC). Decimals are fetched from the mint automatically.

Pass `--raw` to supply raw base units instead (integer; useful for agents that have already done the conversion):

- SOL → lamports (1 SOL = 1_000_000_000)
- USDC / USDT → 6-decimal base (1 USDC = 1_000_000)
- BONK → 5-decimal base

Default slippage is 50 bps (0.5%). Quote examples:

```bash
# Quote 0.001 SOL → USDC
solw swap quote SOL USDC 0.001

# Quote 1 USDC → BONK with 1% slippage, JSON output
solw swap quote USDC BONK 1 --slippage-bps 100 --json

# Full mint addresses also work
solw swap quote So11111111111111111111111111111111111111112 \
                EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v 0.001

# Equivalent --raw form (back-compat with prior versions)
solw swap quote SOL USDC 1000000 --raw
```

Execute example (mainnet wallet must be funded):

```bash
solw -n mainnet swap execute SOL USDC 0.002   # 0.002 SOL → USDC
# interactive confirm prompt unless --confirmed or --json
```

**Migration note.** Prior versions took raw base units as the positional (`solw swap quote SOL USDC 1000000`). The positional is now UI units. Pass `--raw` to preserve the old behavior.

### Built-in Token Aliases

| Alias | Mint |
|-------|------|
| `SOL`, `WSOL` | `So11111111111111111111111111111111111111112` |
| `USDC` | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| `USDT` | `Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB` |
| `BONK` | `DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263` |
| `JUP` | `JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN` |

Any other string is treated as a literal base58 mint address.

## Global Options

| Option | Applies to | Description |
|--------|------------|-------------|
| `-n, --name <wallet>` | all commands | Select wallet (falls back to default) |
| `--network <net>` | all commands | Override stored network: `mainnet`, `devnet`, `testnet` |
| `--confirmed` | `send`, `send-all`, `token send`, `nft send`, `swap execute` | Skip the interactive confirmation prompt |
| `--json` | most commands | Machine-readable JSON output |

## Storage

```
~/.solw/                       # 0700
├── default                    # name of default wallet
└── wallets/                   # 0700
    ├── <name>                 # BIP39 mnemonic (0600)
    └── <name>.net             # network: mainnet|devnet|testnet
```

Override the storage root with `SOLW_HOME=/path/to/dir`.

## Security

- **Key storage** — plaintext seed phrases with `0600` file permissions; private keys never leave disk. Same model as the Solana CLI's file-system wallets.
- **Confirmation prompts** — all value-moving commands (`send`, `send-all`, `token send`, `nft send`, `swap execute`) require interactive confirmation. Use `--confirmed` for automation.
- **Mainnet guard on swaps** — `swap execute` refuses to run on any network other than mainnet (Jupiter only routes mainnet liquidity).
- **HTTPS-only RPC** — all built-in cluster endpoints are HTTPS.
- **Reserved names** — wallet names `default` and `config` are rejected to prevent collisions with metadata files.

## Dependencies

`solw` deliberately avoids the heavyweight `solana-sdk` crate. The Solana wire format (compact-u16 shortvec, System Program `Transfer`, SPL `TransferChecked`, `CreateAssociatedTokenAccountIdempotent`) and PDA / associated-token-account derivation are hand-rolled and cross-validated against `@solana/spl-token` and `@solana/web3.js` via pinned test vectors.

Swaps go through Jupiter's v1 lite-api at https://lite-api.jup.ag/swap/v1 — `/quote` for route pricing and `/swap` for the unsigned legacy transaction, which `solw` then signs locally with the wallet's ed25519 key and submits via standard JSON-RPC `sendTransaction`.

## License

MIT
