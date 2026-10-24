//! R18 compile-fail case (AAP 0.3.3.1): `Rate + Rate` must not compile.
//!
//! `DIS-INT-RATE PIC S9(04)V99` (`app/cpy/CVTRA02Y.cpy:9`) is `Rate<4>`. A rate supports only
//! `is_zero` and multiplication by money, so two rates cannot be added.
//! Expected: exactly one error, at `first + second`.

use domain::money::Rate;

fn add_rates(first: Rate<4>, second: Rate<4>) {
    let _ = first + second;
}

fn main() {
    let _case: fn(Rate<4>, Rate<4>) = add_rates;
}
