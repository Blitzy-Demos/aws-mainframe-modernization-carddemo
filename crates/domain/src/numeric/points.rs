//! Reward points: the exact product of an amount and a reward rate, and the
//! whole points that product accrues.
//!
//! Reward points have no COBOL oracle; the reward rule fixes their arithmetic as
//! `points = ceil(amount × rate)`. `Money<D> * RewardRate` yields the exact
//! [`PointsExpr`]: an amount of scale 2 times a rate of scale at most 4 has scale
//! at most 6, so nothing is rounded before [`PointsExpr::ceil`] turns the product
//! into [`RewardPoints`].
//!
//! [`RewardPoints`] is a whole number in `0..=i64::MAX`. The `BIGINT` columns of
//! the reward tables hold it through [`RewardPoints::to_i64`] and
//! `RewardPoints::try_from(i64)`. The API carries it as a decimal digit string
//! (`^[0-9]+$`), because a JSON number above 2^53 − 1 loses precision in a
//! browser; [`Display`](fmt::Display) and [`FromStr`] are that string form, so a
//! value such as `9007199254740993` round-trips exactly. No total is ever
//! negative: points are never redeemed, so `available = earned − redeemed`
//! never drops below zero, and a negative result is reported as [`Overflow`].

use std::fmt;
use std::ops::{Add, Sub};
use std::str::FromStr;

use rust_decimal::Decimal;

use super::{NumericError, Overflow};

/// The exact product of a money amount and a [`RewardRate`](super::RewardRate),
/// before it is rounded up to whole points.
///
/// Its only operation is [`ceil`](Self::ceil): it adds to, compares with and
/// converts to nothing, so an unrounded product cannot reach a balance.
#[derive(Debug, Clone, Copy)]
pub struct PointsExpr(Decimal);

impl PointsExpr {
    /// Wraps the exact product computed by `Money<D> * RewardRate`.
    #[must_use]
    pub(crate) fn from_exact(product: Decimal) -> Self {
        Self(product)
    }

    /// Rounds the product up to whole points: the reward rule's `ceil`.
    ///
    /// A product in `(-1, 0]` rounds to zero points, and the result is built
    /// from its integer value, so it has scale 0 and is never a negative zero.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when the rounded value is negative or above `i64::MAX`,
    /// outside the range of [`RewardPoints`].
    pub fn ceil(self) -> Result<RewardPoints, Overflow> {
        let whole = i64::try_from(self.0.ceil()).map_err(|_| Overflow)?;
        RewardPoints::from_points(whole)
    }
}

/// Whole reward points: an accrual, or the earned, redeemed or available total
/// of a balance.
///
/// Invariant: an integer of scale 0 in `0..=i64::MAX`, the range of the `BIGINT`
/// columns that store it and of the `^[0-9]+$` text the API exchanges. Every
/// constructor checks it, and the checked arithmetic reports [`Overflow`] rather
/// than leave it.
///
/// `RewardPoints + RewardPoints` and `RewardPoints - RewardPoints` give
/// `Result<RewardPoints, Overflow>`. No operator mixes points with money, a
/// rate, a `Decimal` or a plain integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RewardPoints(Decimal);

impl RewardPoints {
    /// No points: the balance of an account without accruals.
    pub const ZERO: Self = Self(Decimal::ZERO);

    /// The points as the `i64` an SQL `BIGINT` column stores.
    ///
    /// # Panics
    ///
    /// Never in practice: every constructor keeps the value a whole number in
    /// `0..=i64::MAX`, which always converts.
    #[must_use]
    pub fn to_i64(self) -> i64 {
        i64::try_from(self.0).expect("RewardPoints holds a whole number within 0..=i64::MAX")
    }

    /// The sum of two point totals.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when the sum exceeds `i64::MAX`.
    pub fn checked_add(self, other: Self) -> Result<Self, Overflow> {
        let sum = self.to_i64().checked_add(other.to_i64()).ok_or(Overflow)?;
        Self::from_points(sum)
    }

    /// The difference of two point totals.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when `other` exceeds `self`, because points are never
    /// negative.
    pub fn checked_sub(self, other: Self) -> Result<Self, Overflow> {
        let difference = self.to_i64().checked_sub(other.to_i64()).ok_or(Overflow)?;
        Self::from_points(difference)
    }

    // The points of an arithmetic result, which outside `0..=i64::MAX` is a size
    // error rather than invalid input.
    fn from_points(value: i64) -> Result<Self, Overflow> {
        Self::try_from(value).map_err(|_| Overflow)
    }
}

/// Points read from an SQL `BIGINT` column.
///
/// A negative value is rejected with [`NumericError::Negative`].
impl TryFrom<i64> for RewardPoints {
    type Error = NumericError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            Err(NumericError::Negative)
        } else {
            Ok(Self(Decimal::from(value)))
        }
    }
}

/// `self + other`, checked: see [`RewardPoints::checked_add`].
impl Add for RewardPoints {
    type Output = Result<Self, Overflow>;

    fn add(self, other: Self) -> Self::Output {
        self.checked_add(other)
    }
}

/// `self - other`, checked: see [`RewardPoints::checked_sub`].
impl Sub for RewardPoints {
    type Output = Result<Self, Overflow>;

    fn sub(self, other: Self) -> Self::Output {
        self.checked_sub(other)
    }
}

