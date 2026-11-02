//! `COACTVWC` (CAVW), the account view: `GET /accounts/{id}`.
//!
//! The 3270 dialogue receives the account number (`2100-RECEIVE-MAP`), edits
//! it (`2200-EDIT-MAP-INPUTS`, `2210-EDIT-ACCOUNT`) and, when it passes, reads
//! the card cross-reference of the account through the `CXACAIX` path, the
//! account and the customer of the cross-reference (`9000-READ-ACCT`)
//! [app/cbl/COACTVWC.cbl:L596-871]. One request performs that turn: the
//! account number is the `{id}` path segment (P-COACTVWC-020), and [`view`]
//! runs [`edit_account_id`] and the reads in that order. Nothing is written.
//!
//! # Outcomes
//!
//! | Outcome | Kind | BMS field | Rows |
//! |---|---|---|---|
//! | Key blank, spaces, `*` or `LOW-VALUES` | `Empty` | `ACCTSID` | V-COACTVWC-001 |
//! | Key not 11 digits, or all zeros | `Invalid` | `ACCTSID` | V-COACTVWC-002 |
//! | Cross-reference not found; not read | `NotFound`, `Unexpected` | `ACCTSID` | V-COACTVWC-003, V-COACTVWC-004 |
//! | Account not found; not read | `NotFound`, `Unexpected` | `ACCTSID` | V-COACTVWC-005, V-COACTVWC-006 |
//! | Customer not found; not read | `NotFound`, `Unexpected` | `ACCTSID` | V-COACTVWC-007, V-COACTVWC-008 |
//! | All three read | none: [`AccountView`], 200 | — | P-COACTVWC-006, P-COACTVWC-011 |
//!
//! Any key error skips the reads (P-COACTVWC-019). A not-found text is moved
//! only while `WS-RETURN-MSG` is blank, while each read's `WHEN OTHER` moves
//! the file-error text unconditionally (P-COACTVWC-006). A cross-reference
//! failure ends the reads; the customer is read even after an account failure,
//! because the `DID-NOT-FIND-ACCT-IN-ACCTDAT` test after the account read
//! never matches (P-COACTVWC-030, E-45). The texts carry the CICS `RESP` and
//! `RESP2` codes E-44 gives: 13 and 80 for `NOTFND`, 17 and 0 for any other
//! failure.
//!
//! A successful read shows no success text: the information line keeps the
//! input prompt (P-COACTVWC-016, P-COACTVWC-025, E-67), so [`AccountView`]
//! carries none. The screen mechanics of `1000-SEND-MAP` (P-COACTVWC-013 to
//! P-COACTVWC-018) and `COMMON-RETURN` (P-COACTVWC-012) become the HTTP
//! response, whose body holds the account and customer records
//! (P-COACTVWC-011). PF3, the key remapping, `SEND-PLAIN-TEXT`,
//! `SEND-LONG-TEXT` and `ABEND-ROUTINE` have no REST counterpart, and the
//! branches no path reaches are not carried (P-COACTVWC-001, P-COACTVWC-002,
//! P-COACTVWC-022 to P-COACTVWC-024, P-COACTVWC-026 to P-COACTVWC-029).

use super::account_update::record_version;
use super::{VersionKey, VersionToken};
use crate::entities::{Account, CardXref, Customer};
use crate::error::DomainError;
use crate::messages::{
    ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT, NO_INPUT_RECEIVED, account_master_not_found_message,
    customer_master_not_found_message, file_error_message, xref_not_found_message,
};
use crate::numeric::{AccountId, Alnum};
use crate::ports::{AccountReader, CustomerReader, PortError, XrefReader};

/// `ACCTSID`, the account number of the `COACTVW` map
/// [app/cpy-bms/COACTVW.CPY:L55-60]: every outcome places the cursor here
/// [app/cbl/COACTVWC.cbl:L546-552] (P-COACTVWC-017).
const FIELD_ACCTSID: &str = "ACCTSID";

