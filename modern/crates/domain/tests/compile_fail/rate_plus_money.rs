//! R18 compile-fail case (AAP 0.3.3.1): `Rate + Money` must not compile.
//!
//! `DIS-INT-RATE PIC S9(04)V99` (`app/cpy/CVTRA02Y.cpy:9`) is `Rate<4>` and
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`) is `Money<9>`. A rate supports only
//! `is_zero` and multiplication by money; it never takes money as an addend.
//! Expected: exactly one error, at `dis_int_rate + tran_cat_bal`.

use domain::money::{Money, Rate};

fn add_money_to_rate(dis_int_rate: Rate<4>, tran_cat_bal: Money<9>) {
    let _ = dis_int_rate + tran_cat_bal;
}

fn main() {
    let _case: fn(Rate<4>, Money<9>) = add_money_to_rate;
}
