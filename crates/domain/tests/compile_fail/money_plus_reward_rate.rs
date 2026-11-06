//! R18 compile-fail case: money plus a reward rate.
//!
//! `Money<10>` is the `S9(10)V99` field `ACCT-CURR-BAL` (`CVACT01Y`). A
//! `RewardRate` meets money only through `Money<D> * RewardRate`, the exact
//! points product, and money adds only money, of any width, so this sum must
//! fail with a type mismatch (E0308) at `rate`. The expected compiler output is
//! `money_plus_reward_rate.stderr`, checked by `tests/newtype_algebra.rs`.

use carddemo_domain::numeric::{Money, RewardRate};

fn main() {
    let amount = Money::<10>::ZERO;
    let rate = RewardRate::ZERO;
    let _ = amount + rate;
}
