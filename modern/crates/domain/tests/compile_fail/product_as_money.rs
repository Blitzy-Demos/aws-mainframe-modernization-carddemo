//! R18 compile-fail case (AAP 0.3.3.1): a rate product cannot be used as money.
//!
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`) is `Money<9>` and
//! `DIS-INT-RATE PIC S9(04)V99` (`app/cpy/CVTRA02Y.cpy:9`) is `Rate<4>`. Their product is a
//! `RateProduct`; it becomes money only through the i2 formula,
//! `((balance * rate) / PERCENT_MONTHS).store::<9>()` (`app/cbl/CBACT04C.cbl:464-465`).
//! Expected: exactly one error, at `let interest: Money<9> = balance * rate;`.

use domain::money::{Money, Rate};

fn interest_without_divisor(balance: Money<9>, rate: Rate<4>) -> Money<9> {
    let interest: Money<9> = balance * rate;
    interest
}

fn main() {
    let _case: fn(Money<9>, Rate<4>) -> Money<9> = interest_without_divisor;
}
