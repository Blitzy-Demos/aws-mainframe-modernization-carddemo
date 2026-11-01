//! R18 compile-fail case: money plus an interest rate.
//!
//! `Money<10>` is the `S9(10)V99` of `ACCT-CURR-BAL` (`CVACT01Y`) and
//! `InterestRate` the `S9(04)V99` of `DIS-INT-RATE` (`CVTRA02Y`). A balance
//! meets a rate only through `Money<D> * InterestRate`, the CBACT04C interest
//! product, so no `Add<InterestRate>` exists and this must fail with E0277 at
//! the `+`.

use carddemo_domain::numeric::{InterestRate, Money};

fn main() {
    let balance = Money::<10>::ZERO;
    let rate = InterestRate::ZERO;
    let _ = balance + rate;
}
