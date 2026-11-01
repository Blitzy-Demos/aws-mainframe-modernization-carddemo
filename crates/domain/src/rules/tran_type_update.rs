//! `COTRTUPC` (CTTU): maintenance of one transaction type in the Db2 master
//! `CARDDEMO.TRANSACTION_TYPE`, table `transaction_types`
//! [app/app-transaction-type-db2/cbl/COTRTUPC.cbl].
//!
//! The COBOL is a pseudo-conversation: ENTER fetches a type by its key
//! (`9000-READ-TRANTYPE`), ENTER again edits the changes
//! (`1200-EDIT-MAP-INPUTS`), F5 writes them (`9600-WRITE-PROCESSING`), F5 on a
//! key not found starts an add, and F4 twice deletes
//! (`9800-DELETE-PROCESSING`). The API runs each operation in one request
//! (E-56), in the order the validation audit ranks the COBOL rejections, and
//! keeps no conversation state between requests (P-COTRTUPC-012):
//!
//! - [`update`] serves `PUT /admin/transaction-types/{id}` without `origin`:
//!   key edits, read, no-change compare, description edit, write
//!   (V-COTRTUPC-001 to V-COTRTUPC-011). With `origin=list` the `COTRTLIC`
//!   rules of `super::tran_type_list` apply instead (AAP 0.3.1).
//! - [`add`] serves `POST /admin/transaction-types`, the F5 add after a key not
//!   found (V-COTRTUPC-012 to V-COTRTUPC-021). A code already on file is 409
//!   with no message: the COBOL fetch shows that row for update and has no add
//!   path for it (E-51). A key typed over the key-not-found screen therefore
//!   never updates a type the screen did not show (E-70, proposed).
//! - [`delete`] is the F4 delete (V-COTRTUPC-022 to V-COTRTUPC-028). E-23
//!   carries F4 as `DELETE /admin/transaction-types/{id}` with these `COTRTUPC`
//!   texts. Which of the `COTRTUPC` and `COTRTLIC` message sets that endpoint
//!   answers with is open for the user, so the handler chooses between this
//!   function and `super::tran_type_list`.
//!
//! [`edit_type_code`], [`edit`] and [`upsert`] are the steps the three
//! compose. Only the Db2 master is written; reports read the processing copy
//! `trantype_file` and see an edit only after `batch seed --only tranextr`
//! (AAP 0.3.2).
//!
//! The 3270 state machine is not reproduced: the PF-key validity, the info
//! line, the screen attributes and the abend of a data scenario no request
//! reaches (P-COTRTUPC-001, P-COTRTUPC-010, P-COTRTUPC-011, P-COTRTUPC-019,
//! P-COTRTUPC-022 to P-COTRTUPC-027; E-63, E-64, E-68, E-69).
//!
//! # Stored description
//!
//! `9600-WRITE-PROCESSING` moves `FUNCTION TRIM` of the new description into
//! the 50-byte VARCHAR text and sets its length to 50, so Db2 holds it
//! blank-padded (P-COTRTUPC-007). The column `transaction_types.tr_description`
//! admits only right-trimmed text (migration 0001, the form of every stored
//! `X(n)` text), so the master carries the trimmed description. The padding is
//! trailing spaces only, which every `COTRTUPC` and `COTRTLIC` comparison trims.
//!
//! # Db2 texts
//!
//! The read and write failure texts embed `WS-DISP-SQLCODE PIC ----9` and
//! `SQLERRM OF SQLCA` (P-COTRTUPC-005, P-COTRTUPC-008, P-COTRTUPC-009,
//! P-COTRTUPC-021). E-30 (open) maps them to `PostgreSQL`; the port reports only
//! the condition ([`PortError`]), and no database text may reach a response
//! (AAP 0.6.4). The rules therefore put the Db2 code of the condition
//! ([`PortError::sqlcode`]) in the SQLCODE slot and leave the SQLERRM slot
//! empty. A failure with no Db2 code, [`PortError::Unavailable`], answers
//! [`UNEXPECTED_ERROR`], since its SQLCODE slot cannot be filled without
//! guessing. The insert failure is always composed: its slot holds the +100 of
//! the UPDATE before it (V-COTRTUPC-011).

use std::cmp::Ordering;

use crate::entities::TranTypeMaster;
use crate::error::DomainError;
use crate::format::unsigned_display;
use crate::messages::{
    CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY, CHANGES_COMMITTED_TO_DATABASE,
    COULD_NOT_LOCK_RECORD_FOR_UPDATE, DELETE_SUCCESSFUL, FIELD_TRAN_TYPE_CODE,
    FIELD_TRANSACTION_DESC, MUST_BE_NUMERIC, MUST_BE_SUPPLIED, MUST_NOT_BE_ZERO,
    NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED, NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE,
    TranTypeSqlFailure, UNEXPECTED_ERROR, field_edit_message, tran_type_sql_message,
};
use crate::numeric::{Alnum, TranTypeCode};
use crate::ports::{PortError, TranTypeMasterStore};

/// The BMS field of the type code, `TRTYPCD` of the `COTRTUP` map
/// [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L60]: the field every key,
/// read, write and delete failure marks.
pub const FIELD_TYPE_CODE: &str = "TRTYPCD";

/// The BMS field of the description, `TRTYDSC` of the `COTRTUP` map
/// [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L66]: the field the
/// description edits mark (V-COTRTUPC-007, V-COTRTUPC-008).
pub const FIELD_DESCRIPTION: &str = "TRTYDSC";

/// `TRTYPCDI PIC X(2)` and `TTUP-NEW-TTYP-TYPE PIC X(02)`.
const TYPE_CODE_LEN: usize = 2;

/// `TRTYDSCI PIC X(50)` and `TTUP-NEW-TTYP-TYPE-DESC PIC X(50)`.
const DESCRIPTION_LEN: usize = 50;

/// The `WS-DISP-SQLCODE` an insert failure shows: the +100 of the UPDATE that
/// found no row, never refreshed by `9700-INSERT-RECORD` (V-COTRTUPC-011,
/// E-51) [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1553-1560].
const UPDATE_NOT_FOUND_SQLCODE: i32 = 100;

/// The `SQLERRM OF SQLCA` slot of the composed Db2 texts: empty, because the
/// port carries no database text that may reach a response (E-30).
const SQLERRM_TEXT: &str = "";

