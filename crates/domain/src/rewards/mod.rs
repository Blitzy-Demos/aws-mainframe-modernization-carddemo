//! The rewards feature: configurable (type, category) → rate rules, accrual on
//! posted transactions, and the per-account points balance (request Rewards
//! system; AAP 0.3.5).
//!
//! Rewards have no COBOL oracle: the README lists DB2 Rewards only as a
//! roadmap item [README.md:L357-369], so no parity audit row gates this module.
//! Rule codes are the fixture codes of `app/data/ASCII/trantype.txt` and
//! `app/data/ASCII/trancatg.txt`, for example type `01` Purchase with category
//! `0001` Regular Sales Draft.
//!
//! - [`engine`]: [`RewardEngine`], the single accrual path. Only CT02
//!   (`POST /transactions`) and POSTTRAN (`batch posttran`) call it, each inside
//!   the database transaction that records the transaction (AAP 0.6.4).
//! - [`ports`]: [`RewardStore`], the reward tables as the engine reads and
//!   writes them, implemented by `PgRewardStore` in `crates/persistence`.
//! - This module: [`RewardError`], the engine's failure, and the admin rule
//!   validation behind `POST` and `PUT /admin/reward-rules`
//!   ([`validate_rule`]) and `DELETE /admin/reward-rules/{id}`
//!   ([`soft_delete_at`]).
//!
//! Points are only earned: nothing redeems or spends them, and there are no
//! tiers, membership levels or external loyalty integrations (AAP 0.2.5).

pub mod engine;
pub mod ports;

pub use engine::{AccrualInput, RewardEngine};
pub use ports::RewardStore;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::entities::{NewRewardRule, RewardRule};
use crate::error::DomainError;
use crate::messages::{REWARD_ACTIVE_TO_BEFORE_FROM, REWARD_MIN_EXCEEDS_MAX, REWARD_RATE_NEGATIVE};
use crate::numeric::{Money, Overflow, RewardRate, TranCategoryCode, TranTypeCode};
use crate::ports::PortError;

/// Why [`RewardEngine::accrue`] failed.
///
/// The display texts are internal: they are logged, never sent to a client.
/// Each caller rolls back its unit of work on any variant (AAP 0.3.5,
/// 0.5.3.5):
///
/// - CT02 rolls back the whole request, the transaction insert included, and
///   answers 500 with the generic unexpected-error message.
/// - POSTTRAN rolls back the record being posted, prints its record number and
///   TRAN-ID to stderr, stops the run and exits with code 8, writing nothing to
///   `DALYREJS`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RewardError {
    /// A [`RewardStore`] call failed; the port's error is kept unchanged.
    #[error("reward store call failed: {0}")]
    Store(#[from] PortError),
    /// The earned points fall outside the range of
    /// [`RewardPoints`](crate::numeric::RewardPoints).
    #[error("reward points out of range")]
    PointsOverflow,
}

/// The checked points arithmetic reports [`Overflow`]; in an accrual that is
/// [`RewardError::PointsOverflow`].
impl From<Overflow> for RewardError {
    fn from(_overflow: Overflow) -> Self {
        Self::PointsOverflow
    }
}

/// A reward rule as `POST` or `PUT /admin/reward-rules` submits it, before
/// [`validate_rule`] checks it (AAP 0.3.5).
///
/// The codes and amounts have already passed their field types; `reward_rate`
/// is the unchecked request value, because a negative rate has a message of its
/// own that [`RewardRate`] cannot carry. Every field is `Copy`, so the draft is
/// too, and [`validate_rule`] takes it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleDraft {
    /// The transaction type code the rule matches (`TRAN-TYPE-CD PIC X(02)`).
    pub transaction_type_cd: TranTypeCode,
    /// The transaction category code the rule matches (`TRAN-CAT-CD PIC 9(04)`).
    pub transaction_category_cd: TranCategoryCode,
    /// Points per currency unit, as submitted.
    pub reward_rate: Decimal,
    /// The inclusive lower amount bound; `None` is unbounded.
    pub min_transaction_amount: Option<Money<9>>,
    /// The inclusive upper amount bound; `None` is unbounded.
    pub max_transaction_amount: Option<Money<9>>,
    /// The first instant the rule is active, inclusive.
    pub active_from: DateTime<Utc>,
    /// The end of the active window, exclusive; `None` is open-ended.
    pub active_to: Option<DateTime<Utc>>,
}

