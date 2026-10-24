//! R18 compile-fail case (AAP 0.3.3.1-0.3.3.2): wide money cannot be assigned to a narrower
//! receiver without `store`.
//!
//! `ACCT-CURR-BAL PIC S9(10)V99` (`app/cpy/CVACT01Y.cpy:7`) is `Money<10>`;
//! `TRAN-AMT PIC S9(09)V99` (`app/cpy/CVTRA05Y.cpy:10`) is `Money<9>`. The COBOL
//! `MOVE ACCT-CURR-BAL TO TRAN-AMT` (`app/cbl/COBIL00C.cbl:224`) truncates high-order digits, so
//! it is written `MoneyExpr::from(wide).store::<9>()`, never as a plain assignment.
//! Expected: exactly one error, at `let narrow: Money<9> = wide;`.

use domain::money::Money;

fn move_without_store(wide: Money<10>) -> Money<9> {
    let narrow: Money<9> = wide;
    narrow
}

fn main() {
    // Called rather than bound to `_case`: an underscore binding with no side effect fails
    // clippy pedantic (`no_effect_underscore_binding`), and the case must carry no second finding.
    move_without_store(Money::ZERO);
}
