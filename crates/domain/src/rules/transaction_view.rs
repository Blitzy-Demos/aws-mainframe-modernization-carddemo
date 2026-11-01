//! `COTRN01C` (CT01): view one transaction of the live transaction master
//! [app/cbl/COTRN01C.cbl], served as `GET /transactions/{id}`.
//!
//! The program edits the Tran ID the user entered in `TRNIDIN`, reads that
//! record from `TRANSACT` and shows its fields. This module ports the two
//! paragraphs that decide the result:
//!
//! - `PROCESS-ENTER-KEY` [app/cbl/COTRN01C.cbl:L144-192]: a blank Tran ID is
//!   rejected before any file access (P-COTRN01C-003, V-COTRN01C-001);
//!   otherwise the entered text becomes the key and, once the read succeeds,
//!   every `CVTRA05Y` field is shown (P-COTRN01C-005).
//! - `READ-TRANSACT-FILE` [app/cbl/COTRN01C.cbl:L267-296]: the keyed read and
//!   its two failure messages (P-COTRN01C-004, V-COTRN01C-002,
//!   V-COTRN01C-003).
//!
//! The read sees the live master only: after `batch tranbkp` every TRAN-ID is
//! not found (E-09), which the [`TransactionReader`] adapter contract
//! guarantees. Every failure sets `WS-ERR-FLG`, which stops the later steps,
//! and places the cursor on `TRNIDIN` (`MOVE -1 TO TRNIDINL`), so each error
//! names that field.
//!
//! The screen mechanics have no counterpart here: the 3270 header
//! (P-COTRN01C-009), the PF3/PF5 transfers (P-COTRN01C-006) become SPA
//! navigation, and PF4 Clear (P-COTRN01C-010, P-COTRN01C-011) has no control
//! (E-62). The amount the screen shows edited as `+99999999.99` is rendered
//! by the response mapping with [`crate::format::PIC_TRAN_AMT_DISPLAY`]; the
//! rule returns the stored record unchanged.

use crate::entities::Transaction;
use crate::error::DomainError;
use crate::messages::{
    TRAN_ID_CAN_NOT_BE_EMPTY, TRANSACTION_ID_NOT_FOUND, UNABLE_TO_LOOKUP_TRANSACTION,
};
use crate::numeric::TranId;
use crate::ports::{PortError, TransactionReader};

/// `TRNIDIN`, the Tran ID input field of map `COTRN1A`
/// [app/cpy-bms/COTRN01.CPY:L55-60]: the BMS field of every `COTRN01C`
/// message (V-COTRN01C-001 to V-COTRN01C-003).
pub const TRAN_ID_FIELD: &str = "TRNIDIN";

/// The request of `GET /transactions/{id}`: what the user enters in `TRNIDIN`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransactionViewInput {
    /// The Tran ID exactly as entered, `None` when nothing was sent. It is
    /// moved into the `PIC X(16)` field `TRAN-ID` unchanged: no numeric test,
    /// no case change and no zero padding.
    pub tran_id: Option<String>,
}

/// COTRN01C PROCESS-ENTER-KEY — P-COTRN01C-003, V-COTRN01C-001
/// [app/cbl/COTRN01C.cbl:L146-156, L172]: the edit of `TRNIDINI X(16)`, then
/// `MOVE TRNIDINI OF COTRN1AI TO TRAN-ID`.
///
/// The text is moved into the 16-byte field as a COBOL alphanumeric `MOVE`
/// does: padded with spaces, or cut on the right when longer. The map field
/// holds 16 characters, so the COBOL never receives a longer key. The field is
/// blank when it holds `SPACES` or `LOW-VALUES` after the move. A text made of
/// NUL characters only stands for `LOW-VALUES` whatever its length, as for the
/// confirmation fields ([`super::Confirm::parse`]); a mix of spaces and NULs
/// matches neither figurative constant and is read as a key.
///
/// # Errors
///
/// [`ErrorKind::Empty`](crate::error::ErrorKind::Empty) with
/// `Tran ID can NOT be empty...` on [`TRAN_ID_FIELD`] when the field is blank.
fn edit_tran_id(raw: Option<&str>) -> Result<TranId, DomainError> {
    let text = raw.unwrap_or_default();
    let low_values = !text.is_empty() && text.bytes().all(|byte| byte == 0);
    let tran_id = TranId::move_from(text);
    if low_values || tran_id.as_alnum().is_blank() {
        return Err(DomainError::empty(TRAN_ID_CAN_NOT_BE_EMPTY).with_field(TRAN_ID_FIELD));
    }
    Ok(tran_id)
}

