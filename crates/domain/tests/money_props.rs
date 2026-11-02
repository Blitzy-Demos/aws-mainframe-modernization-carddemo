//! Property tests of the money and points algebra (AAP 0.5.3.1 permitted
//! arithmetic, AAP 0.9.2; goal o3).
//!
//! The unit tests of `numeric/money.rs`, `numeric/rates.rs` and
//! `numeric/points.rs` pin worked values. These properties check the same rules
//! over the whole range of each field, through the public API only:
//!
//! - **The store rule** (audit section C, P-CBACT04C-011): no COBOL program
//!   uses `ROUNDED` and no in-scope receiving field has `ON SIZE ERROR`, so a
//!   store truncates excess fraction digits toward zero, keeps the sign and the
//!   low-order integer digits, and stores a zero as positive zero
//!   (P-CBACT04C-018). [`reference_store`] restates that rule with its own
//!   arithmetic and shares no code with `money.rs`.
//! - **Exactness, ordering and sign** of `Money` and `MoneyExpr`.
//! - **Reward points**: `ceil(amount × rate)` over the exact product (AAP
//!   0.3.5 step 4), and the checked `RewardPoints` range `0..=i64::MAX`.
//!
//! Every strategy draws integer counts of cents, rate hundredths, `0.0001`
//! rate units or points; no value passes through a binary floating-point type.

