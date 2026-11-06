//! Rates: the COBOL annual interest rate `DIS-INT-RATE`, the reward rate of the
//! reward rules, and the two products they form with money.
//!
//! | Type | Field | SQL column | Audit row |
//! |---|---|---|---|
//! | [`InterestRate`] | `DIS-INT-RATE PIC S9(04)V99`: signed, scale 2, at most 4 integer digits | `NUMERIC(6,2)` | P-CVTRA02Y-004 |
//! | [`RewardRate`] | reward rule `reward_rate` (new): non-negative, at most 4 fraction and 5 integer digits | `NUMERIC(9,4)` | none (no COBOL oracle) |
//!
//! # The interest product (audit section C)
//!
//! CBACT04C `1300-COMPUTE-INTEREST` computes
//! `WS-MONTHLY-INT = (TRAN-CAT-BAL * DIS-INT-RATE) / 1200` into
//! `WS-MONTHLY-INT PIC S9(09)V99`, without `ROUNDED` and without
//! `ON SIZE ERROR` (P-CBACT04C-007, P-CBACT04C-011, P-CBACT04C-012). The
//! product of an `S9(09)V99` balance and an `S9(04)V99` rate is exact with 13
//! integer and 4 fraction digits; the COBOL quotient keeps 4 fraction digits and
//! the store truncates it toward zero to 2, then drops the high-order integer
//! digits beyond 9.
//!
//! `Money<D> * InterestRate` gives that exact product as a [`RatedBalance`]: an
//! integer count of `0.0001` units (cents times rate hundredths). The quotient
//! by 1200 is generally a repeating fraction (`0.01 * 1.00 / 1200` is
//! `0.0000083...`), so it is never formed as a decimal. Instead
//! [`RatedBalance::monthly`] divides the unit count by `1200 * 100 = 120_000`
//! with integer division, which gives the quotient in whole cents truncated
//! toward zero, exactly, for every input. Truncating to 4 places and then to 2
//! equals truncating the exact quotient to 2, so this is the COBOL result. The
//! caller then stores it with `MoneyExpr::store::<9>()`, which keeps the sign and
//! the low-order 9 integer digits and stores a zero as positive zero
//! (P-CBACT04C-018, escalation E-42).
//!
//! | `TRAN-CAT-BAL` | `DIS-INT-RATE` | `monthly()` | stored `S9(09)V99` | Audit row |
//! |---|---|---|---|---|
//! | 0.01 | 1.00 | 0.00 | 0.00 | P-CBACT04C-013 |
//! | 1.00 | 15.00 | 0.01 | 0.01 | P-CBACT04C-014 |
//! | 1200.00 | 1.00 | 1.00 | 1.00 | P-CBACT04C-015 |
//! | 1199.99 | 1.00 | 0.99 | 0.99 | P-CBACT04C-016 |
//! | -1.00 | 15.00 | -0.01 | -0.01 | P-CBACT04C-017 |
//! | -0.01 | 1.00 | 0.00 | 0.00, positive | P-CBACT04C-018 |
//! | 999999999.99 | 9999.99 | 8333324999.91 | 333324999.91 | P-CBACT04C-019 |
//!
//! A zero rate computes nothing: the ported rule tests
//! [`InterestRate::is_zero`] and skips the computation, the SYSTRAN record and
//! the suffix increment (P-CBACT04C-006, P-CBACT04C-022).
//!
//! # The points product
//!
//! `Money<D> * RewardRate` gives the exact [`PointsExpr`]: an amount of scale 2
//! times a rate of scale 4 has scale 6, and nothing is rounded before
//! [`PointsExpr::ceil`] applies the reward rule `points = ceil(amount * rate)`.
//!
//! No other operator is defined: a rate adds to, subtracts from and compares with
//! no money value, and `InterestRate * Money` has no `impl`, so each fails to
//! compile (`crates/domain/tests/newtype_algebra.rs`). No rate converts from any
//! floating-point type.

use std::fmt;
use std::ops::Mul;

use rust_decimal::Decimal;

