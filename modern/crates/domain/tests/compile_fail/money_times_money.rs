//! R18 compile-fail case (AAP 0.3.3.1): `Money * Money` must not compile.
//!
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`) and `TRAN-AMT PIC S9(09)V99`
//! (`app/cpy/CVTRA05Y.cpy:10`) are both `Money<9>`. Money multiplies only by a rate, and the
//! product of two amounts has no unit.
//! Expected: exactly one error, at `tran_cat_bal * tran_amt`.

use domain::money::Money;

fn multiply_money(tran_cat_bal: Money<9>, tran_amt: Money<9>) {
    let _ = tran_cat_bal * tran_amt;
}

fn main() {
    let _case: fn(Money<9>, Money<9>) = multiply_money;
}
