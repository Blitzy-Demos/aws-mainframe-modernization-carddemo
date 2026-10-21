//! COTRN02C, transaction add: the service of endpoint 13, `POST /api/v1/transactions`
//! (transaction `CT02`, map `COTRN2A`; AAP 0.3.4.1).
//!
//! It ports the flow and the file statements of the program (`COTRN02C.cbl:164-749`); every
//! edit, literal and record rule is `domain::online::cotrn02c`'s, and this module only reads,
//! writes and dispatches (Thin Request Handlers).
//!
//! # Flow
//!
//! - **ENTER**, `PROCESS-ENTER-KEY`: the key edits and the cross-reference read
//!   (COTRN02C-009 to COTRN02C-019), the data edits on the request's fields (COTRN02C-020 to
//!   COTRN02C-039), then the confirmation (COTRN02C-040 to COTRN02C-042). `Y` or `y` adds the
//!   transaction (COTRN02C-043 to COTRN02C-051); blank, `N` or `n` answers with the prompt and
//!   writes nothing; any other value is refused.
//! - **PF5**, `COPY-LAST-TRAN-DATA`: the key edits first (COTRN02C-052), the copy of the data
//!   fields of the transaction with the highest ID in the whole table (COTRN02C-053 to
//!   COTRN02C-055), then `PROCESS-ENTER-KEY` on the copied fields with the request's
//!   confirmation, the key edits and the cross-reference read included, as the program runs them
//!   again (COTRN02C-056).
//!
//! Every send of the COBOL map ends the task, so the first failure is the response and nothing
//! after it runs (COTRN02C-008): each step returns its failure through `?`, and nothing is
//! written unless every edit passed and the confirmation is `Y`.
//!
//! # Store access
//!
//! - The reads run on a clone of the application's `PgStore`, each statement on its own.
//! - The add runs in one `PgTx`: the highest-ID browse (`find_last` for the FILLER the browse
//!   leaves in the record, `TransactionRepository::next_id` for the ID), then the insert, then
//!   the commit. A failed browse or insert rolls the transaction back, so nothing is written
//!   (COTRN02C-044, COTRN02C-050, COTRN02C-051).
//! - The browse takes no lock, exactly as the CICS browse takes none: two concurrent adds can
//!   compute the same ID, and the second insert fails with the duplicate message the COBOL
//!   gives on `DUPREC` (COTRN02C-043, COTRN02C-050).
//! - A store result the program answers with a text of its own becomes the domain's
//!   `XrefLookup` or one of its `AddFailure` constructors. Only `begin`, `commit` and `rollback`
//!   reach `ServiceError::Store`.
//!
//! # Rules applied
//!
//! - Cardholder Data Redaction: the typed card number is request input, handed to the key edit
//!   alone; the card the response carries is the domain's `CardNumber`, which serialises redacted
//!   (COTRN02C-012, COTRN02C-017, COTRN02C-057, `E32`). The region-log event of a failed file
//!   statement holds the paragraph and the store error's SQLSTATE and constraint name only,
//!   never a key or a field.
//! - Typed Dates and Timestamps: the original and processing dates reach the domain edit as the
//!   raw text of their map fields, the raw-edit-input exception of AAP 0.3.4.1; the edit returns
//!   them typed, and the record stores them as layout-`S` timestamps (COTRN02C-046, `E33`).
//! - Repository Method Naming: `find_all_by_acct_id`, `find_by_id`, `find_last`, `next_id`,
//!   `insert`.

use core::fmt;

use domain::copybook::PicX;
use domain::online::cotrn02c::{
    self, AddFailure, AddOutcome, Confirmation, CopiedFields, FilledKey, KeyRequest, TranAddInput,
    ValidatedTran, XrefLookup,
};
use domain::records::CardXrefRecord;
use persistence::{CardXrefRepository, PersistenceError, PgStore, PgTx, TransactionRepository};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cotrn02c::{TranAddFields, TranAddRequest};
use crate::error::ServiceError;

/// The paragraph of the account path's cross-reference read (COTRN02C-011, COTRN02C-014).
const READ_CXACAIX: &str = "READ-CXACAIX-FILE";

/// The paragraph of the card path's cross-reference read (COTRN02C-016, COTRN02C-019).
const READ_CCXREF: &str = "READ-CCXREF-FILE";

/// The paragraphs of the highest-ID browse, of the add and of PF5 (COTRN02C-044, COTRN02C-053).
const BROWSE_TRANSACT: &str = "STARTBR-TRANSACT-FILE/READPREV-TRANSACT-FILE";

/// The paragraph of the insert (COTRN02C-051).
const WRITE_TRANSACT: &str = "WRITE-TRANSACT-FILE";

/// The condition the account path reports when two or more cross-references share the
/// account: CICS `DUPKEY` on the `NONUNIQUEKEY` path `CXACAIX` (COTRN02C-014).
const DUPKEY: &str = "DUPKEY";

