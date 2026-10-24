//! Money and rate types for the copybook amount pictures, and the COBOL store rule.
//!
//! Each signed amount picture maps to a type that carries its integer width, so a value cannot be
//! put into a narrower field, or combined with a different unit, except through the verb method
//! COBOL would use (copybook rows `PIC-005`, `PIC-010` of `modern/audit/copybooks.md`):
//!
//! | Picture | Rust type | Copybook fields |
//! |---|---|---|
//! | `S9(10)V99` | `Money<10>` | `ACCT-CURR-BAL`, `ACCT-CREDIT-LIMIT`, `ACCT-CASH-CREDIT-LIMIT`, `ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT` (`CVACT01Y-003`, `-004`, `-005`, `-009`, `-010`) |
//! | `S9(09)V99` | `Money<9>` | `TRAN-AMT`, `DALYTRAN-AMT`, `TRAN-CAT-BAL`, the `CIPAUSMY` packed amounts (`CVTRA05Y-006`, `CVTRA06Y-006`, `CVTRA01Y-004`, `CIPAUSMY-005` to `-012`) |
//! | `S9(04)V99` | `Rate<4>` | `DIS-INT-RATE` (`CVTRA02Y-004`) |
//!
//! # Algebra
//!
//! - `Money<N> ± Money<M>` yields a [`MoneyExpr`], the exact intermediate of a `COMPUTE`, `ADD`
//!   or `SUBTRACT`. A `MoneyExpr` chains with further `+` and `-` (for example
//!   `cyc_credit - cyc_debit + amt`, `ARITH-023`) and leaves only through
//!   [`MoneyExpr::store`]. It has no comparison, no conversion into [`Money`] and no accessor.
//! - `MoneyExpr::from(money)` is the source of a COBOL `MOVE`: a `MOVE` into a narrower field is
//!   `MoneyExpr::from(x).store::<K>()` (`ARITH-020`, `ARITH-040`).
//! - `Money<N> * Rate<R>` yields a [`RateProduct`], whose only operation is division by
//!   [`PERCENT_MONTHS`]. [`monthly_interest`] is that formula and the only path from a rate to
//!   money (`ARITH-001`).
//! - Two [`Money`] values compare across widths with `==`, `<`, `<=`, `>` and `>=`: a COBOL
//!   comparison is exact and stores nothing (`ARITH-026`, `ARITH-037`).
//! - Nothing converts a floating-point value into any of these types, and no rate is added to,
//!   subtracted from or multiplied by another rate.
//!
//! # The store rule
//!
//! No COBOL source in the repository carries `ROUNDED` or `ON SIZE ERROR`, so every receiving
//! field truncates toward zero at its scale and drops the integer digits it has no room for,
//! keeping the sign (the store rule of `modern/audit/arithmetic.md`). [`MoneyExpr::store`]
//! implements that once; every `ARITH-<nnn>` row whose expected result names `store::<K>()`
//! reproduces through it.
//!
//! # Exact comparisons of sums
//!
//! A comparison of a sum with a field, where no receiving picture exists, stores the sum in a
//! field wide enough that no digit can drop, then compares [`Money`] values. The i10 credit-limit
//! guard (`ARITH-045`, escalation `E30`) is written that way:
//!
//! ```
//! use domain::money::{Decimal, Money, MoneyError};
//! # fn main() -> Result<(), MoneyError> {
//! let bal = Money::<10>::new(Decimal::new(0, 2))?;
//! let approved = Money::<9>::new(Decimal::new(230_000, 2))?;
//! let amount = Money::<10>::new(Decimal::new(10_000, 2))?;
//! let limit = Money::<10>::new(Decimal::new(245_000, 2))?;
//! // Three operands below 10^10 sum below 10^11, so 12 integer digits keep every digit.
//! assert!((bal + approved + amount).store::<12>() <= limit);
//! # Ok(())
//! # }
//! ```
//!
//! # Bounds
//!
//! Every [`Money`] value is below 10^18 in magnitude at scale 2, and every [`Rate`] below 10^6.
//! `Decimal` holds about 7.9 × 10^28, so a product of a money and a rate (below 10^24 at scale 4)
//! is exact, and no audited chain of additions and subtractions comes near the representable
//! range. The arithmetic below saturates instead of panicking, so no input can abort a caller;
//! saturation is reachable only through [`MoneyExpr::from_decimal`] given a value no copybook
//! field can hold.

