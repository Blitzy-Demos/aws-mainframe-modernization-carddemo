//! COBOL signed money fields, `S9(D)V99`, and the exact results of their
//! arithmetic.
//!
//! [`Money<D>`](Money) is a stored field: scale exactly 2 and at most `D`
//! integer digits, the COBOL `S9(D)V99` in DISPLAY or COMP-3 usage and the SQL
//! column `NUMERIC(D+2, 2)`. [`MoneyExpr`] is the exact value of an `ADD`,
//! `SUBTRACT` or `COMPUTE` before it reaches a receiving field, and it becomes
//! money again only through [`MoneyExpr::store`], the COBOL store rule.
//!
//! # Widths in use (audit section A)
//!
//! | Type | PIC clause | Fields | Audit rows |
//! |---|---|---|---|
//! | `Money<10>` | `S9(10)V99` | `ACCT-CURR-BAL`, `ACCT-CREDIT-LIMIT`, `ACCT-CASH-CREDIT-LIMIT`, `ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT`; COMP-3 `PA-TRANSACTION-AMT`, `PA-APPROVED-AMT` | P-CVACT01Y-003, P-CVACT01Y-004, P-CVACT01Y-005, P-CVACT01Y-009, P-CVACT01Y-010, P-CIPAUDTY-014, P-CIPAUDTY-015 |
//! | `Money<9>` | `S9(09)V99` | `TRAN-AMT`, `DALYTRAN-AMT`, `TRAN-CAT-BAL`; COMP-3 `PA-CREDIT-LIMIT`, `PA-CASH-LIMIT`, `PA-CREDIT-BALANCE`, `PA-CASH-BALANCE`, `PA-APPROVED-AUTH-AMT`, `PA-DECLINED-AUTH-AMT`; working storage `WS-MONTHLY-INT`, `WS-TOTAL-INT`, `WS-TEMP-BAL`; the reward rule amount bounds | P-CVTRA05Y-006, P-CVTRA06Y-006, P-CVTRA01Y-004, P-CIPAUSMY-005 to P-CIPAUSMY-008, P-CIPAUSMY-011, P-CIPAUSMY-012, P-CBACT04C-020, P-CBTRN02C-022 |
//!
//! # The store rule (audit section C)
//!
//! No COBOL program or copybook in the repository uses `ROUNDED`
//! (P-CBACT04C-011), and no in-scope receiving field has `ON SIZE ERROR`. A
//! store into `Money<D>` therefore:
//!
//! 1. truncates excess fraction digits toward zero;
//! 2. keeps the sign and the low-order `D` integer digits and drops the
//!    high-order ones;
//! 3. stores a zero result as positive zero, also when it came from a negative
//!    value (P-CBACT04C-018; escalation E-42 records the choice).
//!
//! The statements it reproduces include:
//!
//! | COBOL statement | Rust | Audit row |
//! |---|---|---|
//! | COBIL00C `MOVE ACCT-CURR-BAL TO TRAN-AMT` | `balance.store::<9>()` | P-COBIL00C-021 |
//! | COBIL00C `COMPUTE ACCT-CURR-BAL = ACCT-CURR-BAL - TRAN-AMT` | `(balance - amount).store::<10>()` | P-COBIL00C-022 |
//! | CBTRN02C `COMPUTE WS-TEMP-BAL = ACCT-CURR-CYC-CREDIT - ACCT-CURR-CYC-DEBIT + DALYTRAN-AMT` | `(credit - debit + amount).store::<9>()`, then compared with the `Money<10>` credit limit | P-CBTRN02C-022 |
//! | CBTRN02C `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` | `(balance + amount).store::<9>()` | P-CBTRN02C-024, P-CBTRN02C-025 |
//! | CBTRN02C `ADD DALYTRAN-AMT TO ACCT-CURR-BAL` and the cycle totals | `(balance + amount).store::<10>()` | P-CBTRN02C-026 |
//! | CBACT04C `ADD WS-MONTHLY-INT TO WS-TOTAL-INT` | `(total + monthly).store::<9>()` | P-CBACT04C-020 |
//! | CBACT04C `ADD WS-TOTAL-INT TO ACCT-CURR-BAL` | `(balance + total).store::<10>()` | P-CBACT04C-021 |
//! | CBPAUP0C `SUBTRACT PA-APPROVED-AMT FROM PA-APPROVED-AUTH-AMT` | `(total - approved).store::<9>()` | P-CBPAUP0C-004 |
//!
//! The COPAUA0C available amount (P-COPAUA0C-017, P-COPAUA0C-018), its summary
//! totals (P-COPAUA0C-019), the CBTRN03C report totals (P-CBTRN03C-010,
//! P-CBTRN03C-011) and the CBSTM03A statement total (P-CBSTM03A-012) store the
//! same way.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Add, Sub};