/// The `'*'` that `1150-STORE-MAP-IN-NEW` treats as an empty field.
const STAR: &[u8] = b"*";

/// The COBOL figurative constant `SPACE`.
const SPACE: u8 = b' ';

/// The COBOL figurative constant `LOW-VALUE`.
const LOW_VALUE: u8 = 0x00;

/// The two fields of the `COTRTUP` map a request carries: `TRTYPCDI` and
/// `TRTYDSCI`, as entered.
///
/// For `PUT` the type code is the path segment. An absent JSON field is the
/// empty text, which the edits treat as the blank field it stands for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranTypeUpsertInput {
    /// The type code as entered (`TRTYPCDI X(2)`).
    pub type_code: String,
    /// The description as entered (`TRTYDSCI X(50)`).
    pub description: String,
}

/// The result of the `1200-EDIT-MAP-INPUTS` edits that let a request through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranTypeEdit {
    /// The changes passed every edit: the master `9600-WRITE-PROCESSING`
    /// writes, with the edited key and the trimmed description.
    Changed(TranTypeMaster),
    /// `1205-COMPARE-OLD-NEW` found no change against the fetched type: no
    /// edit runs and nothing is written. The response is 200 with
    /// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`] (V-COTRTUPC-006).
    Unchanged,
}

/// A write that `9600-WRITE-PROCESSING` or `9700-INSERT-RECORD` committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeSaved {
    /// The master as written.
    pub master: TranTypeMaster,
    /// Whether the UPDATE found no row, so that `9700-INSERT-RECORD` added it.
    pub inserted: bool,
    /// The info text the COBOL shows: [`CHANGES_COMMITTED_TO_DATABASE`]
    /// (P-COTRTUPC-007, P-COTRTUPC-009).
    pub message: String,
}

/// The outcome of a `PUT` that the edits let through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranTypeUpsertOutcome {
    /// The change was written.
    Saved(TranTypeSaved),
    /// The request repeats the stored values: nothing is written
    /// (V-COTRTUPC-006).
    Unchanged {
        /// The type as fetched.
        master: TranTypeMaster,
        /// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`].
        message: String,
    },
}

/// A delete that `9800-DELETE-PROCESSING` committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeDeleted {
    /// The code of the deleted type.
    pub code: TranTypeCode,
    /// The info text the COBOL shows: [`DELETE_SUCCESSFUL`] (P-COTRTUPC-004).
    pub message: String,
}

/// COTRTUPC 1210-EDIT-TRANTYPE, 1245-EDIT-NUM-REQD — P-COTRTUPC-014,
/// P-COTRTUPC-015, P-COTRTUPC-018, V-COTRTUPC-001 to V-COTRTUPC-003,
/// V-COTRTUPC-012 to V-COTRTUPC-014, V-COTRTUPC-022 to V-COTRTUPC-024.
///
/// The entered code is moved into the `X(2)` map field and stored by
/// `1150-STORE-MAP-IN-NEW` (`*` or spaces become `LOW-VALUES`, anything else
/// its `FUNCTION TRIM`), then edited: blank, not a `FUNCTION NUMVAL` argument,
/// or zero is rejected. A valid code is `FUNCTION NUMVAL` stored in the
/// unsigned `9(02)` `WS-EDIT-NUMERIC-2` and moved back as two digits, so `1`
/// and `-1` both become `01`.
///
/// # Errors
///
/// - [`ErrorKind::Empty`](crate::error::ErrorKind::Empty), the
///   [`MUST_BE_SUPPLIED`] text of [`FIELD_TRAN_TYPE_CODE`], for a blank code;
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid), its
///   [`MUST_BE_NUMERIC`] or [`MUST_NOT_BE_ZERO`] text;
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected),
///   [`UNEXPECTED_ERROR`], for a fraction of exactly one half, whose `9(02)`
///   store E-40 leaves unpinned (halt).
///
/// Each carries the field [`FIELD_TYPE_CODE`].
pub fn edit_type_code(raw: &str) -> Result<TranTypeCode, DomainError> {
    let field = map_input::<TYPE_CODE_LEN>(raw);
    let edit_error = |error: fn(String) -> DomainError, suffix: &str| {
        error(field_edit_message(FIELD_TRAN_TYPE_CODE, suffix)).with_field(FIELD_TYPE_CODE)
    };
    if field.is_blank() {
        return Err(edit_error(DomainError::empty, MUST_BE_SUPPLIED));
    }
    let Some(number) = NumvalText::parse(field.as_bytes()) else {
        return Err(edit_error(DomainError::invalid, MUST_BE_NUMERIC));
    };
    if number.is_zero() {
        return Err(edit_error(DomainError::invalid, MUST_NOT_BE_ZERO));
    }
    let value = number
        .numeric_2()
        .ok_or_else(|| DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_TYPE_CODE))?;
    // MOVE WS-EDIT-NUMERIC-2 TO WS-EDIT-ALPHANUMERIC-2: the two DISPLAY
    // digits; the INSPECT REPLACING SPACES BY ZEROS that follows finds none.
    let digits = unsigned_display(value, TYPE_CODE_LEN);
    Ok(TranTypeCode::move_from(&digits))
}

/// COTRTUPC 1200-EDIT-MAP-INPUTS, 1205-COMPARE-OLD-NEW, 1230-EDIT-ALPHANUM-REQD
/// — P-COTRTUPC-014 to P-COTRTUPC-017, V-COTRTUPC-006 to V-COTRTUPC-008,
/// V-COTRTUPC-017, V-COTRTUPC-018.
///
/// Edits the key ([`edit_type_code`]), compares the request with `current`,
/// the type fetched by that key, and edits the description. `current` is
/// `None` on the add path, where the failed read left the old details blank,
/// so every request is a change. [`update`] and [`add`] run the read between
/// the key edit and the rest, as the audit orders them.
///
/// The result is a [`TranTypeEdit`] rather than the master alone, because no
/// change is an outcome, not an error: the COBOL sets no error flag and the
/// API answers 200 (V-COTRTUPC-006).
///
/// # Errors
///
/// Those of [`edit_type_code`], then for the description, with the field
/// [`FIELD_DESCRIPTION`]:
///
/// - [`ErrorKind::Empty`](crate::error::ErrorKind::Empty), the
///   [`MUST_BE_SUPPLIED`] text of [`FIELD_TRANSACTION_DESC`], for a blank
///   description or `*`;
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid), its
///   [`CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY`] text, for any character other than
///   `A-Z`, `a-z`, `0-9` and space.
pub fn edit(
    input: &TranTypeUpsertInput,
    current: Option<&TranTypeMaster>,
) -> Result<TranTypeEdit, DomainError> {
    let code = edit_type_code(&input.type_code)?;
    let description = map_input::<DESCRIPTION_LEN>(&input.description);
    if current.is_some_and(|current| unchanged(code, &description, current)) {
        return Ok(TranTypeEdit::Unchanged);
    }
    validated_master(code, &description).map(TranTypeEdit::Changed)
}

