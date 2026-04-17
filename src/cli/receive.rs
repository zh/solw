use anyhow::Result;
use owo_colors::OwoColorize;
use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

use crate::storage;
use crate::wallet;

pub fn run(wallet_name: Option<&str>, cli_network: Option<&str>, no_qr: bool) -> Result<()> {
    let name = storage::resolve_wallet_name(wallet_name)?;
    let mnemonic = storage::get_mnemonic(&name)?
        .ok_or_else(|| anyhow::anyhow!("mnemonic not found for '{}'", name))?;
    let kp = wallet::Keypair::from_mnemonic(&mnemonic)?;
    let address = kp.address();
    let network = storage::resolve_network(Some(&name), cli_network);

    println!("{}", "Receive".green().bold());
    println!("  wallet:  {}", name);
    println!("  network: {}", network);
    println!("  address: {}", address);
    if !no_qr {
        let code = QrCode::new(address.as_bytes())?;
        let rendered = code
            .render::<Dense1x2>()
            .dark_color(Dense1x2::Light)
            .light_color(Dense1x2::Dark)
            .build();
        println!();
        println!("{}", rendered);
    }
    Ok(())
}