use rust_decimal::Decimal;

use super::{NumericError, Overflow};

/// The scale of every money value: the `V99` of the PIC clause.
const SCALE: u32 = 2;

/// The `expect` message of a money addition or subtraction; [`MoneyExpr`]
/// states the bound that keeps every such result exact.
const EXACT_SUM: &str = "a money sum or difference stays below 10^26 at scale 2, \
                         inside the 96-bit Decimal mantissa, so it is exact";

/// `10^n` as an `i64`: the cent limit of a field of `n - 2` integer digits.
const fn pow10(n: u8) -> i64 {
    let mut value: i64 = 1;
    let mut exponent = 0;
    while exponent < n {
        value *= 10;
        exponent += 1;
    }
    value
}

/// A signed COBOL money field `S9(D)V99`: scale exactly 2 and at most `D`
/// integer digits.
///
/// The value never carries more than two fraction digits, its magnitude stays
/// below `10^D`, and a zero is always positive. `D` ranges over `1..=16`, so
/// every cent count fits an `i64`; any other width fails to compile where a
/// value of that width is built.
///
/// Arithmetic gives an exact [`MoneyExpr`], which [`MoneyExpr::store`] turns
/// back into a field of any width. Money of different widths compares directly,
/// as COBOL compares numeric fields algebraically
/// (`IF ACCT-CREDIT-LIMIT >= WS-TEMP-BAL`, P-CBTRN02C-022). Because the
/// comparison is generic over the other width, a comparison with the zero
/// constant names it, as in `balance <= Money::<10>::ZERO` (the COBIL00C
/// `IF ACCT-CURR-BAL <= ZEROS`, P-COBIL00C-020); [`Money::is_zero`] and
/// [`Money::is_negative`] need no width.
///
/// Deliberately not implemented, so that each fails to compile: `Div`, `Neg`,
/// `Rem` and `Sum`; `Mul` by anything but an `InterestRate` or a `RewardRate`
/// (the two products `rates.rs` defines); any `Add` or `Sub` with an
/// `InterestRate`, a `RewardRate`, `RewardPoints`, a `Decimal` or an integer;
/// `From` or `TryFrom` of any floating-point or integer type; comparisons with
/// raw numbers or `Decimal`; and any rounding.
#[derive(Debug, Clone, Copy)]
pub struct Money<const D: u8>(Decimal);

impl<const D: u8> Money<D> {
    /// The smallest cent count the field cannot hold, `10^(D+2)`.
    ///
    /// Every constructor evaluates it, so a width outside `1..=16` is rejected
    /// at compile time.
    const LIMIT_CENTS: i64 = {
        assert!(D >= 1, "Money<D> needs at least one integer digit");
        assert!(D <= 16, "Money<D> holds at most 16 integer digits");
        pow10(D + 2)
    };