/// COTRTUPC 9600-WRITE-PROCESSING, 9700-INSERT-RECORD — P-COTRTUPC-007 to
/// P-COTRTUPC-009, V-COTRTUPC-009 to V-COTRTUPC-011, V-COTRTUPC-019 to
/// V-COTRTUPC-021.
///
/// Updates the description of `master`'s type; when no row has the code
/// (SQLCODE +100) it inserts the type instead, as the add path and a type
/// deleted since its read both reach. Each statement is one legacy write; the
/// caller commits on `Ok` (the COBOL `SYNCPOINT`).
///
/// # Errors
///
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the field
/// [`FIELD_TYPE_CODE`], and nothing written:
///
/// - [`COULD_NOT_LOCK_RECORD_FOR_UPDATE`] when the UPDATE meets a lock
///   (SQLCODE -911);
/// - the [`TranTypeSqlFailure::Update`] text with the Db2 code for any other
///   UPDATE failure that has one, else [`UNEXPECTED_ERROR`];
/// - the [`TranTypeSqlFailure::Insert`] text for any INSERT failure, its
///   SQLCODE slot still holding the UPDATE's +100.
pub async fn upsert<S: TranTypeMasterStore>(
    store: &mut S,
    master: TranTypeMaster,
) -> Result<TranTypeSaved, DomainError> {
    match store.update_tran_type(master.clone()).await {
        Ok(()) => Ok(committed(master, false)),
        Err(PortError::NotFound) => insert(store, master).await,
        Err(PortError::Contention) => {
            Err(DomainError::unexpected(COULD_NOT_LOCK_RECORD_FOR_UPDATE)
                .with_field(FIELD_TYPE_CODE))
        }
        Err(error) => Err(
            DomainError::unexpected(sql_message(TranTypeSqlFailure::Update, &error))
                .with_field(FIELD_TYPE_CODE),
        ),
    }
}

/// COTRTUPC 9000-READ-TRANTYPE, 1200-EDIT-MAP-INPUTS, 9600-WRITE-PROCESSING —
/// P-COTRTUPC-015 to P-COTRTUPC-021, V-COTRTUPC-001 to V-COTRTUPC-011.
///
/// `PUT /admin/transaction-types/{id}` without `origin`: the ENTER fetch, the
/// ENTER edits and the F5 write of the `COTRTUP` screen in one request. The
/// key is edited first, the type read by the edited key, the request compared
/// with it and the description edited; a change is written by [`upsert`].
///
/// # Errors
///
/// Those of [`edit_type_code`], then, with the field [`FIELD_TYPE_CODE`],
/// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound)
/// [`NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE`] when no type has the code, and
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the
/// [`TranTypeSqlFailure::Access`] text and the Db2 code when the read fails
/// ([`UNEXPECTED_ERROR`] without a Db2 code); then those of [`edit`] for
/// the description and of [`upsert`].
pub async fn update<S: TranTypeMasterStore>(
    store: &mut S,
    input: &TranTypeUpsertInput,
) -> Result<TranTypeUpsertOutcome, DomainError> {
    let code = edit_type_code(&input.type_code)?;
    let current = fetch(store, code).await?.ok_or_else(record_not_found)?;
    let description = map_input::<DESCRIPTION_LEN>(&input.description);
    if unchanged(code, &description, &current) {
        return Ok(TranTypeUpsertOutcome::Unchanged {
            master: current,
            message: NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED.to_owned(),
        });
    }
    let master = validated_master(code, &description)?;
    upsert(store, master)
        .await
        .map(TranTypeUpsertOutcome::Saved)
}

/// COTRTUPC 9000-READ-TRANTYPE, 1200-EDIT-MAP-INPUTS, 9600-WRITE-PROCESSING,
/// 9700-INSERT-RECORD — P-COTRTUPC-006, P-COTRTUPC-009, P-COTRTUPC-031,
/// V-COTRTUPC-012 to V-COTRTUPC-021.
///
/// `POST /admin/transaction-types`: the read that finds no type (F5 to add),
/// the edits of the new details and the F5 write, which updates first and
/// inserts on SQLCODE +100 ([`upsert`]).
///
/// # Errors
///
/// Those of [`edit_type_code`]; then, with the field [`FIELD_TYPE_CODE`],
/// [`ErrorKind::Duplicate`](crate::error::ErrorKind::Duplicate) with no message
/// when a type already has the code (V-COTRTUPC-015, E-51), and the read
/// failure of [`update`]; then those of [`edit`] for the description and of
/// [`upsert`].
pub async fn add<S: TranTypeMasterStore>(
    store: &mut S,
    input: &TranTypeUpsertInput,
) -> Result<TranTypeSaved, DomainError> {
    let code = edit_type_code(&input.type_code)?;
    if fetch(store, code).await?.is_some() {
        // V-COTRTUPC-015 is a no-message row: the COBOL shows the existing
        // row for update and sets no text, so the error carries none (E-51).
        return Err(DomainError::duplicate(String::new()).with_field(FIELD_TYPE_CODE));
    }
    let description = map_input::<DESCRIPTION_LEN>(&input.description);
    let master = validated_master(code, &description)?;
    upsert(store, master).await
}