/// COTRN01C READ-TRANSACT-FILE — P-COTRN01C-004, V-COTRN01C-002,
/// V-COTRN01C-003 [app/cbl/COTRN01C.cbl:L267-296]: the CICS `READ` of
/// `TRANSACT` by `TRAN-ID`.
///
/// The COBOL reads with `UPDATE` and rewrites nothing, so the record lock it
/// holds until the task ends changes no result, and the read here is a plain
/// one. The `DISPLAY` of `RESP` and `RESP2` on the `WHEN OTHER` branch is a
/// console log: the adapter logs the failure detail, which never reaches the
/// response.
///
/// # Errors
///
/// - [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) with
///   `Transaction ID NOT found...` when no live transaction has the key
///   (`RESP NOTFND`).
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///   `Unable to lookup Transaction...` on any other read failure.
///
/// Both name [`TRAN_ID_FIELD`].
async fn read_transact_file<S: TransactionReader>(
    store: &mut S,
    tran_id: TranId,
) -> Result<Transaction, DomainError> {
    match store.read_transaction(tran_id).await {
        Ok(tran) => Ok(tran),
        Err(PortError::NotFound) => {
            Err(DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(TRAN_ID_FIELD))
        }
        Err(
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_),
        ) => Err(DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(TRAN_ID_FIELD)),
    }
}

/// COTRN01C PROCESS-ENTER-KEY — P-COTRN01C-003, P-COTRN01C-004,
/// P-COTRN01C-005, V-COTRN01C-001, V-COTRN01C-002, V-COTRN01C-003
/// [app/cbl/COTRN01C.cbl:L144-192]: the transaction `input` names, as the view
/// shows it.
///
/// The Tran ID edit comes first and a blank one stops before any file access;
/// otherwise the record is read by its key and returned with every
/// `CVTRA05Y` field as stored, the timestamps as their 26 characters of text.
///
/// # Errors
///
/// The first failure, in the order the COBOL checks them, each on
/// [`TRAN_ID_FIELD`]:
///
/// 1. [`ErrorKind::Empty`](crate::error::ErrorKind::Empty),
///    `Tran ID can NOT be empty...`: the Tran ID is absent, `SPACES` or
///    `LOW-VALUES` (V-COTRN01C-001).
/// 2. [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound),
///    `Transaction ID NOT found...`: no live transaction has the key
///    (V-COTRN01C-002).
/// 3. [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected),
///    `Unable to lookup Transaction...`: the read failed otherwise
///    (V-COTRN01C-003).
pub async fn view<S: TransactionReader>(
    store: &mut S,
    input: TransactionViewInput,
) -> Result<Transaction, DomainError> {
    let tran_id = edit_tran_id(input.tran_id.as_deref())?;
    read_transact_file(store, tran_id).await
}

