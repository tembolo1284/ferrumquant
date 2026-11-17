//! Metals on top of `fq-commod-core`.
//!
//! Metals split into two markets that share almost nothing but the word.
//!
//! **Precious** (gold, silver, platinum, palladium) trade like currencies:
//! spot settles T+2, a forward is spot carried at the USD rate less the
//! metal lease rate, and options are vanillas on that forward. [`precious`]
//! carries a spot plus two discount curves and prices forwards, GOFO and
//! implied lease rates, and Black vanillas.
//!
//! **Base** (copper, aluminium, zinc, nickel, lead, tin) trade on the LME
//! with a prompt-date structure unlike any other futures market: every
//! business day out to three months, Wednesdays to six months, then the
//! third Wednesday of each month. [`lme`] implements those date rules and
//! [`lme_curve`] a curve keyed by prompt date with cash-to-3M carries,
//! forwards and options on third-Wednesday prompts.

pub mod error;
pub mod lme;
pub mod lme_curve;
pub mod metal;
pub mod precious;

pub use error::MetalsError;
pub use lme::{LmeCalendar, PromptKind, cash_prompt, classify_prompt, declaration_day, is_valid_prompt, third_wednesday, three_month_prompt};
pub use lme_curve::{LmeCarry, LmeCurve, LmeForward, LmeForwardValuation, LmeOption, LmeOptionValuation};
pub use metal::Metal;
pub use precious::{PreciousForward, PreciousForwardValuation, PreciousMarket, PreciousVanilla, PreciousVanillaValuation};

#[cfg(test)]
mod thread_safety {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn shared_types_are_send_sync() {
        assert_send_sync::<Metal>();
        assert_send_sync::<PreciousMarket>();
        assert_send_sync::<PreciousForward>();
        assert_send_sync::<PreciousVanilla>();
        assert_send_sync::<LmeCurve>();
        assert_send_sync::<LmeForward>();
        assert_send_sync::<LmeOption>();
    }
}