use core::cmp::Ordering;
use core::fmt;
use core::ops::{Add, Div, Mul, Sub};

use rust_decimal::RoundingStrategy;

/// The decimal type behind every amount and rate, re-exported so consumers name one type path.
pub use rust_decimal::Decimal;

/// Scale of every amount and rate picture in scope (`V99`).
const SCALE: u32 = 2;

/// Widest integer part a [`Money`] may declare: the widest money picture is `S9(10)V99`, and the
/// widest exact sum the guards store is `Money<12>`; 18 digits keep `10^INT` within a `u64`.
const MAX_MONEY_INT: u32 = 18;

/// Widest integer part a [`Rate`] may declare; the only rate picture is `S9(04)V99`.
const MAX_RATE_INT: u32 = 6;

/// `10^digits` as a `Decimal`. Callers const-assert `digits <= 18`, so `checked_pow` stays below
/// `u64::MAX` and the `Decimal::MAX` fallback is never taken.
fn power_of_ten(digits: u32) -> Decimal {
    10_u64
        .checked_pow(digits)
        .map_or(Decimal::MAX, Decimal::from)
}

/// Why a decimal value does not fit a money or rate picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoneyError {
    /// The value has more than two significant decimal places, so the `V99` picture cannot hold
    /// it exactly.
    ScaleTooLarge,
    /// The magnitude reaches `10^INT`, beyond the picture's integer digits.
    OutOfRange,
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ScaleTooLarge => "value has more than two decimal places",
            Self::OutOfRange => "value exceeds the integer digits of its picture",
        })
    }
}

impl std::error::Error for MoneyError {}

/// Validates `value` against a `V99` picture with `int_digits` integer digits and returns it at
/// scale exactly 2.
fn fit(value: Decimal, int_digits: u32) -> Result<Decimal, MoneyError> {
    // normalize strips trailing zeros, so the scale left is the count of significant decimals,
    // and turns a negative zero into zero.
    let mut fitted = value.normalize();
    if fitted.scale() > SCALE {
        return Err(MoneyError::ScaleTooLarge);
    }
    if fitted.abs() >= power_of_ten(int_digits) {
        return Err(MoneyError::OutOfRange);
    }
    fitted.rescale(SCALE);
    Ok(fitted)
}

/// A signed amount of picture `S9(INT)V99`, DISPLAY or `COMP-3`: at most `INT` integer digits and
/// exactly two decimals.
///
/// `INT` must lie between 1 and 18; any other width fails to compile. The value is always held
/// at scale 2 and is never a negative zero.
#[derive(Clone, Copy, Debug)]
pub struct Money<const INT: u32>(Decimal);

impl<const INT: u32> Money<INT> {
    /// Zero, at scale 2.
    pub const ZERO: Self = Self::from_scaled(Decimal::from_parts(0, 0, 0, false, SCALE));

    /// The single constructor every [`Money`] value passes through. The inline const block
    /// rejects an invalid width at compile time, when the width is instantiated.
    const fn from_scaled(value: Decimal) -> Self {
        const {
            assert!(
                INT != 0 && INT <= MAX_MONEY_INT,
                "Money<INT> needs 1 <= INT <= 18"
            );
        };
        Self(value)
    }

    /// The amount `value`, which must fit the picture exactly.
    ///
    /// Trailing zeros beyond two decimals are accepted (`1.230` is `1.23`); a negative zero
    /// becomes zero.
    ///
    /// # Errors
    ///
    /// [`MoneyError::ScaleTooLarge`] when `value` has a non-zero digit beyond the second decimal
    /// place, and [`MoneyError::OutOfRange`] when its magnitude is `10^INT` or more.
    pub fn new(value: Decimal) -> Result<Self, MoneyError> {
        fit(value, INT).map(Self::from_scaled)
    }

    /// Whether the amount is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// Whether the amount is below zero.
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0.is_sign_negative() && !self.0.is_zero()
    }

    /// Whether the amount is above zero.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.0.is_sign_positive() && !self.0.is_zero()
    }

    /// The amount at scale 2, read-only, for codecs, edited pictures and SQL binds.
    #[must_use]
    pub const fn as_decimal(self) -> Decimal {
        self.0
    }
}