/// The plain decimal integer: no sign, no separators, no leading zeros.
impl fmt::Display for RewardPoints {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The integer's formatting ignores a precision such as `{:.2}`, which
        // would give a `Decimal` fraction digits.
        fmt::Display::fmt(&self.to_i64(), formatter)
    }
}

/// Points from the API's digit-string form `^[0-9]+$`; leading zeros are
/// accepted.
///
/// Text that is empty or holds any byte other than an ASCII digit (a sign, a
/// space, a decimal point, an underscore) is rejected with
/// [`NumericError::NotNumeric`]; digits above `i64::MAX` with
/// [`NumericError::OutOfRange`].
impl FromStr for RewardPoints {
    type Err = NumericError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // `i64::from_str` alone would also accept a leading `+` or `-`.
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(NumericError::NotNumeric);
        }
        let value = text.parse::<i64>().map_err(|_| NumericError::OutOfRange)?;
        Ok(Self(Decimal::from(value)))
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::{NumericError, Overflow, PointsExpr, RewardPoints};

    fn points(value: i64) -> RewardPoints {
        RewardPoints::try_from(value).expect("non-negative points")
    }

    #[test]
    fn digit_strings_round_trip_exactly() {
        let cases = [
            (
                "9007199254740993",
                9_007_199_254_740_993,
                "9007199254740993",
            ),
            ("9223372036854775807", i64::MAX, "9223372036854775807"),
            ("0", 0, "0"),
            ("007", 7, "7"),
            ("0000000000000000000000001", 1, "1"),
        ];
        for (text, value, shown) in cases {
            let parsed: RewardPoints = text.parse().expect(text);
            assert_eq!(parsed.to_i64(), value, "{text}");
            assert_eq!(parsed.to_string(), shown, "{text}");
            assert_eq!(parsed.0.scale(), 0, "{text}");
        }
    }

    #[test]
    fn text_outside_the_digit_pattern_or_range_is_rejected() {
        let cases = [
            ("", NumericError::NotNumeric),
            ("-1", NumericError::NotNumeric),
            ("+1", NumericError::NotNumeric),
            ("1.0", NumericError::NotNumeric),
            (" 1", NumericError::NotNumeric),
            ("1 ", NumericError::NotNumeric),
            ("1_000", NumericError::NotNumeric),
            ("9223372036854775808", NumericError::OutOfRange),
            ("99999999999999999999999", NumericError::OutOfRange),
        ];
        for (text, error) in cases {
            assert_eq!(text.parse::<RewardPoints>(), Err(error), "{text:?}");
        }
    }

    #[test]
    fn integers_convert_only_when_non_negative() {
        assert_eq!(RewardPoints::try_from(-1), Err(NumericError::Negative));
        assert_eq!(
            RewardPoints::try_from(i64::MIN),
            Err(NumericError::Negative)
        );
        assert_eq!(points(0), RewardPoints::ZERO);
        assert_eq!(points(0).to_string(), "0");
        assert_eq!(RewardPoints::ZERO.to_i64(), 0);
        assert_eq!(points(i64::MAX).to_i64(), i64::MAX);
    }

    #[test]
    fn display_is_the_plain_integer_whatever_the_precision() {
        assert_eq!(format!("{:.2}", points(1500)), "1500");
    }

    #[test]
    fn checked_arithmetic_stays_within_the_points_range() {
        let cases = [
            (points(2), points(3), Ok(points(5)), Err(Overflow)),
            (points(7), points(5), Ok(points(12)), Ok(points(2))),
            (points(5), points(7), Ok(points(12)), Err(Overflow)),
            (points(5), points(5), Ok(points(10)), Ok(RewardPoints::ZERO)),
            (
                points(i64::MAX),
                points(1),
                Err(Overflow),
                Ok(points(i64::MAX - 1)),
            ),
            (
                points(i64::MAX),
                RewardPoints::ZERO,
                Ok(points(i64::MAX)),
                Ok(points(i64::MAX)),
            ),
        ];
        for (left, right, sum, difference) in cases {
            assert_eq!(left.checked_add(right), sum, "{left} + {right}");
            assert_eq!(left + right, sum, "{left} + {right}");
            assert_eq!(left.checked_sub(right), difference, "{left} - {right}");
            assert_eq!(left - right, difference, "{left} - {right}");
        }
    }

    #[test]
    fn ceil_rounds_the_exact_product_up_to_whole_points() {
        let cases: [(Decimal, Result<i64, Overflow>); 12] = [
            (dec!(12.000001), Ok(13)),
            (dec!(12.000000), Ok(12)),
            (dec!(0.000001), Ok(1)),
            (dec!(0), Ok(0)),
            (dec!(0.999999), Ok(1)),
            (dec!(-0.5), Ok(0)),
            (dec!(-0.999999), Ok(0)),
            (dec!(-1), Err(Overflow)),
            (dec!(-1.5), Err(Overflow)),
            (dec!(9223372036854775806.5), Ok(i64::MAX)),
            (dec!(9223372036854775807.000001), Err(Overflow)),
            (dec!(9223372036854775808), Err(Overflow)),
        ];
        for (product, expected) in cases {
            let result = PointsExpr::from_exact(product).ceil();
            assert_eq!(result, expected.map(points), "ceil({product})");
            if let Ok(whole) = result {
                assert_eq!(whole.0.scale(), 0, "ceil({product})");
                assert!(!whole.0.is_sign_negative(), "ceil({product})");
            }
        }
    }
}
