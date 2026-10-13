//! ferrumquant rates: indices, curves, bonds, bond futures, swaps, and swaptions.

pub mod bond;
pub mod curve;
pub mod future;
pub mod index;
pub mod swap;
pub mod swaption;

// Everything a risk engine shares across threads must stay plain owned data
// (no Rc/RefCell/Cell). This fails to compile if that ever changes.
const _: () = {
    const fn assert_shareable<T: Send + Sync>() {}
    assert_shareable::<curve::DiscountCurve>();
    assert_shareable::<index::OvernightIndex>();
    assert_shareable::<swap::OisSwap>();
    assert_shareable::<bond::FixedRateBond>();
    assert_shareable::<future::TreasuryFuture>();
    assert_shareable::<swaption::Swaption>();
};