    /// Zero at scale 2, so it displays as `0.00`.
    pub const ZERO: Self = {
        assert!(Self::LIMIT_CENTS > 0, "Money<D> needs a width in 1..=16");
        Self(Decimal::from_parts(0, 0, 0, false, SCALE))
    };

    /// The field holding `value`, which must already fit it exactly.
    ///
    /// Trailing fraction zeros are accepted (`12.340` is `12.34`) and a
    /// negative zero becomes positive zero. No digit is ever dropped: use
    /// [`MoneyExpr::store`] for the COBOL truncation.
    ///
    /// # Errors
    ///
    /// [`NumericError::TooManyDecimals`] when `value` has a non-zero digit
    /// beyond the second fraction place, and [`NumericError::OutOfRange`] when
    /// its magnitude is `10^D` or more.
    pub fn from_decimal(value: Decimal) -> Result<Self, NumericError> {
        let value = value.normalize();
        let scale = value.scale();
        if scale > SCALE {
            return Err(NumericError::TooManyDecimals);
        }
        let cents = value.mantissa() * 10_i128.pow(SCALE - scale);
        let cents = i64::try_from(cents).map_err(|_| NumericError::OutOfRange)?;
        Self::from_cents(cents)
    }

    /// The field holding `cents` hundredths, as a codec decodes a zoned or
    /// packed `S9(D)V99` field.
    ///
    /// # Errors
    ///
    /// [`NumericError::OutOfRange`] when the magnitude of `cents` is
    /// `10^(D+2)` or more.
    pub fn from_cents(cents: i64) -> Result<Self, NumericError> {
        if cents.unsigned_abs() < Self::LIMIT_CENTS.unsigned_abs() {
            Ok(Self(Decimal::new(cents, SCALE)))
        } else {
            Err(NumericError::OutOfRange)
        }
    }

    /// The value as a scale-2 `Decimal`, for the SQL column and formatting.
    #[must_use]
    pub fn to_decimal(self) -> Decimal {
        self.0
    }

    /// The value in hundredths, the digits of the `S9(D)V99` field with their
    /// sign.
    ///
    /// # Panics
    ///
    /// Never: the scale is always 2 and the magnitude below `10^18`, so the
    /// mantissa fits an `i64`.
    #[must_use]
    pub fn cents(self) -> i64 {
        i64::try_from(self.0.mantissa())
            .expect("a Money mantissa is a cent count below 10^18, which fits i64")
    }

    /// Whether the value is below zero. A zero is never negative.
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.0.is_sign_negative()
    }

    /// Whether the value is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// The numeric `MOVE` of this field into an `S9(E)V99` receiver: the sign
    /// and the low-order `E` integer digits are kept.
    ///
    /// COBIL00C `MOVE ACCT-CURR-BAL TO TRAN-AMT` (P-COBIL00C-021) is
    /// `balance.store::<9>()`: 1234567890.12 becomes 234567890.12.
    #[must_use]
    pub fn store<const E: u8>(self) -> Money<E> {
        MoneyExpr(self.0).store::<E>()
    }
}

impl<const A: u8, const B: u8> PartialEq<Money<B>> for Money<A> {
    fn eq(&self, other: &Money<B>) -> bool {
        self.0 == other.0
    }
}

impl<const D: u8> Eq for Money<D> {}