impl<const A: u32, const B: u32> PartialEq<Money<B>> for Money<A> {
    fn eq(&self, other: &Money<B>) -> bool {
        self.0 == other.0
    }
}

impl<const A: u32> Eq for Money<A> {}

impl<const A: u32, const B: u32> PartialOrd<Money<B>> for Money<A> {
    fn partial_cmp(&self, other: &Money<B>) -> Option<Ordering> {
        Some(self.0.cmp(&other.0))
    }
}

/// The exact intermediate of a COBOL `COMPUTE`, `ADD`, `SUBTRACT` or `MOVE`, before it reaches
/// its receiving field.
///
/// It is not money: it cannot be compared, read or converted, and becomes a [`Money`] only
/// through [`MoneyExpr::store`].
#[derive(Clone, Copy, Debug)]
#[must_use = "an intermediate reaches a field only through `store`"]
pub struct MoneyExpr(Decimal);

impl MoneyExpr {
    /// The exact intermediate of a `COMPUTE` over a parsed value, such as the `FUNCTION NUMVAL`
    /// amount of `COPAUA0C.cbl:378` or the `NUMVAL-C` of screen input (`X-ARITH-1` of
    /// `modern/audit/arithmetic.md`). The caller must [`store`](Self::store) it into the
    /// receiving field.
    pub const fn from_decimal(value: Decimal) -> Self {
        Self(value)
    }

    /// Stores the intermediate into a receiving field of picture `S9(K)V99`, as COBOL does with
    /// no `ROUNDED` and no `ON SIZE ERROR` phrase: truncated toward zero at 0.01, then the
    /// integer part modulo `10^K` with the sign kept.
    ///
    /// A result whose kept digits are all zero is positive zero. Where the true result is
    /// negative, the COBOL sign of that zero has no single value (`X-ARITH-7`), and codec A
    /// rejects a negative zero (`PIC-008`, `PIC-010`), so positive zero is stored and not claimed
    /// as parity. `K` must lie between 1 and 18; any other width fails to compile.
    #[must_use]
    pub fn store<const K: u32>(self) -> Money<K> {
        const {
            assert!(
                K != 0 && K <= MAX_MONEY_INT,
                "store::<K> needs 1 <= K <= 18"
            );
        };
        // TRUNCATION: COBOL COMPUTE, ADD, SUBTRACT and MOVE without ROUNDED or ON SIZE ERROR
        // truncate toward zero at the receiver's scale and drop the high-order integer digits
        // the receiver has no room for, keeping the sign. This narrowing is that mandate, as the
        // store rule of modern/audit/arithmetic.md states for the rows that store through it:
        // ARITH-001 to ARITH-008, ARITH-010, ARITH-011, ARITH-013, ARITH-020 to ARITH-025,
        // ARITH-028 to ARITH-036, ARITH-040, ARITH-043, ARITH-047 and ARITH-048.
        let truncated = self
            .0
            .round_dp_with_strategy(SCALE, RoundingStrategy::ToZero);
        // The divisor 10^K is never zero, so checked_rem always yields a value; the remainder
        // takes the dividend's sign.
        let mut kept = truncated.checked_rem(power_of_ten(K)).unwrap_or(truncated);
        if kept.is_zero() {
            kept.set_sign_positive(true);
        }
        kept.rescale(SCALE);
        Money::from_scaled(kept)
    }
}

impl<const N: u32> From<Money<N>> for MoneyExpr {
    fn from(money: Money<N>) -> Self {
        Self(money.0)
    }
}

impl<const N: u32, const M: u32> Add<Money<M>> for Money<N> {
    type Output = MoneyExpr;

    fn add(self, rhs: Money<M>) -> MoneyExpr {
        MoneyExpr(self.0.saturating_add(rhs.0))
    }
}

impl<const N: u32, const M: u32> Sub<Money<M>> for Money<N> {
    type Output = MoneyExpr;

    fn sub(self, rhs: Money<M>) -> MoneyExpr {
        MoneyExpr(self.0.saturating_sub(rhs.0))
    }
}

impl<const K: u32> Add<Money<K>> for MoneyExpr {
    type Output = Self;

