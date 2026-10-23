//! Endpoint 14, `POST /api/v1/reports/transactions` (transaction `CR00`, program CORPT00C, map
//! `CORPT0A`): the request that submits the transaction report and the result it answers with.
//!
//! - [`ReportRequest`] carries the ten inputs of the map as typed: the three report selectors,
//!   the six date parts and the confirmation (CORPT00C-042). Its rules are only the map's
//!   `LENGTH` and `NUM` attributes, the server half of the `both` rows `CORPT00-012` to
//!   `CORPT00-038` of `modern/audit/screens.md`, and the `PIC X(n)` storage of each input
//!   (`super::pic_x`, X-COUSR01C-3). Every content message, from
//!   `Select a report type to print report...` (CORPT00C-029) through the blank-part and range
//!   edits (CORPT00C-011 to CORPT00C-023) to `Please confirm to print the Monthly report...`
//!   (CORPT00C-030), is CORPT00C's own and comes from `domain::online::corpt00c`.
//! - [`ReportResult`] carries the message row and, when a deck was written, the name of its
//!   generation file `TRNRPT00.G<nnnn>` (CORPT00C-038). The endpoint writes the job deck and
//!   nothing else: it runs no report and returns no report content, no file content and no
//!   directory, because report and statement files hold full card numbers (Cardholder Data
//!   Redaction; CORPT00C-039).
//!
//! The date parts stay raw text, the raw-edit-input exception to Typed Dates and Timestamps:
//! typing them would replace CORPT00C's month, day and year messages and CSUTLDTC's date-validity
//! verdict (halt H5) with a deserialisation error. No part is trimmed, padded or case-folded
//! here; the program reads each as a `MOVE` into its `PIC X(n)` field (CORPT00C-017).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// The ten inputs of `CORPT0A` exactly as the screen sends them (CORPT00C-042). An absent
/// member is the empty text, which the program treats as blank.
///
/// Monthly and Yearly take their dates from the server's clock and ignore the date parts
/// (CORPT00C-007 to CORPT00C-010); Custom edits the six parts (CORPT00C-011 to CORPT00C-028).
/// The confirmation is read only once the report and its dates are set (CORPT00C-030 to
/// CORPT00C-033).
///
/// A member that does not fit its `PIC X(n)` map field byte for byte as printable ASCII, being
/// longer than the field or holding a character outside ASCII or a control byte, is a 400 naming
/// it, with the field-length message `FIELD_LENGTH_MESSAGE` (X-COUSR01C-3). A date part that
/// fits, but holds a character other than a digit apart from spaces before and after the
/// digits, is a 400 naming it, with the digits-only message `FIELD_NUMERIC_MESSAGE`
/// (X-CORPT00C-5); a part of digits with surrounding spaces, such as `" 5"`, reaches the
/// program, which reads it as the value of its digits (CORPT00C-017).
// The schema name `ReportRequest` is the frontend's `components["schemas"]["ReportRequest"]`
// (Generated API Types Only). The domain's own `ReportRequest`, the edited report, is a different
// type, so `api::services::corpt00c` imports this one under an alias.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({
    "monthly": "",
    "yearly": "",
    "custom": "X",
    "start_month": "01",
    "start_day": "01",
    "start_year": "2022",
    "end_month": "07",
    "end_day": "06",
    "end_year": "2022",
    "confirm": "Y"
}))]
pub struct ReportRequest {
    /// `MONTHLY`, `PIC X(1)` (`CORPT00-012`): any character other than a space selects the
    /// report of the current month (CORPT00C-007, CORPT00C-008).
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub monthly: String,

    /// `YEARLY`, `PIC X(1)` (`CORPT00-015`): selects the report of the current year when
    /// `monthly` is blank (CORPT00C-007, CORPT00C-010).
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub yearly: String,

    /// `CUSTOM`, `PIC X(1)` (`CORPT00-018`): selects the report of the date range below when
    /// `monthly` and `yearly` are blank (CORPT00C-007).
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub custom: String,

