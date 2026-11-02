//! The wire types of the REST API: the request and response bodies and query
//! parameters that replace the BMS symbolic maps (`app/cpy-bms/*.CPY`,
//! `app/app-authorization-ims-db2-mq/cpy-bms/*.cpy`,
//! `app/app-transaction-type-db2/cpy-bms/*.cpy`).
//!
//! `carddemo_domain` names no serialization crate, so every type that crosses
//! the HTTP boundary lives in this module tree, one submodule per resource
//! family, and converts to and from the domain's rule inputs and entities.
//!
//! # Conventions
//!
//! - JSON names are the Rust field names, in `snake_case`.
//! - Requests carry each map field as entered, as an `Option<String>`: a blank
//!   or missing field reaches the rule unchanged, so the rule answers it with
//!   the audited COBOL message. The only edit a request applies itself is the
//!   BMS field length (`#[schema(max_length)]` and `#[validate(length(max))]`),
//!   rejected through `reject_invalid`.
//! - Responses carry the entity field names: the copybook names in lower
//!   `snake_case`, prefix and spelling kept (`acct_expiraion_date`,
//!   `pa_merchant_catagory_code`). The request-fixed keys are the exceptions:
//!   `account_id`, `card_number`, `customer_id`, `user_id`, `tran_id`,
//!   `auth_id`, `rule_id` and `accrual_id`.
//! - `X(n)` fields, `X(10)` dates and `X(26)` timestamps included, are
//!   right-trimmed text (`text`). Money is a scale-2 `Decimal` serialized as
//!   a string (`money` with `#[serde(with = "rust_decimal::serde::str")]`),
//!   never a floating-point number. Identifiers, codes and keys are strings
//!   (`id_string`): zero-padded for `9(n)` fields, as the `DISPLAY` field
//!   holds them.
//! - [`crate::error::ErrorBody`] is the only error shape. Its `fields[].field`
//!   values are the BMS field names a `<Dto>Field` enum lists; each such enum
//!   implements `BmsField`.
//!
//! `crate::openapi` registers the types by their module paths, so nothing is
//! re-exported here.

pub mod accounts;
pub mod admin_reward_rules;
pub mod admin_transaction_types;
pub mod admin_users;
pub mod auth;
pub mod authorizations;
pub mod cards;
pub mod payments;
pub mod reports;
pub mod rewards;
pub mod transactions;

use std::collections::BTreeSet;

use carddemo_domain::error::ErrorKind;
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, AccrualId, Alnum, AuthId, CardNumber, CustSsn, CustomerId,
    FicoScore, MerchantId, Money, NumericCode, RuleId, TranCategoryCode, TranId, TranTypeCode,
    TransactionRowId, UserId,
};
use rust_decimal::Decimal;
use validator::ValidationErrors;

use crate::error::{ApiError, ErrorFieldBody};

/// The text of an `X(n)` field as a response carries it: the stored bytes
/// without their trailing spaces.
///
/// Leading spaces and trailing low-values are kept, as
/// [`Alnum::trimmed`] keeps them.
#[must_use]
pub(crate) fn text<const N: usize>(value: &Alnum<N>) -> String {
    value.trimmed().into_owned()
}

/// A money field `S9(D)V99` as a response carries it: the scale-2 `Decimal`,
/// so `1234.5` serializes as `"1234.50"` under
/// `#[serde(with = "rust_decimal::serde::str")]`.
#[must_use]
pub(crate) fn money<const D: u8>(value: Money<D>) -> Decimal {
    value.to_decimal()
}

/// An identifier, code or key as a response carries it.
///
/// The `X(n)` keys display at their full COBOL width, trailing spaces
/// included, so a bound on `Display` would send `"USER1   "` for a user ID.
/// This trait gives each key type its wire text instead: the zero-padded
/// `DISPLAY` digits of a `9(n)` field, the right-trimmed text of an `X(n)` key
/// and the plain decimal of a row identity. `Cvv` has no implementation, so a
/// card verification code cannot reach a response through [`id_string`].
pub(crate) trait KeyText {
    /// The wire text of the value.
    fn key_text(&self) -> String;
}

/// [`KeyText`] through `Display`: the zero-padded digits of a `9(n)` field,
/// or the plain decimal of a row identity.
macro_rules! key_text_by_display {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl KeyText for $ty {
                fn key_text(&self) -> String {
                    self.to_string()
                }
            }
        )+
    };
}

