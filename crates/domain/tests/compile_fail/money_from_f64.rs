//! Float gate: no money type converts from a floating-point value.
//!
//! `Money<10>` is the `S9(10)V99` field `ACCT-CURR-BAL` (`CVACT01Y`). Money
//! enters the domain only as a `Decimal`, a cent count or digit text, so
//! `From<f64>` has no `impl` and this conversion must fail with E0277.

use carddemo_domain::numeric::Money;

fn main() {
    // The trait is named so the error is the missing `From<f64>` impl (E0277).
    // `Money::<10>::from(1.5_f64)` would resolve to the reflexive
    // `From<Money<10>>` impl and report a type mismatch (E0308) instead.
    let _ = <Money<10> as From<f64>>::from(1.5_f64);
}