impl<const A: u8, const B: u8> PartialOrd<Money<B>> for Money<A> {
    fn partial_cmp(&self, other: &Money<B>) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

impl<const D: u8> Ord for Money<D> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

// The scale is fixed at 2, so equal values have identical decimals and hash
// alike, as `Eq` requires.
impl<const D: u8> Hash for Money<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<const D: u8> PartialEq<MoneyExpr> for Money<D> {
    fn eq(&self, other: &MoneyExpr) -> bool {
        self.0 == other.0
    }
}

impl<const D: u8> PartialOrd<MoneyExpr> for Money<D> {
    fn partial_cmp(&self, other: &MoneyExpr) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

/// Plain decimal with exactly two fraction digits and a sign only when
/// negative: `-12.34`, `0.00`, `1234567890.12`.
impl<const D: u8> fmt::Display for Money<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The exact value of a money `ADD`, `SUBTRACT` or `COMPUTE`, before it reaches
/// a receiving field.
///
/// It comes from adding and subtracting [`Money`] of any widths and other
/// `MoneyExpr` values, and from `RatedBalance::monthly()` for the CBACT04C
/// interest (P-CBACT04C-012). It leaves the algebra only through
/// [`MoneyExpr::store`], [`MoneyExpr::store_checked`] or
/// [`MoneyExpr::to_decimal`], and compares with `Money` of any width.
///
/// Every addition and subtraction is exact. Operands are scale-2 values below
/// `10^16`, and a `Decimal` holds a 96-bit mantissa, about `7.9 × 10^26` at
/// scale 2, so a result loses a digit only after on the order of `10^10`
/// chained maximal operations, which no COBOL statement performs. An operation
/// that would lose one panics instead of rounding.
///
/// Deliberately not implemented, so that each fails to compile: `Mul`, `Div`,
/// `Neg`, `Rem` and `Sum`; any `Add`, `Sub` or `Mul` with an `InterestRate`, a
/// `RewardRate`, `RewardPoints`, a `Decimal` or an integer; `From` or `TryFrom`
/// of any floating-point or integer type; and any rounding method, because no
/// COBOL statement in the repository rounds (P-CBACT04C-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MoneyExpr(Decimal);

impl MoneyExpr {
    /// The exact amount of `cents` hundredths.
    ///
    /// `RatedBalance::monthly()` passes its truncated cent quotient here.
    ///
    /// # Panics
    ///
    /// When the magnitude of `cents` exceeds the 96-bit `Decimal` mantissa
    /// (about `7.9 × 10^28`). The interest quotient stays below `10^20`, so it
    /// never does.
    #[must_use]
    pub(crate) fn from_cent_count(cents: i128) -> Self {
        Self(
            Decimal::try_from_i128_with_scale(cents, SCALE)
                .expect("an interest cent quotient stays below 10^20, inside the Decimal mantissa"),
        )
    }

    /// The exact value, for edited-picture formatting and tests.
    #[must_use]
    pub fn to_decimal(self) -> Decimal {
        self.0
    }

    /// The store into an `S9(D)V99` receiving field without `ON SIZE ERROR`
    /// and without `ROUNDED`.
    ///
    /// Excess fraction digits are truncated toward zero, the sign and the
    /// low-order `D` integer digits are kept, and a zero result is positive
    /// zero (P-CBACT04C-011, P-CBACT04C-018, P-CBACT04C-020, P-CBACT04C-021,
    /// P-COBIL00C-022, P-CBTRN02C-022, P-CBTRN02C-024 to P-CBTRN02C-026). For
    /// example 1500000000.00 stored into `S9(09)V99` is 500000000.00, and
    /// -1000000000.00 is 0.00.
    ///
    /// # Panics
    ///
    /// Never: the kept remainder is below `10^18` in magnitude, so it fits an
    /// `i64`.
    #[must_use]
    pub fn store<const D: u8>(self) -> Money<D> {
        // Rust's `%` keeps the sign of the dividend: the sign and the low-order
        // digits survive, the high-order digits are dropped, and an integer
        // zero carries no sign, so the stored zero is positive.
        let kept = self.truncated_cents() % i128::from(Money::<D>::LIMIT_CENTS);
        let kept =
            i64::try_from(kept).expect("a cent count reduced modulo 10^(D+2), D <= 16, fits i64");
        Money(Decimal::new(kept, SCALE))
    }