/// [`KeyText`] through `trimmed`: the text of an `X(n)` key without its
/// trailing spaces.
macro_rules! key_text_trimmed {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl KeyText for $ty {
                fn key_text(&self) -> String {
                    self.trimmed().into_owned()
                }
            }
        )+
    };
}

key_text_by_display!(
    AccountId,
    CustomerId,
    TranCategoryCode,
    MerchantId,
    CustSsn,
    FicoScore,
    TransactionRowId,
    RuleId,
    AccrualId,
    AuthId,
);

key_text_trimmed!(CardNumber, TranId, TranTypeCode, UserId, AccountGroupId);

impl<const N: u8> KeyText for NumericCode<N> {
    fn key_text(&self) -> String {
        self.to_string()
    }
}

/// The wire text of an identifier, code or key: `"00000000001"` for
/// `AccountId` 1, `"0001"` for `TranCategoryCode` 1, `"USER1"` for the
/// `UserId` stored as `USER1   `, `"42"` for `RuleId` 42.
#[must_use]
pub(crate) fn id_string(value: &impl KeyText) -> String {
    value.key_text()
}

/// A BMS field a request DTO's errors can name: the variants of a
/// `<Dto>Field` enum.
///
/// Each enum also keeps an inherent `pub const fn as_str`, to which its
/// implementation delegates.
pub(crate) trait BmsField: Copy {
    /// The BMS symbolic field name without its `I`/`L` suffix, such as
    /// `ACCTSID`: the `fields[].field` value of the error body.
    fn as_str(self) -> &'static str;

    /// The field's position in the order the COBOL program rejects fields:
    /// the `Order` column of `docs/parity/validation-audit.md`, or the
    /// declaration order where the audit gives none. Lower ranks come first.
    fn rank(self) -> u16;
}

/// One error-body field per distinct BMS field in `fields`, each carrying
/// `message`, in rank order.
///
/// Fields of equal rank are ordered by name, and a field named twice is
/// listed once, so the body is the same whatever order `fields` arrives in.
#[must_use]
pub(crate) fn bms_errors<F: BmsField>(
    fields: impl IntoIterator<Item = F>,
    message: &str,
) -> Vec<ErrorFieldBody> {
    let mut fields: Vec<F> = fields.into_iter().collect();
    fields.sort_unstable_by_key(|field| (field.rank(), field.as_str()));
    let mut seen = BTreeSet::new();
    fields
        .into_iter()
        .filter(|field| seen.insert(field.as_str()))
        .map(|field| ErrorFieldBody {
            field: field.as_str().to_owned(),
            message: message.to_owned(),
        })
        .collect()
}

/// The error-body fields of a failed length check: each failing request field
/// mapped to its BMS field by `map`, through [`bms_errors`].
///
/// `map` takes the request field's JSON name, which `validator` reports, and
/// must cover every field the DTO validates; a name it maps to `None` is left
/// out. Two request fields that share a BMS field give one entry.
#[must_use]
pub(crate) fn length_errors<F: BmsField>(
    errors: &ValidationErrors,
    map: impl Fn(&str) -> Option<F>,
    message: &str,
) -> Vec<ErrorFieldBody> {
    bms_errors(errors.field_errors().into_keys().filter_map(map), message)
}

