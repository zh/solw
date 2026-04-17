//! Shared amount conversion helpers. Pure so they're testable without RPC.
//!
//! SOL ↔ lamports (fixed 9 decimals) and generic UI ↔ raw for SPL mints.
use anyhow::{bail, Result};

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Convert lamports to a UI `f64` SOL amount. Loses precision past ~15 significant
/// digits; fine for display, not for accounting.
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

/// Convert raw base units to a UI `f64` amount for a given mint's decimals.
/// Display-only — use integer comparisons for any value checks.
pub fn raw_to_ui(amount_raw: u64, decimals: u8) -> f64 {
    amount_raw as f64 / 10f64.powi(decimals as i32)
}

/// Convert a UI-unit SOL amount to lamports with the same rejection rules as
/// `ui_to_raw(amount, 9)` — just specialized so callers don't have to pass 9
/// at every site.
pub fn sol_to_lamports(amount_sol: f64) -> Result<u64> {
    ui_to_raw(amount_sol, 9)
}

/// Convert a UI-unit amount (e.g. `1.5` USDC) to raw base units given a
/// mint's decimals (e.g. `1_500_000` for decimals=6).
///
/// Rejects non-finite, zero, or negative amounts, integer overflow, and
/// amounts that round to zero at the given decimal precision.
pub fn ui_to_raw(amount_ui: f64, decimals: u8) -> Result<u64> {
    if !amount_ui.is_finite() || amount_ui <= 0.0 {
        bail!("amount must be positive and finite");
    }
    let raw_f = amount_ui * 10f64.powi(decimals as i32);
    if raw_f > u64::MAX as f64 {
        bail!("amount exceeds u64");
    }
    let raw = raw_f.round() as u64;
    if raw == 0 {
        bail!("amount rounds to zero at mint decimals={}", decimals);
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_to_raw_sol_round_trip() {
        assert_eq!(ui_to_raw(1.5, 9).unwrap(), 1_500_000_000);
        assert_eq!(ui_to_raw(0.001, 9).unwrap(), 1_000_000);
        assert_eq!(ui_to_raw(1.0, 9).unwrap(), 1_000_000_000);
    }

    #[test]
    fn ui_to_raw_usdc_round_trip() {
        assert_eq!(ui_to_raw(1.0, 6).unwrap(), 1_000_000);
        assert_eq!(ui_to_raw(1.5, 6).unwrap(), 1_500_000);
    }

    #[test]
    fn ui_to_raw_nft_decimals_zero() {
        assert_eq!(ui_to_raw(1.0, 0).unwrap(), 1);
    }

    #[test]
    fn ui_to_raw_rejects_zero() {
        let err = ui_to_raw(0.0, 9).unwrap_err();
        assert!(err.to_string().contains("positive and finite"), "got: {}", err);
    }

    #[test]
    fn ui_to_raw_rejects_negative() {
        assert!(ui_to_raw(-1.0, 9).is_err());
    }

    #[test]
    fn ui_to_raw_rejects_nan_and_inf() {
        assert!(ui_to_raw(f64::NAN, 9).is_err());
        assert!(ui_to_raw(f64::INFINITY, 9).is_err());
        assert!(ui_to_raw(f64::NEG_INFINITY, 9).is_err());
    }

    #[test]
    fn ui_to_raw_rejects_overflow() {
        let err = ui_to_raw(1e30, 9).unwrap_err();
        assert!(err.to_string().contains("exceeds u64"), "got: {}", err);
    }

    #[test]
    fn ui_to_raw_rejects_rounds_to_zero() {
        let err = ui_to_raw(1e-12, 9).unwrap_err();
        assert!(err.to_string().contains("rounds to zero"), "got: {}", err);
    }

    #[test]
    fn lamports_to_sol_basic() {
        assert_eq!(lamports_to_sol(LAMPORTS_PER_SOL), 1.0);
        assert_eq!(lamports_to_sol(LAMPORTS_PER_SOL / 2), 0.5);
        assert_eq!(lamports_to_sol(0), 0.0);
    }

    #[test]
    fn sol_to_lamports_basic() {
        assert_eq!(sol_to_lamports(1.0).unwrap(), 1_000_000_000);
        assert_eq!(sol_to_lamports(0.5).unwrap(), 500_000_000);
        assert_eq!(sol_to_lamports(0.000000001).unwrap(), 1);
    }

    #[test]
    fn sol_to_lamports_rejects_nonpositive() {
        assert!(sol_to_lamports(0.0).is_err());
        assert!(sol_to_lamports(-1.0).is_err());
        assert!(sol_to_lamports(f64::NAN).is_err());
        assert!(sol_to_lamports(f64::INFINITY).is_err());
    }
}
