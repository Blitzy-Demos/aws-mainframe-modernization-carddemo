//! R18: the copybook type algebra (AAP 0.3.3.1). Each signed amount picture is its own type in
//! `domain::money`, so `Money + Rate` and every other combination COBOL never performs fails to
//! compile, and the i2 formula is the one path from a rate to money that typechecks.
//!
//! The cases are not Cargo targets; `trybuild` builds each one as a separate crate against
//! `domain`, with paths relative to this crate's root:
//!
//! - `tests/compile_fail/*.rs`, eight cases, each of which must fail with exactly one error at its
//!   forbidden expression:
//!   - `money_plus_rate.rs`, the headline case: `ACCT-CURR-BAL PIC S9(10)V99` plus
//!     `DIS-INT-RATE PIC S9(04)V99`, that is `Money<10> + Rate<4>`;
//!   - `rate_plus_money.rs` and `rate_plus_rate.rs`: a rate is never an addend;
//!   - `money_times_money.rs`: money multiplies only by a rate;
//!   - `money_from_f64.rs`, the floating-literal case: no money is built from a binary
//!     floating-point value (R1);
//!   - `wide_into_narrow.rs`: `Money<10>` reaches a `Money<9>` receiver only through `store`;
//!   - `product_as_money.rs`: `balance * rate` is a `RateProduct`, money only after
//!     `/ PERCENT_MONTHS` and `store`;
//!   - `expr_without_store.rs`: `bal - amt` is a `MoneyExpr`, money only after `store`.
//! - `tests/compile_pass/i2_formula.rs`: `((balance * rate) / PERCENT_MONTHS).store::<9>()`, the
//!   `COMPUTE WS-MONTHLY-INT = ( TRAN-CAT-BAL * DIS-INT-RATE) / 1200` of `CBACT04C.cbl:464-465`,
//!   compiles without warnings and runs to completion.
//!
//! Each case's `.stderr` beside it is committed and holds the diagnostic the case must produce.
//! It is generated on Rust 1.85.0 with `TRYBUILD=overwrite cargo test -p domain --test
//! money_algebra`, then reviewed so that it shows exactly the one intended error and no warning.
//! Any change to the case or to the declarations of `domain::money` it reports on means
//! regenerating and reviewing it again.

// audit: ARITH-001
#[test]
fn money_algebra() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
    t.pass("tests/compile_pass/i2_formula.rs");
}
