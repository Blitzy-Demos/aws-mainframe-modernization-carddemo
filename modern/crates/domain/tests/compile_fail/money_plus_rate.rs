//! R18 compile-fail case (AAP 0.3.3.1, user example of AAP 0.11.5): `Money + Rate` must not
//! compile.
//!
//! `ACCT-CURR-BAL PIC S9(10)V99` (`app/cpy/CVACT01Y.cpy:7`) is `Money<10>` and
//! `DIS-INT-RATE PIC S9(04)V99` (`app/cpy/CVTRA02Y.cpy:9`) is `Rate<4>`. A rate is never added to
//! money: the only path from a rate to money is the i2 formula, `monthly_interest`.
//! Expected: exactly one error, at `acct_curr_bal + dis_int_rate`.

use domain::money::{Money, Rate};

fn add_rate_to_balance(acct_curr_bal: Money<10>, dis_int_rate: Rate<4>) {
    let _ = acct_curr_bal + dis_int_rate;
}

fn main() {
    let _case: fn(Money<10>, Rate<4>) = add_rate_to_balance;
}