/// `CC-ACCT-ID PIC X(11)`, the receiver of `ACCTSIDI`
/// [app/cpy/CVCRD01Y.cpy:L34-36].
const ACCOUNT_ID_LEN: usize = 11;

/// `ERROR-OPNAME` of a failed read [app/cbl/COACTVWC.cbl:L762].
const ERROR_OPNAME_READ: &str = "READ";

/// `LIT-CARDXREFNAME-ACCT-PATH`, the `CXACAIX` path of the cross-reference
/// [app/cbl/COACTVWC.cbl:L192-193].
const LIT_CARDXREFNAME_ACCT_PATH: &str = "CXACAIX ";

/// `LIT-ACCTFILENAME` [app/cbl/COACTVWC.cbl:L184-185].
const LIT_ACCTFILENAME: &str = "ACCTDAT ";

/// `LIT-CUSTFILENAME` [app/cbl/COACTVWC.cbl:L188-189].
const LIT_CUSTFILENAME: &str = "CUSTDAT ";

/// The CICS `RESP` of `NOTFND` on `READ`, as E-44 records it.
const RESP_NOTFND: u32 = 13;

/// The `RESP2` of `NOTFND` on `READ` (E-44).
const RESP2_NOTFND: u32 = 80;

/// The CICS `RESP` E-44 proposes for a database failure, which has no CICS
/// code: `IOERR`.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 proposes alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// One `GET /accounts/{id}` request: the `COACTVW` map input as entered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountViewInput {
    /// `ACCTSIDI`, the `{id}` path segment exactly as received; `None` is a
    /// key not entered.
    pub account_id: Option<String>,
}

/// The account a successful request shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountView {
    /// The `ACCTDAT` record of the account (P-COACTVWC-014).
    pub account: Account,
    /// The `CUSTDAT` record of the cross-reference's customer
    /// (P-COACTVWC-015).
    pub customer: Customer,
    /// The `CXACAIX` record the customer was found through: the account's
    /// first card in card-number order (P-COACTVWC-007).
    pub card_xref: CardXref,
    /// The token `PUT /accounts/{id}` echoes back:
    /// [`record_version`] of [`AccountView::account`] and
    /// [`AccountView::customer`] under the server's [`VersionKey`].
    pub version: VersionToken,
}

/// COACTVWC 2200-EDIT-MAP-INPUTS, 2210-EDIT-ACCOUNT — P-COACTVWC-004,
/// P-COACTVWC-005, P-COACTVWC-021, V-COACTVWC-001, V-COACTVWC-002
///
/// The account number `raw`, received into `CC-ACCT-ID X(11)` and edited
/// [app/cbl/COACTVWC.cbl:L622-685]. `'*'` or `SPACES` is moved as
/// `LOW-VALUES`, and `LOW-VALUES` or `SPACES` is a key not provided; any
/// other entry must pass `IS NUMERIC` over all 11 bytes and not equal
/// `ZEROES`. The `MOVE` pads a shorter entry with spaces, so it is not
/// numeric, as are embedded and trailing spaces.
///
/// An entry longer than 11 bytes has no 3270 counterpart, the `ACCTSID` field
/// holding 11 characters [app/bms/COACTVW.bms:L84-90]. It is rejected as not
/// 11 digits rather than cut, so a path segment naming a longer number never
/// reads the account its first 11 characters name.
///
/// # Errors
///
/// - [`ErrorKind::Empty`](crate::error::ErrorKind::Empty) with
///   [`NO_INPUT_RECEIVED`] at `ACCTSID` when the key is absent, empty, spaces,
///   `*` or `LOW-VALUES`: `2210` sets `Account number not provided` and the
///   cross edit of `2200` replaces it unconditionally (E-45).
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with
///   [`ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT`] at `ACCTSID` when it is not
///   11 digits or is all zeros.
pub fn edit_account_id(raw: Option<&str>) -> Result<AccountId, DomainError> {
    let not_valid =
        || DomainError::invalid(ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT).with_field(FIELD_ACCTSID);
    let text = raw.unwrap_or_default();
    if text.len() > ACCOUNT_ID_LEN {
        return Err(not_valid());
    }
    let cc_acct_id = Alnum::<ACCOUNT_ID_LEN>::move_from(text);
    if cc_acct_id.cobol_cmp(b"*").is_eq() || cc_acct_id.is_blank() {
        return Err(DomainError::empty(NO_INPUT_RECEIVED).with_field(FIELD_ACCTSID));
    }
    let zeroes = cc_acct_id.as_bytes().iter().all(|&byte| byte == b'0');
    if zeroes || !cc_acct_id.is_numeric() {
        return Err(not_valid());
    }
    // Eleven digits always fit `9(11)`; the mapping only keeps the conversion
    // free of a panic.
    cc_acct_id.as_text().parse().map_err(|_| not_valid())
}

