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
//!   the audited COBOL message. The only edits a request applies itself, each
//!   published as `#[schema(max_length, pattern)]` and rejected through
//!   `reject_invalid` before any rule runs, are the BMS field width
//!   (`#[validate(length(max))]`) and the encoding checks that apply to the
//!   member: ASCII ([`ascii_text`]) where a rule moves the value byte for
//!   byte into an `X(n)` field with no edit that reads those bytes, or with
//!   an edit whose answer quotes that field back (the `CORPT00C`
//!   confirmation, `reports`), and, for free text a rule stores, no NUL
//!   character beside other text ([`storable_text`],
//!   [`storable_text_of_width`]), which `PostgreSQL` text cannot hold. Where
//!   a program's audited edit reads the value, or its rule searches the value
//!   as entered, the rule answers it and no encoding check precedes it: the
//!   `COTRTUPC` type code and description and the `COTRTLIC` row description
//!   and description filter (`admin_transaction_types`), the `CORPT00C` date
//!   parts (`reports`), and the sign-on fields, which get the width check and
//!   then the `COSGN00C` blank edits (`auth`).
//!   The reward DTOs (`admin_reward_rules`, `rewards`) serve no COBOL program
//!   or BMS map and are the exception: their requests carry their own
//!   required members and request-written checks.
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
use validator::{ValidationError, ValidationErrors};

use crate::error::{ApiError, ErrorFieldBody};

/// The `validator` check that request text is ASCII, for the members and keys
/// a rule moves byte for byte into an `X(n)` field.
///
/// The rules receive text through [`Alnum::move_from`], the COBOL `MOVE` of
/// the UTF-8 bytes, which stores a non-ASCII character as two to four bytes
/// and cuts it at the field width; the stored bytes then read back one
/// Latin-1 character each, so `é` would come back as `Ã©`. Within ASCII,
/// characters and bytes agree, so the width the length check counts is the
/// width the `MOVE` fills. Spaces and low-values are ASCII: blank members
/// still reach the rule, which answers them with its own message. Free text
/// the rule stores is checked by [`storable_text`] or
/// [`storable_text_of_width`] instead, which add the NUL check to this one.
///
/// The published API document carries this check on each such member as the
/// pattern `^[\x00-\x7F]*$`. A failure is reported under the `ascii` code,
/// and the DTO maps it to the member's BMS field as it maps a length failure.
///
/// # Errors
///
/// A [`ValidationError`] with code `ascii` when `value` holds a character
/// above `U+007F`.
pub fn ascii_text(value: &str) -> Result<(), ValidationError> {
    if value.is_ascii() {
        Ok(())
    } else {
        Err(ValidationError::new("ascii"))
    }
}

/// The `validator` check of the free text a rule stores in an `X(n)` record
/// field whenever it is not blank, for a rule that reads any text made only
/// of NUL characters as `LOW-VALUES`: ASCII, as [`ascii_text`] checks it, and
/// either no NUL character or nothing but NUL characters.
///
/// The `COTRN02C` source, description and merchant name, city and zip
/// (`transaction_add`) and the `COUSR02C` first and last names
/// (`user_update`) are read that way, so an all-NUL value reaches the rule,
/// which answers it with its own `can NOT be empty` message. Any other value
/// is written as entered, and `PostgreSQL` text cannot hold a NUL character
/// (E-07), so the write could only fail. A 3270 field never sends NUL
/// characters mixed with other text, so such a value has no COBOL counterpart
/// and is answered as the non-ASCII and over-length ones are: 400 `Invalid`,
/// no message, its BMS field named (E-49). The value is never stripped or
/// altered.
///
/// The published API document carries this check on each such member as the
/// pattern `^(?:\x00+|[\x01-\x7F]*)$`.
///
/// # Errors
///
/// A [`ValidationError`] with code `ascii` when `value` holds a character
/// above `U+007F`, otherwise with code `nul` when it holds a NUL character
/// beside any other character.
pub fn storable_text(value: &str) -> Result<(), ValidationError> {
    storable(value, low_values(value))
}

