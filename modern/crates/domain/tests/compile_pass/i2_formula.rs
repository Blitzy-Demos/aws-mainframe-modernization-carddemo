//! trybuild pass case of the copybook type algebra (R18, AAP 0.3.3.1): the i2 formula is the
//! path from a rate to money, and it typechecks.
//!
//! `COMPUTE WS-MONTHLY-INT = ( TRAN-CAT-BAL * DIS-INT-RATE) / 1200`
//! (`app/cbl/CBACT04C.cbl:464-465`, paragraph `1300-COMPUTE-INTEREST`) stores into
//! `WS-MONTHLY-INT PIC S9(09)V99` (`CBACT04C.cbl:168`), so `Money<9>`. Its operands are
//! `TRAN-CAT-BAL PIC S9(09)V99` (`app/cpy/CVTRA01Y.cpy:9`), so `Money<9>`, and
//! `DIS-INT-RATE PIC S9(04)V99` (`app/cpy/CVTRA02Y.cpy:9`), so `Rate<4>`. No `ROUNDED` phrase
//! applies, so the quotient truncates toward zero at 0.01. Audit rows: `ARITH-001` (the
//! statement) and `ARITH-002` (its worked case) of `modern/audit/arithmetic.md`.
//!
//! `tests/money_algebra.rs` builds this file as a standalone binary and requires it to compile
//! cleanly and exit 0. Each case under `tests/compile_fail/` is the illegal counterpart of an
//! expression written legally here: the rate reaches money only as `Money * Rate`, divided by
//! `PERCENT_MONTHS`, then stored; sums and moves reach a field only through `store`.

use domain::money::{
    Decimal, Money, MoneyError, MoneyExpr, PERCENT_MONTHS, Rate, monthly_interest,
};

fn main() -> Result<(), MoneyError> {
    // TRAN-CAT-BAL 1234.56 and DIS-INT-RATE 15.00, the operands of the worked case.
    let balance = Money::<9>::new(Decimal::new(123_456, 2))?;
    let rate = Rate::<4>::new(Decimal::new(1_500, 2))?;

    // audit: ARITH-001
    // The formula through the function and spelled out: `Money * Rate` is a rate product whose
    // only operation is the division by 1200; the quotient reaches the field only through store.
    let via_fn: Money<9> = monthly_interest(balance, rate);
    let via_formula: Money<9> = ((balance * rate) / PERCENT_MONTHS).store::<9>();
    assert!(via_fn == via_formula);

    // audit: ARITH-002
    // 1234.56 × 15.00 = 18518.4000; / 1200 = 15.432 exactly; truncated toward zero: 15.43.
    assert!(via_fn == Money::<9>::new(Decimal::new(1_543, 2))?);

    // An intermediate chains with money of any width and leaves only through store; the stored
    // results compare as money across widths.
    let chained: Money<10> = (MoneyExpr::from(balance) + balance - balance).store::<10>();
    assert!(chained == balance);

    // audit: ARITH-023
    // `Money - Money` is an intermediate too. CBTRN02C stores ACCT-CURR-CYC-CREDIT minus
    // ACCT-CURR-CYC-DEBIT plus DALYTRAN-AMT, from S9(10)V99 operands, into the narrower
    // WS-TEMP-BAL S9(09)V99: 1000.00 - (-250.00) + 100.00 = 1350.00.
    let cyc_credit = Money::<10>::new(Decimal::new(100_000, 2))?;
    let cyc_debit = Money::<10>::new(Decimal::new(-25_000, 2))?;
    let amt = Money::<9>::new(Decimal::new(10_000, 2))?;
    let temp: Money<9> = (cyc_credit - cyc_debit + amt).store::<9>();
    assert!(temp == Money::<9>::new(Decimal::new(135_000, 2))?);

    // audit: ARITH-020
    // MOVE ACCT-CURR-BAL TO TRAN-AMT: an S9(10)V99 value reaches S9(09)V99 only through store;
    // 843.00 fits, so the whole balance moves.
    let acct_curr_bal = Money::<10>::new(Decimal::new(84_300, 2))?;
    let tran_amt: Money<9> = MoneyExpr::from(acct_curr_bal).store::<9>();
    assert!(tran_amt == acct_curr_bal);

    Ok(())
}