/// COTRTUPC 9000-READ-TRANTYPE, 9800-DELETE-PROCESSING — P-COTRTUPC-003 to
/// P-COTRTUPC-005, P-COTRTUPC-028, P-COTRTUPC-030, V-COTRTUPC-022 to
/// V-COTRTUPC-028.
///
/// The F4 delete, confirmed: the key is edited, the type read (F4 needs its
/// details shown) and deleted by the code read. The confirmation is the
/// dialog of the SPA, and the server keeps no delete state (E-63, E-69).
///
/// # Errors
///
/// Those of [`edit_type_code`] and the read failures of [`update`]; then, with
/// the field [`FIELD_TYPE_CODE`] and nothing deleted,
/// [`ErrorKind::Conflict`](crate::error::ErrorKind::Conflict) with the
/// [`TranTypeSqlFailure::DeleteChildRecords`] text when categories reference
/// the type (SQLCODE -532), and
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the
/// [`TranTypeSqlFailure::Delete`] text and the Db2 code for any other failure
/// that has one, else [`UNEXPECTED_ERROR`].
pub async fn delete<S: TranTypeMasterStore>(
    store: &mut S,
    type_code: &str,
) -> Result<TranTypeDeleted, DomainError> {
    let code = edit_type_code(type_code)?;
    let current = fetch(store, code).await?.ok_or_else(record_not_found)?;
    match store.delete_tran_type(current.tr_type).await {
        Ok(()) => Ok(TranTypeDeleted {
            code: current.tr_type,
            message: DELETE_SUCCESSFUL.to_owned(),
        }),
        Err(error @ PortError::Restricted) => Err(DomainError::conflict(sql_message(
            TranTypeSqlFailure::DeleteChildRecords,
            &error,
        ))
        .with_field(FIELD_TYPE_CODE)),
        Err(error) => Err(
            DomainError::unexpected(sql_message(TranTypeSqlFailure::Delete, &error))
                .with_field(FIELD_TYPE_CODE),
        ),
    }
}

/// COTRTUPC 9000-READ-TRANTYPE, 9100-GET-TRANSACTION-TYPE — P-COTRTUPC-020,
/// P-COTRTUPC-021, V-COTRTUPC-005, V-COTRTUPC-016, V-COTRTUPC-026.
///
/// The type with `code`, or `None` when the SELECT answers +100. Every other
/// failure is the [`TranTypeSqlFailure::Access`] text, or [`UNEXPECTED_ERROR`] without a
/// Db2 code.
async fn fetch<S: TranTypeMasterStore>(
    store: &mut S,
    code: TranTypeCode,
) -> Result<Option<TranTypeMaster>, DomainError> {
    match store.read_tran_type(code).await {
        Ok(master) => Ok(Some(master)),
        Err(PortError::NotFound) => Ok(None),
        Err(error) => Err(
            DomainError::unexpected(sql_message(TranTypeSqlFailure::Access, &error))
                .with_field(FIELD_TYPE_CODE),
        ),
    }
}

/// COTRTUPC 9700-INSERT-RECORD — P-COTRTUPC-009, V-COTRTUPC-011,
/// V-COTRTUPC-021.
///
/// Any INSERT failure gives the insert text with the UPDATE's +100, whatever
/// the INSERT answered (E-51).
async fn insert<S: TranTypeMasterStore>(
    store: &mut S,
    master: TranTypeMaster,
) -> Result<TranTypeSaved, DomainError> {
    match store.insert_tran_type(master.clone()).await {
        Ok(()) => Ok(committed(master, true)),
        Err(_) => Err(DomainError::unexpected(tran_type_sql_message(
            TranTypeSqlFailure::Insert,
            UPDATE_NOT_FOUND_SQLCODE,
            SQLERRM_TEXT,
        ))
        .with_field(FIELD_TYPE_CODE)),
    }
}

/// The `SYNCPOINT` outcome of a write: `master` with the
/// [`CHANGES_COMMITTED_TO_DATABASE`] info text.
fn committed(master: TranTypeMaster, inserted: bool) -> TranTypeSaved {
    TranTypeSaved {
        master,
        inserted,
        message: CHANGES_COMMITTED_TO_DATABASE.to_owned(),
    }
}

/// COTRTUPC 9100-GET-TRANSACTION-TYPE — V-COTRTUPC-004, V-COTRTUPC-025: the
/// SELECT found no row for the code (SQLCODE +100).
fn record_not_found() -> DomainError {
    DomainError::not_found(NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE).with_field(FIELD_TYPE_CODE)
}

/// The `failure` text for `error`, its SQLCODE slot holding the error's Db2
/// code and its SQLERRM slot empty, or [`UNEXPECTED_ERROR`] when the error has
/// no Db2 code (E-30).
fn sql_message(failure: TranTypeSqlFailure, error: &PortError) -> String {
    error.sqlcode().map_or_else(
        || UNEXPECTED_ERROR.to_owned(),
        |sqlcode| tran_type_sql_message(failure, sqlcode, SQLERRM_TEXT),
    )
}

/// COTRTUPC 1150-STORE-MAP-IN-NEW — P-COTRTUPC-014.
///
/// The entered text moved into the `X(N)` map field, then stored in the new
/// details: a field holding `*` or spaces becomes `LOW-VALUES`, any other its
/// `FUNCTION TRIM` (leading and trailing spaces removed), left-justified and
/// space-padded.
fn map_input<const N: usize>(raw: &str) -> Alnum<N> {
    let field = Alnum::<N>::move_from(raw);
    if field.cobol_cmp(STAR) == Ordering::Equal
        || field.as_bytes().iter().all(|&byte| byte == SPACE)
    {
        Alnum::from_array([LOW_VALUE; N])
    } else {
        Alnum::move_from_bytes(trim_spaces(field.as_bytes()))
    }
}

/// `FUNCTION TRIM`: `bytes` without leading and trailing spaces. Only spaces
/// are removed; `LOW-VALUES` stay.
fn trim_spaces(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|&byte| byte != SPACE)
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|&byte| byte != SPACE)
        .map_or(start, |last| last + 1);
    bytes.get(start..end).unwrap_or_default()
}

/// COTRTUPC 1205-COMPARE-OLD-NEW — P-COTRTUPC-016, V-COTRTUPC-006.
///
/// No change when the codes are equal after `FUNCTION UPPER-CASE` and the
/// `FUNCTION TRIM` of the descriptions are equal after `FUNCTION UPPER-CASE`
/// with the same length. The old description is the stored text moved into
/// the `X(50)` `TTUP-OLD-TTYP-TYPE-DESC` (P-COTRTUPC-020). Upper-casing maps
/// `a-z` only: the parity corpus is ASCII, and any other byte compares as
/// stored.
fn unchanged(
    code: TranTypeCode,
    description: &Alnum<DESCRIPTION_LEN>,
    current: &TranTypeMaster,
) -> bool {
    let old_description = Alnum::<DESCRIPTION_LEN>::move_from(&current.tr_description);
    // Equal trimmed operands have equal lengths, so the LENGTH test is part
    // of the slice equality.
    code.as_bytes()
        .eq_ignore_ascii_case(current.tr_type.as_bytes())
        && trim_spaces(description.as_bytes())
            .eq_ignore_ascii_case(trim_spaces(old_description.as_bytes()))
}