use super::NumericError;
use super::money::{Money, MoneyExpr};
use super::points::PointsExpr;

/// The scale of `DIS-INT-RATE`: the `V99` of `S9(04)V99`.
const INTEREST_SCALE: u32 = 2;

/// The smallest rate magnitude `S9(04)V99` cannot hold, in hundredths: `10^6`.
const INTEREST_LIMIT_HUNDREDTHS: u32 = 1_000_000;

/// The divisor that turns a [`RatedBalance`] unit count into monthly cents:
/// the COBOL divisor 1200 times the 100 `0.0001` units of one cent.
const MONTHLY_DIVISOR_UNITS: i128 = 1200 * 100;

/// The scale every reward rate is stored at: the 4 fraction digits of
/// `NUMERIC(9,4)`.
const REWARD_SCALE: u32 = 4;

/// The smallest reward rate `NUMERIC(9,4)` cannot hold, in `0.0001` units:
/// `10^9`, the rate `100000`.
const REWARD_LIMIT_UNITS: i64 = 1_000_000_000;

/// The scale of an amount times a reward rate: 2 plus 4.
const POINTS_PRODUCT_SCALE: u32 = INTEREST_SCALE + REWARD_SCALE;

/// The `expect` message of the points product; the `Mul<RewardRate>` impl
/// states the bound.
const EXACT_POINTS_PRODUCT: &str = "an amount below 10^16 times a reward rate below 10^5 has a \
                                    mantissa below 10^27 at scale 6, inside the 96-bit Decimal \
                                    mantissa, so the product is exact";

/// The annual interest rate of a disclosure group, `DIS-INT-RATE PIC S9(04)V99`
/// (P-CVTRA02Y-004).
///
/// Invariant: scale exactly 2, magnitude below `10^4` (fewer than `10^6`
/// hundredths), and a zero is always positive. The field is signed, so negative
/// rates are held as the COBOL holds them. Equal rates have identical decimals,
/// so equality, ordering and hashing agree.
///
/// Its only arithmetic is `Money<D> * InterestRate`, which gives the
/// [`RatedBalance`] of the CBACT04C interest computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterestRate(Decimal);

impl InterestRate {
    /// The zero rate at scale 2, displayed as `0.00`. CBACT04C computes no
    /// interest for it (P-CBACT04C-006).
    pub const ZERO: Self = Self(Decimal::from_parts(0, 0, 0, false, INTEREST_SCALE));

    /// The rate holding `value`, as read from the `NUMERIC(6,2)` column.
    ///
    /// Trailing fraction zeros are accepted (`1.500` is `1.50`) and a negative
    /// zero becomes positive zero. No digit is ever dropped.
    ///
    /// # Errors
    ///
    /// [`NumericError::TooManyDecimals`] when `value` has a non-zero digit
    /// beyond the second fraction place, and [`NumericError::OutOfRange`] when
    /// its magnitude is `10^4` or more.
    pub fn from_decimal(value: Decimal) -> Result<Self, NumericError> {
        let value = value.normalize();
        let scale = value.scale();
        if scale > INTEREST_SCALE {
            return Err(NumericError::TooManyDecimals);
        }
        // A normalized mantissa is below 2^96, so the product fits an i128.
        let hundredths = value.mantissa() * 10_i128.pow(INTEREST_SCALE - scale);
        let hundredths = i32::try_from(hundredths).map_err(|_| NumericError::OutOfRange)?;
        Self::from_hundredths(hundredths)
    }

    /// The rate holding `hundredths`, as a codec decodes the zoned
    /// `S9(04)V99` field.
    ///
    /// # Errors
    ///
    /// [`NumericError::OutOfRange`] when the magnitude of `hundredths` is
    /// `10^6` or more.
    pub fn from_hundredths(hundredths: i32) -> Result<Self, NumericError> {
        if hundredths.unsigned_abs() < INTEREST_LIMIT_HUNDREDTHS {
            Ok(Self(Decimal::new(i64::from(hundredths), INTEREST_SCALE)))
        } else {
            Err(NumericError::OutOfRange)
        }
    }