    /// `SDTMM`, `PIC X(2)`, `NUM` (`CORPT00-022`): the start month of a custom report, digits
    /// with or without spaces around them, or blank (CORPT00C-011, CORPT00C-017, CORPT00C-018).
    #[serde(default)]
    #[schema(max_length = 2, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 2),
        custom(function = "super::pic_x::<2>"),
        custom(function = "super::numeric")
    )]
    pub start_month: String,

    /// `SDTDD`, `PIC X(2)`, `NUM` (`CORPT00-024`): the start day of a custom report, digits with
    /// or without spaces around them, or blank (CORPT00C-012, CORPT00C-017, CORPT00C-019).
    #[serde(default)]
    #[schema(max_length = 2, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 2),
        custom(function = "super::pic_x::<2>"),
        custom(function = "super::numeric")
    )]
    pub start_day: String,

    /// `SDTYYYY`, `PIC X(4)`, `NUM` (`CORPT00-026`): the start year of a custom report, digits
    /// with or without spaces around them, or blank (CORPT00C-013, CORPT00C-017).
    #[serde(default)]
    #[schema(max_length = 4, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 4),
        custom(function = "super::pic_x::<4>"),
        custom(function = "super::numeric")
    )]
    pub start_year: String,

    /// `EDTMM`, `PIC X(2)`, `NUM` (`CORPT00-030`): the end month of a custom report, digits with
    /// or without spaces around them, or blank (CORPT00C-014, CORPT00C-017, CORPT00C-021).
    #[serde(default)]
    #[schema(max_length = 2, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 2),
        custom(function = "super::pic_x::<2>"),
        custom(function = "super::numeric")
    )]
    pub end_month: String,

    /// `EDTDD`, `PIC X(2)`, `NUM` (`CORPT00-032`): the end day of a custom report, digits with or
    /// without spaces around them, or blank (CORPT00C-015, CORPT00C-017, CORPT00C-022).
    #[serde(default)]
    #[schema(max_length = 2, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 2),
        custom(function = "super::pic_x::<2>"),
        custom(function = "super::numeric")
    )]
    pub end_day: String,

    /// `EDTYYYY`, `PIC X(4)`, `NUM` (`CORPT00-034`): the end year of a custom report, digits with
    /// or without spaces around them, or blank (CORPT00C-016, CORPT00C-017).
    #[serde(default)]
    #[schema(max_length = 4, pattern = "^ *[0-9]* *$")]
    #[validate(
        length(max = 4),
        custom(function = "super::pic_x::<4>"),
        custom(function = "super::numeric")
    )]
    pub end_year: String,

    /// `CONFIRM`, `PIC X(1)` (`CORPT00-038`): `Y` or `y` writes the job deck, `N` or `n` clears
    /// the screen, blank asks for confirmation and any other character is rejected
    /// (CORPT00C-030 to CORPT00C-033).
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    pub confirm: String,
}

/// The 200 of endpoint 14: the message row and, when the job deck was written, its file name.
///
/// - Written deck: `message` is the report name and ` report submitted for printing ...`, such
///   as `Monthly report submitted for printing ...`, and `job_deck` the generation file's name
///   (CORPT00C-038).
/// - Confirmation `N` or `n`: an empty `message` and no `job_deck`; the screen empties every
///   input (CORPT00C-032).
///
/// Every other outcome, the prompt of a blank confirmation included, is a `ScreenError` with the
/// status its CORPT00C row assigns (CORPT00C-029 to CORPT00C-033, CORPT00C-037).
// The schema name `ReportResult` is the frontend's `components["schemas"]["ReportResult"]`
// (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "message": "Monthly report submitted for printing ...",
    "job_deck": "TRNRPT00.G0001"
}))]
pub struct ReportResult {
    /// The message row as CORPT00C words it, without trailing spaces.
    pub message: String,

    /// The name of the generation file the deck was written to, such as `TRNRPT00.G0001`:
    /// the file name alone, never a directory or an absolute path, so the response does not
    /// disclose where the server keeps its files. Absent when no deck was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "TRNRPT00.G0001")]
    pub job_deck: Option<String>,
}