    /// The store into an `S9(D)V99` receiving field with `ON SIZE ERROR`.
    ///
    /// Truncating excess fraction digits is not a size error; only an integer
    /// part wider than `D` digits is. No in-scope COBOL statement has
    /// `ON SIZE ERROR` (audit section C), so the ported rules use
    /// [`MoneyExpr::store`].
    ///
    /// # Errors
    ///
    /// [`Overflow`] when the value, truncated to cents, is `10^D` or more in
    /// magnitude.
    pub fn store_checked<const D: u8>(self) -> Result<Money<D>, Overflow> {
        i64::try_from(self.truncated_cents())
            .ok()
            .and_then(|cents| Money::<D>::from_cents(cents).ok())
            .ok_or(Overflow)
    }

    /// The value in hundredths after truncating excess fraction digits toward
    /// zero.
    fn truncated_cents(self) -> i128 {
        // `trunc_with_scale` truncates, where `rescale` and `round` would round,
        // and it leaves a scale of at most 2.
        let truncated = self.0.trunc_with_scale(SCALE);
        truncated.mantissa() * 10_i128.pow(SCALE - truncated.scale())
    }

    /// The exact result of an addition or subtraction, zero kept positive.
    ///
    /// Operands have scale 2, so an exact result has scale 2; `Decimal`
    /// reduces the scale only when the mantissa overflows, which `EXACT_SUM`
    /// rules out.
    fn exact(result: Option<Decimal>) -> Self {
        let mut value = result
            .filter(|value| value.scale() == SCALE)
            .expect(EXACT_SUM);
        // `Decimal` can carry a negative zero (`-0.01 - -0.01`); an exact value
        // has a single zero.
        if value.is_zero() {
            value.set_sign_positive(true);
        }
        Self(value)
    }
}

/// `Money<A> + Money<B>`: the exact sum, whatever the two widths.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const A: u8, const B: u8> Add<Money<B>> for Money<A> {
    type Output = MoneyExpr;

    fn add(self, rhs: Money<B>) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_add(rhs.0))
    }
}

/// `Money<A> - Money<B>`: the exact difference, whatever the two widths.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const A: u8, const B: u8> Sub<Money<B>> for Money<A> {
    type Output = MoneyExpr;

    fn sub(self, rhs: Money<B>) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_sub(rhs.0))
    }
}

/// `MoneyExpr + Money<B>`: a further exact term of a `COMPUTE`.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const B: u8> Add<Money<B>> for MoneyExpr {
    type Output = MoneyExpr;

    fn add(self, rhs: Money<B>) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_add(rhs.0))
    }
}

/// `MoneyExpr - Money<B>`: a further exact term of a `COMPUTE`.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const B: u8> Sub<Money<B>> for MoneyExpr {
    type Output = MoneyExpr;

    fn sub(self, rhs: Money<B>) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_sub(rhs.0))
    }
}

/// `Money<A> + MoneyExpr`: an exact sum with an intermediate result.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const A: u8> Add<MoneyExpr> for Money<A> {
    type Output = MoneyExpr;

    fn add(self, rhs: MoneyExpr) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_add(rhs.0))
    }
}

/// `Money<A> - MoneyExpr`: an exact difference with an intermediate result.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl<const A: u8> Sub<MoneyExpr> for Money<A> {
    type Output = MoneyExpr;

    fn sub(self, rhs: MoneyExpr) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_sub(rhs.0))
    }
}

/// `MoneyExpr + MoneyExpr`: the exact sum of two intermediate results.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl Add for MoneyExpr {
    type Output = MoneyExpr;

    fn add(self, rhs: MoneyExpr) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_add(rhs.0))
    }
}

/// `MoneyExpr - MoneyExpr`: the exact difference of two intermediate results.
///
/// # Panics
///
/// Only if the result would lose a digit, which takes on the order of `10^10`
/// chained maximal operations (see [`MoneyExpr`]).
impl Sub for MoneyExpr {
    type Output = MoneyExpr;

    fn sub(self, rhs: MoneyExpr) -> MoneyExpr {
        MoneyExpr::exact(self.0.checked_sub(rhs.0))
    }
}