/// COTRTUPC 1230-EDIT-ALPHANUM-REQD, 9600-WRITE-PROCESSING — P-COTRTUPC-007,
/// P-COTRTUPC-017, V-COTRTUPC-007, V-COTRTUPC-008, V-COTRTUPC-017,
/// V-COTRTUPC-018.
///
/// The description edit, then the master the write stores: the edited key and
/// the trimmed description (see the module's notes on the stored description).
fn validated_master(
    code: TranTypeCode,
    description: &Alnum<DESCRIPTION_LEN>,
) -> Result<TranTypeMaster, DomainError> {
    let edit_error = |error: fn(String) -> DomainError, suffix: &str| {
        error(field_edit_message(FIELD_TRANSACTION_DESC, suffix)).with_field(FIELD_DESCRIPTION)
    };
    if description.is_blank() {
        return Err(edit_error(DomainError::empty, MUST_BE_SUPPLIED));
    }
    let alphanumeric = description
        .as_bytes()
        .iter()
        .all(|&byte| byte.is_ascii_alphanumeric() || byte == SPACE);
    if !alphanumeric {
        return Err(edit_error(
            DomainError::invalid,
            CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
        ));
    }
    let text: String = trim_spaces(description.as_bytes())
        .iter()
        .copied()
        .map(char::from)
        .collect();
    // The edit passed, so the text is at most 50 ASCII characters and
    // `try_new` accepts it; the mapping only keeps the call free of a panic.
    TranTypeMaster::try_new(code, text)
        .map_err(|_| DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_DESCRIPTION))
}

/// A text that `FUNCTION TEST-NUMVAL` accepts, split into the digits before
/// and after its decimal point.
struct NumvalText<'a> {
    /// The digits before the decimal point.
    integer: &'a [u8],
    /// The digits after the decimal point.
    fraction: &'a [u8],
}

impl<'a> NumvalText<'a> {
    /// COTRTUPC 1245-EDIT-NUM-REQD `FUNCTION TEST-NUMVAL(...) = 0` —
    /// P-COTRTUPC-018, V-COTRTUPC-002.
    ///
    /// The `NUMVAL` argument forms: optional spaces, an optional leading `+`
    /// or `-` with optional spaces, digits with an optional decimal point (at
    /// least one digit), optional spaces, and, without a leading sign, an
    /// optional trailing `+` or `-` with optional spaces. The trailing `CR`
    /// and `DB` forms need two bytes beside a digit, more than the two-byte
    /// key holds, so no key text passes with them. `None` when the text is
    /// not an argument; `LOW-VALUES` never are.
    fn parse(text: &'a [u8]) -> Option<Self> {
        let rest = skip_spaces(text);
        let (signed, rest) = match rest.split_first() {
            Some((b'+' | b'-', tail)) => (true, skip_spaces(tail)),
            _ => (false, rest),
        };
        let (integer, rest) = split_digits(rest);
        let (fraction, rest) = match rest.split_first() {
            Some((b'.', tail)) => split_digits(tail),
            _ => rest.split_at(0),
        };
        if integer.is_empty() && fraction.is_empty() {
            return None;
        }
        let rest = skip_spaces(rest);
        let rest = match rest.split_first() {
            Some((b'+' | b'-', tail)) if !signed => skip_spaces(tail),
            _ => rest,
        };
        rest.is_empty().then_some(Self { integer, fraction })
    }

    /// `FUNCTION NUMVAL(...) = 0`: every digit is zero (P-COTRTUPC-018,
    /// V-COTRTUPC-003).
    fn is_zero(&self) -> bool {
        self.integer
            .iter()
            .chain(self.fraction)
            .all(|&digit| digit == b'0')
    }

    /// COTRTUPC 1210-EDIT-TRANTYPE `COMPUTE WS-EDIT-NUMERIC-2 = FUNCTION
    /// NUMVAL(...)` — P-COTRTUPC-015.
    ///
    /// The unsigned `9(02)` receiver drops the sign. `NUMVAL` is floating
    /// point, so a fraction converts to the nearest integer (E-40, pinned);
    /// `None` for a fraction of exactly one half, the midpoint E-40 leaves
    /// unpinned (halt). A two-byte key has at most two integer digits, so
    /// the receiver never overflows.
    fn numeric_2(&self) -> Option<u64> {
        let integer = self
            .integer
            .iter()
            .fold(0, |value, &digit| value * 10 + u64::from(digit - b'0'));
        let rounding = match self.fraction.split_first() {
            None => Ordering::Less,
            Some((&first, rest)) => match first.cmp(&b'5') {
                Ordering::Equal if rest.iter().all(|&digit| digit == b'0') => Ordering::Equal,
                Ordering::Equal => Ordering::Greater,
                other => other,
            },
        };
        match rounding {
            Ordering::Less => Some(integer),
            Ordering::Greater => Some(integer + 1),
            Ordering::Equal => None,
        }
    }
}

/// `text` without its leading spaces.
fn skip_spaces(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|&byte| byte != SPACE)
        .unwrap_or(text.len());
    text.get(start..).unwrap_or_default()
}