/// COACTVWC 9200-GETCARDXREF-BYACCT, 9300-GETACCTDATA-BYACCT,
/// 9400-GETCUSTDATA-BYCUST `WHEN OTHER` — P-COACTVWC-006, P-COACTVWC-008,
/// V-COACTVWC-004, V-COACTVWC-006, V-COACTVWC-008
///
/// `WS-FILE-ERROR-MESSAGE` of a failed `READ` of `file`
/// [app/cbl/COACTVWC.cbl:L86-105], with the `RESP` and `RESP2` E-44 gives a
/// database failure.
fn read_failure(file: &str) -> DomainError {
    DomainError::unexpected(file_error_message(
        ERROR_OPNAME_READ,
        file,
        RESP_IOERR,
        RESP2_IOERR,
    ))
    .with_field(FIELD_ACCTSID)
}

/// COACTVWC 9000-READ-ACCT, 9200-GETCARDXREF-BYACCT, 9300-GETACCTDATA-BYACCT,
/// 9400-GETCUSTDATA-BYCUST — P-COACTVWC-006, P-COACTVWC-007, P-COACTVWC-009,
/// P-COACTVWC-010, P-COACTVWC-014, P-COACTVWC-030, V-COACTVWC-003 to
/// V-COACTVWC-008
///
/// The reads of a valid key [app/cbl/COACTVWC.cbl:L687-871], each a CICS
/// `READ` without a lock: the cross-reference by `account_id` through the
/// `CXACAIX` path, then `ACCTDAT` by the same key (`WS-CARD-RID-ACCT-ID-X`,
/// L778), then `CUSTDAT` by the cross-reference's `XREF-CUST-ID` (L739, L708).
/// The records found are shown with their token computed under `key`.
///
/// # Errors
///
/// All at `ACCTSID`, with the text the reads leave in `WS-RETURN-MSG`:
///
/// - the cross-reference: [`xref_not_found_message`] on `NOTFND`, the
///   `CXACAIX` [`file_error_message`] otherwise; either ends the reads;
/// - a customer read failure other than `NOTFND`: the `CUSTDAT` file-error
///   text, which replaces any account text;
/// - else an account failure: [`account_master_not_found_message`] on
///   `NOTFND`, the `ACCTDAT` file-error text otherwise. A found customer does
///   not make the account shown (P-COACTVWC-014);
/// - else the customer's [`customer_master_not_found_message`];
/// - with both records found, the token's error, as [`record_version`]
///   documents it.
async fn read_acct<S: XrefReader + AccountReader + CustomerReader>(
    store: &mut S,
    key: &VersionKey,
    account_id: AccountId,
) -> Result<AccountView, DomainError> {
    let account_text = account_id.to_string();
    let card_xref = match store.read_xref_by_account(account_id).await {
        Ok(card_xref) => card_xref,
        Err(PortError::NotFound) => {
            let text = xref_not_found_message(&account_text, RESP_NOTFND, RESP2_NOTFND);
            return Err(DomainError::not_found(text).with_field(FIELD_ACCTSID));
        }
        Err(
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_),
        ) => return Err(read_failure(LIT_CARDXREFNAME_ACCT_PATH)),
    };
    let account = store.read_account(account_id).await;
    let customer_id = card_xref.xref_cust_id;
    let customer = store.read_customer(customer_id).await;
    match (account, customer) {
        (Ok(account), Ok(customer)) => {
            let version = record_version(key, &account, &customer)?;
            Ok(AccountView {
                account,
                customer,
                card_xref,
                version,
            })
        }
        (_, Err(error)) if error != PortError::NotFound => Err(read_failure(LIT_CUSTFILENAME)),
        (Err(PortError::NotFound), _) => {
            let text = account_master_not_found_message(&account_text, RESP_NOTFND, RESP2_NOTFND);
            Err(DomainError::not_found(text).with_field(FIELD_ACCTSID))
        }
        (Err(_), _) => Err(read_failure(LIT_ACCTFILENAME)),
        (Ok(_), Err(_)) => {
            let text = customer_master_not_found_message(
                &customer_id.to_string(),
                RESP_NOTFND,
                RESP2_NOTFND,
            );
            Err(DomainError::not_found(text).with_field(FIELD_ACCTSID))
        }
    }
}