impl<const D: u8> PartialEq<Money<D>> for MoneyExpr {
    fn eq(&self, other: &Money<D>) -> bool {
        self.0 == other.0
    }
}

impl<const D: u8> PartialOrd<Money<D>> for MoneyExpr {
    fn partial_cmp(&self, other: &Money<D>) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

/// Plain decimal at the value's scale, a sign only when negative: `-0.50`,
/// `1500000000.00`.
impl fmt::Display for MoneyExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::{Money, MoneyExpr, NumericError, Overflow};

    fn m9(cents: i64) -> Money<9> {
        Money::from_cents(cents).expect("test value fits S9(09)V99")
    }

    fn m10(cents: i64) -> Money<10> {
        Money::from_cents(cents).expect("test value fits S9(10)V99")
    }

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn from_decimal_accepts_exact_values_and_rejects_excess_fraction_digits() {
        assert_eq!(
            Money::<9>::from_decimal(dec!(12.345)),
            Err(NumericError::TooManyDecimals)
        );
        let trailing_zero = Money::<9>::from_decimal(dec!(12.340)).expect("12.340 is 12.34");
        assert_eq!(trailing_zero.cents(), 1234);
        assert_eq!(trailing_zero.to_string(), "12.34");
        let whole = Money::<9>::from_decimal(dec!(5)).expect("5 is 5.00");
        assert_eq!(whole.to_string(), "5.00");
        assert_eq!(whole.to_decimal().scale(), 2);
    }

    #[test]
    fn from_decimal_enforces_the_integer_width() {
        let cases = [
            (dec!(1000000000.00), Err(NumericError::OutOfRange)),
            (dec!(-1000000000.00), Err(NumericError::OutOfRange)),
            (dec!(999999999.99), Ok(99_999_999_999)),
            (dec!(-999999999.99), Ok(-99_999_999_999)),
            (Decimal::MAX, Err(NumericError::OutOfRange)),
            (Decimal::MIN, Err(NumericError::OutOfRange)),
        ];
        for (value, expected) in cases {
            assert_eq!(
                Money::<9>::from_decimal(value).map(Money::cents),
                expected,
                "{value}"
            );
        }
    }

    #[test]
    fn negative_zero_enters_as_positive_zero() {
        for value in [dec!(-0.00), Decimal::from_parts(0, 0, 0, true, 2)] {
            let zero = Money::<9>::from_decimal(value).expect("zero fits");
            assert!(zero.is_zero());
            assert!(!zero.is_negative());
            assert!(!zero.to_decimal().is_sign_negative());
            assert_eq!(zero.to_string(), "0.00");
        }
    }

