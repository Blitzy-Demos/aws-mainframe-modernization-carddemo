//! R18 compile-fail case (AAP 0.3.3.1-0.3.3.2): an arithmetic expression cannot be used as money
//! without `store`.
//!
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`) and `TRAN-AMT PIC S9(09)V99`
//! (`app/cpy/CVTRA05Y.cpy:10`) are `Money<9>`. Their difference is a `MoneyExpr`, the exact
//! intermediate of a COBOL COMPUTE such as `app/cbl/COBIL00C.cbl:234`, and it becomes money only
//! through `store::<K>()`, which truncates at the receiving picture.
//! Expected: exactly one error, at `let left: Money<9> = bal - amt;`.

use domain::money::Money;

fn subtract_without_store(bal: Money<9>, amt: Money<9>) -> Money<9> {
    let left: Money<9> = bal - amt;
    left
}

fn main() {
    let _case: fn(Money<9>, Money<9>) -> Money<9> = subtract_without_store;
}