    /// The rate as a scale-2 `Decimal`, for the SQL column and formatting.
    #[must_use]
    pub fn to_decimal(self) -> Decimal {
        self.0
    }

    /// The rate in hundredths, the digits of the `S9(04)V99` field with their
    /// sign.
    ///
    /// # Panics
    ///
    /// Never: the scale is always 2 and the magnitude below `10^6` hundredths,
    /// so the mantissa fits an `i32`.
    #[must_use]
    pub fn hundredths(self) -> i32 {
        i32::try_from(self.0.mantissa())
            .expect("an InterestRate mantissa is a hundredths count below 10^6, which fits i32")
    }

    /// Whether the rate is zero, the CBACT04C test `IF DIS-INT-RATE NOT = 0`
    /// (P-CBACT04C-006).
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }
}

/// Plain decimal with exactly two fraction digits and a sign only when
/// negative: `15.00`, `-1.50`, `0.00`.
impl fmt::Display for InterestRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The exact product `TRAN-CAT-BAL * DIS-INT-RATE` of CBACT04C
/// `1300-COMPUTE-INTEREST` (P-CBACT04C-012), before the division by 1200.
///
/// It holds an integer count of `0.0001` units: cents times rate hundredths.
/// Built only by `Money<D> * InterestRate`; its only operation is
/// [`monthly`](Self::monthly). It adds to, compares with and converts to
/// nothing, so the undivided product cannot reach a money field.
#[derive(Debug, Clone, Copy)]
pub struct RatedBalance {
    units: i128,
}

impl RatedBalance {
    /// CBACT04C 1300-COMPUTE-INTEREST — P-CBACT04C-012: the monthly interest
    /// `(TRAN-CAT-BAL * DIS-INT-RATE) / 1200` in whole cents, truncated toward
    /// zero with the sign kept.
    ///
    /// The unit count is divided by `120_000` (the COBOL divisor 1200 times the
    /// 100 units of a cent) with integer division, which truncates toward zero.
    /// That is exact for every input: no repeating decimal is ever formed. The
    /// COBOL intermediate quotient keeps 4 fraction digits and its store
    /// truncates them to 2 (P-CBACT04C-011, P-CBACT04C-012); truncating to 4
    /// places and then to 2 equals truncating the exact quotient to 2, which is
    /// this division.
    ///
    /// The result is the exact `MoneyExpr` quotient, whose integer part can
    /// exceed 9 digits. Storing it into `WS-MONTHLY-INT PIC S9(09)V99` is the
    /// caller's job, through `MoneyExpr::store::<9>()`, which drops the
    /// high-order digits and stores a zero as positive zero. Worked values
    /// (P-CBACT04C-013 to P-CBACT04C-019):
    ///
    /// - 0.01 × 1.00 gives 0.00, and 1.00 × 15.00 gives 0.01 (0.0125 truncated);
    /// - 1200.00 × 1.00 gives 1.00, and 1199.99 × 1.00 gives 0.99;
    /// - -1.00 × 15.00 gives -0.01, and -0.01 × 1.00 gives 0.00;
    /// - 999999999.99 × 9999.99 gives 8333324999.91, stored as 333324999.91.
    #[must_use]
    pub fn monthly(self) -> MoneyExpr {
        MoneyExpr::from_cent_count(self.units / MONTHLY_DIVISOR_UNITS)
    }
}

/// `Money<D> * InterestRate`: the exact interest product of CBACT04C
/// `1300-COMPUTE-INTEREST` (P-CBACT04C-012).
///
/// It cannot overflow: a cent count is below `10^18` and a rate below `10^6`
/// hundredths, so the unit count stays below `10^24`, far inside an `i128`.
impl<const D: u8> Mul<InterestRate> for Money<D> {
    type Output = RatedBalance;

    fn mul(self, rate: InterestRate) -> RatedBalance {
        RatedBalance {
            units: i128::from(self.cents()) * i128::from(rate.hundredths()),
        }
    }
}

