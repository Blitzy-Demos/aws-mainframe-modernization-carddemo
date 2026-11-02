//! R18 newtype algebra: the operations the copybook PIC classes forbid fail to
//! compile, and the operations they permit compile and give the COBOL result.
//!
//! The fields behind the types: `ACCT-CURR-BAL PIC S9(10)V99` (`CVACT01Y`) is
//! `Money<10>`, `TRAN-AMT PIC S9(09)V99` (`CVTRA05Y`) is `Money<9>`, and
//! `DIS-INT-RATE PIC S9(04)V99` (`CVTRA02Y`) is `InterestRate`. `RewardRate` and
//! `RewardPoints` are the reward rule's rate and whole points, which no copybook
//! declares. A balance meets a rate only in the CBACT04C `1300-COMPUTE-INTEREST`
//! product `(TRAN-CAT-BAL * DIS-INT-RATE) / 1200`, written
//! `(balance * rate).monthly()`.
//!
//! The forbidden half is a trybuild suite over `tests/compile_fail/*.rs`: each
//! case must fail to compile with exactly the output of its `.stderr`. After a
//! change to the numeric types or the toolchain pin, regenerate those files with
//! `TRYBUILD=overwrite cargo test -p carddemo-domain --test newtype_algebra` and
//! review each by hand: it must hold only the intended trait or type error.
//!
//! The permitted half runs here, so the compile-fail suite cannot pass merely
//! because the types are unusable.

use std::error::Error;

use carddemo_domain::numeric::{
    InterestRate, Money, MoneyExpr, Overflow, PointsExpr, RatedBalance, RewardPoints, RewardRate,
};
use rust_decimal_macros::dec;

/// Lets a test use `?` on both `NumericError` (constructors) and `Overflow`
/// (checked arithmetic).
type TestResult = Result<(), Box<dyn Error>>;

/// R18: every operation outside the permitted algebra fails to compile.
///
/// - `money_plus_interest_rate.rs`: `Money<10> + InterestRate` (E0308: the
///   only `Add` of money takes `Money` of any width);
/// - `money_plus_reward_rate.rs`: `Money<10> + RewardRate` (E0308, likewise);
/// - `money_plus_reward_points.rs`: `Money<10> + RewardPoints` (E0308,
///   likewise);
/// - `reward_points_plus_money.rs`: `RewardPoints + Money<10>` (E0308: the only
///   `Add` of `RewardPoints` takes `RewardPoints`);
/// - `money_from_f64.rs`: `Money<10>` converted from a binary floating-point
///   value (E0277).
///
/// The glob is relative to the crate root, where Cargo runs this test.
#[test]
fn forbidden_operations_fail_to_compile() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/compile_fail/*.rs");
}

/// `Money<A> ± Money<B>` and `MoneyExpr ± Money<B>` give an exact `MoneyExpr`,
/// which `store` returns to a field of any width.
#[test]
fn money_sums_and_differences_are_exact_across_widths() -> TestResult {
    let balance = Money::<10>::from_decimal(dec!(1.25))?;
    let amount = Money::<9>::from_decimal(dec!(2.50))?;
    let sum: MoneyExpr = balance + amount;
    assert_eq!(sum.store::<10>(), Money::<10>::from_decimal(dec!(3.75))?);

    let limit = Money::<10>::from_decimal(dec!(10.00))?;
    let difference: MoneyExpr = balance - limit;
    assert_eq!(
        difference.store::<10>(),
        Money::<10>::from_decimal(dec!(-8.75))?
    );

    // CBTRN02C `WS-TEMP-BAL`: cycle credit minus cycle debit plus the amount,
    // two `S9(10)V99` operands and one `S9(09)V99`, stored into `S9(09)V99`.
    let credit = Money::<10>::from_decimal(dec!(500.00))?;
    let debit = Money::<10>::from_decimal(dec!(125.50))?;
    let temp_bal: Money<9> = (credit - debit + amount).store::<9>();
    assert_eq!(temp_bal, Money::<9>::from_decimal(dec!(377.00))?);

    // One intermediate joins another only once stored into a field.
    let combined: MoneyExpr = sum + difference.store::<10>();
    assert_eq!(combined.to_decimal(), dec!(-5.00));
    Ok(())
}

/// A store without `ON SIZE ERROR` drops the high-order integer digits the
/// receiving field cannot hold; the `ON SIZE ERROR` form reports them instead.
#[test]
fn stores_truncate_high_order_digits_or_report_the_size_error() -> TestResult {
    // 1500000000.00 has ten integer digits: it fits `S9(10)V99` only.
    let wide = Money::<10>::from_cents(150_000_000_000)?;
    let expr: MoneyExpr = wide + Money::<9>::ZERO;
    let kept = Money::<9>::from_cents(50_000_000_000)?;

    assert_eq!(expr.store::<9>(), kept);
    assert_eq!(wide.store::<9>(), kept);
    assert_eq!(expr.store_checked::<9>(), Err(Overflow));
    assert_eq!(expr.store_checked::<10>(), Ok(wide));
    Ok(())
}

/// `Money<D> * InterestRate` is the one product of a balance and a rate: the
/// exact `RatedBalance`, whose `monthly` gives the CBACT04C cent quotient of the
/// division by 1200.
#[test]
fn money_times_interest_rate_gives_the_monthly_interest() -> TestResult {
    let balance = Money::<9>::from_decimal(dec!(1200.00))?;
    let rate = InterestRate::from_decimal(dec!(1.00))?;
    let rated: RatedBalance = balance * rate;
    let monthly: Money<9> = rated.monthly().store::<9>();
    assert_eq!(monthly, Money::<9>::from_decimal(dec!(1.00))?);
    Ok(())
}

/// `Money<D> * RewardRate` is the exact `PointsExpr`, whose `ceil` gives whole
/// `RewardPoints`; points combine only with points, checked, giving
/// `RewardPoints`. `checked_ceil`, `checked_add` and `checked_sub` are the
/// forms that report `Overflow`.
#[test]
fn money_times_reward_rate_gives_whole_points() -> TestResult {
    let amount = Money::<9>::from_decimal(dec!(10.01))?;
    let rate = RewardRate::from_decimal(dec!(1.0000))?;
    let product: PointsExpr = amount * rate;
    let points: RewardPoints = product.ceil();
    assert_eq!(points, RewardPoints::try_from(11_i64)?);
    assert_eq!(product.checked_ceil(), Ok(points));

    let sum: RewardPoints = points + points;
    assert_eq!(sum, RewardPoints::try_from(22_i64)?);
    let difference: RewardPoints = sum - points;
    assert_eq!(difference, points);
    assert_eq!(points.checked_add(points), Ok(sum));
    assert_eq!(RewardPoints::ZERO.checked_sub(points), Err(Overflow));
    Ok(())
}

/// Money compares with money of any width and with `MoneyExpr`, as COBOL
/// compares numeric fields algebraically (`IF ACCT-CREDIT-LIMIT >= WS-TEMP-BAL`).
#[test]
fn money_compares_across_widths_and_with_expressions() -> TestResult {
    let small = Money::<9>::from_decimal(dec!(1.00))?;
    let large = Money::<10>::from_decimal(dec!(2.00))?;
    assert!(small < large);
    assert!(large > small);
    assert_ne!(small, large);

    let doubled: MoneyExpr = small + small;
    assert_eq!(doubled, large);
    assert!(large >= doubled);
    assert!(doubled > small);

    let overdrawn = Money::<10>::from_decimal(dec!(-0.01))?;
    assert!(overdrawn < Money::<9>::ZERO);
    Ok(())
}