/// [`storable_text`] for a rule that reads NUL characters as `LOW-VALUES`
/// only when they fill the `X(N)` field: ASCII, and either no NUL character or
/// exactly `N` of them.
///
/// The `COUSR01C` edits (`user_add`) `MOVE` the entry into its `X(N)` field
/// first, which pads it with spaces, so only `N` NUL characters stay
/// `LOW-VALUES` and get the empty-field message; a shorter run of NUL
/// characters is stored with the padding, which `PostgreSQL` text cannot hold
/// (E-07). The published API document carries this check as the pattern
/// `^(?:\x00{N}|[\x01-\x7F]*)$`.
///
/// # Errors
///
/// A [`ValidationError`] with code `ascii` when `value` holds a character
/// above `U+007F`, otherwise with code `nul` when it holds a NUL character
/// and is not exactly `N` of them.
pub fn storable_text_of_width<const N: usize>(value: &str) -> Result<(), ValidationError> {
    storable(value, value.len() == N && low_values(value))
}

/// Whether `value` is a text made only of NUL characters, which a rule reads
/// as `LOW-VALUES`.
fn low_values(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte == 0)
}

/// The ASCII check of `value`, then its NUL characters refused unless the
/// rule reads the whole value as `LOW-VALUES` (`blank`).
fn storable(value: &str, blank: bool) -> Result<(), ValidationError> {
    ascii_text(value)?;
    if blank || !value.contains('\0') {
        Ok(())
    } else {
        Err(ValidationError::new("nul"))
    }
}

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

/// The error-body fields of a failed DTO check: each request field that fails
/// its width or encoding check mapped to its BMS field by `map`, through
/// [`bms_errors`].
///
/// `map` takes the request field's JSON name, which `validator` reports
/// whatever the failing check, and must cover every field the DTO validates;
/// a name it maps to `None` is left out. A field that fails more than one
/// check, and two request fields that share a BMS field, give one entry.
#[must_use]
pub(crate) fn length_errors<F: BmsField>(
    errors: &ValidationErrors,
    map: impl Fn(&str) -> Option<F>,
    message: &str,
) -> Vec<ErrorFieldBody> {
    bms_errors(errors.field_errors().into_keys().filter_map(map), message)
}

/// The 400 answer to a request whose fields fail their width or encoding
/// checks: kind `Invalid`, `message`, and the failing BMS fields from
/// [`length_errors`].
///
/// `message` is the `carddemo_domain::messages` constant the validation
/// audit's over-length row names for the program, or `""` where the audit
/// records the path as `no-message` and links its escalation. A DTO with an
/// encoding check passes `""`: no 3270 field sends a non-ASCII character or
/// NUL characters beside other text, so neither has an audited text (E-49).
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
    use std::borrow::Cow;

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

    use super::{
        BmsField, ascii_text, bms_errors, id_string, length_errors, money, reject_invalid,
        storable_text, storable_text_of_width, text,
    };
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
    fn ascii_text_accepts_ascii_only() {
        let accepted = ["", " ", "\0\0", "ABC 123", "\u{7f}", "~`!@#$%^&*()"];
        for value in accepted {
            assert!(ascii_text(value).is_ok(), "{value:?}");
        }
        let rejected = ["\u{e9}", "A\u{e9}", "\u{80}", "\u{20ac}", "\u{1f600}"];
        for value in rejected {
            let error = ascii_text(value).expect_err(value);
            assert_eq!(error.code, "ascii", "{value:?}");
        }
    }

    #[test]
    fn storable_text_admits_nul_only_as_the_low_values_the_rule_reads_as_blank() {
        // (value, code of `storable_text`, code of `storable_text_of_width::<4>`)
        let cases = [
            ("", None, None),
            (" ", None, None),
            ("    ", None, None),
            ("ABC 123", None, None),
            ("\u{1}\u{7f}", None, None),
            ("\0", None, Some("nul")),
            ("\0\0", None, Some("nul")),
            ("\0\0\0", None, Some("nul")),
            ("\0\0\0\0", None, None),
            ("A\0", Some("nul"), Some("nul")),
            ("\0A", Some("nul"), Some("nul")),
            ("A\0B", Some("nul"), Some("nul")),
            ("\0 ", Some("nul"), Some("nul")),
            (" \0", Some("nul"), Some("nul")),
            ("\0\0\0 ", Some("nul"), Some("nul")),
            ("\u{e9}", Some("ascii"), Some("ascii")),
            ("\0\u{e9}", Some("ascii"), Some("ascii")),
            ("\u{1f600}", Some("ascii"), Some("ascii")),
        ];
        for (value, any_length, full_width) in cases {
            let codes = [
                storable_text(value).err().map(|error| error.code),
                storable_text_of_width::<4>(value)
                    .err()
                    .map(|error| error.code),
            ];
            assert_eq!(
                codes,
                [any_length.map(Cow::from), full_width.map(Cow::from)],
                "{value:?}"
            );
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
