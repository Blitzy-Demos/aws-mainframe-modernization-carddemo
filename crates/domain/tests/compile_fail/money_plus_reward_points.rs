//! R18 compile-fail case: money plus reward points. `Money<10>` is the
//! `S9(10)V99` of `ACCT-CURR-BAL`, and no `Add<RewardPoints>` exists for any
//! money width, so this must fail with E0277 at the `+`.

use carddemo_domain::numeric::{Money, RewardPoints};

fn main() {
    let amount = Money::<10>::ZERO;
    let points = RewardPoints::ZERO;
    let _ = amount + points;
}