/// A rule that passed [`validate_rule`]: the entity's insert shape, so no
/// second rule type exists.
pub type ValidRule = NewRewardRule;

/// The JSON field names a rule rejection names. Reward DTOs have no BMS map,
/// so `fields[].field` carries the request's own field names.
const FIELD_REWARD_RATE: &str = "reward_rate";
const FIELD_MIN_TRANSACTION_AMOUNT: &str = "min_transaction_amount";
const FIELD_ACTIVE_TO: &str = "active_to";

/// One failed check: the field it concerns and the text shown for it.
type Failure = (&'static str, String);

/// Checks `draft` and returns the rule to store (AAP 0.3.5).
///
/// The checks run in this order, and every failing one is reported:
///
/// 1. `reward_rate` is not negative.
/// 2. When both bounds are present, `min_transaction_amount` does not exceed
///    `max_transaction_amount`; equal bounds are valid, as both are inclusive.
/// 3. When `active_to` is present, it does not precede `active_from`; an equal
///    instant is valid and leaves a window that contains no instant.
///
/// # Errors
///
/// An [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) error whose
/// message is the text of the first failing check, with one `fields` entry per
/// failing check in the order above, each carrying its own text:
///
/// - `reward_rate`: [`REWARD_RATE_NEGATIVE`] (`reward_rate must not be
///   negative`). A non-negative rate that [`RewardRate`] cannot hold, with more
///   than 4 decimal places or beyond `NUMERIC(9,4)`, carries the
///   [`NumericError`](crate::numeric::NumericError) text instead; the API
///   DTO's pattern normally rejects such input first.
/// - `min_transaction_amount`: [`REWARD_MIN_EXCEEDS_MAX`]
///   (`min_transaction_amount must not exceed max_transaction_amount`).
/// - `active_to`: [`REWARD_ACTIVE_TO_BEFORE_FROM`] (`active_to must not
///   precede active_from`).
pub fn validate_rule(draft: RuleDraft) -> Result<ValidRule, DomainError> {
    let RuleDraft {
        transaction_type_cd,
        transaction_category_cd,
        reward_rate,
        min_transaction_amount,
        max_transaction_amount,
        active_from,
        active_to,
    } = draft;

    let rate = checked_rate(reward_rate);
    let bounds_failure = match (min_transaction_amount, max_transaction_amount) {
        (Some(min), Some(max)) if min > max => Some((
            FIELD_MIN_TRANSACTION_AMOUNT,
            REWARD_MIN_EXCEEDS_MAX.to_owned(),
        )),
        _ => None,
    };
    let window_failure = active_to
        .filter(|end| *end < active_from)
        .map(|_| (FIELD_ACTIVE_TO, REWARD_ACTIVE_TO_BEFORE_FROM.to_owned()));
    let mut later_failures = [bounds_failure, window_failure].into_iter().flatten();

    match rate {
        Err(message) => Err(rejection((FIELD_REWARD_RATE, message), later_failures)),
        Ok(reward_rate) => match later_failures.next() {
            Some(first) => Err(rejection(first, later_failures)),
            None => Ok(NewRewardRule {
                transaction_type_cd,
                transaction_category_cd,
                reward_rate,
                min_transaction_amount,
                max_transaction_amount,
                active_from,
                active_to,
            }),
        },
    }
}

/// Check (1): the submitted rate as a [`RewardRate`], or the text of its
/// failure.
fn checked_rate(reward_rate: Decimal) -> Result<RewardRate, String> {
    if reward_rate < Decimal::ZERO {
        return Err(REWARD_RATE_NEGATIVE.to_owned());
    }
    RewardRate::from_decimal(reward_rate).map_err(|error| error.to_string())
}

/// The rejection for the failed checks `first` and then `rest`: `first`'s text
/// is the message, and every failure adds its field.
fn rejection(first: Failure, rest: impl IntoIterator<Item = Failure>) -> DomainError {
    let (field, message) = first;
    rest.into_iter().fold(
        DomainError::invalid(message).with_field(field),
        |error, (field, message)| error.with_field_message(field, message),
    )
}

/// The `active_to` that `DELETE /admin/reward-rules/{id}` stores for `rule` at
/// instant `now`, or `None` when the rule is already soft-deleted and nothing
/// changes (AAP 0.3.5).
///
/// `Some(now)` when `active_to` is absent or later than `now`; `None` when it
/// is at or before `now`, because a rule is active only while
/// `t < active_to`, so it has already ended, and the handler answers 200
/// without a write. A rule that has not started yet (`now < active_from`) is
/// ended the same way and then never becomes active.
///
/// `now` is the processing clock ([`Clock`](crate::clock::Clock), which
/// `CARDDEMO_CLOCK_OVERRIDE` pins), read by the handler.
#[must_use]
pub fn soft_delete_at(rule: &RewardRule, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    rule.active_to
        .is_none_or(|active_to| active_to > now)
        .then_some(now)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeDelta, TimeZone, Utc};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::{RewardError, RuleDraft, ValidRule, soft_delete_at, validate_rule};
    use crate::entities::RewardRule;
    use crate::error::{DomainError, ErrorKind, FieldError};
    use crate::messages::{
        REWARD_ACTIVE_TO_BEFORE_FROM, REWARD_MIN_EXCEEDS_MAX, REWARD_RATE_NEGATIVE,
    };
    use crate::numeric::{
        Money, NumericError, Overflow, RewardRate, RuleId, TranCategoryCode, TranTypeCode,
    };
    use crate::ports::PortError;

    fn midnight(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    fn amount(cents: i64) -> Money<9> {
        Money::from_cents(cents).expect("an S9(09)V99 amount")
    }

    /// The retail seed rule of AAP 0.3.6: type `01` Purchase
    /// (`app/data/ASCII/trantype.txt`), category `0001` Regular Sales Draft
    /// (`app/data/ASCII/trancatg.txt`), rate 1.0000, unbounded, from
    /// 2022-01-01T00:00:00Z.
    fn retail_draft() -> RuleDraft {
        RuleDraft {
            transaction_type_cd: TranTypeCode::move_from("01"),
            transaction_category_cd: TranCategoryCode::new(1).expect("a 9(04) category code"),
            reward_rate: dec!(1.0000),
            min_transaction_amount: None,
            max_transaction_amount: None,
            active_from: midnight(2022, 1, 1),
            active_to: None,
        }
    }

    fn field(field: &'static str, message: &str) -> FieldError {
        FieldError {
            field,
            message: message.to_owned(),
        }
    }

    fn rejected(draft: RuleDraft) -> DomainError {
        validate_rule(draft).expect_err("the draft is rejected")
    }

    #[test]
    fn a_valid_draft_becomes_the_rule_to_store_unchanged() {
        let draft = retail_draft();
        assert_eq!(
            validate_rule(draft),
            Ok(ValidRule {
                transaction_type_cd: draft.transaction_type_cd,
                transaction_category_cd: draft.transaction_category_cd,
                reward_rate: RewardRate::from_decimal(dec!(1.0000)).expect("a reward rate"),
                min_transaction_amount: None,
                max_transaction_amount: None,
                active_from: draft.active_from,
                active_to: None,
            })
        );
    }

    #[test]
    fn inclusive_bounds_an_empty_window_and_a_zero_rate_are_valid() {
        let start = midnight(2022, 1, 1);
        let cases = [
            // Equal bounds: both are inclusive.
            RuleDraft {
                min_transaction_amount: Some(amount(5_047)),
                max_transaction_amount: Some(amount(5_047)),
                ..retail_draft()
            },
            // A single bound, and min below max.
            RuleDraft {
                min_transaction_amount: Some(amount(-100)),
                ..retail_draft()
            },
            RuleDraft {
                min_transaction_amount: Some(amount(1_000)),
                max_transaction_amount: Some(amount(10_000)),
                ..retail_draft()
            },
            // `active_to` equal to `active_from`, and later than it.
            RuleDraft {
                active_to: Some(start),
                ..retail_draft()
            },
            RuleDraft {
                active_to: Some(midnight(2022, 7, 1)),
                ..retail_draft()
            },
            // Zero, negative zero, four decimals, and the NUMERIC(9,4) maximum.
            RuleDraft {
                reward_rate: Decimal::ZERO,
                ..retail_draft()
            },
            RuleDraft {
                reward_rate: -Decimal::ZERO,
                ..retail_draft()
            },
            RuleDraft {
                reward_rate: dec!(1.5000),
                ..retail_draft()
            },
            RuleDraft {
                reward_rate: dec!(99999.9999),
                ..retail_draft()
            },
        ];
        for draft in cases {
            let valid = validate_rule(draft);
            assert!(valid.is_ok(), "{draft:?}: {valid:?}");
            let valid = valid.expect("checked above");
            assert_eq!(valid.min_transaction_amount, draft.min_transaction_amount);
            assert_eq!(valid.max_transaction_amount, draft.max_transaction_amount);
            assert_eq!(valid.active_to, draft.active_to);
            assert_eq!(
                valid.reward_rate,
                RewardRate::from_decimal(draft.reward_rate).expect("a reward rate")
            );
        }
    }

    #[test]
    fn each_failing_check_alone_names_its_field_and_message() {
        let start = midnight(2022, 1, 1);
        // (draft, field, message)
        let cases = [
            (
                RuleDraft {
                    reward_rate: dec!(-0.0001),
                    ..retail_draft()
                },
                "reward_rate",
                REWARD_RATE_NEGATIVE,
            ),
            (
                RuleDraft {
                    reward_rate: dec!(-1.23456),
                    ..retail_draft()
                },
                "reward_rate",
                REWARD_RATE_NEGATIVE,
            ),
            (
                RuleDraft {
                    min_transaction_amount: Some(amount(10_001)),
                    max_transaction_amount: Some(amount(10_000)),
                    ..retail_draft()
                },
                "min_transaction_amount",
                REWARD_MIN_EXCEEDS_MAX,
            ),
            (
                RuleDraft {
                    active_to: Some(start - TimeDelta::nanoseconds(1)),
                    ..retail_draft()
                },
                "active_to",
                REWARD_ACTIVE_TO_BEFORE_FROM,
            ),
        ];
        for (draft, name, message) in cases {
            let error = rejected(draft);
            assert_eq!(error.kind, ErrorKind::Invalid, "{draft:?}");
            assert_eq!(error.code, ErrorKind::Invalid.code());
            assert_eq!(error.message, message, "{draft:?}");
            assert_eq!(error.fields, vec![field(name, message)], "{draft:?}");
        }
    }

    #[test]
    fn every_failing_check_is_reported_in_order_under_the_first_message() {
        let error = rejected(RuleDraft {
            reward_rate: dec!(-1),
            min_transaction_amount: Some(amount(2)),
            max_transaction_amount: Some(amount(1)),
            active_to: Some(midnight(2021, 12, 31)),
            ..retail_draft()
        });
        assert_eq!(error.kind, ErrorKind::Invalid);
        assert_eq!(error.message, REWARD_RATE_NEGATIVE);
        assert_eq!(
            error.fields,
            vec![
                field("reward_rate", REWARD_RATE_NEGATIVE),
                field("min_transaction_amount", REWARD_MIN_EXCEEDS_MAX),
                field("active_to", REWARD_ACTIVE_TO_BEFORE_FROM),
            ]
        );

        // Without the rate failure, the bounds message leads.
        let error = rejected(RuleDraft {
            min_transaction_amount: Some(amount(2)),
            max_transaction_amount: Some(amount(1)),
            active_to: Some(midnight(2021, 12, 31)),
            ..retail_draft()
        });
        assert_eq!(error.message, REWARD_MIN_EXCEEDS_MAX);
        assert_eq!(
            error.fields,
            vec![
                field("min_transaction_amount", REWARD_MIN_EXCEEDS_MAX),
                field("active_to", REWARD_ACTIVE_TO_BEFORE_FROM),
            ]
        );
    }

    #[test]
    fn a_rate_the_column_cannot_hold_is_invalid_on_reward_rate() {
        // (rate, the reward-rate failure)
        let cases = [
            (dec!(1.23456), NumericError::TooManyDecimals),
            (dec!(0.00001), NumericError::TooManyDecimals),
            (dec!(100000), NumericError::OutOfRange),
            (Decimal::MAX, NumericError::OutOfRange),
        ];
        for (reward_rate, failure) in cases {
            let error = rejected(RuleDraft {
                reward_rate,
                active_to: Some(midnight(2021, 12, 31)),
                ..retail_draft()
            });
            let text = failure.to_string();
            assert_eq!(error.kind, ErrorKind::Invalid, "{reward_rate}");
            assert_eq!(error.message, text, "{reward_rate}");
            assert_eq!(
                error.fields,
                vec![
                    field("reward_rate", &text),
                    field("active_to", REWARD_ACTIVE_TO_BEFORE_FROM),
                ],
                "{reward_rate}"
            );
        }
    }

    #[test]
    fn soft_delete_ends_an_active_or_future_window_and_keeps_an_ended_one() {
        let now = midnight(2022, 7, 1);
        let tick = TimeDelta::nanoseconds(1);
        // (active_from, active_to, stored active_to)
        let cases = [
            (midnight(2022, 1, 1), None, Some(now)),
            (midnight(2022, 1, 1), Some(now + tick), Some(now)),
            (midnight(2022, 1, 1), Some(midnight(2023, 1, 1)), Some(now)),
            (midnight(2022, 1, 1), Some(now), None),
            (midnight(2022, 1, 1), Some(now - tick), None),
            (midnight(2022, 1, 1), Some(midnight(2022, 6, 1)), None),
            // A rule that has not started is ended all the same.
            (midnight(2023, 1, 1), None, Some(now)),
        ];
        for (active_from, active_to, expected) in cases {
            let rule = RewardRule {
                rule_id: RuleId::from(1),
                transaction_type_cd: TranTypeCode::move_from("01"),
                transaction_category_cd: TranCategoryCode::new(1).expect("a 9(04) code"),
                reward_rate: RewardRate::from_decimal(dec!(1.0000)).expect("a reward rate"),
                min_transaction_amount: None,
                max_transaction_amount: None,
                active_from,
                active_to,
            };
            assert_eq!(
                soft_delete_at(&rule, now),
                expected,
                "{active_from} until {active_to:?}"
            );
            if let Some(ended) = expected {
                let deleted = RewardRule {
                    active_to: Some(ended),
                    ..rule
                };
                assert!(!deleted.is_active_at(now), "{active_from}");
                assert_eq!(soft_delete_at(&deleted, now), None, "{active_from}");
            }
        }
    }

    #[test]
    fn port_and_overflow_failures_convert_to_reward_errors() {
        assert_eq!(RewardError::from(Overflow), RewardError::PointsOverflow);
        assert_eq!(
            RewardError::from(PortError::Duplicate),
            RewardError::Store(PortError::Duplicate)
        );
        let failure = PortError::Unavailable("connection reset".to_owned());
        assert_eq!(
            RewardError::from(failure.clone()),
            RewardError::Store(failure)
        );
        assert_eq!(
            RewardError::Store(PortError::Duplicate).to_string(),
            "reward store call failed: duplicate record key"
        );
        assert_eq!(
            RewardError::PointsOverflow.to_string(),
            "reward points out of range"
        );
    }
}