/// The leading ASCII digits of `text`, and the rest.
fn split_digits(text: &[u8]) -> (&[u8], &[u8]) {
    let end = text
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_DESCRIPTION, FIELD_TYPE_CODE, TranTypeDeleted, TranTypeEdit, TranTypeSaved,
        TranTypeUpsertInput, TranTypeUpsertOutcome, add, delete, edit, edit_type_code, update,
        upsert,
    };
    use crate::entities::{TranCategoryMaster, TranTypeMaster};
    use crate::error::{DomainError, ErrorKind};
    use crate::messages::{
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY, CHANGES_COMMITTED_TO_DATABASE,
        COULD_NOT_LOCK_RECORD_FOR_UPDATE, DELETE_SUCCESSFUL, FIELD_TRAN_TYPE_CODE,
        FIELD_TRANSACTION_DESC, MUST_BE_NUMERIC, MUST_BE_SUPPLIED, MUST_NOT_BE_ZERO,
        NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
        NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE, TranTypeSqlFailure, UNEXPECTED_ERROR,
        field_edit_message, tran_type_sql_message,
    };
    use crate::numeric::{Alnum, TranCategoryCode, TranTypeCode};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The three operations that share the key edits and the read.
    #[derive(Clone, Copy, Debug)]
    enum Operation {
        Update,
        Add,
        Delete,
    }

    const OPERATIONS: [Operation; 3] = [Operation::Update, Operation::Add, Operation::Delete];

    fn code(text: &str) -> TranTypeCode {
        TranTypeCode::move_from(text)
    }

    fn master(type_code: &str, description: &str) -> TranTypeMaster {
        TranTypeMaster::try_new(code(type_code), description.to_owned())
            .expect("a description that fits TR_DESCRIPTION")
    }

    fn input(type_code: &str, description: &str) -> TranTypeUpsertInput {
        TranTypeUpsertInput {
            type_code: type_code.to_owned(),
            description: description.to_owned(),
        }
    }

    fn rejected(kind: ErrorKind, message: &str, field: &'static str) -> DomainError {
        DomainError::new(kind, message).with_field(field)
    }

    /// Types 01 (with a category, P-TRNTYCAT-005) and 03 of the DB2LTTYP load
    /// (P-TRNTYPE-004); 99 is missing (P-TRNTYPE-005).
    fn seeded() -> FakeStore {
        FakeStore::default()
            .with_tran_type(master("01", "PURCHASE"))
            .with_tran_type(master("03", "CREDIT"))
            .with_tran_category(TranCategoryMaster {
                trc_type_code: code("01"),
                trc_type_category: TranCategoryCode::new(1).expect("a 9(04) category"),
                trc_cat_data: Alnum::move_from("REGULAR SALES DRAFT"),
            })
    }

    /// Runs `operation` for the entered code, with `description` for the
    /// writes.
    fn perform(
        store: &mut FakeStore,
        operation: Operation,
        type_code: &str,
        description: &str,
    ) -> Result<(), DomainError> {
        let request = input(type_code, description);
        match operation {
            Operation::Update => block_on(update(store, &request)).map(|_| ()),
            Operation::Add => block_on(add(store, &request)).map(|_| ()),
            Operation::Delete => block_on(delete(store, type_code)).map(|_| ()),
        }
    }

    // V-COTRTUPC-001 to -003 (PUT), -012 to -014 (POST), -022 to -024
    // (DELETE); P-COTRTUPC-014, P-COTRTUPC-018.
    #[test]
    fn key_edits_reject_before_any_read() {
        let supplied = field_edit_message(FIELD_TRAN_TYPE_CODE, MUST_BE_SUPPLIED);
        let numeric = field_edit_message(FIELD_TRAN_TYPE_CODE, MUST_BE_NUMERIC);
        let zero = field_edit_message(FIELD_TRAN_TYPE_CODE, MUST_NOT_BE_ZERO);
        let cases = [
            ("", ErrorKind::Empty, &supplied),
            ("  ", ErrorKind::Empty, &supplied),
            ("*", ErrorKind::Empty, &supplied),
            ("\0\0", ErrorKind::Empty, &supplied),
            ("0A", ErrorKind::Invalid, &numeric),
            ("A", ErrorKind::Invalid, &numeric),
            (" *", ErrorKind::Invalid, &numeric),
            ("**", ErrorKind::Invalid, &numeric),
            ("+", ErrorKind::Invalid, &numeric),
            (".", ErrorKind::Invalid, &numeric),
            ("+-", ErrorKind::Invalid, &numeric),
            ("1\0", ErrorKind::Invalid, &numeric),
            ("00", ErrorKind::Invalid, &zero),
            ("0", ErrorKind::Invalid, &zero),
            ("-0", ErrorKind::Invalid, &zero),
            ("0-", ErrorKind::Invalid, &zero),
            (".0", ErrorKind::Invalid, &zero),
            ("0.", ErrorKind::Invalid, &zero),
        ];
        for (raw, kind, message) in cases {
            let expected = rejected(kind, message, FIELD_TYPE_CODE);
            assert_eq!(edit_type_code(raw), Err(expected.clone()), "{raw:?}");
            assert_eq!(
                edit(&input(raw, "NEW TYPE"), Some(&master("01", "PURCHASE"))),
                Err(expected.clone()),
                "{raw:?}"
            );
            for operation in OPERATIONS {
                let mut store = seeded();
                assert_eq!(
                    perform(&mut store, operation, raw, ""),
                    Err(expected.clone()),
                    "{operation:?} {raw:?}"
                );
                assert!(store.calls.is_empty(), "{operation:?} {raw:?}");
            }
        }
    }

    // P-COTRTUPC-015: FUNCTION NUMVAL stored in the unsigned 9(02), moved back
    // as two digits; a fraction converts to the nearest integer (E-40).
    #[test]
    fn valid_keys_become_two_unsigned_digits() {
        let cases = [
            ("01", "01"),
            ("1", "01"),
            (" 1", "01"),
            ("1 ", "01"),
            ("+7", "07"),
            ("-1", "01"),
            ("1-", "01"),
            ("4+", "04"),
            ("7.", "07"),
            (".7", "01"),
            (".3", "00"),
            ("99", "99"),
            ("123", "12"),
        ];
        for (raw, expected) in cases {
            assert_eq!(edit_type_code(raw), Ok(code(expected)), "{raw:?}");
        }
    }

    // E-40 (halt: midpoints): the 9(02) store of one half is not guessed.
    #[test]
    fn a_key_of_one_half_is_the_numval_halt() {
        assert_eq!(
            edit_type_code(".5"),
            Err(rejected(
                ErrorKind::Unexpected,
                UNEXPECTED_ERROR,
                FIELD_TYPE_CODE
            ))
        );
    }

    // V-COTRTUPC-007, -008, -017, -018; P-COTRTUPC-014, P-COTRTUPC-017.
    #[test]
    fn description_edits_follow_1230() {
        let supplied = field_edit_message(FIELD_TRANSACTION_DESC, MUST_BE_SUPPLIED);
        let alphanumeric =
            field_edit_message(FIELD_TRANSACTION_DESC, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY);
        let cases = [
            ("", ErrorKind::Empty, &supplied),
            ("   ", ErrorKind::Empty, &supplied),
            ("*", ErrorKind::Empty, &supplied),
            ("PURCHASE-RETAIL", ErrorKind::Invalid, &alphanumeric),
            (" *", ErrorKind::Invalid, &alphanumeric),
            ("Café", ErrorKind::Invalid, &alphanumeric),
            ("A\0", ErrorKind::Invalid, &alphanumeric),
        ];
        let current = master("01", "PURCHASE");
        for (raw, kind, message) in cases {
            let expected = rejected(kind, message, FIELD_DESCRIPTION);
            for fetched in [None, Some(&current)] {
                assert_eq!(
                    edit(&input("1", raw), fetched),
                    Err(expected.clone()),
                    "{raw:?} {fetched:?}"
                );
            }
        }
    }

    // P-COTRTUPC-007, P-COTRTUPC-014: the master holds the edited key and the
    // trimmed description, internal spaces kept, cut at the X(50) map field.
    #[test]
    fn a_change_carries_the_trimmed_description() {
        let cases = [
            ("  Big  Purchase  ".to_owned(), "Big  Purchase".to_owned()),
            ("A".repeat(50), "A".repeat(50)),
            ("B".repeat(60), "B".repeat(50)),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                edit(&input("2", &raw), None),
                Ok(TranTypeEdit::Changed(master("02", &expected))),
                "{raw:?}"
            );
        }
    }

    // V-COTRTUPC-006, P-COTRTUPC-016: the compare upper-cases and trims, and
    // runs before the description edit.
    #[test]
    fn the_compare_finds_no_change_before_editing_the_description() {
        let purchase = master("01", "PURCHASE");
        let stored_by_batch = master("01", "PURCHASE-RETAIL");
        let changed =
            |type_code, description| TranTypeEdit::Changed(master(type_code, description));
        let cases = [
            ("01", "PURCHASE", &purchase, TranTypeEdit::Unchanged),
            ("1", "purchase", &purchase, TranTypeEdit::Unchanged),
            ("01", "  Purchase  ", &purchase, TranTypeEdit::Unchanged),
            (
                "01",
                "purchase-retail",
                &stored_by_batch,
                TranTypeEdit::Unchanged,
            ),
            ("01", "PURCHASES", &purchase, changed("01", "PURCHASES")),
            ("01", "PUR CHASE", &purchase, changed("01", "PUR CHASE")),
            ("02", "PURCHASE", &purchase, changed("02", "PURCHASE")),
        ];
        for (raw_code, raw_description, current, expected) in cases {
            assert_eq!(
                edit(&input(raw_code, raw_description), Some(current)),
                Ok(expected),
                "{raw_code:?} {raw_description:?}"
            );
        }
    }

    // P-COTRTUPC-007, P-COTRTUPC-020: PUT reads the type, then updates it.
    #[test]
    fn update_writes_a_change_to_the_type_it_read() {
        let mut store = seeded();
        let written = master("01", "Retail Purchase");
        assert_eq!(
            block_on(update(&mut store, &input("1", "  Retail Purchase "))),
            Ok(TranTypeUpsertOutcome::Saved(TranTypeSaved {
                master: written.clone(),
                inserted: false,
                message: CHANGES_COMMITTED_TO_DATABASE.to_owned(),
            }))
        );
        assert_eq!(store.tran_types.get(&code("01")), Some(&written));
        assert_eq!(store.calls, ["read_tran_type", "update_tran_type"]);
    }

    // V-COTRTUPC-006: no change answers its text and writes nothing.
    #[test]
    fn update_without_a_change_writes_nothing() {
        let mut store = seeded();
        assert_eq!(
            block_on(update(&mut store, &input("01", "purchase"))),
            Ok(TranTypeUpsertOutcome::Unchanged {
                master: master("01", "PURCHASE"),
                message: NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED.to_owned(),
            })
        );
        assert_eq!(store.calls, ["read_tran_type"]);
        assert_eq!(store.tran_types, seeded().tran_types);
    }

    // V-COTRTUPC-004 and V-COTRTUPC-025 (Order 4) come before the description
    // edits (Order 6) and write nothing.
    #[test]
    fn update_and_delete_of_a_missing_type_are_404() {
        let expected = rejected(
            ErrorKind::NotFound,
            NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE,
            FIELD_TYPE_CODE,
        );
        for operation in [Operation::Update, Operation::Delete] {
            for description in ["NEW TYPE", ""] {
                let mut store = seeded();
                assert_eq!(
                    perform(&mut store, operation, "99", description),
                    Err(expected.clone()),
                    "{operation:?} {description:?}"
                );
                assert_eq!(store.calls, ["read_tran_type"], "{operation:?}");
            }
        }
    }

    // V-COTRTUPC-005, V-COTRTUPC-016, V-COTRTUPC-026; P-COTRTUPC-021; E-30.
    #[test]
    fn read_failures_answer_the_access_text() {
        let cases = [
            (
                PortError::Contention,
                tran_type_sql_message(TranTypeSqlFailure::Access, -911, ""),
            ),
            (
                PortError::Restricted,
                tran_type_sql_message(TranTypeSqlFailure::Access, -532, ""),
            ),
            (
                PortError::Unavailable("connection reset".to_owned()),
                UNEXPECTED_ERROR.to_owned(),
            ),
        ];
        for (error, message) in cases {
            for operation in OPERATIONS {
                let mut store = seeded();
                store.fail_next("read_tran_type", error.clone());
                assert_eq!(
                    perform(&mut store, operation, "01", "NEW TYPE"),
                    Err(rejected(ErrorKind::Unexpected, &message, FIELD_TYPE_CODE)),
                    "{operation:?} {error:?}"
                );
                assert_eq!(store.calls, ["read_tran_type"], "{operation:?}");
            }
        }
    }

    // P-COTRTUPC-006, P-COTRTUPC-009: POST of an unknown code updates, finds
    // no row and inserts.
    #[test]
    fn add_inserts_a_new_type() {
        let mut store = seeded();
        let written = master("99", "Annual Fee");
        assert_eq!(
            block_on(add(&mut store, &input("99", " Annual Fee"))),
            Ok(TranTypeSaved {
                master: written.clone(),
                inserted: true,
                message: CHANGES_COMMITTED_TO_DATABASE.to_owned(),
            })
        );
        assert_eq!(store.tran_types.get(&code("99")), Some(&written));
        assert_eq!(
            store.calls,
            ["read_tran_type", "update_tran_type", "insert_tran_type"]
        );
    }

    // V-COTRTUPC-015 (Order 4, before the description edits): no-message 409
    // (E-51).
    #[test]
    fn add_of_an_existing_code_is_409_without_a_message() {
        for description in ["NEW TYPE", ""] {
            let mut store = seeded();
            assert_eq!(
                block_on(add(&mut store, &input("1", description))),
                Err(rejected(ErrorKind::Duplicate, "", FIELD_TYPE_CODE)),
                "{description:?}"
            );
            assert_eq!(store.calls, ["read_tran_type"]);
        }
    }

    // V-COTRTUPC-009 to -011 (PUT), -019 to -021 (POST); P-COTRTUPC-008,
    // P-COTRTUPC-009: the insert text keeps the UPDATE's +100 (E-51).
    #[test]
    fn write_failures_answer_the_cobol_texts_and_change_nothing() {
        let lock = COULD_NOT_LOCK_RECORD_FOR_UPDATE.to_owned();
        let insert_failed = tran_type_sql_message(TranTypeSqlFailure::Insert, 100, "");
        let unavailable = || PortError::Unavailable("connection reset".to_owned());
        let cases = [
            (
                Operation::Update,
                "01",
                vec![("update_tran_type", PortError::Contention)],
                lock.clone(),
            ),
            (
                Operation::Update,
                "01",
                vec![("update_tran_type", PortError::Duplicate)],
                tran_type_sql_message(TranTypeSqlFailure::Update, -803, ""),
            ),
            (
                Operation::Update,
                "01",
                vec![("update_tran_type", unavailable())],
                UNEXPECTED_ERROR.to_owned(),
            ),
            (
                Operation::Update,
                "01",
                vec![("update_tran_type", PortError::NotFound)],
                insert_failed.clone(),
            ),
            (
                Operation::Update,
                "01",
                vec![
                    ("update_tran_type", PortError::NotFound),
                    ("insert_tran_type", unavailable()),
                ],
                insert_failed.clone(),
            ),
            (
                Operation::Add,
                "99",
                vec![("update_tran_type", PortError::Contention)],
                lock,
            ),
            (
                Operation::Add,
                "99",
                vec![("update_tran_type", PortError::Restricted)],
                tran_type_sql_message(TranTypeSqlFailure::Update, -532, ""),
            ),
            (
                Operation::Add,
                "99",
                vec![("insert_tran_type", PortError::Duplicate)],
                insert_failed.clone(),
            ),
            (
                Operation::Add,
                "99",
                vec![("insert_tran_type", unavailable())],
                insert_failed,
            ),
        ];
        for (operation, type_code, failures, message) in cases {
            let mut store = seeded();
            for (method, error) in failures {
                store.fail_next(method, error);
            }
            assert_eq!(
                perform(&mut store, operation, type_code, "NEW TYPE"),
                Err(rejected(ErrorKind::Unexpected, &message, FIELD_TYPE_CODE)),
                "{operation:?} {message:?}"
            );
            assert_eq!(store.tran_types, seeded().tran_types, "{message:?}");
        }
    }

    // P-COTRTUPC-007 to P-COTRTUPC-009: UPDATE, or INSERT on SQLCODE +100.
    #[test]
    fn upsert_updates_or_inserts() {
        let cases = [
            (master("01", "RETAIL"), false, vec!["update_tran_type"]),
            (
                master("05", "REFUND"),
                true,
                vec!["update_tran_type", "insert_tran_type"],
            ),
        ];
        for (written, inserted, calls) in cases {
            let mut store = seeded();
            assert_eq!(
                block_on(upsert(&mut store, written.clone())),
                Ok(TranTypeSaved {
                    master: written.clone(),
                    inserted,
                    message: CHANGES_COMMITTED_TO_DATABASE.to_owned(),
                })
            );
            assert_eq!(store.tran_types.get(&written.tr_type), Some(&written));
            assert_eq!(store.calls, calls);
        }
    }

    // P-COTRTUPC-004: the confirmed F4 deletes the type read.
    #[test]
    fn delete_removes_the_type_it_read() {
        let mut store = seeded();
        assert_eq!(
            block_on(delete(&mut store, "3")),
            Ok(TranTypeDeleted {
                code: code("03"),
                message: DELETE_SUCCESSFUL.to_owned(),
            })
        );
        assert!(!store.tran_types.contains_key(&code("03")));
        assert_eq!(store.calls, ["read_tran_type", "delete_tran_type"]);
    }

    // V-COTRTUPC-027, V-COTRTUPC-028; P-COTRTUPC-005; E-30.
    #[test]
    fn delete_failures_answer_the_cobol_texts() {
        let cases = [
            (
                "01",
                None,
                ErrorKind::Conflict,
                tran_type_sql_message(TranTypeSqlFailure::DeleteChildRecords, -532, ""),
            ),
            (
                "03",
                Some(PortError::Contention),
                ErrorKind::Unexpected,
                tran_type_sql_message(TranTypeSqlFailure::Delete, -911, ""),
            ),
            (
                "03",
                Some(PortError::NotFound),
                ErrorKind::Unexpected,
                tran_type_sql_message(TranTypeSqlFailure::Delete, 100, ""),
            ),
            (
                "03",
                Some(PortError::Unavailable("connection reset".to_owned())),
                ErrorKind::Unexpected,
                UNEXPECTED_ERROR.to_owned(),
            ),
        ];
        for (type_code, failure, kind, message) in cases {
            let mut store = seeded();
            if let Some(error) = failure {
                store.fail_next("delete_tran_type", error);
            }
            assert_eq!(
                block_on(delete(&mut store, type_code)),
                Err(rejected(kind, &message, FIELD_TYPE_CODE)),
                "{message:?}"
            );
            assert_eq!(store.tran_types, seeded().tran_types, "{message:?}");
        }
    }

    /// The futures hold only the store and the request, so they are `Send`
    /// wherever the store is, as the multi-threaded API runtime requires.
    #[test]
    fn rule_futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = seeded();
        let request = input("01", "PURCHASE");
        assert_send(&update(&mut store, &request));
        assert_send(&add(&mut store, &request));
        assert_send(&delete(&mut store, "01"));
        assert_send(&upsert(&mut store, master("01", "PURCHASE")));
        assert!(store.calls.is_empty(), "an unpolled future does nothing");
    }
}