    fn add(self, rhs: Money<K>) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl<const K: u32> Sub<Money<K>> for MoneyExpr {
    type Output = Self;

    fn sub(self, rhs: Money<K>) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Add for MoneyExpr {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for MoneyExpr {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

/// A signed rate of picture `S9(INT)V99`, such as `DIS-INT-RATE PIC S9(04)V99` (`CVTRA02Y-004`).
///
/// `INT` must lie between 1 and 6; any other width fails to compile. A rate is never added to
/// anything; it reaches money only through [`monthly_interest`].
// Eq, beyond equality, lets record structs that hold a rate derive Eq.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate<const INT: u32>(Decimal);

impl<const INT: u32> Rate<INT> {
    /// The rate `value`, which must fit the picture exactly.
    ///
    /// # Errors
    ///
    /// [`MoneyError::ScaleTooLarge`] when `value` has a non-zero digit beyond the second decimal
    /// place, and [`MoneyError::OutOfRange`] when its magnitude is `10^INT` or more.
    pub fn new(value: Decimal) -> Result<Self, MoneyError> {
        const {
            assert!(
                INT != 0 && INT <= MAX_RATE_INT,
                "Rate<INT> needs 1 <= INT <= 6"
            );
        };
        fit(value, INT).map(Self)
    }

    /// Whether the rate is zero: CBACT04C computes no interest for such a record
    /// (`IF DIS-INT-RATE NOT = 0`, `CBACT04C.cbl:214`, `ARITH-015`).
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// The rate at scale 2, read-only, for codecs and SQL binds.
    #[must_use]
    pub const fn as_decimal(self) -> Decimal {
        self.0
    }
}

/// The exact product of a [`Money`] and a [`Rate`], at scale 4. It is not money: its only
/// operation is division by [`PERCENT_MONTHS`].
#[derive(Debug)]
#[must_use = "a rate product reaches money only through `/ PERCENT_MONTHS` and `store`"]
pub struct RateProduct(Decimal);

impl<const N: u32, const R: u32> Mul<Rate<R>> for Money<N> {
    type Output = RateProduct;

    /// Below `10^18` times below `10^6`, the product is below `10^24` with a mantissa below
    /// `10^28`, so it is exact and never saturates.
    fn mul(self, rate: Rate<R>) -> RateProduct {
        RateProduct(self.0.saturating_mul(rate.0))
    }
}

/// A constant divisor of a [`RateProduct`]. It has no public constructor: [`PERCENT_MONTHS`] is
/// the only one.
#[derive(Clone, Copy, Debug)]
pub struct Divisor(Decimal);

/// The `1200` of `( TRAN-CAT-BAL * DIS-INT-RATE) / 1200` (`CBACT04C.cbl:464-465`): a percentage
/// rate per year applied for one month, 100 × 12.
pub const PERCENT_MONTHS: Divisor = Divisor(Decimal::from_parts(1_200, 0, 0, false, 0));

impl Div<Divisor> for RateProduct {
    type Output = MoneyExpr;

    /// The quotient as `Decimal` division computes it. The precision argument of
    /// `modern/audit/arithmetic.md` (`ARITH-001`) shows that truncating it at 0.01 equals
    /// truncating the exact quotient.
    fn div(self, divisor: Divisor) -> MoneyExpr {
        // The divisor is a non-zero constant and the dividend is below 10^24, so the quotient
        // always exists; the zero fallback is deterministic and never taken.
        MoneyExpr(self.0.checked_div(divisor.0).unwrap_or(Decimal::ZERO))
    }
}

/// The monthly interest of one category balance:
/// `COMPUTE WS-MONTHLY-INT = ( TRAN-CAT-BAL * DIS-INT-RATE) / 1200` into
/// `WS-MONTHLY-INT PIC S9(09)V99` (`CBACT04C.cbl:168,464-465`, `ARITH-001` to `ARITH-008`).
///
/// This is the i2 formula and the only path from a rate to money.
///
/// ```
/// use domain::money::{Decimal, Money, MoneyError, PERCENT_MONTHS, Rate, monthly_interest};
/// # fn main() -> Result<(), MoneyError> {
/// let balance = Money::<9>::new(Decimal::new(123_456, 2))?; // 1234.56
/// let rate = Rate::<4>::new(Decimal::new(1_500, 2))?; // 15.00
/// let interest = monthly_interest(balance, rate);
/// assert!(interest == ((balance * rate) / PERCENT_MONTHS).store::<9>());
/// assert!(interest == Money::<9>::new(Decimal::new(1_543, 2))?); // 15.432 truncated
/// # Ok(())
/// # }
/// ```
#[must_use]
pub fn monthly_interest(balance: Money<9>, rate: Rate<4>) -> Money<9> {
    ((balance * rate) / PERCENT_MONTHS).store::<9>()
}

#[cfg(test)]
mod tests {
    use super::{Decimal, Money, MoneyError, MoneyExpr, PERCENT_MONTHS, Rate, monthly_interest};

    /// `cents / 100` as a `Decimal` at scale 2.
    fn dec(cents: i64) -> Decimal {
        Decimal::new(cents, 2)
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, MoneyError> {
        Money::new(dec(cents))
    }

    fn rate4(cents: i64) -> Result<Rate<4>, MoneyError> {
        Rate::new(dec(cents))
    }

    /// The largest magnitude of `S9(18)V99` and of `S9(06)V99`, as scale-2 mantissas.
    const MAX_MONEY18_CENTS: i128 = 99_999_999_999_999_999_999;
    const MAX_RATE6_CENTS: i128 = 99_999_999;

    // audit: ARITH-001
    #[test]
    fn store_truncates_toward_zero_at_two_decimals() {
        // (intermediate at scale 3, stored cents)
        const CASES: [(i64, i64); 4] = [(1_239, 123), (-1_239, -123), (999, 99), (-5, 0)];
        for (thousandths, cents) in CASES {
            let stored = MoneyExpr::from_decimal(Decimal::new(thousandths, 3)).store::<9>();
            assert_eq!(stored.as_decimal(), dec(cents), "{thousandths} thousandths");
            assert_eq!(stored.as_decimal().scale(), 2);
        }
    }

    // audit: ARITH-008, ARITH-011, ARITH-021, ARITH-030
    #[test]
    fn store_drops_high_order_digits_keeping_the_sign() {
        // (intermediate cents, cents stored into S9(09)V99)
        const CASES: [(i64, i64); 6] = [
            (-123_456_789_012, -23_456_789_012),
            (123_456_789_012, 23_456_789_012),
            (100_000_000_000, 0),
            (-100_000_000_001, -1),
            (-100_000_000_099, -99),
            (99_999_999_999, 99_999_999_999),
        ];
        for (input, kept) in CASES {
            let stored = MoneyExpr::from_decimal(dec(input)).store::<9>();
            assert_eq!(stored.as_decimal(), dec(kept), "{input} cents");
        }
    }

    // audit: ARITH-011, ARITH-013
    #[test]
    fn a_zero_result_is_positive_zero_at_scale_two() -> Result<(), MoneyError> {
        let zeros = [
            MoneyExpr::from_decimal(dec(-200_000_000_000)).store::<9>(),
            (money::<9>(99_999_999_999)? + money::<9>(1)?).store::<9>(),
            (money::<10>(-500)? - money::<10>(-500)?).store::<9>(),
            MoneyExpr::from_decimal(Decimal::new(-1, 3)).store::<9>(),
            Money::<9>::ZERO,
        ];
        for zero in zeros {
            assert!(zero.is_zero() && !zero.is_negative() && !zero.is_positive());
            assert!(zero.as_decimal().is_sign_positive());
            assert_eq!(zero.as_decimal().scale(), 2);
        }
        assert_eq!(
            (money::<10>(999_999_999_999)? + money::<10>(1)?).store::<10>(),
            Money::<10>::ZERO
        );
        Ok(())
    }

    // audit: ARITH-001, ARITH-002, ARITH-003, ARITH-004, ARITH-005, ARITH-006, ARITH-007, ARITH-008
    #[test]
    fn monthly_interest_truncates_the_exact_quotient() -> Result<(), MoneyError> {
        // (balance cents, rate cents, interest cents)
        const CASES: [(i64, i64, i64); 12] = [
            (100_000, 1_800, 1_500),
            (12_345, 199, 20),
            (123_456, 1_500, 1_543),
            (-123_456, 1_500, -1_543),
            (240_000, 1_500, 3_000),
            (10_000, 1_000, 83),
            (20_000, 1_000, 166),
            (-20_000, 1_000, -166),
            (119_999, 100, 99),
            (99_999_999_999, 999_999, 33_332_499_991),
            (-99_999_999_999, 999_999, -33_332_499_991),
            (0, 1_500, 0),
        ];
        for (balance, rate, interest) in CASES {
            let (balance_m, rate_r) = (money::<9>(balance)?, rate4(rate)?);
            let expected = money::<9>(interest)?;
            assert_eq!(
                monthly_interest(balance_m, rate_r),
                expected,
                "{balance} × {rate}"
            );
            assert_eq!(
                ((balance_m * rate_r) / PERCENT_MONTHS).store::<9>(),
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn maximum_width_products_are_exact() -> Result<(), MoneyError> {
        let max_money = Money::<18>::new(Decimal::from_i128_with_scale(MAX_MONEY18_CENTS, 2))?;
        let min_money = Money::<18>::new(Decimal::from_i128_with_scale(-MAX_MONEY18_CENTS, 2))?;
        let max_rate = Rate::<6>::new(Decimal::from_i128_with_scale(MAX_RATE6_CENTS, 2))?;
        // Exact reference in integers: the scale-4 product divided by 1200 and truncated at
        // scale 2 is the product mantissa divided by 120_000; then 18 integer digits are kept.
        let product = MAX_MONEY18_CENTS * MAX_RATE6_CENTS;
        let kept = (product / 120_000) % (MAX_MONEY18_CENTS + 1);
        let expected = Decimal::from_i128_with_scale(kept, 2);
        let stored = ((max_money * max_rate) / PERCENT_MONTHS).store::<18>();
        assert_eq!(stored.as_decimal(), expected);
        let stored_negative = ((min_money * max_rate) / PERCENT_MONTHS).store::<18>();
        assert_eq!(stored_negative.as_decimal(), -expected);
        Ok(())
    }

    // audit: ARITH-023, ARITH-025, ARITH-033
    #[test]
    fn expressions_chain_across_widths() -> Result<(), MoneyError> {
        let (credit, debit, amt) = (
            money::<10>(100_000)?,
            money::<10>(-25_000)?,
            money::<9>(10_000)?,
        );
        assert_eq!((credit - debit + amt).store::<9>(), money::<9>(135_000)?);
        let wide = money::<10>(150_000_000_000)?;
        let temp = (wide - Money::<10>::ZERO + amt).store::<9>();
        assert_eq!(temp, money::<9>(50_000_010_000)?);
        let (credit2, debit2, next) = (
            money::<10>(110_000)?,
            money::<10>(-30_000)?,
            money::<9>(1_000)?,
        );
        assert_eq!((credit2 - debit2 + next).store::<9>(), money::<9>(141_000)?);
        let left = credit + amt;
        let right = debit - amt;
        assert_eq!((left + right).store::<10>(), money::<10>(75_000)?);
        assert_eq!((left - right).store::<10>(), money::<10>(145_000)?);
        assert_eq!(
            (MoneyExpr::from(credit) - amt).store::<10>(),
            money::<10>(90_000)?
        );
        Ok(())
    }

    // audit: ARITH-020, ARITH-021, ARITH-040
    #[test]
    fn a_move_into_a_narrower_field_stores_through_the_expression() -> Result<(), MoneyError> {
        // (S9(10)V99 cents, S9(09)V99 cents)
        const CASES: [(i64, i64); 4] = [
            (84_300, 84_300),
            (123_456_789_012, 23_456_789_012),
            (999_999_999_999, 99_999_999_999),
            (100_000_000_000, 0),
        ];
        for (wide, narrow) in CASES {
            let moved: Money<9> = MoneyExpr::from(money::<10>(wide)?).store::<9>();
            assert_eq!(moved, money::<9>(narrow)?, "{wide} cents");
        }
        Ok(())
    }

    // audit: ARITH-026, ARITH-037
    #[test]
    fn money_compares_exactly_across_widths() -> Result<(), MoneyError> {
        let available = money::<9>(380_000)?;
        let over = money::<10>(380_001)?;
        let same = money::<10>(380_000)?;
        // Each direction is a separate impl, so each is asserted on its own.
        assert!(over > available);
        assert!(available < over);
        assert!(over >= available);
        assert!(same == available);
        assert!(available == same);
        assert!(same <= available);
        assert!(same != over);
        assert_eq!(
            available.partial_cmp(&same),
            Some(core::cmp::Ordering::Equal)
        );
        assert!(money::<9>(-1)?.is_negative() && money::<9>(1)?.is_positive());
        Ok(())
    }

    // audit: ARITH-045
    #[test]
    fn i10_sums_stored_in_twelve_digits_stay_exact() -> Result<(), MoneyError> {
        let max_bal = money::<10>(999_999_999_999)?;
        let max_approved = money::<9>(99_999_999_999)?;
        let sum = (max_bal + max_approved + max_bal).store::<12>();
        assert_eq!(sum.as_decimal(), dec(2_099_999_999_997));
        assert!(sum > max_bal);
        let min_bal = money::<10>(-999_999_999_999)?;
        let negative = (min_bal + money::<9>(-99_999_999_999)? + min_bal).store::<12>();
        assert_eq!(negative.as_decimal(), dec(-2_099_999_999_997));
        let limit = money::<10>(245_000)?;
        let amount = money::<10>(10_000)?;
        let breach = (Money::<10>::ZERO + money::<9>(240_000)? + amount).store::<12>();
        let within = (Money::<10>::ZERO + money::<9>(230_000)? + amount).store::<12>();
        assert!(breach > limit && within <= limit);
        Ok(())
    }

    #[test]
    fn new_rejects_values_the_picture_cannot_hold() -> Result<(), MoneyError> {
        assert_eq!(
            Money::<9>::new(Decimal::new(1_239, 3)),
            Err(MoneyError::ScaleTooLarge)
        );
        assert_eq!(
            Money::<9>::new(Decimal::from(1_000_000_000_u64)),
            Err(MoneyError::OutOfRange)
        );
        assert_eq!(money::<9>(-100_000_000_000), Err(MoneyError::OutOfRange));
        assert_eq!(
            Rate::<4>::new(Decimal::new(1, 3)),
            Err(MoneyError::ScaleTooLarge)
        );
        assert_eq!(rate4(1_000_000), Err(MoneyError::OutOfRange));
        let trailing = Money::<9>::new(Decimal::new(1_230, 3))?;
        assert_eq!(trailing.as_decimal(), dec(123));
        assert_eq!(trailing.as_decimal().scale(), 2);
        assert_eq!(
            money::<9>(-99_999_999_999)?.as_decimal(),
            dec(-99_999_999_999)
        );
        assert_eq!(
            money::<10>(100_000_000_000)?.as_decimal(),
            dec(100_000_000_000)
        );
        let mut negative_zero = Decimal::new(0, 2);
        negative_zero.set_sign_positive(false);
        assert!(negative_zero.is_sign_negative());
        let zero = Money::<9>::new(negative_zero)?;
        assert!(zero.as_decimal().is_sign_positive() && zero.is_zero());
        assert_eq!(rate4(999_999)?.as_decimal(), dec(999_999));
        Ok(())
    }

    // audit: ARITH-015
    #[test]
    fn rate_zero_is_recognised() -> Result<(), MoneyError> {
        assert!(rate4(0)?.is_zero());
        assert!(!rate4(1_500)?.is_zero());
        assert_eq!(rate4(1_500)?, Rate::<4>::new(Decimal::from(15_u64))?);
        assert_eq!(rate4(1_500)?.as_decimal().scale(), 2);
        Ok(())
    }

    #[test]
    fn errors_and_debug_output_describe_the_value() -> Result<(), MoneyError> {
        assert_eq!(
            MoneyError::ScaleTooLarge.to_string(),
            "value has more than two decimal places"
        );
        assert_eq!(
            MoneyError::OutOfRange.to_string(),
            "value exceeds the integer digits of its picture"
        );
        let source = std::error::Error::source(&MoneyError::OutOfRange);
        assert!(source.is_none());
        let balance = money::<9>(123)?;
        let product = balance * rate4(100)?;
        assert!(format!("{product:?}").contains("1.2300"));
        assert!(format!("{balance:?} {:?}", MoneyExpr::from(balance)).contains("1.23"));
        assert!(format!("{PERCENT_MONTHS:?} {:?}", rate4(100)?).contains("1200"));
        Ok(())
    }
}