/// `(message, written file name)`, as `api::services::corpt00c::submit` returns them: the
/// submission message with the generation file's name, or the empty message of a cleared screen
/// with `None`.
impl From<(String, Option<String>)> for ReportResult {
    fn from((message, job_deck): (String, Option<String>)) -> Self {
        Self { message, job_deck }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use chrono::NaiveDate;
    use domain::FixedClock;
    use domain::online::corpt00c::{self as rules, ConfirmOutcome, ReportInput};
    use utoipa::ToSchema;

    use super::{ReportRequest, ReportResult};
    use crate::dto::{FIELD_LENGTH_MESSAGE, FIELD_NUMERIC_MESSAGE, ValidJson};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The wire names of the ten members, in map order.
    const MEMBERS: [&str; 10] = [
        "monthly",
        "yearly",
        "custom",
        "start_month",
        "start_day",
        "start_year",
        "end_month",
        "end_day",
        "end_year",
        "confirm",
    ];

    /// `body` as endpoint 14 receives it, through the extractor every handler uses.
    async fn extract(body: &str) -> Result<Result<ReportRequest, ScreenError>, Box<dyn StdError>> {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/reports/transactions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))?;
        Ok(ValidJson::<ReportRequest>::from_request(request, &())
            .await
            .map(|ValidJson(value)| value))
    }

    /// The status, message and field of `error`.
    fn parts(error: &ScreenError) -> (StatusCode, &str, Option<&str>) {
        (error.status(), error.message(), error.field())
    }

    /// The domain input `api::services::corpt00c` builds from the request: every member as is.
    fn input(request: &ReportRequest) -> ReportInput<'_> {
        ReportInput {
            monthly: &request.monthly,
            yearly: &request.yearly,
            custom: &request.custom,
            start_month: &request.start_month,
            start_day: &request.start_day,
            start_year: &request.start_year,
            end_month: &request.end_month,
            end_day: &request.end_day,
            end_year: &request.end_year,
            confirm: &request.confirm,
        }
    }

    fn clock() -> Result<FixedClock, &'static str> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_opt(10, 30, 0))
            .map(FixedClock::new)
            .ok_or("invalid test instant")
    }

    #[tokio::test]
    async fn members_travel_under_their_rust_names_and_default_to_empty() -> TestResult {
        let wire = serde_json::to_value(ReportRequest::default())?;
        let keys: Vec<&str> = wire
            .as_object()
            .ok_or("not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected = MEMBERS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);

        assert_eq!(extract("{}").await??, ReportRequest::default());

        // audit: CORPT00C-042
        let typed = extract(r#"{"monthly":"n","start_month":"  ","confirm":"y"}"#).await??;
        assert_eq!(
            (
                typed.monthly.as_str(),
                typed.start_month.as_str(),
                typed.confirm.as_str()
            ),
            ("n", "  ", "y")
        );
        Ok(())
    }

    #[tokio::test]
    async fn map_length_and_num_are_revalidated_naming_the_member() -> TestResult {
        let cases = [
            ("monthly", "XX", FIELD_LENGTH_MESSAGE),
            ("yearly", "XX", FIELD_LENGTH_MESSAGE),
            ("custom", "XX", FIELD_LENGTH_MESSAGE),
            ("confirm", "YY", FIELD_LENGTH_MESSAGE),
            ("start_month", "123", FIELD_LENGTH_MESSAGE),
            ("end_year", "20225", FIELD_LENGTH_MESSAGE),
            ("start_month", "AB", FIELD_NUMERIC_MESSAGE),
            ("start_day", "1A", FIELD_NUMERIC_MESSAGE),
            ("start_year", "20X2", FIELD_NUMERIC_MESSAGE),
            ("end_month", "-3", FIELD_NUMERIC_MESSAGE),
            ("end_day", "1.", FIELD_NUMERIC_MESSAGE),
            ("end_year", "22.0", FIELD_NUMERIC_MESSAGE),
            // A space between digits is not one of the spaces NUMVAL-C reads around them.
            ("start_year", "20 2", FIELD_NUMERIC_MESSAGE),
            ("end_year", " 2 2", FIELD_NUMERIC_MESSAGE),
            // `LOW-VALUES` fits the field but is not digits.
            ("start_month", "\0\0", FIELD_NUMERIC_MESSAGE),
        ];
        for (member, value, message) in cases {
            let body = format!("{{\"{member}\":{}}}", serde_json::to_string(value)?);
            let error = extract(&body)
                .await?
                .err()
                .ok_or_else(|| format!("{member} = {value:?} accepted"))?;
            assert_eq!(
                parts(&error),
                (StatusCode::BAD_REQUEST, message, Some(member)),
                "{member} = {value:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn text_the_pic_x_field_cannot_hold_is_a_400_naming_the_member() -> TestResult {
        // Within `length` in characters, but not printable ASCII that fits the field byte for
        // byte: a character outside ASCII, a control byte, or NUL that does not fill the field.
        // The date parts answer the field-length message, not the digits-only one, because the
        // field cannot hold the value at all.
        let mut cases: Vec<(&str, &str)> = MEMBERS
            .iter()
            .flat_map(|&member| ["é", "\t", "\u{7f}", "\u{1F600}"].map(|value| (member, value)))
            .collect();
        cases.extend([
            ("start_day", "\t5"),
            ("start_year", "20é"),
            ("end_month", "\0"),
            ("end_day", "5\u{7f}"),
            ("end_year", "ééé"),
        ]);
        for (member, value) in cases {
            let body = format!("{{\"{member}\":{}}}", serde_json::to_string(value)?);
            let error = extract(&body)
                .await?
                .err()
                .ok_or_else(|| format!("{member} = {value:?} accepted"))?;
            assert_eq!(
                parts(&error),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(member)),
                "{member} = {value:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn values_the_map_admits_reach_the_program_edits() -> TestResult {
        for (member, value) in [
            ("monthly", "N"),
            ("monthly", "\0"),
            ("start_month", "13"),
            ("start_month", "00"),
            ("start_day", "  "),
            ("start_year", "0000"),
            ("start_year", "    "),
            ("end_year", "9999"),
            ("end_day", ""),
            ("confirm", "x"),
            ("confirm", " "),
        ] {
            let body = format!("{{\"{member}\":{}}}", serde_json::to_string(value)?);
            assert!(extract(&body).await?.is_ok(), "{member} = {value:?}");
        }

        // Digits with spaces before or after them are admitted and reach the program as typed,
        // untrimmed; NUMVAL-C, not this DTO, reads each as the value of its digits.
        let padded = extract(
            r#"{"custom":"X","start_month":" 5","start_day":"5 ","start_year":"  22",
                "end_month":"7 ","end_day":" 6","end_year":"22  "}"#,
        )
        .await??;
        assert_eq!(
            [
                padded.start_month.as_str(),
                padded.start_day.as_str(),
                padded.start_year.as_str(),
                padded.end_month.as_str(),
                padded.end_day.as_str(),
                padded.end_year.as_str(),
            ],
            [" 5", "5 ", "  22", "7 ", " 6", "22  "]
        );
        // The space-padded month `5 ` is `05` and passes the month edit, which its text, above
        // `12`, would fail; the day `99` is then the first part rejected.
        // audit: CORPT00C-017
        let month_passed = extract(
            r#"{"custom":"X","start_month":"5 ","start_day":"99","start_year":"2022",
                "end_month":"07","end_day":"06","end_year":"2022"}"#,
        )
        .await??;
        let failure = rules::edit(&input(&month_passed), &clock()?)
            .err()
            .ok_or("day 99 passed the day edit")?;
        assert_eq!(
            parts(&ScreenError::from(failure)),
            (
                StatusCode::BAD_REQUEST,
                "Start Date - Not a valid Day...",
                Some("start_day")
            )
        );

        // audit: CORPT00C-018
        let request = extract(
            r#"{"custom":"X","start_month":"13","start_day":"01","start_year":"2022",
                "end_month":"07","end_day":"06","end_year":"2022"}"#,
        )
        .await??;
        let failure = rules::edit(&input(&request), &clock()?)
            .err()
            .ok_or("month 13 passed the month edit")?;
        assert_eq!(
            parts(&ScreenError::from(failure)),
            (
                StatusCode::BAD_REQUEST,
                "Start Date - Not a valid Month...",
                Some("start_month")
            )
        );
        Ok(())
    }

    #[tokio::test]
    async fn every_report_failure_names_a_request_member() -> TestResult {
        let clock = clock()?;
        // The first edit that fails names the member: CORPT00C-029, then the blank-part edits
        // CORPT00C-011 to CORPT00C-016 in program order.
        let steps = [
            ("monthly", "{}"),
            ("start_month", r#"{"custom":"X"}"#),
            ("start_day", r#"{"custom":"X","start_month":"01"}"#),
            (
                "start_year",
                r#"{"custom":"X","start_month":"01","start_day":"01"}"#,
            ),
            (
                "end_month",
                r#"{"custom":"X","start_month":"01","start_day":"01","start_year":"2022"}"#,
            ),
            (
                "end_day",
                r#"{"custom":"X","start_month":"01","start_day":"01","start_year":"2022",
                    "end_month":"07"}"#,
            ),
            (
                "end_year",
                r#"{"custom":"X","start_month":"01","start_day":"01","start_year":"2022",
                    "end_month":"07","end_day":"06"}"#,
            ),
        ];
        let mut failures = Vec::new();
        for (member, body) in steps {
            let request = extract(body).await??;
            failures.push((member, rules::edit(&input(&request), &clock).err()));
        }
        // audit: CORPT00C-030
        let monthly = extract(r#"{"monthly":"X"}"#).await??;
        let edited = rules::edit(&input(&monthly), &clock)?;
        failures.push(("confirm", rules::confirm(edited, &monthly.confirm).err()));

        for (member, failure) in failures {
            let error = ScreenError::from(failure.ok_or_else(|| format!("{member} passed"))?);
            assert!(MEMBERS.contains(&member));
            assert_eq!(
                (error.status(), error.field()),
                (StatusCode::BAD_REQUEST, Some(member))
            );
        }

        // audit: CORPT00C-037
        let write = ScreenError::from(rules::tdq_write_failure());
        assert_eq!(
            (write.status(), write.field()),
            (StatusCode::INTERNAL_SERVER_ERROR, Some("monthly"))
        );

        // audit: CORPT00C-032
        let cleared = extract(r#"{"monthly":"X","confirm":"n"}"#).await??;
        let edited = rules::edit(&input(&cleared), &clock)?;
        assert_eq!(
            rules::confirm(edited, &cleared.confirm)?,
            ConfirmOutcome::Cleared
        );
        Ok(())
    }

    #[test]
    fn the_result_carries_the_message_and_the_deck_name_only() -> TestResult {
        // audit: CORPT00C-038
        let submitted = ReportResult::from((
            rules::submitted_message(rules::ReportKind::Monthly),
            Some("TRNRPT00.G0001".to_owned()),
        ));
        assert_eq!(
            serde_json::to_value(&submitted)?,
            serde_json::json!({
                "message": "Monthly report submitted for printing ...",
                "job_deck": "TRNRPT00.G0001"
            })
        );

        // audit: CORPT00C-032
        let cleared = ReportResult::from((String::new(), None));
        assert_eq!(
            serde_json::to_value(&cleared)?,
            serde_json::json!({"message": ""})
        );
        assert_eq!(
            serde_json::from_str::<ReportResult>(r#"{"message":""}"#)?,
            cleared
        );
        Ok(())
    }

    #[test]
    fn schema_names_are_the_contract_names() {
        assert_eq!(ReportRequest::name(), "ReportRequest");
        assert_eq!(ReportResult::name(), "ReportResult");
    }
}