/// The rate of a reward rule: points per currency unit of a qualifying amount.
///
/// Invariant: `0 <= v < 10^5`, held at scale exactly 4 (fewer than `10^9` units
/// of `0.0001`), the range of the `NUMERIC(9,4)` column. The canonical scale
/// makes equality, ordering and hashing agree and fixes the display: the seed
/// rules' rates show as `1.0000` and `3.0000`. The reward engine picks the
/// highest rate among matching rules through `Ord`.
///
/// Its only arithmetic is `Money<D> * RewardRate`, the exact [`PointsExpr`].
/// The reward rule's validation message for a negative rate belongs to the
/// rewards module; this type reports [`NumericError::Negative`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RewardRate(Decimal);

impl RewardRate {
    /// The zero rate at scale 4, displayed as `0.0000`: a rule that accrues no
    /// points.
    pub const ZERO: Self = Self(Decimal::from_parts(0, 0, 0, false, REWARD_SCALE));

    /// The rate holding `value`, as read from the `NUMERIC(9,4)` column or
    /// accepted from a reward rule request.
    ///
    /// Trailing fraction zeros are accepted (`1.50000` is `1.5000`) and a
    /// negative zero is zero, not negative. No digit is ever dropped or rounded.
    ///
    /// # Errors
    ///
    /// [`NumericError::Negative`] when `value` is below zero,
    /// [`NumericError::TooManyDecimals`] when it has a non-zero digit beyond the
    /// fourth fraction place, and [`NumericError::OutOfRange`] when it is
    /// `10^5` or more.
    pub fn from_decimal(value: Decimal) -> Result<Self, NumericError> {
        if value.is_sign_negative() && !value.is_zero() {
            return Err(NumericError::Negative);
        }
        let value = value.normalize();
        let scale = value.scale();
        if scale > REWARD_SCALE {
            return Err(NumericError::TooManyDecimals);
        }
        // A normalized mantissa is below 2^96, so the product fits an i128.
        let units = value.mantissa() * 10_i128.pow(REWARD_SCALE - scale);
        let units = i64::try_from(units)
            .ok()
            .filter(|units| *units < REWARD_LIMIT_UNITS)
            .ok_or(NumericError::OutOfRange)?;
        Ok(Self(Decimal::new(units, REWARD_SCALE)))
    }

    /// The rate as a scale-4 `Decimal`, for the SQL column and the API.
    #[must_use]
    pub fn to_decimal(self) -> Decimal {
        self.0
    }

    /// Whether the rate is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }
}

/// Plain decimal with exactly four fraction digits: `1.0000`, `1.5000`.
impl fmt::Display for RewardRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// `Money<D> * RewardRate`: the exact product of an amount and a reward rate,
/// at scale 6, which [`PointsExpr::ceil`] rounds up to whole points.
///
/// The product is exact: an amount below `10^16` (fewer than `10^18` cents)
/// times a rate below `10^5` (fewer than `10^9` units of `0.0001`) is below
/// `10^21` at scale 6, a mantissa below `10^27`, inside the `2^96` mantissa of
/// a `Decimal`.
///
/// # Panics
///
/// Never: the bound above keeps every product inside the mantissa. A product
/// outside it would panic rather than accrue rounded points.
impl<const D: u8> Mul<RewardRate> for Money<D> {
    type Output = PointsExpr;