/// Endpoint 13: adds a transaction, or answers with the confirmation prompt, after COTRN02C's
/// edits (COTRN02C-007 to COTRN02C-057).
///
/// `aid` ENTER runs `PROCESS-ENTER-KEY` on the request's fields; PF5 runs
/// `COPY-LAST-TRAN-DATA`, which replaces them with the highest-ID transaction's before it does
/// the same. The request's typed card number is read only by the key edit.
///
/// # Errors
///
/// - [`ServiceError::Rule`]: the first COTRN02C edit, read or write that fails, with its audited
///   message, field and status class: the key edits and cross-reference reads (COTRN02C-009 to
///   COTRN02C-019), the data edits (COTRN02C-020 to COTRN02C-039), an unknown confirmation
///   (COTRN02C-042), the highest-ID browse (COTRN02C-044, COTRN02C-053) and the insert
///   (COTRN02C-050, COTRN02C-051).
/// - [`ServiceError::Store`]: the add's transaction could not be opened, committed or rolled
///   back; nothing is written.
pub(crate) async fn add(
    state: &AppState,
    _user: &SignedOnUser,
    request: TranAddRequest,
) -> Result<AddOutcome, ServiceError<AddFailure>> {
    let TranAddRequest {
        aid,
        acct_id,
        card_num,
        fields,
        confirm,
    } = request;
    let keys = Keys {
        acct_id: acct_id.as_deref(),
        card_num: card_num.as_deref(),
    };
    let mut store = state.store.clone();
    match cotrn02c::Aid::from(aid) {
        cotrn02c::Aid::Enter => {
            process_enter(&mut store, keys, entered(&fields, &confirm), None).await
        }
        cotrn02c::Aid::Pf5 => {
            // `COPY-LAST-TRAN-DATA` resolves the key before its browse, and an error there ends
            // the request with nothing copied; the key it resolves is not kept, because
            // `PROCESS-ENTER-KEY` resolves it again (COTRN02C-052, COTRN02C-056).
            fill(&mut store, keys).await?;
            let copied = copy_last_fields(&mut store).await?;
            process_enter(&mut store, keys, copied.as_input(&confirm), Some(&copied)).await
        }
    }
}

/// The raw key fields of the request: `ACTIDINI` and `CARDNINI`, `None` when absent.
#[derive(Clone, Copy)]
struct Keys<'a> {
    acct_id: Option<&'a str>,
    card_num: Option<&'a str>,
}

/// The request's eleven data fields and its confirmation, as the input of the data edits
/// (COTRN02C-060).
fn entered<'a>(fields: &'a TranAddFields, confirm: &'a str) -> TranAddInput<'a> {
    TranAddInput {
        type_cd: &fields.type_cd,
        cat_cd: &fields.cat_cd,
        source: &fields.source,
        description: &fields.description,
        amount: &fields.amount,
        orig_date: &fields.orig_date,
        proc_date: &fields.proc_date,
        merchant_id: &fields.merchant_id,
        merchant_name: &fields.merchant_name,
        merchant_city: &fields.merchant_city,
        merchant_zip: &fields.merchant_zip,
        confirm,
    }
}

/// `PROCESS-ENTER-KEY`: the key edits and their read, the data edits on `input`, then the
/// `EVALUATE CONFIRMI` (COTRN02C-009 to COTRN02C-051). `copied` is the PF5 copy the prompt
/// shows (COTRN02C-056).
///
/// The confirmation is tested only after every edit has passed, so a `Y` with a failing field
/// gets that field's message (COTRN02C-040).
async fn process_enter(
    store: &mut PgStore,
    keys: Keys<'_>,
    input: TranAddInput<'_>,
    copied: Option<&CopiedFields>,
) -> Result<AddOutcome, ServiceError<AddFailure>> {
    let key = fill(store, keys).await?;
    let fields = cotrn02c::edit_data(&input).map_err(ServiceError::Rule)?;
    match cotrn02c::confirm(input.confirm) {
        Confirmation::Yes => insert_new(store, &key, &fields)
            .await
            .map(AddOutcome::added),
        Confirmation::NotConfirmed => Ok(AddOutcome::prompt(key, fields, copied.cloned())),
        Confirmation::Invalid => Err(ServiceError::Rule(AddFailure::invalid_confirmation())),
    }
}

/// `VALIDATE-INPUT-KEY-FIELDS`: the key edit, then the cross-reference read it names, then the
/// fill-in of the other key (COTRN02C-009 to COTRN02C-019).
///
/// The account path reads every cross-reference of the account, because the `CXACAIX` path is
/// `NONUNIQUEKEY`: one row is `NORMAL`, none is `NOTFND`, and two or more are the `DUPKEY` that
/// the program answers as a failed read (COTRN02C-012 to COTRN02C-014). It therefore uses
/// `find_all_by_acct_id`, not `find_by_acct_id`, which returns the first row and hides that
/// `DUPKEY`. The card path reads by the primary key (COTRN02C-017 to COTRN02C-019).
async fn fill(store: &mut PgStore, keys: Keys<'_>) -> Result<FilledKey, ServiceError<AddFailure>> {
    let request = cotrn02c::edit_keys(keys.acct_id, keys.card_num).map_err(ServiceError::Rule)?;
    let (paragraph, read): (_, Result<Vec<CardXrefRecord>, PersistenceError>) = match &request {
        KeyRequest::Account(acct_id) => (
            READ_CXACAIX,
            CardXrefRepository::find_all_by_acct_id(store, *acct_id).await,
        ),
        KeyRequest::Card(card_num) => (
            READ_CCXREF,
            CardXrefRepository::find_by_id(store, card_num)
                .await
                .map(|found| found.into_iter().collect()),
        ),
    };
    let lookup = match read.as_deref() {
        Ok([xref]) => XrefLookup::Found(xref),
        Ok([]) => XrefLookup::NotFound,
        Ok([_, _, ..]) => {
            region_log(paragraph, &DUPKEY);
            XrefLookup::Error
        }
        Err(error) => {
            region_log(paragraph, error);
            XrefLookup::Error
        }
    };
    cotrn02c::fill_key(request, lookup).map_err(ServiceError::Rule)
}