    #[test]
    fn from_cents_enforces_the_width_and_displays_two_places() {
        assert_eq!(m9(-1234).to_string(), "-12.34");
        assert_eq!(Money::<9>::ZERO.to_string(), "0.00");
        assert_eq!(m10(123_456_789_012).to_string(), "1234567890.12");
        assert_eq!(
            Money::<9>::from_cents(100_000_000_000),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(
            Money::<1>::from_cents(999).map(Money::cents),
            Ok(999),
            "S9(01)V99 holds 9.99"
        );
        assert_eq!(Money::<1>::from_cents(-1000), Err(NumericError::OutOfRange));
        assert_eq!(
            Money::<16>::from_cents(999_999_999_999_999_999).map(Money::cents),
            Ok(999_999_999_999_999_999)
        );
        assert_eq!(
            Money::<16>::from_cents(i64::MIN),
            Err(NumericError::OutOfRange)
        );
    }

    #[test]
    fn accessors_report_the_value() {
        let debit = m10(-3000);
        assert_eq!(debit.to_decimal(), dec!(-30.00));
        assert_eq!(debit.cents(), -3000);
        assert!(debit.is_negative());
        assert!(!debit.is_zero());
        assert!(Money::<10>::ZERO.is_zero());
        assert!(!Money::<10>::ZERO.is_negative());
        assert_eq!(Money::<10>::ZERO.to_decimal().scale(), 2);
    }

    #[test]
    fn move_between_widths_truncates_high_order_digits() {
        // P-COBIL00C-021: MOVE ACCT-CURR-BAL S9(10)V99 TO TRAN-AMT S9(09)V99.
        let cases = [
            (123_456_789_012, 23_456_789_012),
            (-123_456_789_012, -23_456_789_012),
            (194_000, 194_000),
            (-500, -500),
        ];
        for (balance, amount) in cases {
            assert_eq!(m10(balance).store::<9>().cents(), amount, "{balance}");
        }
        let dropped = m10(-100_000_000_000).store::<9>();
        assert!(dropped.is_zero());
        assert!(!dropped.is_negative());
        assert_eq!(dropped.to_string(), "0.00");
    }

    #[test]
    fn store_truncates_excess_fraction_digits_toward_zero() {
        let cases = [
            (dec!(1.239), 123),
            (dec!(-1.239), -123),
            (dec!(0.009), 0),
            (dec!(-0.009), 0),
            (dec!(1234567890.129), 23_456_789_012),
        ];
        for (value, cents) in cases {
            let stored = MoneyExpr(value).store::<9>();
            assert_eq!(stored.cents(), cents, "{value}");
            assert_eq!(stored.to_decimal().scale(), 2, "{value}");
            assert_eq!(stored.is_negative(), cents < 0, "{value}");
        }
    }

    #[test]
    fn store_checked_reports_only_integer_overflow() {
        let at_limit = m10(100_000_000_000) + Money::<10>::ZERO;
        assert_eq!(at_limit.store_checked::<9>(), Err(Overflow));
        let below = m10(-100_000_000_000) + Money::<10>::ZERO;
        assert_eq!(below.store_checked::<9>(), Err(Overflow));
        let largest = m10(99_999_999_999) + Money::<10>::ZERO;
        assert_eq!(largest.store_checked::<9>(), Ok(m9(99_999_999_999)));
        assert_eq!(
            MoneyExpr(dec!(999999999.999)).store_checked::<9>(),
            Ok(m9(99_999_999_999)),
            "fraction truncation is not a size error"
        );
        assert_eq!(MoneyExpr(Decimal::MAX).store_checked::<16>(), Err(Overflow));
    }

    #[test]
    fn bill_payment_shape() {
        // P-COBIL00C-020 to P-COBIL00C-022.
        let balance = m10(123_456_789_012);
        let amount = balance.store::<9>();
        let after = (balance - amount).store::<10>();
        assert_eq!(after, m10(100_000_000_000));
        assert_eq!(after.to_string(), "1000000000.00");

        let balance = m10(194_000);
        let after = (balance - balance.store::<9>()).store::<10>();
        assert!(after.is_zero());
        assert!(after <= Money::<10>::ZERO);
        assert!(m10(-500) <= Money::<10>::ZERO);
        assert!(m10(1) > Money::<10>::ZERO);
    }

    #[test]
    fn posting_limit_check_shape() {
        // P-CBTRN02C-022: WS-TEMP-BAL S9(09)V99 is narrower than its operands.
        let credit = m10(150_000_000_000);
        let debit = Money::<10>::ZERO;
        let amount = Money::<9>::ZERO;
        let temp = (credit - debit + amount).store::<9>();
        assert_eq!(temp, m9(50_000_000_000));
        assert!(m10(60_000_000_000) >= temp);

        // P-CBTRN02C-023: credit 100.00, debit -30.00, amount 10.00.
        let temp = (m10(10_000) - m10(-3000) + m9(1000)).store::<9>();
        assert_eq!(temp.to_string(), "140.00");
    }

    #[test]
    fn category_balance_and_account_adds_shape() {
        // P-CBTRN02C-025, P-CBTRN02C-026, P-CBACT04C-020, P-CBACT04C-021.
        let tcatbal = (m9(99_999_999_999) + m9(2)).store::<9>();
        assert_eq!(tcatbal.cents(), 1);
        let balance = (m10(-1000) + m9(250)).store::<10>();
        assert_eq!(balance.to_string(), "-7.50");
        let total = (Money::<9>::ZERO + MoneyExpr::from_cent_count(1)).store::<9>();
        let account = (m10(100) + total).store::<10>();
        assert_eq!(account.cents(), 101);
    }

    #[test]
    fn from_cent_count_feeds_the_interest_store() {
        // P-CBACT04C-019: the monthly quotient 8333324999.91 keeps 9 digits.
        let monthly = MoneyExpr::from_cent_count(833_332_499_991);
        assert_eq!(monthly.to_string(), "8333324999.91");
        assert_eq!(monthly.store::<9>(), m9(33_332_499_991));
        assert_eq!(MoneyExpr::from_cent_count(-1).to_string(), "-0.01");
        let zero = MoneyExpr::from_cent_count(0).store::<9>();
        assert!(zero.is_zero());
        assert!(!zero.is_negative());
    }

    #[test]
    fn expressions_combine_exactly() {
        let a = m9(100);
        let b = m10(150);
        assert_eq!((a - b).to_string(), "-0.50");
        assert_eq!((a - b).to_decimal(), dec!(-0.50));
        assert_eq!((a + b) + (a - b), m9(200));
        assert_eq!((a + b) - (a - b), m9(300));
        assert_eq!((a + b) - a, b);
        assert_eq!(a - (a + b), m9(-150));
        assert_eq!(a + (a - b), m9(50));
        let widest = Money::<16>::from_cents(999_999_999_999_999_999).expect("fits");
        assert_eq!(
            (widest + widest).store::<16>().cents(),
            999_999_999_999_999_998
        );
        assert_eq!((widest + widest).to_decimal(), dec!(19999999999999999.98));
    }

    #[test]
    fn a_zero_difference_is_positive_zero() {
        let first = m9(-1);
        let second = m9(-1);
        let difference = first - second;
        assert!(!difference.to_decimal().is_sign_negative());
        assert_eq!(difference.to_string(), "0.00");
        let stored = difference.store::<9>();
        assert!(!stored.is_negative());
        assert_eq!(stored.to_string(), "0.00");
    }

    #[test]
    fn comparisons_work_across_widths_and_with_expressions() {
        assert!(
            Money::<10>::from_cents(100).expect("fits")
                >= Money::<9>::from_cents(100).expect("fits")
        );
        assert!(m10(100) == m9(100));
        assert!(m10(99) < m9(100));
        assert!(m9(-1) < m10(0));
        assert!(m10(250) == m9(100) + m9(150));
        assert!(m9(100) + m9(150) == m10(250));
        assert!(m9(100) + m9(150) > m10(249));
        assert!(m10(249) < m9(100) + m9(150));
        assert!(m9(1) - m9(2) < m9(0) + m9(0));
    }

    #[test]
    fn ordering_and_hashing_agree_with_equality() {
        let mut values = vec![m9(500), m9(-200), m9(0), m9(1)];
        values.sort();
        assert_eq!(values, [m9(-200), m9(0), m9(1), m9(500)]);

        let from_cents = m9(1234);
        let from_decimal = Money::<9>::from_decimal(dec!(12.340)).expect("fits");
        let stored = (m9(1000) + m9(234)).store::<9>();
        assert_eq!(from_cents, from_decimal);
        assert_eq!(from_cents, stored);
        assert_eq!(hash_of(&from_cents), hash_of(&from_decimal));
        assert_eq!(hash_of(&from_cents), hash_of(&stored));
    }
}
