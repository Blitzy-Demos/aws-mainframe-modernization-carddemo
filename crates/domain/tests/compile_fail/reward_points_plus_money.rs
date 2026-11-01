//! R18, reverse operand order: reward points plus money must not compile.
//!
//! `RewardPoints` adds only to `RewardPoints`, so a points total cannot absorb an
//! `S9(10)V99` amount such as `ACCT-CURR-BAL`. Because that is its only `Add`
//! impl, rustc infers the right operand from it and rejects `amount` with a type
//! mismatch (E0308) rather than the missing-impl E0277 the mirror case
//! `money_plus_reward_points.rs` gets; both are the one intended rejection. The
//! expected compiler output is `reward_points_plus_money.stderr`, checked by
//! `tests/newtype_algebra.rs`.

use carddemo_domain::numeric::{Money, RewardPoints};

fn main() {
    let points = RewardPoints::ZERO;
    let amount = Money::<10>::ZERO;
    let _ = points + amount;
}