/// COACTVWC 2000-PROCESS-INPUTS, 9000-READ-ACCT — P-COACTVWC-004 to
/// P-COACTVWC-011, P-COACTVWC-019 to P-COACTVWC-021, P-COACTVWC-030,
/// V-COACTVWC-001 to V-COACTVWC-008
///
/// The whole `GET /accounts/{id}`: [`edit_account_id`], then, when the key
/// passes, the cross-reference, account and customer reads. The records found
/// are shown with their [`VersionToken`], computed under `key`. Nothing is
/// written; the caller owns the read-only unit of work.
///
/// # Errors
///
/// The error of the first step that fails: the key edit (V-COACTVWC-001,
/// V-COACTVWC-002, no read), then the reads (V-COACTVWC-003 to
/// V-COACTVWC-008), each a [`DomainError`] at `ACCTSID` as
/// [`edit_account_id`] and the module's outcome table describe, then the
/// token, as [`record_version`] documents it.
pub async fn view<S: XrefReader + AccountReader + CustomerReader>(
    store: &mut S,
    key: &VersionKey,
    input: AccountViewInput,
) -> Result<AccountView, DomainError> {
    let AccountViewInput { account_id } = input;
    let account_id = edit_account_id(account_id.as_deref())?;
    read_acct(store, key, account_id).await
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::future::Future;

    use super::{AccountView, AccountViewInput, edit_account_id, view};
    use crate::entities::{Account, CardXref, Customer};
    use crate::error::{DomainError, ErrorKind};
    use crate::messages;
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustSsn, CustomerId, FicoScore, Money,
    };
    use crate::ports::PortError;
    use crate::rules::VersionToken;
    use crate::rules::account_update::record_version;
    use crate::rules::test_support::{FakeStore, block_on, version_key};

    /// `ACCTSID`, as the validation audit names it.
    const ACCTSID: &str = "ACCTSID";

    /// The fixture account (P-CVACT01Y-015) [app/data/ASCII/acctdata.txt:L1].
    const ACCOUNT: &str = "00000000001";

    /// An account no fixture record has (P-CVACT01Y-016).
    const MISSING_ACCOUNT: &str = "99999999999";

    /// The card of account 00000000001 and customer 000000001
    /// [app/data/ASCII/cardxref.txt:L49].
    const CARD: &str = "9680294154603697";

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn customer_id(value: u32) -> CustomerId {
        CustomerId::new(value).expect("a customer number of at most 9 digits")
    }

    fn cents(value: i64) -> Money<10> {
        Money::from_cents(value).expect("an amount of at most 10 integer digits")
    }

    /// Account 00000000001 as `acctdata.txt` L1 decodes (P-CVACT01Y-015).
    fn fixture_account() -> Account {
        Account {
            acct_id: account_id(1),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(19_400),
            acct_credit_limit: cents(202_000),
            acct_cash_credit_limit: cents(102_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    /// Customer 000000001 as `custdata.txt` L1 decodes (P-CVCUS01Y-021).
    fn fixture_customer() -> Customer {
        Customer {
            cust_id: customer_id(1),
            cust_first_name: Alnum::move_from("Immanuel"),
            cust_middle_name: Alnum::move_from("Madeline"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    /// The cross-reference of [`CARD`] (P-CVACT03Y-006).
    fn fixture_xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(CARD),
            xref_cust_id: customer_id(1),
            xref_acct_id: account_id(1),
        }
    }

    /// The seeded state of account 00000000001: its cross-reference, account
    /// and customer.
    fn fixture_store() -> FakeStore {
        FakeStore::default()
            .with_xref(fixture_xref())
            .with_account(fixture_account())
            .with_customer(fixture_customer())
    }

    fn input(account_id: Option<&str>) -> AccountViewInput {
        AccountViewInput {
            account_id: account_id.map(str::to_owned),
        }
    }

    /// The token `PUT /accounts/{id}` recomputes for `account` and `customer`
    /// under the tests' server key.
    fn version_of(account: &Account, customer: &Customer) -> VersionToken {
        record_version(&version_key(), account, customer).expect("the records' token")
    }

    /// Asserts that `result` is the rejection `kind`, `message` at `ACCTSID`.
    fn assert_rejected<T: Debug>(
        result: Result<T, DomainError>,
        (kind, message): (ErrorKind, &str),
        case: &str,
    ) {
        match result {
            Err(error) => {
                assert_eq!(error.kind, kind, "{case}");
                assert_eq!(error.code, kind.code(), "{case}");
                assert_eq!(error.message, message, "{case}");
                assert_eq!(error.first_field(), Some(ACCTSID), "{case}");
                assert_eq!(error.fields.len(), 1, "{case}");
                assert_eq!(error.fields[0].message, message, "{case}");
            }
            Ok(value) => panic!("{case}: expected a rejection, got {value:?}"),
        }
    }

    /// The three reads of a valid key, in `9000-READ-ACCT` order.
    const ALL_READS: [&str; 3] = ["read_xref_by_account", "read_account", "read_customer"];

    #[test]
    fn the_fixture_account_is_shown_with_its_customer_and_token() {
        // P-COACTVWC-006, P-COACTVWC-007, P-COACTVWC-011, P-COACTVWC-014,
        // P-COACTVWC-015.
        let mut store = fixture_store();
        let shown = block_on(view(&mut store, &version_key(), input(Some(ACCOUNT))))
            .expect("account 00000000001 is on file");
        let expected = AccountView {
            account: fixture_account(),
            customer: fixture_customer(),
            card_xref: fixture_xref(),
            version: version_of(&fixture_account(), &fixture_customer()),
        };
        assert_eq!(shown, expected);
        assert_eq!(store.calls, ALL_READS);
    }

    #[test]
    fn the_token_is_the_account_update_token_of_the_rows_read() {
        // P-COACTUPC-012: the GET token is the one PUT /accounts/{id}
        // recomputes from the rows it locks.
        let changed = Account {
            acct_curr_bal: cents(19_401),
            ..fixture_account()
        };
        let mut store = fixture_store().with_account(changed.clone());
        let shown =
            block_on(view(&mut store, &version_key(), input(Some(ACCOUNT)))).expect("the account");
        assert_eq!(shown.version, version_of(&changed, &fixture_customer()));
        assert_ne!(
            shown.version,
            version_of(&fixture_account(), &fixture_customer())
        );
        assert_eq!(
            VersionToken::parse(shown.version.as_str()),
            Some(shown.version.clone()),
            "16 lower-case hex digits"
        );
    }

    #[test]
    fn key_edits_reject_before_any_read() {
        // P-COACTVWC-004, P-COACTVWC-005, P-COACTVWC-019, P-COACTVWC-021,
        // V-COACTVWC-001, V-COACTVWC-002.
        let empty = (ErrorKind::Empty, messages::NO_INPUT_RECEIVED);
        let invalid = (
            ErrorKind::Invalid,
            messages::ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT,
        );
        let low_values = "\0".repeat(11);
        let cases = [
            // V-COACTVWC-001: absent, empty, spaces, '*' and LOW-VALUES; the
            // cross edit replaces messages::ACCOUNT_NUMBER_NOT_PROVIDED.
            (None, empty),
            (Some(""), empty),
            (Some(" "), empty),
            (Some("           "), empty),
            (Some("*"), empty),
            (Some("*          "), empty),
            (Some(low_values.as_str()), empty),
            // V-COACTVWC-002: not numeric over the 11 bytes, all zeros, short
            // (space-padded by the MOVE), embedded, leading and trailing
            // spaces, signed, a mix of LOW-VALUES and spaces, and over-long.
            (Some("abc"), invalid),
            (Some("0000000000A"), invalid),
            (Some("00000000000"), invalid),
            (Some("0"), invalid),
            (Some("123"), invalid),
            (Some("00000 00001"), invalid),
            (Some(" 0000000001"), invalid),
            (Some("0000000001 "), invalid),
            (Some("-0000000001"), invalid),
            (Some("**"), invalid),
            (Some("\0\0"), invalid),
            (Some("000000000012"), invalid),
            (Some("00000000001 "), invalid),
        ];
        for (raw, expected) in cases {
            let case = format!("{raw:?}");
            assert_rejected(edit_account_id(raw), expected, &case);

            let mut store = fixture_store();
            assert_rejected(
                block_on(view(&mut store, &version_key(), input(raw))),
                expected,
                &case,
            );
            assert!(
                store.calls.is_empty(),
                "{case}: a key error skips the reads"
            );
        }
    }

    #[test]
    fn eleven_digits_not_all_zero_pass_the_edit() {
        // P-COACTVWC-005: MOVE CC-ACCT-ID TO CDEMO-ACCT-ID.
        for (raw, value) in [
            (ACCOUNT, 1),
            (MISSING_ACCOUNT, 99_999_999_999),
            ("00000000050", 50),
        ] {
            assert_eq!(edit_account_id(Some(raw)), Ok(account_id(value)), "{raw}");
        }
    }

    #[test]
    fn an_account_without_a_cross_reference_is_not_found() {
        // V-COACTVWC-003, P-COACTVWC-007, E-44: the other reads are skipped.
        let mut store = fixture_store();
        let result = block_on(view(
            &mut store,
            &version_key(),
            input(Some(MISSING_ACCOUNT)),
        ));
        let text = messages::xref_not_found_message(MISSING_ACCOUNT, 13, 80);
        assert_rejected(result, (ErrorKind::NotFound, &text), MISSING_ACCOUNT);
        assert_eq!(store.calls, ["read_xref_by_account"]);
    }

    #[test]
    fn a_failed_cross_reference_read_ends_the_reads() {
        // V-COACTVWC-004, P-COACTVWC-008, E-44.
        let text = messages::file_error_message("READ", "CXACAIX ", 17, 0);
        let failures = [
            PortError::Unavailable("connection reset".to_owned()),
            PortError::Contention,
            PortError::Duplicate,
            PortError::Restricted,
        ];
        for failure in failures {
            let case = format!("{failure:?}");
            let mut store = fixture_store();
            store.fail_next("read_xref_by_account", failure);
            let result = block_on(view(&mut store, &version_key(), input(Some(ACCOUNT))));
            assert_rejected(result, (ErrorKind::Unexpected, &text), &case);
            assert_eq!(store.calls, ["read_xref_by_account"], "{case}");
        }
    }

    #[test]
    fn the_customer_is_read_after_any_account_outcome_and_its_failure_wins() {
        // V-COACTVWC-005 to V-COACTVWC-008, P-COACTVWC-006, P-COACTVWC-009,
        // P-COACTVWC-010, P-COACTVWC-014, P-COACTVWC-030, E-45.
        let account_text = messages::account_master_not_found_message(ACCOUNT, 13, 80);
        let customer_text = messages::customer_master_not_found_message("000000001", 13, 80);
        let account_error = messages::file_error_message("READ", "ACCTDAT ", 17, 0);
        let customer_error = messages::file_error_message("READ", "CUSTDAT ", 17, 0);
        let unavailable = || PortError::Unavailable("database".to_owned());
        let cases = [
            // V-COACTVWC-005: a found customer does not show the account.
            (
                Some(PortError::NotFound),
                None,
                ErrorKind::NotFound,
                account_text.as_str(),
            ),
            // V-COACTVWC-005: the guarded customer text keeps the account's.
            (
                Some(PortError::NotFound),
                Some(PortError::NotFound),
                ErrorKind::NotFound,
                account_text.as_str(),
            ),
            // V-COACTVWC-006.
            (
                Some(unavailable()),
                None,
                ErrorKind::Unexpected,
                account_error.as_str(),
            ),
            (
                Some(PortError::Contention),
                Some(PortError::NotFound),
                ErrorKind::Unexpected,
                account_error.as_str(),
            ),
            // V-COACTVWC-007.
            (
                None,
                Some(PortError::NotFound),
                ErrorKind::NotFound,
                customer_text.as_str(),
            ),
            // V-COACTVWC-008: the unguarded move replaces any account text.
            (
                None,
                Some(unavailable()),
                ErrorKind::Unexpected,
                customer_error.as_str(),
            ),
            (
                Some(PortError::NotFound),
                Some(unavailable()),
                ErrorKind::Unexpected,
                customer_error.as_str(),
            ),
            (
                Some(unavailable()),
                Some(PortError::Contention),
                ErrorKind::Unexpected,
                customer_error.as_str(),
            ),
        ];
        for (index, (account, customer, kind, message)) in cases.into_iter().enumerate() {
            let case = format!("case {index}");
            let mut store = fixture_store();
            if let Some(error) = account {
                store.fail_next("read_account", error);
            }
            if let Some(error) = customer {
                store.fail_next("read_customer", error);
            }
            let result = block_on(view(&mut store, &version_key(), input(Some(ACCOUNT))));
            assert_rejected(result, (kind, message), &case);
            assert_eq!(store.calls, ALL_READS, "{case}");
        }
    }

    #[test]
    fn an_account_missing_from_the_master_file_is_not_found() {
        // V-COACTVWC-005: the cross-reference exists, the account does not.
        let mut store = FakeStore::default()
            .with_xref(fixture_xref())
            .with_customer(fixture_customer());
        let result = block_on(view(&mut store, &version_key(), input(Some(ACCOUNT))));
        let text = messages::account_master_not_found_message(ACCOUNT, 13, 80);
        assert_rejected(result, (ErrorKind::NotFound, &text), "no account");
        assert_eq!(store.calls, ALL_READS);
    }

    #[test]
    fn the_customer_is_the_cross_reference_customer() {
        // P-COACTVWC-006, P-COACTVWC-010: CUSTDAT is read by XREF-CUST-ID.
        let xref = CardXref {
            xref_cust_id: customer_id(7),
            ..fixture_xref()
        };
        let customer = Customer {
            cust_id: customer_id(7),
            ..fixture_customer()
        };
        let mut store = FakeStore::default()
            .with_xref(xref.clone())
            .with_account(fixture_account())
            .with_customer(fixture_customer())
            .with_customer(customer.clone());
        let shown =
            block_on(view(&mut store, &version_key(), input(Some(ACCOUNT)))).expect("the account");
        assert_eq!(shown.customer, customer);
        assert_eq!(shown.card_xref, xref);

        let mut store = FakeStore::default()
            .with_xref(xref)
            .with_account(fixture_account())
            .with_customer(fixture_customer());
        let result = block_on(view(&mut store, &version_key(), input(Some(ACCOUNT))));
        let text = messages::customer_master_not_found_message("000000007", 13, 80);
        assert_rejected(result, (ErrorKind::NotFound, &text), "customer 7 missing");
    }

    #[test]
    fn the_view_future_is_send() {
        // Adapters run the rule on a multi-threaded runtime.
        fn assert_send<F: Future + Send>(future: F) -> F {
            future
        }
        let mut store = fixture_store();
        let key = version_key();
        let future = assert_send(view(&mut store, &key, input(Some(ACCOUNT))));
        assert!(block_on(future).is_ok());
    }
}
