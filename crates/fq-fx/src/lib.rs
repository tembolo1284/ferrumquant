//! FX for ferrumquant: currencies and pairs with market conventions, spot
//! and forward markets on two discount curves, deliverable forwards and NDFs.
//! Vanilla options and vol surfaces follow.
//!
//! Market data is passed explicitly, as in `fq-rates`: an `FxMarket` is a
//! value (spot plus two curves) that clones cheaply for bumps.

pub mod currency;
pub mod forward;
pub mod vanilla;
pub mod vol_surface;

pub use currency::{Currency, CurrencyError, CurrencyPair};
pub use forward::{FxDirection, FxError, FxForward, FxForwardValuation, FxMarket, FxNdf};
pub use vanilla::{AtmConvention, DeltaConvention, FxSlice, FxVanilla, FxVanillaValuation, PremiumCurrency, expiry_and_delivery};
pub use vol_surface::{FxSmile, FxVolSurface, SmileQuote, SurfaceConfig, delivery_for_expiry};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<CurrencyPair>();
        assert_send_sync::<FxMarket>();
        assert_send_sync::<FxForward>();
        assert_send_sync::<FxNdf>();
        assert_send_sync::<FxVanilla>();
        assert_send_sync::<FxVolSurface>();
    }
}
