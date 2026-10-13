//! ferrumquant core: time, math, and market-data primitives.

pub mod math;
pub mod time;

// Everything shared across threads by a risk engine must stay plain owned
// data (no Rc/RefCell/Cell). This fails to compile if that ever changes.
const _: () = {
    const fn assert_shareable<T: Send + Sync>() {}
    assert_shareable::<time::Calendar>();
    assert_shareable::<time::Schedule>();
    assert_shareable::<time::Date>();
};
