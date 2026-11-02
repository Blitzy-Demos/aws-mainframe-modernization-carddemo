//! R18 compile-fail case: money plus reward points. `Money<10>` is the
//! `S9(10)V99` of `ACCT-CURR-BAL`, and money adds only money, of any width,
//! so this must fail with a type mismatch (E0308) at `points`.

use carddemo_domain::numeric::{Money, RewardPoints};

fn main() {
    let amount = Money::<10>::ZERO;
    let points = RewardPoints::ZERO;
    let _ = amount + points;
}