use carddemo_domain::numeric::{
    InterestRate, Money, MoneyExpr, NumericError, Overflow, RewardPoints, RewardRate,
};
use proptest::prelude::{
    Just, ProptestConfig, Strategy, TestCaseError, prop_assert, prop_assert_eq, prop_oneof,
    proptest,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// The largest cent count of `S9(10)V99` (`ACCT-CURR-BAL`, P-CVACT01Y-003):
/// 9999999999.99.
const MAX_CENTS_10: i64 = 999_999_999_999;

/// The largest cent count of `S9(09)V99` (`TRAN-AMT`, P-CVTRA05Y-006;
/// `TRAN-CAT-BAL`, P-CVTRA01Y-004): 999999999.99.
const MAX_CENTS_9: i64 = 99_999_999_999;

/// The smallest magnitude `S9(09)V99` cannot hold: the `ON SIZE ERROR` bound
/// of a store into nine integer digits.
const SIZE_ERROR_9: Decimal = dec!(1000000000.00);

/// The largest `DIS-INT-RATE S9(04)V99` magnitude in hundredths (P-CVTRA02Y-004):
/// 9999.99.
const MAX_RATE_HUNDREDTHS: i32 = 999_999;

/// The smallest reward rate `NUMERIC(9,4)` cannot hold, in `0.0001` units:
/// the rate 100000.0000.
const REWARD_RATE_LIMIT_UNITS: i64 = 1_000_000_000;

/// 2^53 + 1, the smallest whole number a JSON number in a browser cannot hold
/// exactly: the large reward balance of the AAP 0.10.1 o8 check.
const BEYOND_2_POW_53: i64 = 9_007_199_254_740_993;

/// The proptest configuration: the default case count, and no failure
/// persistence, so a failing run writes no `proptest-regressions/` directory
/// into the checkout. The shrunk failing case is still printed.
fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// The COBOL store of `value` into an `S9(digits)V99` receiving field, without
/// `ROUNDED` and without `ON SIZE ERROR` (P-CBACT04C-011, AAP 0.9.2).
///
/// Truncates toward zero to two fraction digits, keeps the low-order
/// `digits + 2` digits of the cent count, puts the sign back and scales by 100.
/// A zero result carries no sign, as a stored zero is positive
/// (P-CBACT04C-018). The arithmetic is this file's own: it restates the rule
/// rather than calling into `money.rs`.
fn reference_store(value: Decimal, digits: u32) -> Decimal {
    let truncated = value.trunc_with_scale(2);
    let magnitude = i128::try_from((truncated * Decimal::ONE_HUNDRED).abs())
        .expect("a value truncated to 2 places is a whole number of cents");
    let kept = magnitude % 10_i128.pow(digits + 2);
    let signed = if value.is_sign_negative() {
        -kept
    } else {
        kept
    };
    Decimal::from_i128_with_scale(signed, 2)
}

/// `cents` hundredths as a scale-2 `Decimal`, the exact value of a money field.
fn amount(cents: i64) -> Decimal {
    Decimal::new(cents, 2)
}

/// The `S9(D)V99` field holding `cents`, which the strategies keep in range.
fn money<const D: u8>(cents: i64) -> Money<D> {
    Money::from_cents(cents).expect("the strategies draw cent counts the field holds")
}

/// The `S9(04)V99` interest rate holding `hundredths`.
fn interest_rate(hundredths: i32) -> InterestRate {
    InterestRate::from_hundredths(hundredths).expect("the strategy draws rates S9(04)V99 holds")
}

/// The reward rate of `units` ten-thousandths.
fn reward_rate(units: i64) -> RewardRate {
    RewardRate::from_decimal(Decimal::new(units, 4))
        .expect("the strategy draws rates NUMERIC(9,4) holds")
}

/// Points held by a `BIGINT` column value the caller keeps non-negative.
fn points(value: i64) -> RewardPoints {
    RewardPoints::try_from(value).expect("the strategies draw non-negative points")
}

/// A `MoneyExpr` holding exactly `value`, built as `value + 0` because the
/// algebra offers no other public constructor.
fn expr<const D: u8>(value: Money<D>) -> MoneyExpr {
    value + Money::<D>::ZERO
}

/// Cent counts of a signed field whose largest count is `max`: uniform over the
/// whole range, small values around zero, and the values next to either limit,
/// where a sum first carries into a dropped digit.
fn cents_up_to(max: i64) -> impl Strategy<Value = i64> {
    prop_oneof![
        4 => -max..=max,
        1 => -1_000_i64..=1_000,
        1 => (max - 1_000)..=max,
        1 => -max..=(1_000 - max),
    ]
}

/// Two cent counts within `max`: equal, one cent apart either way, or
/// independent, so that equality and the neighbouring orders are drawn often.
fn pair_up_to(max: i64) -> impl Strategy<Value = (i64, i64)> {
    (cents_up_to(max), 0_u8..4, cents_up_to(max)).prop_map(move |(first, kind, other)| {
        let second = match kind {
            0 => first,
            1 => (first + 1).min(max),
            2 => (first - 1).max(-max),
            _ => other,
        };
        (first, second)
    })
}

/// Annual interest rates in hundredths over the whole signed `S9(04)V99`
/// range, with the common small rates drawn often.
fn rate_hundredths() -> impl Strategy<Value = i32> {
    prop_oneof![
        3 => -MAX_RATE_HUNDREDTHS..=MAX_RATE_HUNDREDTHS,
        1 => 0_i32..=3_000,
    ]
}

/// Non-negative `S9(09)V99` amounts in cents, with amounts of a few currency
/// units drawn often, where the fraction of a product decides its ceiling.
fn amount_cents() -> impl Strategy<Value = i64> {
    prop_oneof![
        3 => 0..=MAX_CENTS_9,
        1 => 0_i64..=1_000,
    ]
}

/// Reward rates in `0.0001` units: mostly 0.0000 to 9.9999, the rates of the
/// seed rules' order of magnitude, and also the whole `NUMERIC(9,4)` range.
fn reward_rate_units() -> impl Strategy<Value = i64> {
    prop_oneof![
        3 => 0_i64..=99_999,
        1 => 0..REWARD_RATE_LIMIT_UNITS,
    ]
}

/// Point totals over the whole `BIGINT` range, with the values next to both
/// ends and next to 2^53 drawn often.
fn point_totals() -> impl Strategy<Value = i64> {
    prop_oneof![
        4 => 0..=i64::MAX,
        1 => 0_i64..=1_000,
        1 => (i64::MAX - 1_000)..=i64::MAX,
        1 => (BEYOND_2_POW_53 - 1_000)..=(BEYOND_2_POW_53 + 1_000),
        1 => Just(i64::MAX),
    ]
}

/// Checks one store into `S9(D)V99` against [`reference_store`] of the exact
/// value: the stored value, the field's scale 2, and a sign only on a value
/// below zero, so a stored zero is positive.
fn check_store<const D: u8>(stored: Money<D>, exact: Decimal) -> Result<(), TestCaseError> {
    let expected = reference_store(exact, u32::from(D));
    prop_assert_eq!(
        stored.to_decimal(),
        expected,
        "S9({})V99 store of {}",
        D,
        exact
    );
    prop_assert_eq!(stored.to_decimal().scale(), 2, "scale of {}", stored);
    let below_zero = expected < Decimal::ZERO;
    prop_assert_eq!(stored.is_negative(), below_zero, "sign of {}", stored);
    prop_assert_eq!(
        stored.to_decimal().is_sign_negative(),
        below_zero,
        "sign bit of {}",
        stored
    );
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// `MoneyExpr::store` of a sum or difference of two `S9(10)V99` values
    /// keeps the sign and the low-order digits of each receiver: 9 integer
    /// digits (`TRAN-CAT-BAL`, P-CBTRN02C-024, P-CBTRN02C-025), 10
    /// (`ACCT-CURR-BAL`, P-CBTRN02C-026, P-CBTRN02C-226, P-CBTRN02C-227,
    /// P-COBIL00C-022) and 3, a width almost every sum overflows.
    #[test]
    fn store_keeps_the_low_order_digits(
        (a, b) in (cents_up_to(MAX_CENTS_10), cents_up_to(MAX_CENTS_10)),
    ) {
        let (x, y) = (money::<10>(a), money::<10>(b));
        for (result, exact) in [(x + y, amount(a) + amount(b)), (x - y, amount(a) - amount(b))] {
            check_store(result.store::<9>(), exact)?;
            check_store(result.store::<10>(), exact)?;
            check_store(result.store::<3>(), exact)?;
        }
    }

    /// CBTRN02C `COMPUTE WS-TEMP-BAL = ACCT-CURR-CYC-CREDIT - ACCT-CURR-CYC-DEBIT
    /// + DALYTRAN-AMT`: two `S9(10)V99` operands and an `S9(09)V99` one, stored
    /// into the narrower `S9(09)V99` (P-CBTRN02C-022).
    #[test]
    fn the_posting_limit_balance_stores_into_nine_digits(
        credit in cents_up_to(MAX_CENTS_10),
        debit in cents_up_to(MAX_CENTS_10),
        tran_amt in cents_up_to(MAX_CENTS_9),
    ) {
        let temp = (money::<10>(credit) - money::<10>(debit) + money::<9>(tran_amt)).store::<9>();
        check_store(temp, amount(credit) - amount(debit) + amount(tran_amt))?;
    }

    /// The store of an interest quotient, the one value the public algebra
    /// yields whose exact amount has more than two fraction digits: CBACT04C
    /// `COMPUTE WS-MONTHLY-INT = (TRAN-CAT-BAL * DIS-INT-RATE) / 1200` into
    /// `S9(09)V99` truncates the fraction toward zero and drops the high-order
    /// digits (P-CBACT04C-012, P-CBACT04C-019).
    #[test]
    fn an_interest_quotient_store_truncates_both_ends(
        balance in cents_up_to(MAX_CENTS_9),
        rate in rate_hundredths(),
    ) {
        let stored = (money::<9>(balance) * interest_rate(rate)).monthly().store::<9>();
        // The exact quotient's digits past the terminating part repeat 3 or 6
        // (1200 = 2^4 * 3 * 5^2), so rounding the 28th significant digit of the
        // `Decimal` division never reaches the cents.
        let exact = amount(balance) * Decimal::new(i64::from(rate), 2) / dec!(1200);
        check_store(stored, exact)?;
    }

    /// `MoneyExpr::store_checked`, the `ON SIZE ERROR` form of a store into
    /// `S9(09)V99`, fails exactly when the result needs a tenth integer digit,
    /// and otherwise holds the exact result (AAP 0.5.3.1).
    #[test]
    fn store_checked_fails_only_when_the_integer_part_overflows(
        (a, b) in (cents_up_to(MAX_CENTS_10), cents_up_to(MAX_CENTS_10)),
    ) {
        let (x, y) = (money::<10>(a), money::<10>(b));
        for (result, exact) in [(x + y, amount(a) + amount(b)), (x - y, amount(a) - amount(b))] {
            let expected = if exact.abs() < SIZE_ERROR_9 { Ok(exact) } else { Err(Overflow) };
            prop_assert_eq!(result.store_checked::<9>().map(Money::to_decimal), expected);
        }
    }

    /// The numeric `MOVE` between widths. COBIL00C `MOVE ACCT-CURR-BAL TO
    /// TRAN-AMT` is `Money::<10>::store::<9>()` (P-COBIL00C-021); the payment
    /// then leaves only the dropped high-order digit in the balance
    /// (P-COBIL00C-022). A `MOVE` into a field at least as wide changes nothing.
    #[test]
    fn a_move_keeps_the_receiver_digits(balance in cents_up_to(MAX_CENTS_10)) {
        let value = money::<10>(balance);
        let tran_amt = value.store::<9>();
        check_store(tran_amt, value.to_decimal())?;
        check_store(value.store::<3>(), value.to_decimal())?;
        let after = (value - tran_amt).store::<10>();
        check_store(after, value.to_decimal() - reference_store(value.to_decimal(), 9))?;
        prop_assert_eq!(value.store::<10>(), value);
        prop_assert_eq!(value.store::<16>().to_decimal(), value.to_decimal());
    }

    /// Money addition and subtraction are exact (AAP 0.5.3.1: `Money<A> ±
    /// Money<B>` gives an exact `MoneyExpr`). Two `S9(10)V99` operands always
    /// fit `S9(11)V99`, so storing there returns the exact `Decimal` result,
    /// and chained terms cancel without loss.
    #[test]
    fn sums_and_differences_are_exact(
        (a, b) in (cents_up_to(MAX_CENTS_10), cents_up_to(MAX_CENTS_10)),
    ) {
        let (x, y) = (money::<10>(a), money::<10>(b));
        let (sum, difference) = (amount(a) + amount(b), amount(a) - amount(b));
        prop_assert_eq!((x + y).to_decimal(), sum);
        prop_assert_eq!((x - y).to_decimal(), difference);
        prop_assert_eq!((x + y).store::<11>().to_decimal(), sum);
        prop_assert_eq!((x - y).store::<11>().to_decimal(), difference);
        prop_assert_eq!(((x + y) - y).store::<10>(), x);
        prop_assert_eq!((x - (x - y)).store::<10>(), y);
        prop_assert_eq!(((x + y) + (x - y)).to_decimal(), amount(a) * Decimal::TWO);
        prop_assert_eq!(((x + y) - (x - y)).to_decimal(), amount(b) * Decimal::TWO);
    }

    /// Operands of different widths combine exactly too, in either order:
    /// `DALYTRAN-AMT S9(09)V99` with `ACCT-CURR-BAL S9(10)V99`, as in CBTRN02C
    /// `ADD DALYTRAN-AMT TO ACCT-CURR-BAL` (P-CBTRN02C-026), and the COBIL00C
    /// payment `ACCT-CURR-BAL - TRAN-AMT` (P-COBIL00C-022).
    #[test]
    fn mixed_width_sums_are_exact(
        tran_amt in cents_up_to(MAX_CENTS_9),
        balance in cents_up_to(MAX_CENTS_10),
    ) {
        let (narrow, wide) = (money::<9>(tran_amt), money::<10>(balance));
        let (small, large) = (amount(tran_amt), amount(balance));
        prop_assert_eq!((narrow + wide).store::<11>().to_decimal(), small + large);
        prop_assert_eq!((wide + narrow).store::<11>().to_decimal(), small + large);
        prop_assert_eq!((narrow - wide).store::<11>().to_decimal(), small - large);
        prop_assert_eq!((wide - narrow).store::<11>().to_decimal(), large - small);
        prop_assert_eq!(expr(narrow) + wide, wide + narrow);
    }
}

proptest! {
    #![proptest_config(config())]

    /// Comparisons are algebraic, as COBOL compares numeric fields
    /// (`IF ACCT-CREDIT-LIMIT >= WS-TEMP-BAL`, P-CBTRN02C-022;
    /// `IF ACCT-CURR-BAL <= ZEROS`, P-COBIL00C-020): within one width, between
    /// `S9(09)V99` and `S9(10)V99`, and between `MoneyExpr` and `Money`, the
    /// order is the `Decimal` order and `==` is `Decimal` equality.
    #[test]
    fn comparisons_follow_the_decimal_order((a, b) in pair_up_to(MAX_CENTS_9)) {
        let order = amount(a).cmp(&amount(b));
        let equal = amount(a) == amount(b);
        let (a9, b9) = (money::<9>(a), money::<9>(b));
        let (a10, b10) = (money::<10>(a), money::<10>(b));
        prop_assert_eq!(a9.partial_cmp(&b9), Some(order));
        prop_assert_eq!(a9.cmp(&b9), order);
        prop_assert_eq!(a10.cmp(&b10), order);
        prop_assert_eq!(a9.partial_cmp(&b10), Some(order));
        prop_assert_eq!(a10.partial_cmp(&b9), Some(order));
        prop_assert_eq!(a9 == b9, equal);
        prop_assert_eq!(a9 == b10, equal);
        prop_assert_eq!(a10 == b9, equal);
        prop_assert_eq!(expr(a9).partial_cmp(&b10), Some(order));
        prop_assert_eq!(a10.partial_cmp(&expr(b9)), Some(order));
        prop_assert_eq!(expr(a10).cmp(&expr(b9)), order);
        prop_assert_eq!(expr(a9) == b10, equal);
        prop_assert_eq!(a9 <= Money::<10>::ZERO, amount(a) <= Decimal::ZERO);
    }

    /// A stored zero is positive zero (P-CBACT04C-018, E-42): the difference of
    /// a value with itself, before and after the store, and a negative value
    /// whose kept low-order digits are all zero, as when `MOVE` of
    /// -1000000000.00 reaches `S9(09)V99` (P-COBIL00C-021).
    #[test]
    fn a_stored_zero_is_positive(value in cents_up_to(MAX_CENTS_10), high in 1_i64..=9) {
        let field = money::<10>(value);
        let difference = field - field;
        prop_assert!(!difference.to_decimal().is_sign_negative());
        let dropped = money::<10>(-high * 100_000_000_000).store::<9>();
        for zero in [difference.store::<9>(), dropped] {
            prop_assert!(zero.is_zero());
            prop_assert!(!zero.is_negative());
            prop_assert!(!zero.to_decimal().is_sign_negative());
            prop_assert_eq!(zero.to_decimal(), Decimal::ZERO);
            prop_assert_eq!(zero.to_string(), "0.00");
        }
    }

    /// `points = ceil(amount × rate)` over the exact product (AAP 0.3.5 step 4,
    /// AAP 0.9.2): for every `S9(09)V99` amount from zero up and every reward
    /// rate, the points are the least whole number not below the product.
    #[test]
    fn points_are_the_ceiling_of_the_exact_product(
        cents in amount_cents(),
        units in reward_rate_units(),
    ) {
        let exact = amount(cents) * Decimal::new(units, 4);
        let accrued = (money::<9>(cents) * reward_rate(units))
            .ceil()
            .map_err(|overflow| TestCaseError::fail(format!("ceil({exact}): {overflow}")))?;
        let whole = Decimal::from(accrued.to_i64());
        prop_assert!(whole >= exact, "{} points for {}", whole, exact);
        prop_assert!(whole < exact + Decimal::ONE, "{} points for {}", whole, exact);
        if exact.fract().is_zero() {
            prop_assert_eq!(whole, exact);
        }
        prop_assert_eq!(accrued.to_string(), whole.to_string());
    }

    /// A negative product never yields negative points: `PointsExpr::ceil`
    /// gives zero for a product above -1 and `Overflow` at -1 or below
    /// (`numeric/points.rs`). The reward engine never reaches this, because it
    /// accrues nothing for an amount at or below zero (AAP 0.3.5 step 1).
    #[test]
    fn negative_products_never_give_negative_points(
        cents in amount_cents(),
        units in reward_rate_units(),
    ) {
        let exact = amount(-cents) * Decimal::new(units, 4);
        let expected = if exact > -Decimal::ONE { Ok(RewardPoints::ZERO) } else { Err(Overflow) };
        prop_assert_eq!((money::<9>(-cents) * reward_rate(units)).ceil(), expected);
    }

    /// `RewardPoints + RewardPoints` is checked (AAP 0.5.3.1): it holds the sum
    /// exactly when the sum fits `0..=i64::MAX`, the `BIGINT` range, and
    /// reports `Overflow` otherwise; the operator and `checked_add` agree.
    #[test]
    fn point_sums_are_checked_at_i64_max(x in point_totals(), y in point_totals()) {
        let sum = i128::from(x) + i128::from(y);
        let expected = i64::try_from(sum).map(points).map_err(|_| Overflow);
        prop_assert_eq!(points(x).checked_add(points(y)), expected);
        prop_assert_eq!(points(x) + points(y), expected);
        let headroom = i64::MAX - x;
        prop_assert_eq!(points(x) + points(headroom), Ok(points(i64::MAX)));
        if let Some(past) = headroom.checked_add(1) {
            prop_assert_eq!(points(x) + points(past), Err(Overflow));
        }
    }

    /// `RewardPoints - RewardPoints` never goes below zero: points are never
    /// redeemed, so `available = earned - redeemed` stays whole and
    /// non-negative (AAP 0.3.5 invariant), and a larger subtrahend is
    /// `Overflow`.
    #[test]
    fn point_differences_never_go_below_zero(x in point_totals(), y in point_totals()) {
        let expected = if x >= y { Ok(points(x - y)) } else { Err(Overflow) };
        prop_assert_eq!(points(x).checked_sub(points(y)), expected);
        prop_assert_eq!(points(x) - points(y), expected);
        prop_assert_eq!(points(x) - points(x), Ok(RewardPoints::ZERO));
        if let Some(above) = x.checked_add(1) {
            prop_assert_eq!(points(x) - points(above), Err(Overflow));
        }
    }

    /// Points read from a `BIGINT` column convert exactly when non-negative and
    /// are rejected as `Negative` otherwise, and the API's digit-string form
    /// round-trips every value, those above 2^53 included (AAP 0.5.3.1).
    #[test]
    fn points_convert_and_round_trip_as_text(value in point_totals(), below in i64::MIN..0) {
        let held = points(value);
        prop_assert_eq!(held.to_i64(), value);
        let text = held.to_string();
        prop_assert_eq!(&text, &value.to_string());
        prop_assert_eq!(text.parse::<RewardPoints>(), Ok(held));
        prop_assert_eq!(RewardPoints::try_from(below), Err(NumericError::Negative));
    }
}

/// The ends of the `RewardPoints` range through the public API (AAP 0.5.3.1):
/// `i64::MAX` is held and one more overflows, -1 is rejected, zero minus one
/// overflows, and the o8 balance 2^53 + 1 (AAP 0.10.1) round-trips as text.
#[test]
fn reward_points_range_ends() {
    let one = points(1);
    let largest = RewardPoints::try_from(i64::MAX).expect("i64::MAX is a valid points total");
    assert_eq!(largest.to_i64(), i64::MAX);
    assert_eq!(largest.checked_add(one), Err(Overflow));
    assert_eq!(RewardPoints::try_from(-1), Err(NumericError::Negative));
    assert_eq!(RewardPoints::ZERO.checked_sub(one), Err(Overflow));
    assert_eq!(BEYOND_2_POW_53, (1_i64 << 53) + 1);
    let balance: RewardPoints = "9007199254740993"
        .parse()
        .expect("2^53 + 1 is a valid digit string");
    assert_eq!(balance.to_i64(), BEYOND_2_POW_53);
    assert_eq!(balance.to_string(), "9007199254740993");
}

/// `PointsExpr::ceil` reports `Overflow` past `i64::MAX`, the range of the
/// `BIGINT` points columns (AAP 0.5.3.1). At the rate 10000.0000 an amount of
/// 92233720368547758.00 accrues 9223372036854775800 points, and one cent more
/// passes the limit.
#[test]
fn points_beyond_i64_max_overflow() {
    let rate = reward_rate(100_000_000);
    let largest = money::<16>(92_233_720_368_547_758);
    assert_eq!(
        (largest * rate).ceil().map(RewardPoints::to_i64),
        Ok(9_223_372_036_854_775_800)
    );
    let past = money::<16>(92_233_720_368_547_759);
    assert_eq!((past * rate).ceil(), Err(Overflow));
}