/// `COPY-LAST-TRAN-DATA`'s browse and copy: the data fields of the transaction with the highest
/// ID in the whole table, whatever its card (COTRN02C-053 to COTRN02C-055, X-COTRN02C-3).
///
/// An empty table answers `Transaction ID NOT found...`, the replacement's choice for halt
/// candidate X-COTRN02C-H1 and no parity result; a failed read answers `Unable to lookup
/// Transaction...` (COTRN02C-053).
async fn copy_last_fields(store: &mut PgStore) -> Result<CopiedFields, ServiceError<AddFailure>> {
    match TransactionRepository::find_last(store).await {
        Ok(Some(last)) => Ok(cotrn02c::copy_last(&last)),
        Ok(None) => Err(ServiceError::Rule(AddFailure::last_not_found())),
        Err(error) => {
            region_log(BROWSE_TRANSACT, &error);
            Err(ServiceError::Rule(AddFailure::transaction_lookup()))
        }
    }
}

/// `ADD-TRANSACTION` and `WRITE-TRANSACT-FILE` in one transaction: the highest-ID browse, the
/// record, the insert and the commit; the new ID on success (COTRN02C-043 to COTRN02C-051).
///
/// A failed browse answers `Unable to lookup Transaction...` (COTRN02C-044); an insert that
/// finds the ID taken answers `Tran ID already exist...` (COTRN02C-050), any other failed insert
/// `Unable to Add Transaction...` (COTRN02C-051). Each rolls the transaction back first, so
/// nothing is written.
async fn insert_new(
    store: &PgStore,
    key: &FilledKey,
    fields: &ValidatedTran,
) -> Result<PicX<16>, ServiceError<AddFailure>> {
    let mut tx = store.begin().await?;
    let (new_id, filler) = match browse_highest(&mut tx).await {
        Ok(browsed) => browsed,
        Err(error) => {
            region_log(BROWSE_TRANSACT, &error);
            tx.rollback().await?;
            return Err(ServiceError::Rule(AddFailure::transaction_lookup()));
        }
    };
    let record = cotrn02c::build_record(new_id.clone(), key, fields, filler);
    match TransactionRepository::insert(&mut tx, &record).await {
        Ok(()) => {
            tx.commit().await?;
            Ok(new_id)
        }
        Err(error) => {
            // The `DUPKEY`/`DUPREC` branch displays nothing; `WHEN OTHER` displays the codes.
            let failure = if error.is_duplicate() {
                AddFailure::duplicate_id()
            } else {
                region_log(WRITE_TRANSACT, &error);
                AddFailure::write_failed()
            };
            tx.rollback().await?;
            Err(ServiceError::Rule(failure))
        }
    }
}

/// The `STARTBR` at `HIGH-VALUES`, `READPREV` and `ENDBR` of `ADD-TRANSACTION`, inside the add's
/// transaction: the next ID, the highest stored ID plus one (`TransactionRepository::next_id`,
/// COTRN02C-043), and the FILLER of the highest-ID transaction, which `INITIALIZE TRAN-RECORD`
/// leaves in the record (COTRN02C-045, PIC-012).
///
/// On an empty table the ID is 1 and the FILLER spaces: the end-of-file branch of halt candidate
/// X-COTRN02C-H1, no parity result (COTRN02C-044).
async fn browse_highest(tx: &mut PgTx<'_>) -> Result<(PicX<16>, PicX<20>), PersistenceError> {
    let filler = TransactionRepository::find_last(tx)
        .await?
        .map_or_else(PicX::spaces, |last| last.filler);
    let new_id = TransactionRepository::next_id(tx).await?;
    Ok((new_id, filler))
}

/// The `DISPLAY 'RESP:' … 'REAS:' …` of a file statement's `WHEN OTHER` branch, which goes to
/// the region log and never to the response (COTRN02C-014, COTRN02C-019, COTRN02C-044,
/// COTRN02C-051).
///
/// `condition` is a store error, which renders only its SQLSTATE and constraint name, or the
/// CICS condition the replacement detects itself; no key or field is recorded.
fn region_log(paragraph: &'static str, condition: &dyn fmt::Display) {
    tracing::warn!(paragraph, condition = %condition, "COTRN02C file statement failed");
}
