use anyhow::Result;
use clap::{Parser, Subcommand};

mod cli;
mod jupiter;
mod metaplex;
mod pda;
mod rpc;
mod storage;
mod tx;
mod util;
mod wallet;
mod x402;

#[derive(Parser)]
#[command(name = "solw", version, about = "solw -- Solana CLI wallet")]
struct Cli {
    /// Wallet name (uses default wallet if omitted)
    #[arg(short = 'n', long = "name", global = true)]
    name: Option<String>,

    /// Network: mainnet, devnet, or testnet (overrides stored network)
    #[arg(long = "network", global = true)]
    network: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Wallet management (create, import, info, export, list, default, delete)
    Wallet {
        #[command(subcommand)]
        command: WalletCommand,
    },
    /// Check SOL and SPL token balances
    Balance {
        /// Filter to a single SPL token mint
        #[arg(long)]
        token: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show receive address and QR code
    Receive {
        /// Suppress QR code display
        #[arg(long)]
        no_qr: bool,
    },
    /// Recent transaction history
    History {
        /// Number of signatures to fetch
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Send SOL to an address
    Send {
        /// Recipient base58 address
        to: String,
        /// Amount in SOL (e.g. 0.01)
        amount: f64,
        /// Skip interactive confirmation prompt
        #[arg(long)]
        confirmed: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Send entire SOL balance to an address (keeps rent-exempt reserve)
    SendAll {
        /// Recipient base58 address
        to: String,
        /// Skip interactive confirmation prompt
        #[arg(long)]
        confirmed: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// SPL token operations (list, info, send)
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// NFT operations (list, info, send)
    Nft {
        #[command(subcommand)]
        command: NftCommand,
    },
    /// Jupiter swap: quote or execute (mainnet)
    Swap {
        #[command(subcommand)]
        command: SwapCommand,
    },
    /// Request a SOL airdrop on devnet/testnet (capped at 2 SOL per call)
    Airdrop {
        /// Amount in SOL (default 1.0, max 2.0)
        amount: Option<f64>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Pay an x402-protected HTTP endpoint with USDC (devnet today)
    Pay {
        /// Resource URL (server replies 402 with a payment quote)
        url: String,
        /// Maximum price in UI units (USDC)
        #[arg(long = "max-price", default_value_t = cli::pay::DEFAULT_MAX_PRICE_UI)]
        max_price: f64,
        /// Fetch the 402 quote, print it, and exit without signing or paying
        #[arg(long)]
        inspect: bool,
        /// Skip interactive confirmation prompt
        #[arg(long)]
        confirmed: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SwapCommand {
    /// Fetch a Jupiter route quote (does not submit a transaction)
    Quote {
        /// Input mint or alias (e.g. SOL, USDC)
        input: String,
        /// Output mint or alias (e.g. USDC)
        output: String,
        /// Amount of the input token in UI units (e.g. 0.001 for 0.001 SOL). Use --raw to pass base units.
        amount: f64,
        /// Treat `amount` as raw base units (integer) instead of UI units
        #[arg(long)]
        raw: bool,
        /// Slippage tolerance in basis points (e.g. 50 = 0.5%)
        #[arg(long, default_value = "50")]
        slippage_bps: u16,
        #[arg(long)]
        json: bool,
    },
    /// Execute a swap on mainnet (requires --confirmed or interactive approval)
    Execute {
        input: String,
        output: String,
        /// Amount of the input token in UI units (e.g. 0.001 for 0.001 SOL). Use --raw to pass base units.
        amount: f64,
        /// Treat `amount` as raw base units (integer) instead of UI units
        #[arg(long)]
        raw: bool,
        #[arg(long, default_value = "50")]
        slippage_bps: u16,
        #[arg(long)]
        confirmed: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TokenCommand {
    /// List SPL token balances (hides zero-balance ATAs by default)
    List {
        /// Include zero-balance token accounts (useful for rent-reclaim workflows)
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show mint info (decimals, supply, authority)
    Info {
        /// Mint address (base58)
        mint: String,
        #[arg(long)]
        json: bool,
    },
    /// Send SPL tokens
    Send {
        /// Mint address
        mint: String,
        /// Recipient wallet owner address
        to: String,
        /// Amount in UI units (e.g. 1.5 for 1.5 USDC)
        amount: f64,
        #[arg(long)]
        confirmed: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NftCommand {
    /// List NFTs (filters token accounts with decimals=0, amount=1)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show NFT mint info + Metaplex metadata PDA
    Info {
        /// Mint address
        mint: String,
        #[arg(long)]
        json: bool,
    },
    /// Send an NFT (calls `token send` internally with amount=1)
    Send {
        mint: String,
        to: String,
        #[arg(long)]
        confirmed: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WalletCommand {
    /// Create a new wallet with a fresh 12-word BIP39 seed
    Create { name: String },
    /// Import an existing wallet from a 12-word seed phrase
    Import { name: String },
    /// Show wallet info (address, network)
    Info,
    /// Export the wallet seed phrase
    Export,
    /// Delete a wallet
    Delete { name: String },
    /// Set a wallet as the default
    Default { name: String },
    /// List all stored wallets
    List,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

#[tokio::main]
async fn run() -> Result<()> {
    let cli = Cli::parse();
    let wallet_name = cli.name.as_deref();
    let cli_network = cli.network.as_deref();

    match cli.command {
        Commands::Wallet { command } => match command {
            WalletCommand::Create { name } => cli::wallet::create(&name, cli_network)?,
            WalletCommand::Import { name } => cli::wallet::import(&name, cli_network)?,
            WalletCommand::Info => cli::wallet::info(wallet_name, cli_network)?,
            WalletCommand::Export => cli::wallet::export(wallet_name)?,
            WalletCommand::Delete { name } => cli::wallet::delete(&name)?,
            WalletCommand::Default { name } => cli::wallet::set_default(&name)?,
            WalletCommand::List => cli::wallet::list()?,
        },
        Commands::Balance { token, json } => {
            cli::balance::run(wallet_name, cli_network, token.as_deref(), json).await?
        }
        Commands::Receive { no_qr } => cli::receive::run(wallet_name, cli_network, no_qr)?,
        Commands::History { limit, json } => {
            cli::history::run(wallet_name, cli_network, limit, json).await?
        }
        Commands::Send { to, amount, confirmed, json } => {
            cli::send::run(wallet_name, cli_network, &to, amount, confirmed, json).await?
        }
        Commands::SendAll { to, confirmed, json } => {
            cli::send::run_all(wallet_name, cli_network, &to, confirmed, json).await?
        }
        Commands::Token { command } => match command {
            TokenCommand::List { all, json } => {
                cli::token::list(wallet_name, cli_network, all, json).await?
            }
            TokenCommand::Info { mint, json } => {
                cli::token::info(wallet_name, cli_network, &mint, json).await?
            }
            TokenCommand::Send { mint, to, amount, confirmed, json } => {
                cli::token::send(wallet_name, cli_network, &mint, &to, amount, confirmed, json)
                    .await?
            }
        },
        Commands::Nft { command } => match command {
            NftCommand::List { json } => cli::nft::list(wallet_name, cli_network, json).await?,
            NftCommand::Info { mint, json } => {
                cli::nft::info(wallet_name, cli_network, &mint, json).await?
            }
            NftCommand::Send { mint, to, confirmed, json } => {
                cli::nft::send(wallet_name, cli_network, &mint, &to, confirmed, json).await?
            }
        },
        Commands::Airdrop { amount, json } => {
            cli::airdrop::run(wallet_name, cli_network, amount, json).await?
        }
        Commands::Pay { url, max_price, inspect, confirmed, json } => {
            let params = cli::pay::PayParams {
                url: &url,
                max_price_ui: max_price,
                inspect,
                confirmed,
                json_out: json,
            };
            cli::pay::run(wallet_name, cli_network, params).await?
        }
        Commands::Swap { command } => match command {
            SwapCommand::Quote { input, output, amount, raw, slippage_bps, json } => {
                cli::swap::quote(&input, &output, amount, raw, slippage_bps, cli_network, json)
                    .await?
            }
            SwapCommand::Execute {
                input,
                output,
                amount,
                raw,
                slippage_bps,
                confirmed,
                json,
            } => {
                let params = cli::swap::SwapParams {
                    input: &input,
                    output: &output,
                    amount,
                    raw,
                    slippage_bps,
                };
                cli::swap::execute(wallet_name, cli_network, params, confirmed, json).await?
            }
        },
    }

    Ok(())
}