#[cfg(test)]
mod tests {
    use super::{TRAN_ID_FIELD, TransactionViewInput, view};
    use crate::entities::{NewTransaction, Transaction};
    use crate::error::DomainError;
    use crate::messages::{
        TRAN_ID_CAN_NOT_BE_EMPTY, TRANSACTION_ID_NOT_FOUND, UNABLE_TO_LOOKUP_TRANSACTION,
    };
    use crate::numeric::{
        Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
        TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// A TRAN-ID posttran stores from the fixture corpus (P-CVTRA05Y-016).
    const EXISTING: &str = "0000000000683580";

    /// A TRAN-ID no fixture holds (P-CVTRA05Y-016).
    const MISSING: &str = "9999999999999999";

    fn tran(id: &str) -> NewTransaction {
        NewTransaction {
            tran_id: TranId::move_from(id),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: TranCategoryCode::new(1).expect("a four-digit category"),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: Money::from_cents(-1_234).expect("an amount within S9(09)V99"),
            tran_merchant_id: MerchantId::new(800_000_000).expect("a nine-digit merchant"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from("4859452612877065"),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from("2022-07-01 12:00:00.000000"),
        }
    }

    /// A store whose live master holds [`EXISTING`] as row 1.
    fn store() -> FakeStore {
        FakeStore::default().with_transaction(tran(EXISTING))
    }

    fn input(tran_id: Option<&str>) -> TransactionViewInput {
        TransactionViewInput {
            tran_id: tran_id.map(str::to_owned),
        }
    }

    fn not_found() -> DomainError {
        DomainError::not_found(TRANSACTION_ID_NOT_FOUND).with_field(TRAN_ID_FIELD)
    }

    /// P-COTRN01C-004, P-COTRN01C-005: RESP NORMAL shows the stored record.
    #[test]
    fn an_existing_tran_id_returns_the_stored_record() {
        let mut store = store();
        let shown = block_on(view(&mut store, input(Some(EXISTING))));
        assert_eq!(
            shown,
            Ok(Transaction::from_new(
                TransactionRowId::from(1),
                tran(EXISTING)
            ))
        );
        assert_eq!(store.calls, ["read_transaction"]);
    }

    /// P-COTRN01C-003, V-COTRN01C-001: a Tran ID of SPACES or LOW-VALUES is
    /// rejected before any file access.
    #[test]
    fn a_blank_tran_id_is_rejected_without_a_read() {
        let blanks = [
            None,
            Some(""),
            Some(" "),
            Some("                "),
            Some("\0"),
            Some("\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            Some("\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            // The MOVE keeps the first 16 characters, all spaces here.
            Some("                0000000000683580"),
        ];
        for blank in blanks {
            let mut store = store();
            let error = block_on(view(&mut store, input(blank)));
            assert_eq!(
                error,
                Err(DomainError::empty(TRAN_ID_CAN_NOT_BE_EMPTY).with_field(TRAN_ID_FIELD)),
                "{blank:?}"
            );
            assert!(store.calls.is_empty(), "{blank:?} read the file");
        }
    }

    /// V-COTRN01C-002: RESP NOTFND. The entered text is the key as moved into
    /// X(16), so a mix of spaces and LOW-VALUES and a short number without its
    /// leading zeros are keys that name no record.
    #[test]
    fn a_tran_id_no_live_record_has_is_not_found() {
        for key in [MISSING, "683580", " \0", "0000000000683581"] {
            let mut store = store();
            let error = block_on(view(&mut store, input(Some(key))));
            assert_eq!(error, Err(not_found()), "{key:?}");
            assert_eq!(store.calls, ["read_transaction"], "{key:?}");
        }
    }

    /// P-COTRN01C-004: the MOVE into TRAN-ID X(16) cuts a longer text on the
    /// right, so the first 16 characters are the key read.
    #[test]
    fn the_key_is_the_first_sixteen_characters() {
        let mut store = store();
        let shown = block_on(view(&mut store, input(Some("0000000000683580XYZ"))));
        assert_eq!(
            shown.map(|tran| tran.tran_id),
            Ok(TranId::move_from(EXISTING))
        );
    }

    /// V-COTRN01C-002, V-COTRN01C-003: NOTFND gives its own message and every
    /// other RESP gives the lookup message, both on TRNIDIN.
    #[test]
    fn a_failed_read_gives_the_message_of_its_resp() {
        let lookup =
            DomainError::unexpected(UNABLE_TO_LOOKUP_TRANSACTION).with_field(TRAN_ID_FIELD);
        let cases = [
            (PortError::NotFound, not_found()),
            (
                PortError::Unavailable("connection reset".to_owned()),
                lookup.clone(),
            ),
            (PortError::Contention, lookup.clone()),
            (PortError::Duplicate, lookup.clone()),
            (PortError::Restricted, lookup),
        ];
        for (failure, expected) in cases {
            let mut store = store();
            store.fail_next("read_transaction", failure.clone());
            let error = block_on(view(&mut store, input(Some(EXISTING))));
            assert_eq!(error, Err(expected), "{failure:?}");
            assert_eq!(store.calls, ["read_transaction"], "{failure:?}");
        }
    }
}
