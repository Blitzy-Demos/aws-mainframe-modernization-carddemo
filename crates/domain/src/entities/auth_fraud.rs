//! `AuthFraud`: a row of the Db2 table `CARDDEMO.AUTHFRDS`, stored as the
//! table `auth_fraud` of `migrations/0004_authorizations.sql`.
//!
//! CPVD writes the table when an operator marks or unmarks an authorization as
//! fraud. `COPAUS1C` links `COPAUS2C`, which moves the pending authorization
//! detail into the host structure `DCLAUTHFRDS` and inserts it
//! [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L103-198]; on
//! `SQLCODE -803` it updates `AUTH_FRAUD` and `FRAUD_RPT_DATE` of the existing
//! row instead [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L221-244]
//! (P-COPAUS2C-001 to P-COPAUS2C-005). `rules::auth_detail` builds the row and
//! persistence stores it.

use chrono::{NaiveDate, NaiveDateTime};

use crate::numeric::{AccountId, Alnum, CardNumber, CustomerId, Money, NumericCode};

/// One authorization reported as fraud, or whose fraud report was removed: a
/// row of `CARDDEMO.AUTHFRDS`.
///
/// # Columns
///
/// The fields are the 26 columns of `AUTHFRDS.ddl`
/// [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L1-28] in DDL order,
/// named by the column name in lower case with the DDL spelling kept
/// (`merchant_catagory_code`). The host structure is `AUTHFRDS.dcl`
/// [app/app-authorization-ims-db2-mq/dcl/AUTHFRDS.dcl:L56-86]. Each field names
/// its reviewed section A row of `docs/parity/parity-oracle-audit.md`, its DDL
/// line, its host variable line in `AUTHFRDS.dcl` and the `COPAUS2C.cbl` line
/// that fills it.
///
/// # Key
///
/// The primary key is (`CARD_NUM`, `AUTH_TS`)
/// [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L28], and the unique
/// index `XAUTHFRD (CARD_NUM ASC, AUTH_TS DESC)`
/// [app/app-authorization-ims-db2-mq/ddl/XAUTHFRD.ddl:L1-4] lists each card's
/// reports newest first. A second report of the same authorization violates
/// the key, which `COPAUS2C` turns into its `FRAUD-UPDATE` path
/// (P-COPAUS2C-004).
///
/// # Nullability
///
/// `card_num` and `auth_ts` are `NOT NULL`; every other column is nullable in
/// the DDL and is an `Option` here, `None` being SQL `NULL`. `COPAUS2C` passes
/// no null indicator, so every row it writes holds every column.
///
/// The record has no constructor: callers build it as a struct literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFraud {
    /// `CARD_NUM CHAR(16) NOT NULL`, the first key column (P-AUTHFRDS-001;
    /// DDL L2, host `CARD-NUM X(16)` L56, `MOVE PA-CARD-NUM` L113).
    pub card_num: CardNumber,
    /// `AUTH_TS TIMESTAMP NOT NULL`, the second key column, descending in
    /// `XAUTHFRD` (P-AUTHFRDS-002; DDL L3, host `AUTH-TS X(26)` L57,
    /// `MOVE WS-AUTH-TS` L114).
    ///
    /// `COPAUS2C` builds the text `WS-AUTH-TS` as `YY-MM-DD HH.MI.SSsss000`
    /// from `PA-AUTH-ORIG-DATE` and the un-complemented `PA-AUTH-TIME-9C`,
    /// `999999999 - PA-AUTH-TIME-9C` (L38-51, L103-114), and inserts it through
    /// `TIMESTAMP_FORMAT(:AUTH-TS, 'YY-MM-DD HH24.MI.SSNNNNNN')` (L171-172)
    /// (P-COPAUS2C-001). The value therefore has microsecond resolution, its
    /// microseconds being the milliseconds times 1000, which the
    /// `TIMESTAMP(6)` column holds exactly. That conversion, including the
    /// century of the two-digit year, belongs to `rules::auth_detail`.
    //
    // A timestamp, not the `Alnum<26>` the section A row labels the column
    // with: 26 is the length of the string form of the same `TIMESTAMP(6)`
    // value that the row's SQL type and migration 0004 store. Db2 compares the
    // column only as a timestamp (the key, the descending `XAUTHFRD` order and
    // `FRAUD-UPDATE`'s `WHERE AUTH_TS = TIMESTAMP_FORMAT(...)`, L227-228), and
    // no program reads it back as text, so no COBOL text comparison needs it.
    pub auth_ts: NaiveDateTime,
    /// `AUTH_TYPE CHAR(4)` (P-AUTHFRDS-003; DDL L4, host L58,
    /// `MOVE PA-AUTH-TYPE` L115).
    pub auth_type: Option<Alnum<4>>,
    /// `CARD_EXPIRY_DATE CHAR(4)` (P-AUTHFRDS-004; DDL L5, host L59,
    /// `MOVE PA-CARD-EXPIRY-DATE` L116).
    pub card_expiry_date: Option<Alnum<4>>,
    /// `MESSAGE_TYPE CHAR(6)` (P-AUTHFRDS-005; DDL L6, host L60,
    /// `MOVE PA-MESSAGE-TYPE` L117).
    pub message_type: Option<Alnum<6>>,
    /// `MESSAGE_SOURCE CHAR(6)` (P-AUTHFRDS-006; DDL L7, host L61,
    /// `MOVE PA-MESSAGE-SOURCE` L118).
    pub message_source: Option<Alnum<6>>,
    /// `AUTH_ID_CODE CHAR(6)` (P-AUTHFRDS-007; DDL L8, host L62,
    /// `MOVE PA-AUTH-ID-CODE` L119).
    pub auth_id_code: Option<Alnum<6>>,
    /// `AUTH_RESP_CODE CHAR(2)` (P-AUTHFRDS-008; DDL L9, host L63,
    /// `MOVE PA-AUTH-RESP-CODE` L120).
    pub auth_resp_code: Option<Alnum<2>>,
    /// `AUTH_RESP_REASON CHAR(4)` (P-AUTHFRDS-009; DDL L10, host L64,
    /// `MOVE PA-AUTH-RESP-REASON` L121).
    pub auth_resp_reason: Option<Alnum<4>>,
    /// `PROCESSING_CODE CHAR(6)` (P-AUTHFRDS-010; DDL L11, host
    /// `PROCESSING-CODE X(6)` L65, `MOVE PA-PROCESSING-CODE` L122). The
    /// unsigned `PA-PROCESSING-CODE 9(06)` is moved in as text: its six digit
    /// characters.
    pub processing_code: Option<Alnum<6>>,
    /// `TRANSACTION_AMT DECIMAL(12,2)` (P-AUTHFRDS-011; DDL L12, host
    /// `S9(10)V9(2) COMP-3` L66, `MOVE PA-TRANSACTION-AMT` L123).
    pub transaction_amt: Option<Money<10>>,
    /// `APPROVED_AMT DECIMAL(12,2)` (P-AUTHFRDS-012; DDL L13, host
    /// `S9(10)V9(2) COMP-3` L67, `MOVE PA-APPROVED-AMT` L124).
    pub approved_amt: Option<Money<10>>,
    /// `MERCHANT_CATAGORY_CODE CHAR(4)` (P-AUTHFRDS-013; DDL L14, host
    /// L68-69, `MOVE PA-MERCHANT-CATAGORY-CODE` L125-126).
    pub merchant_catagory_code: Option<Alnum<4>>,
    /// `ACQR_COUNTRY_CODE CHAR(3)` (P-AUTHFRDS-014; DDL L15, host L70,
    /// `MOVE PA-ACQR-COUNTRY-CODE` L127).
    pub acqr_country_code: Option<Alnum<3>>,
    /// `POS_ENTRY_MODE SMALLINT` (P-AUTHFRDS-015; DDL L16, host `S9(4) COMP`
    /// L71, `MOVE PA-POS-ENTRY-MODE` L128). The moved field is the unsigned
    /// `PA-POS-ENTRY-MODE 9(02)`, so a value `COPAUS2C` stores lies in
    /// `0..=99`.
    //
    // A two-digit code, not the `i16` of the column: the section A row labels
    // the column `PosEntryMode`, which `numeric::ids` binds for this row to
    // `NumericCode<2>`, the type of the detail field it is copied from.
    // Persistence converts it at the `SMALLINT` boundary.
    pub pos_entry_mode: Option<NumericCode<2>>,
    /// `MERCHANT_ID CHAR(15)` (P-AUTHFRDS-016; DDL L17, host L72,
    /// `MOVE PA-MERCHANT-ID` L129).
    pub merchant_id: Option<Alnum<15>>,
    /// `MERCHANT_NAME VARCHAR(22)` (P-AUTHFRDS-017; DDL L18, host
    /// `MERCHANT-NAME-LEN S9(4) COMP` and `MERCHANT-NAME-TEXT X(22)` L73-77,
    /// `MOVE PA-MERCHANT-NAME` L130-131).
    ///
    /// `COPAUS2C` always sets the length to `LENGTH OF PA-MERCHANT-NAME`, 22
    /// (L130), so the stored value is always the full 22 bytes, trailing
    /// spaces included, and the fixed width is exact. Migration 0004 stores
    /// the text right-trimmed, and padding it back to 22 bytes restores the
    /// same value.
    pub merchant_name: Option<Alnum<22>>,
    /// `MERCHANT_CITY CHAR(13)` (P-AUTHFRDS-018; DDL L19, host L78,
    /// `MOVE PA-MERCHANT-CITY` L132).
    pub merchant_city: Option<Alnum<13>>,
    /// `MERCHANT_STATE CHAR(02)` (P-AUTHFRDS-019; DDL L20, host L79,
    /// `MOVE PA-MERCHANT-STATE` L133).
    pub merchant_state: Option<Alnum<2>>,
    /// `MERCHANT_ZIP CHAR(09)` (P-AUTHFRDS-020; DDL L21, host L80,
    /// `MOVE PA-MERCHANT-ZIP` L134).
    pub merchant_zip: Option<Alnum<9>>,
    /// `TRANSACTION_ID CHAR(15)` (P-AUTHFRDS-021; DDL L22, host L81,
    /// `MOVE PA-TRANSACTION-ID` L135): the authorization's own transaction
    /// reference, unrelated to the row identity of the transaction master.
    pub transaction_id: Option<Alnum<15>>,
    /// `MATCH_STATUS CHAR(1)` (P-AUTHFRDS-022; DDL L23, host L82,
    /// `MOVE PA-MATCH-STATUS` L136).
    pub match_status: Option<Alnum<1>>,
    /// `AUTH_FRAUD CHAR(1)` (P-AUTHFRDS-023; DDL L24, host L83,
    /// `MOVE WS-FRD-ACTION` L137): the action `COPAUS1C` requested, `F` to
    /// report fraud or `R` to remove the report (L80-82).
    pub auth_fraud: Option<Alnum<1>>,
    /// `FRAUD_RPT_DATE DATE` (P-AUTHFRDS-024; DDL L25, host `X(10)` L84): Db2
    /// `CURRENT DATE`, on the insert (L194) and on the `FRAUD-UPDATE` update
    /// (L225) (P-COPAUS2C-002, P-COPAUS2C-004), which the replacement takes
    /// from the processing clock. It is not the IMS detail's
    /// `PA-FRAUD-RPT-DATE`, the `MM/DD/YY` text that `COPAUS2C` builds from
    /// `FORMATTIME` (L91-101, P-COPAUS2C-006).
    //
    // A date, not the `Alnum<10>` of the section A row, for the reason given
    // at `auth_ts`: 10 is the length of the `DATE` string form, the column is
    // `DATE` in the row and in migration 0004, and no program reads it back
    // as text.
    pub fraud_rpt_date: Option<NaiveDate>,
    /// `ACCT_ID DECIMAL(11)` (P-AUTHFRDS-025; DDL L26, host `S9(11)V COMP-3`
    /// L85, `MOVE WS-ACCT-ID` L138), moved from the unsigned commarea field
    /// `WS-ACCT-ID 9(11)` (L75).
    pub acct_id: Option<AccountId>,
    /// `CUST_ID DECIMAL(9)` (P-AUTHFRDS-026; DDL L27, host `S9(9)V COMP-3`
    /// L86, `MOVE WS-CUST-ID` L139), moved from the unsigned commarea field
    /// `WS-CUST-ID 9(9)` (L76).
    pub cust_id: Option<CustomerId>,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use chrono::{NaiveDate, NaiveDateTime, TimeDelta};

    use super::AuthFraud;
    use crate::numeric::{AccountId, Alnum, CardNumber, CustomerId, Money, NumericCode};

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    /// 2022-07-18 12.34.56.789000, as `COPAUS2C` builds it from the original
    /// date `220718` and the complemented time `999999999 - 123456789`.
    fn auth_ts() -> TestResult<NaiveDateTime> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_micro_opt(12, 34, 56, 789_000))
            .ok_or_else(|| "2022-07-18 12:34:56.789000 is a valid timestamp".into())
    }

    /// A row holding only the two `NOT NULL` key columns.
    fn key_only_row() -> TestResult<AuthFraud> {
        Ok(AuthFraud {
            card_num: CardNumber::move_from("4859452612877065"),
            auth_ts: auth_ts()?,
            auth_type: None,
            card_expiry_date: None,
            message_type: None,
            message_source: None,
            auth_id_code: None,
            auth_resp_code: None,
            auth_resp_reason: None,
            processing_code: None,
            transaction_amt: None,
            approved_amt: None,
            merchant_catagory_code: None,
            acqr_country_code: None,
            pos_entry_mode: None,
            merchant_id: None,
            merchant_name: None,
            merchant_city: None,
            merchant_state: None,
            merchant_zip: None,
            transaction_id: None,
            match_status: None,
            auth_fraud: None,
            fraud_rpt_date: None,
            acct_id: None,
            cust_id: None,
        })
    }

    /// The same key with every column present, as `COPAUS2C` inserts it.
    fn populated_row() -> TestResult<AuthFraud> {
        Ok(AuthFraud {
            auth_type: Some(Alnum::move_from("0100")),
            card_expiry_date: Some(Alnum::move_from("0925")),
            message_type: Some(Alnum::move_from("0100")),
            message_source: Some(Alnum::move_from("POS")),
            auth_id_code: Some(Alnum::move_from("A12345")),
            auth_resp_code: Some(Alnum::move_from("00")),
            auth_resp_reason: Some(Alnum::move_from("0000")),
            processing_code: Some(Alnum::move_from("000000")),
            transaction_amt: Some(Money::from_cents(12_345)?),
            approved_amt: Some(Money::from_cents(12_345)?),
            merchant_catagory_code: Some(Alnum::move_from("5411")),
            acqr_country_code: Some(Alnum::move_from("840")),
            pos_entry_mode: Some(NumericCode::new(5)?),
            merchant_id: Some(Alnum::move_from("000000000012345")),
            merchant_name: Some(Alnum::move_from("CORNER GROCERY")),
            merchant_city: Some(Alnum::move_from("SEATTLE")),
            merchant_state: Some(Alnum::move_from("WA")),
            merchant_zip: Some(Alnum::move_from("98101")),
            transaction_id: Some(Alnum::move_from("000000000000001")),
            match_status: Some(Alnum::move_from("P")),
            auth_fraud: Some(Alnum::move_from("F")),
            fraud_rpt_date: NaiveDate::from_ymd_opt(2022, 7, 19),
            acct_id: Some(AccountId::new(1)?),
            cust_id: Some(CustomerId::new(1)?),
            ..key_only_row()?
        })
    }

    #[test]
    fn rows_equal_their_clones_and_differ_by_their_columns() -> TestResult {
        let key_only = key_only_row()?;
        let populated = populated_row()?;
        assert!(populated.fraud_rpt_date.is_some());
        assert_eq!(key_only.clone(), key_only);
        assert_eq!(populated.clone(), populated);
        assert_eq!(key_only_row()?, key_only);
        assert_eq!(populated_row()?, populated);
        assert_ne!(key_only, populated);
        Ok(())
    }

    #[test]
    fn a_different_auth_ts_is_a_different_row() -> TestResult {
        for row in [key_only_row()?, populated_row()?] {
            for step in [TimeDelta::microseconds(1), TimeDelta::milliseconds(-1)] {
                let moved = AuthFraud {
                    auth_ts: row
                        .auth_ts
                        .checked_add_signed(step)
                        .ok_or("the timestamp moves by one unit")?,
                    ..row.clone()
                };
                assert_ne!(moved, row, "{step}");
            }
        }
        Ok(())
    }

    #[test]
    fn debug_masks_the_card_number() -> TestResult {
        let debug = format!("{:?}", populated_row()?);
        assert!(debug.contains("************7065"), "{debug}");
        assert!(!debug.contains("4859452612877065"), "{debug}");
        Ok(())
    }
}