    fn mul(self, rate: RewardRate) -> PointsExpr {
        // Not `Decimal::checked_mul`: it reduces the scale of a product whose
        // mantissa passes 96 bits, rounding it, instead of failing. The integer
        // product of cents and rate units (the mantissa of a scale-4 rate) is
        // exact, and its conversion fails rather than round.
        let units = i128::from(self.cents()) * rate.0.mantissa();
        let product = Decimal::try_from_i128_with_scale(units, POINTS_PRODUCT_SCALE)
            .expect(EXACT_POINTS_PRODUCT);
        PointsExpr::from_exact(product)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::{InterestRate, Money, NumericError, RewardRate};
    use crate::numeric::{Overflow, RewardPoints};

    fn interest(rate: Decimal) -> InterestRate {
        InterestRate::from_decimal(rate).expect("test rate fits S9(04)V99")
    }

    fn reward(rate: Decimal) -> RewardRate {
        RewardRate::from_decimal(rate).expect("test rate fits NUMERIC(9,4)")
    }

    fn balance(value: Decimal) -> Money<9> {
        Money::from_decimal(value).expect("test balance fits S9(09)V99")
    }

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn interest_rate_from_decimal_holds_exact_s9_04_v99_values() {
        let cases = [
            (dec!(15.00), 1500, "15.00"),
            (dec!(15), 1500, "15.00"),
            (dec!(1.5), 150, "1.50"),
            (dec!(1.500), 150, "1.50"),
            (dec!(-1.50), -150, "-1.50"),
            (dec!(9999.99), 999_999, "9999.99"),
            (dec!(-9999.99), -999_999, "-9999.99"),
            (dec!(0.01), 1, "0.01"),
        ];
        for (value, hundredths, shown) in cases {
            let rate = interest(value);
            assert_eq!(rate.hundredths(), hundredths, "{value}");
            assert_eq!(rate.to_string(), shown, "{value}");
            assert_eq!(rate.to_decimal().scale(), 2, "{value}");
            assert!(!rate.is_zero(), "{value}");
        }
    }

    #[test]
    fn interest_rate_from_decimal_rejects_values_the_field_cannot_hold() {
        let cases = [
            (dec!(10000.00), NumericError::OutOfRange),
            (dec!(-10000), NumericError::OutOfRange),
            (Decimal::MAX, NumericError::OutOfRange),
            (Decimal::MIN, NumericError::OutOfRange),
            (dec!(1.234), NumericError::TooManyDecimals),
            (dec!(-0.001), NumericError::TooManyDecimals),
        ];
        for (value, error) in cases {
            assert_eq!(InterestRate::from_decimal(value), Err(error), "{value}");
        }
    }

    #[test]
    fn interest_rate_from_hundredths_enforces_six_digits() {
        let cases = [
            (999_999, Ok(dec!(9999.99))),
            (-999_999, Ok(dec!(-9999.99))),
            (1500, Ok(dec!(15.00))),
            (1_000_000, Err(NumericError::OutOfRange)),
            (-1_000_000, Err(NumericError::OutOfRange)),
            (i32::MAX, Err(NumericError::OutOfRange)),
            (i32::MIN, Err(NumericError::OutOfRange)),
        ];
        for (hundredths, expected) in cases {
            assert_eq!(
                InterestRate::from_hundredths(hundredths).map(InterestRate::to_decimal),
                expected,
                "{hundredths}"
            );
        }
    }

    #[test]
    fn interest_rate_zero_is_a_single_positive_zero() {
        let zeros = [
            InterestRate::ZERO,
            interest(dec!(0)),
            interest(dec!(-0.00)),
            interest(Decimal::from_parts(0, 0, 0, true, 2)),
            InterestRate::from_hundredths(0).expect("zero fits"),
        ];
        for zero in zeros {
            assert!(zero.is_zero());
            assert_eq!(zero, InterestRate::ZERO);
            assert_eq!(zero.hundredths(), 0);
            assert_eq!(zero.to_string(), "0.00");
            assert_eq!(zero.to_decimal().scale(), 2);
            assert!(!zero.to_decimal().is_sign_negative());
            assert_eq!(hash_of(&zero), hash_of(&InterestRate::ZERO));
        }
    }

    #[test]
    fn interest_rates_compare_and_hash_by_value() {
        let from_decimal = interest(dec!(1.50));
        let from_hundredths = InterestRate::from_hundredths(150).expect("fits");
        assert_eq!(from_decimal, from_hundredths);
        assert_eq!(hash_of(&from_decimal), hash_of(&from_hundredths));
        let mut rates = vec![interest(dec!(15)), interest(dec!(-1.5)), InterestRate::ZERO];
        rates.sort();
        assert_eq!(
            rates,
            [interest(dec!(-1.5)), InterestRate::ZERO, interest(dec!(15))]
        );
    }

    #[test]
    fn monthly_interest_reproduces_the_audited_worked_values() -> Result<(), NumericError> {
        // P-CBACT04C-013 to P-CBACT04C-019: balance, rate, exact monthly cents,
        // stored S9(09)V99 text.
        let cases = [
            (dec!(0.01), dec!(1.00), 0, "0.00"),
            (dec!(1.00), dec!(15.00), 1, "0.01"),
            (dec!(1200.00), dec!(1.00), 100, "1.00"),
            (dec!(1199.99), dec!(1.00), 99, "0.99"),
            (dec!(-1.00), dec!(15.00), -1, "-0.01"),
            (dec!(-0.01), dec!(1.00), 0, "0.00"),
            (
                dec!(999999999.99),
                dec!(9999.99),
                833_332_499_991,
                "333324999.91",
            ),
        ];
        for (bal, rate, cents, stored) in cases {
            let monthly =
                (Money::<9>::from_decimal(bal)? * InterestRate::from_decimal(rate)?).monthly();
            assert_eq!(
                monthly.to_decimal(),
                Decimal::new(cents, 2),
                "{bal} x {rate}"
            );
            let field = monthly.store::<9>();
            assert_eq!(field.to_string(), stored, "{bal} x {rate}");
            assert!(
                !field.to_decimal().is_sign_negative() || cents < 0,
                "{bal} x {rate}"
            );
        }
        Ok(())
    }

    #[test]
    fn monthly_interest_keeps_the_sign_of_the_product() {
        let cases = [
            (dec!(1.00), dec!(-15.00), "-0.01"),
            (dec!(-1.00), dec!(-15.00), "0.01"),
            (dec!(-999999999.99), dec!(9999.99), "-333324999.91"),
            (dec!(-999999999.99), dec!(-9999.99), "333324999.91"),
            (dec!(0.00), dec!(9999.99), "0.00"),
        ];
        for (bal, rate, stored) in cases {
            let field = (balance(bal) * interest(rate)).monthly().store::<9>();
            assert_eq!(field.to_string(), stored, "{bal} x {rate}");
        }
    }

    #[test]
    fn monthly_interest_equals_the_truncated_decimal_quotient() {
        // Independent check: the exact quotient p / 1200, every cent fraction
        // of which is a multiple of 1/120000, truncated to 2 places.
        let balances = [
            dec!(0.01),
            dec!(-0.01),
            dec!(1199.99),
            dec!(-1200.01),
            dec!(123456.78),
            dec!(999999999.99),
            dec!(-999999999.99),
        ];
        let rates = [
            dec!(0.01),
            dec!(-0.01),
            dec!(1.00),
            dec!(15.00),
            dec!(24.99),
            dec!(-9999.99),
            dec!(9999.99),
        ];
        for bal in balances {
            for rate in rates {
                let expected = (bal * rate / dec!(1200)).trunc_with_scale(2);
                let monthly = (balance(bal) * interest(rate)).monthly();
                assert_eq!(monthly.to_decimal(), expected, "{bal} x {rate}");
            }
        }
    }

    #[test]
    fn monthly_interest_accepts_every_money_width() {
        let widest = Money::<16>::from_cents(999_999_999_999_999_999).expect("fits");
        let monthly = (widest * interest(dec!(9999.99))).monthly();
        assert_eq!(
            monthly.to_decimal(),
            dec!(83333249999999999.91),
            "10^24 units stay exact"
        );
        let account = Money::<10>::from_cents(120_000).expect("fits");
        assert_eq!(
            (account * interest(dec!(1))).monthly().store::<9>().cents(),
            100
        );
    }

    #[test]
    fn reward_rate_from_decimal_holds_canonical_scale_4_values() {
        let cases = [
            (dec!(1), "1.0000"),
            (dec!(3.0000), "3.0000"),
            (dec!(1.5), "1.5000"),
            (dec!(1.50000), "1.5000"),
            (dec!(0.0001), "0.0001"),
            (dec!(99999.9999), "99999.9999"),
        ];
        for (value, shown) in cases {
            let rate = reward(value);
            assert_eq!(rate.to_string(), shown, "{value}");
            assert_eq!(rate.to_decimal().scale(), 4, "{value}");
            assert_eq!(rate.to_decimal(), value, "{value}");
            assert!(!rate.is_zero(), "{value}");
        }
    }

    #[test]
    fn reward_rate_from_decimal_rejects_values_the_column_cannot_hold() {
        let cases = [
            (dec!(-0.0001), NumericError::Negative),
            (dec!(-1), NumericError::Negative),
            (dec!(-1.23456), NumericError::Negative),
            (Decimal::MIN, NumericError::Negative),
            (dec!(1.23456), NumericError::TooManyDecimals),
            (dec!(0.00001), NumericError::TooManyDecimals),
            (dec!(100000), NumericError::OutOfRange),
            (dec!(100000.0000), NumericError::OutOfRange),
            (Decimal::MAX, NumericError::OutOfRange),
        ];
        for (value, error) in cases {
            assert_eq!(RewardRate::from_decimal(value), Err(error), "{value}");
        }
    }

    #[test]
    fn reward_rate_zero_is_zero_not_negative() {
        for value in [
            dec!(0),
            dec!(-0.0000),
            Decimal::from_parts(0, 0, 0, true, 4),
        ] {
            let zero = reward(value);
            assert!(zero.is_zero(), "{value}");
            assert_eq!(zero, RewardRate::ZERO, "{value}");
            assert_eq!(zero.to_string(), "0.0000", "{value}");
            assert!(!zero.to_decimal().is_sign_negative(), "{value}");
            assert_eq!(hash_of(&zero), hash_of(&RewardRate::ZERO), "{value}");
        }
    }

    #[test]
    fn reward_rates_order_and_hash_by_value() {
        assert!(reward(dec!(3.0000)) > reward(dec!(1.0000)));
        assert!(reward(dec!(1.5)) > reward(dec!(1.4999)));
        assert_eq!(reward(dec!(1.5)), reward(dec!(1.50000)));
        assert_eq!(hash_of(&reward(dec!(1.5))), hash_of(&reward(dec!(1.50000))));
        let best = [reward(dec!(1)), reward(dec!(3)), RewardRate::ZERO]
            .into_iter()
            .max();
        assert_eq!(best, Some(reward(dec!(3))));
    }

    #[test]
    fn points_product_is_exact_before_ceil() {
        let cases = [
            (12_345, dec!(1.5), Ok(186)),
            (12_345, dec!(1), Ok(124)),
            (10_000, dec!(3), Ok(300)),
            (1, dec!(0.0001), Ok(1)),
            (0, dec!(3), Ok(0)),
            (12_345, dec!(0), Ok(0)),
            (99_999_999_999, dec!(99999.9999), Ok(99_999_999_899_001)),
            (-500, dec!(1), Err(Overflow)),
        ];
        for (cents, rate, expected) in cases {
            let amount = Money::<9>::from_cents(cents).expect("test amount fits S9(09)V99");
            let points = (amount * reward(rate)).checked_ceil();
            let expected =
                expected.map(|value| RewardPoints::try_from(value).expect("non-negative points"));
            assert_eq!(points, expected, "{cents} cents x {rate}");
            if let Ok(whole) = points {
                assert_eq!(
                    (amount * reward(rate)).ceil(),
                    whole,
                    "{cents} cents x {rate}"
                );
            }
        }
    }

    #[test]
    fn points_product_stays_exact_at_the_widest_amount() {
        let widest = Money::<16>::from_cents(999_999_999_999_999_999).expect("fits");
        assert_eq!(
            (widest * reward(dec!(99999.9999))).checked_ceil(),
            Err(Overflow)
        );
        let largest = Money::<16>::from_cents(92_233_720_368_547_758).expect("fits");
        assert_eq!(
            (largest * reward(dec!(1)))
                .checked_ceil()
                .map(RewardPoints::to_i64),
            Ok(922_337_203_685_478)
        );
        assert_eq!(
            (largest * reward(dec!(1))).ceil().to_i64(),
            922_337_203_685_478
        );
    }
}