/// The 400 answer to a request whose fields fail their length checks: kind
/// `Invalid`, `message`, and the failing BMS fields from [`length_errors`].
///
/// `message` is the `carddemo_domain::messages` constant the validation
/// audit's over-length row names for the program, or `""` where the audit
/// records the path as `no-message` and links its escalation.
#[must_use]
pub(crate) fn reject_invalid<F: BmsField>(
    errors: &ValidationErrors,
    map: impl Fn(&str) -> Option<F>,
    message: &'static str,
) -> ApiError {
    ApiError::no_message(
        ErrorKind::Invalid,
        message,
        length_errors(errors, map, message),
    )
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, AccrualId, Alnum, AuthId, CardNumber, CustSsn, CustomerId,
        FicoScore, MerchantId, Money, NumericCode, RuleId, TranCategoryCode, TranId, TranTypeCode,
        TransactionRowId, UserId,
    };
    use rust_decimal::Decimal;
    use serde::Serialize;
    use serde_json::json;
    use validator::Validate;

    use super::{BmsField, bms_errors, id_string, length_errors, money, reject_invalid, text};
    use crate::error::ErrorFieldBody;

    /// A field enum shaped as the `<Dto>Field` enums are, with ranks that
    /// differ from both declaration and name order.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ProbeField {
        Year,
        Month,
        Day,
        Confirm,
    }

    impl BmsField for ProbeField {
        fn as_str(self) -> &'static str {
            match self {
                Self::Year => "SDTYYYY",
                Self::Month => "SDTMM",
                Self::Day => "SDTDD",
                Self::Confirm => "CONFIRM",
            }
        }

        fn rank(self) -> u16 {
            match self {
                Self::Month => 1,
                Self::Day => 2,
                Self::Year => 3,
                Self::Confirm => 9,
            }
        }
    }

    /// A request DTO with one unmapped field and two fields that share a BMS
    /// field.
    #[derive(Debug, Default, Validate)]
    struct Probe {
        #[validate(length(max = 4))]
        start_year: Option<String>,
        #[validate(length(max = 2))]
        start_month: Option<String>,
        #[validate(length(max = 2))]
        start_day: Option<String>,
        #[validate(length(max = 10))]
        start_date: Option<String>,
        #[validate(length(max = 1))]
        confirm: Option<String>,
        #[validate(length(max = 1))]
        unmapped: Option<String>,
    }

    fn probe_field(name: &str) -> Option<ProbeField> {
        match name {
            "start_year" | "start_date" => Some(ProbeField::Year),
            "start_month" => Some(ProbeField::Month),
            "start_day" => Some(ProbeField::Day),
            "confirm" => Some(ProbeField::Confirm),
            _ => None,
        }
    }

    fn field(name: &str, message: &str) -> ErrorFieldBody {
        ErrorFieldBody {
            field: name.to_owned(),
            message: message.to_owned(),
        }
    }

    /// A value one character longer than `max`.
    fn over(max: usize) -> String {
        "9".repeat(max + 1)
    }

    /// Every field of `Probe` one character over its length.
    fn all_over() -> Probe {
        Probe {
            start_year: Some(over(4)),
            start_month: Some(over(2)),
            start_day: Some(over(2)),
            start_date: Some(over(10)),
            confirm: Some(over(1)),
            unmapped: Some(over(1)),
        }
    }

    #[test]
    fn text_trims_trailing_spaces_only() {
        let cases: [(&[u8], &str); 5] = [
            (b"AB      ", "AB"),
            (b"  AB    ", "  AB"),
            (b"ABCDEFGH", "ABCDEFGH"),
            (b"        ", ""),
            (b"A\0      ", "A\0"),
        ];
        for (bytes, expected) in cases {
            let item = Alnum::<8>::move_from_bytes(bytes);
            assert_eq!(text(&item), expected, "{bytes:?}");
        }
        let date = Alnum::<10>::move_from("2025-05-31");
        assert_eq!(text(&date), "2025-05-31");
    }

    #[derive(Serialize)]
    struct Wire {
        #[serde(with = "rust_decimal::serde::str")]
        amount: Decimal,
    }

    #[test]
    fn money_keeps_scale_two_on_the_wire() {
        // (cents, JSON)
        let cases = [
            (123_450, r#"{"amount":"1234.50"}"#),
            (-1_234, r#"{"amount":"-12.34"}"#),
            (0, r#"{"amount":"0.00"}"#),
            (99_999_999_999_i64, r#"{"amount":"999999999.99"}"#),
        ];
        for (cents, expected) in cases {
            let wide = Money::<10>::from_cents(cents).expect("fits S9(10)V99");
            let narrow = Money::<9>::from_cents(cents).expect("fits S9(09)V99");
            for amount in [money(wide), money(narrow)] {
                assert_eq!(amount.scale(), 2, "{cents}");
                let json = serde_json::to_string(&Wire { amount }).expect("serializes");
                assert_eq!(json, expected, "{cents}");
            }
        }
    }

    #[test]
    fn id_string_zero_pads_digit_fields_and_trims_text_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (id_string(&AccountId::new(1)?), "00000000001"),
            (id_string(&AccountId::new(99_999_999_999)?), "99999999999"),
            (id_string(&CustomerId::new(42)?), "000000042"),
            (id_string(&TranCategoryCode::new(1)?), "0001"),
            (id_string(&MerchantId::new(800_000_000)?), "800000000"),
            (id_string(&CustSsn::new(20_973_888)?), "020973888"),
            (id_string(&FicoScore::new(7)?), "007"),
            (id_string(&NumericCode::<2>::new(5)?), "05"),
            (id_string(&NumericCode::<6>::new(0)?), "000000"),
            (id_string(&UserId::move_from("USER1")), "USER1"),
            (id_string(&UserId::move_from("ADMIN001")), "ADMIN001"),
            (id_string(&TranTypeCode::move_from("01")), "01"),
            (
                id_string(&TranId::move_from("0000000000000001")),
                "0000000000000001",
            ),
            (
                id_string(&CardNumber::move_from("4111111111111111")),
                "4111111111111111",
            ),
            (id_string(&AccountGroupId::move_from("")), ""),
            (id_string(&AccountGroupId::move_from("DEFAULT")), "DEFAULT"),
            (id_string(&TransactionRowId::from(7)), "7"),
            (id_string(&RuleId::from(42)), "42"),
            (
                id_string(&AccrualId::from(9_007_199_254_740_993)),
                "9007199254740993",
            ),
            (id_string(&AuthId::from(1)), "1"),
        ];
        for (actual, expected) in cases {
            assert_eq!(actual, expected);
        }
        // The full-width `Display` the text keys keep, which `id_string` drops.
        assert_eq!(UserId::move_from("USER1").to_string(), "USER1   ");
        Ok(())
    }

    #[test]
    fn bms_errors_dedupes_and_sorts_by_rank_then_name() {
        let fields = [
            ProbeField::Confirm,
            ProbeField::Year,
            ProbeField::Month,
            ProbeField::Year,
            ProbeField::Day,
            ProbeField::Month,
        ];
        assert_eq!(
            bms_errors(fields, "too long"),
            vec![
                field("SDTMM", "too long"),
                field("SDTDD", "too long"),
                field("SDTYYYY", "too long"),
                field("CONFIRM", "too long"),
            ]
        );
        assert!(bms_errors(Vec::<ProbeField>::new(), "").is_empty());
    }

    #[test]
    fn bms_errors_orders_equal_ranks_by_name() {
        #[derive(Clone, Copy)]
        struct Tied(&'static str);
        impl BmsField for Tied {
            fn as_str(self) -> &'static str {
                self.0
            }
            fn rank(self) -> u16 {
                1
            }
        }
        let names: Vec<String> = bms_errors([Tied("PASSWD"), Tied("USERID"), Tied("ACCTSID")], "")
            .into_iter()
            .map(|entry| entry.field)
            .collect();
        assert_eq!(names, ["ACCTSID", "PASSWD", "USERID"]);
    }

    #[test]
    fn length_errors_maps_json_names_in_audit_order() {
        let errors = all_over().validate().expect_err("every field is too long");
        let expected = vec![
            field("SDTMM", ""),
            field("SDTDD", ""),
            field("SDTYYYY", ""),
            field("CONFIRM", ""),
        ];
        // The map iterates in random order; the body must not.
        for _ in 0..16 {
            assert_eq!(length_errors(&errors, probe_field, ""), expected);
        }

        let one = Probe {
            start_day: Some(over(2)),
            ..Probe::default()
        };
        let errors = one.validate().expect_err("the day is too long");
        assert_eq!(
            length_errors(&errors, probe_field, messages::CCDA_MSG_INVALID_KEY),
            vec![field("SDTDD", messages::CCDA_MSG_INVALID_KEY)]
        );

        let at_limit = Probe {
            start_year: Some("2022".to_owned()),
            start_month: Some(String::new()),
            ..Probe::default()
        };
        assert!(at_limit.validate().is_ok());
    }

    #[test]
    fn reject_invalid_answers_400_invalid_with_the_fields() {
        let errors = all_over().validate().expect_err("every field is too long");
        for message in ["", messages::CCDA_MSG_INVALID_KEY] {
            let error = reject_invalid(&errors, probe_field, message);
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                serde_json::to_value(error.body()).expect("serializes"),
                json!({
                    "code": ErrorKind::Invalid.code(),
                    "message": message,
                    "fields": [
                        {"field": "SDTMM", "message": message},
                        {"field": "SDTDD", "message": message},
                        {"field": "SDTYYYY", "message": message},
                        {"field": "CONFIRM", "message": message},
                    ],
                })
            );
        }
    }
}
