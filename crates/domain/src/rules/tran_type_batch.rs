//! MNTTRDB2 transaction type maintenance: `COBTUPDT`
//! [app/app-transaction-type-db2/cbl/COBTUPDT.cbl], run by `IKJEFT01` as
//! `RUN PROGRAM(COBTUPDT) PLAN(CARDDEMO)` over the `INPFILE` dataset
//! [app/app-transaction-type-db2/jcl/MNTTRDB2.jcl:L21-30].
//!
//! Each 53-byte input record names an action, a type code and a description
//! (P-COBTUPDT-001 to P-COBTUPDT-003). `1003-TREAT-RECORD` inserts (`A`),
//! updates (`U`) or deletes (`D`) the Db2 master `CARDDEMO.TRANSACTION_TYPE`,
//! skips a comment (`*`) and rejects any other action. A rejected record or a
//! failed statement performs `9999-ABEND`, which only displays the message and
//! sets `RETURN-CODE` 4: the run goes on with the next record, and the job ends
//! with 4 when any record failed (P-COBTUPDT-010).
//!
//! Only the Db2 master changes, table `transaction_types`. Reports read the
//! processing copy `trantype_file`, which sees these changes only after
//! `batch seed --only tranextr` (TRANEXTR).
//!
//! The batch adapter `crates/batch/src/jobs/mnttrdb2.rs` owns the file and the
//! job: `0001-OPEN-FILES` and its failure (P-COBTUPDT-004, P-COBTUPDT-011,
//! E-36), the read loop of `1001-READ-NEXT-RECORDS`, `2001-CLOSE-STOP`
//! (P-COBTUPDT-012), the commit at `STOP RUN` and the job log. It hands every
//! record to this module:
//!
//! 1. [`parse_record`]: `READ TR-RECORD NEXT RECORD INTO WS-INPUT-REC`;
//! 2. [`MaintRecord::processing_message`]: the `1002-READ-RECORDS` display;
//! 3. [`apply`]: `1003-TREAT-RECORD` and the paragraph it performs, giving a
//!    [`RecordOutcome`] whose [`RecordOutcome::display_text`] is the success
//!    display;
//! 4. [`MaintRun::record`]: `9999-ABEND`, which returns the failure display,
//!    and [`MaintRun::return_code`], the job's return code.
//!
//! The `ADDING RECORD`, `UPDATING RECORD` and `DELETING RECORD` displays of
//! `1003-TREAT-RECORD` are not audited messages, so the catalog holds no text
//! for them and this module shows none.

use crate::entities::TranTypeMaster;
use crate::messages::{
    ERROR_TYPE_NOT_VALID, IGNORING_COMMENTED_LINE, NO_RECORDS_FOUND, RECORD_DELETED_SUCCESSFULLY,
    RECORD_INSERTED_SUCCESSFULLY, RECORD_UPDATED_SUCCESSFULLY, UNEXPECTED_ERROR,
    cobtupdt_return_msg, processing_record_message, tran_type_batch_sql_message,
};
use crate::numeric::{Alnum, TranTypeCode};
use crate::ports::{PortResult, TranTypeMasterStore};

/// The length of `WS-INPUT-REC`, the `RECORDING MODE F` record of `INPFILE`:
/// `INPUT-REC-TYPE X(1)`, `INPUT-REC-NUMBER X(2)` and `INPUT-REC-DESC X(50)`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L71-77].
pub const INPUT_REC_LEN: usize = 53;

/// `MOVE 4 TO RETURN-CODE` in `9999-ABEND`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L232].
const ABEND_RETURN_CODE: u8 = 4;

/// The return code of a run in which no record failed.
const NORMAL_RETURN_CODE: u8 = 0;

/// `SQLCODE = +100`, the no-row condition `10032-UPDATE-DB` and
/// `10033-DELETE-DB` test for.
const SQLCODE_NOT_FOUND: i32 = 100;

/// One `INPFILE` record as `WS-INPUT-REC` holds it after the `READ … INTO`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L71-77].
///
/// The fields keep the stored bytes, trailing spaces included, with the types
/// section A of the parity audit gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaintRecord {
    /// `INPUT-REC-TYPE X(1)`, offset 0 (P-COBTUPDT-001): `A`, `U`, `D` or `*`,
    /// in upper case, for a valid record.
    pub kind: Alnum<1>,
    /// `INPUT-REC-NUMBER X(2)`, offset 1 (P-COBTUPDT-002): the `TR_TYPE` the
    /// statement names.
    pub code: TranTypeCode,
    /// `INPUT-REC-DESC X(50)`, offset 3 (P-COBTUPDT-003): the
    /// `TR_DESCRIPTION` an insert or update writes.
    pub description: Alnum<50>,
}

impl MaintRecord {
    /// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-005.
    ///
    /// The `DISPLAY 'PROCESSING   ' WS-INPUT-REC` the paragraph shows for every
    /// record read [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L104-106]:
    /// the literal and the 53 record bytes, as
    /// [`processing_record_message`] composes them.
    #[must_use]
    pub fn processing_message(&self) -> String {
        let bytes = [
            self.kind.as_bytes().as_slice(),
            self.code.as_bytes(),
            self.description.as_bytes(),
        ]
        .concat();
        let record = Alnum::<INPUT_REC_LEN>::move_from_bytes(&bytes);
        // The record goes in without its trailing spaces, which the composer
        // restores when it pads to 53 bytes: a byte above 0x7F reads as one
        // Latin-1 character but two UTF-8 bytes, and the composer's cut at 53
        // UTF-8 bytes would otherwise drop the end of the record.
        processing_record_message(&record.trimmed())
    }
}

/// COBTUPDT 1002-READ-RECORDS — P-COBTUPDT-001, P-COBTUPDT-002,
/// P-COBTUPDT-003, P-COBTUPDT-005.
///
/// `READ TR-RECORD NEXT RECORD INTO WS-INPUT-REC`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L101]: the record is moved
/// into the 53-byte `WS-INPUT-REC` as an alphanumeric `MOVE`, so a shorter
/// line is padded with spaces and the group is then split at offsets 0, 1 and
/// 3. `line` is one record without its line terminator. The adapter frames the
/// fixed 53-byte records and stops a run on a longer one, so the cut a `MOVE`
/// makes past byte 53 is never reached in a job.
#[must_use]
pub fn parse_record(line: &[u8]) -> MaintRecord {
    let [kind, code_high, code_low, description @ ..] =
        *Alnum::<INPUT_REC_LEN>::move_from_bytes(line).as_bytes();
    MaintRecord {
        kind: Alnum::from_array([kind]),
        code: TranTypeCode::from_alnum(Alnum::from_array([code_high, code_low])),
        description: Alnum::from_array(description),
    }
}

/// What one record did, in the terms of the paragraph that handled it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// `10031-INSERT-DB`, `SQLCODE` 0: the type was added (P-COBTUPDT-007).
    Inserted,
    /// `10032-UPDATE-DB`, `SQLCODE` 0: the description was replaced
    /// (P-COBTUPDT-008).
    Updated,
    /// `10033-DELETE-DB`, `SQLCODE` 0: the type was removed (P-COBTUPDT-009).
    Deleted,
    /// `*` in `INPUT-REC-TYPE`: a comment, which changes nothing
    /// (P-COBTUPDT-006).
    Ignored,
    /// A statement ended with a positive `SQLCODE` its `EVALUATE TRUE` has no
    /// `WHEN` for: a warning on `10031-INSERT-DB`, which tests only 0 and
    /// below [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L151-163].
    /// Nothing is displayed and the return code is unchanged.
    Unreported,
    /// `9999-ABEND` was performed: `message` is the text the `STRING` wrote
    /// into `WS-RETURN-MSG` (P-COBTUPDT-006 to P-COBTUPDT-010).
    /// [`MaintRun::record`] gives the line `9999-ABEND` displays.
    Failed {
        /// The text written, without the tail an earlier message may leave in
        /// the receiver.
        message: String,
    },
}

impl RecordOutcome {
    /// COBTUPDT 1003-TREAT-RECORD, 10031-INSERT-DB, 10032-UPDATE-DB,
    /// 10033-DELETE-DB — P-COBTUPDT-006, P-COBTUPDT-007, P-COBTUPDT-008,
    /// P-COBTUPDT-009.
    ///
    /// The text the paragraph displays for an outcome that performs no
    /// `9999-ABEND`: `RECORD INSERTED SUCCESSFULLY`, `RECORD UPDATED
    /// SUCCESSFULLY`, `RECORD DELETED SUCCESSFULLY` or `IGNORING COMMENTED
    /// LINE`. `None` for [`RecordOutcome::Unreported`], which displays
    /// nothing, and for [`RecordOutcome::Failed`], whose display
    /// [`MaintRun::record`] returns.
    ///
    /// Each text is the `DISPLAY` of one branch:
    ///
    /// - [`RecordOutcome::Inserted`]: `WHEN SQLCODE = ZERO` of
    ///   `10031-INSERT-DB`
    ///   [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L152-153];
    /// - [`RecordOutcome::Updated`]: `WHEN SQLCODE = ZERO` of
    ///   `10032-UPDATE-DB`
    ///   [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L178-179];
    /// - [`RecordOutcome::Deleted`]: `WHEN SQLCODE = ZERO` of
    ///   `10033-DELETE-DB`
    ///   [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L208-209];
    /// - [`RecordOutcome::Ignored`]: `WHEN '*'` of `1003-TREAT-RECORD`
    ///   [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L120-121].
    #[must_use]
    pub const fn display_text(&self) -> Option<&'static str> {
        match self {
            Self::Inserted => Some(RECORD_INSERTED_SUCCESSFULLY),
            Self::Updated => Some(RECORD_UPDATED_SUCCESSFULLY),
            Self::Deleted => Some(RECORD_DELETED_SUCCESSFULLY),
            Self::Ignored => Some(IGNORING_COMMENTED_LINE),
            Self::Unreported | Self::Failed { .. } => None,
        }
    }

    /// Whether `9999-ABEND` was performed for the record.
    #[must_use]
    pub const fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// COBTUPDT 1003-TREAT-RECORD — P-COBTUPDT-006.
///
/// `EVALUATE INPUT-REC-TYPE`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L109-130]: `A` performs
/// `10031-INSERT-DB`, `U` `10032-UPDATE-DB`, `D` `10033-DELETE-DB`, `*` is a
/// comment and any other value, a lower-case letter or a space included, fails
/// with [`ERROR_TYPE_NOT_VALID`]. Every statement is one write the adapter runs
/// under its own savepoint, so a failed statement changes nothing and leaves
/// the earlier ones in place (P-COBTUPDT-007).
///
/// The returned future is `Send` whenever the store is.
pub async fn apply<S: TranTypeMasterStore>(store: &mut S, record: &MaintRecord) -> RecordOutcome {
    match record.kind.as_bytes() {
        b"A" => insert_db(store, record).await,
        b"U" => update_db(store, record).await,
        b"D" => delete_db(store, record.code).await,
        b"*" => RecordOutcome::Ignored,
        _ => failed(ERROR_TYPE_NOT_VALID),
    }
}

/// COBTUPDT 10031-INSERT-DB — P-COBTUPDT-007.
///
/// `INSERT INTO CARDDEMO.TRANSACTION_TYPE (TR_TYPE, TR_DESCRIPTION) VALUES
/// (:INPUT-REC-NUMBER, :INPUT-REC-DESC)`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L132-164].
async fn insert_db<S: TranTypeMasterStore>(store: &mut S, record: &MaintRecord) -> RecordOutcome {
    match master(record) {
        Ok(row) => Statement::Insert.evaluate(store.insert_tran_type(row).await),
        Err(outcome) => outcome,
    }
}

/// COBTUPDT 10032-UPDATE-DB — P-COBTUPDT-008.
///
/// `UPDATE CARDDEMO.TRANSACTION_TYPE SET TR_DESCRIPTION = :INPUT-REC-DESC
/// WHERE TR_TYPE = :INPUT-REC-NUMBER`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L166-195].
async fn update_db<S: TranTypeMasterStore>(store: &mut S, record: &MaintRecord) -> RecordOutcome {
    match master(record) {
        Ok(row) => Statement::Update.evaluate(store.update_tran_type(row).await),
        Err(outcome) => outcome,
    }
}

/// COBTUPDT 10033-DELETE-DB — P-COBTUPDT-009.
///
/// `DELETE FROM CARDDEMO.TRANSACTION_TYPE WHERE TR_TYPE = :INPUT-REC-NUMBER`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L196-226]. A type that
/// categories still reference fails with `SQLCODE` -532.
async fn delete_db<S: TranTypeMasterStore>(store: &mut S, code: TranTypeCode) -> RecordOutcome {
    Statement::Delete.evaluate(store.delete_tran_type(code).await)
}

/// COBTUPDT 10031-INSERT-DB, 10032-UPDATE-DB — P-COBTUPDT-007,
/// P-COBTUPDT-008.
///
/// The row the statement writes: `TR_TYPE` from `INPUT-REC-NUMBER` and
/// `TR_DESCRIPTION` from `INPUT-REC-DESC`.
///
/// `:INPUT-REC-DESC` is a fixed `X(50)` host variable, so Db2 receives all 50
/// input bytes. `transaction_types` keeps every `X(n)` text right-trimmed (its
/// `CHECK` rejects trailing spaces), and the `X(50)` item rebuilt from that
/// text holds the same 50 bytes, so the right-trimmed text is the stored form
/// of the 50 bytes and the one written.
///
/// An `X(50)` item always yields at most 50 Latin-1 characters, which
/// [`TranTypeMaster::try_new`] accepts; its error is still answered as a
/// failed statement rather than a panic.
fn master(record: &MaintRecord) -> Result<TranTypeMaster, RecordOutcome> {
    TranTypeMaster::try_new(record.code, record.description.trimmed().into_owned())
        .map_err(|_| failed(UNEXPECTED_ERROR))
}

/// The three statements of COBTUPDT, each followed by its own
/// `EVALUATE TRUE` of the `SQLCODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Statement {
    /// `10031-INSERT-DB`.
    Insert,
    /// `10032-UPDATE-DB`.
    Update,
    /// `10033-DELETE-DB`.
    Delete,
}

impl Statement {
    /// The outcome of `WHEN SQLCODE = ZERO`.
    const fn done(self) -> RecordOutcome {
        match self {
            Self::Insert => RecordOutcome::Inserted,
            Self::Update => RecordOutcome::Updated,
            Self::Delete => RecordOutcome::Deleted,
        }
    }

    /// Whether the `EVALUATE` has a `WHEN SQLCODE = +100`: the update and
    /// delete do, the insert does not.
    const fn tests_not_found(self) -> bool {
        matches!(self, Self::Update | Self::Delete)
    }

    /// COBTUPDT 10031-INSERT-DB, 10032-UPDATE-DB, 10033-DELETE-DB —
    /// P-COBTUPDT-007, P-COBTUPDT-008, P-COBTUPDT-009.
    ///
    /// `MOVE SQLCODE TO WS-VAR-SQLCODE` and the `EVALUATE TRUE` that follows
    /// the statement:
    ///
    /// - 0: the success outcome;
    /// - +100 on the update or delete: [`NO_RECORDS_FOUND`] and `9999-ABEND`;
    /// - below 0: [`tran_type_batch_sql_message`] with the code edited
    ///   `----9` (-803 a duplicate type, -532 a type categories reference,
    ///   -911 a lock, E-30) and `9999-ABEND`;
    /// - any other positive code: no `WHEN` applies;
    /// - no Db2 code at all
    ///   ([`PortError::Unavailable`](crate::ports::PortError::Unavailable)):
    ///   [`UNEXPECTED_ERROR`] and `9999-ABEND`.
    fn evaluate(self, result: PortResult<()>) -> RecordOutcome {
        let Err(error) = result else {
            return self.done();
        };
        match error.sqlcode() {
            Some(SQLCODE_NOT_FOUND) if self.tests_not_found() => failed(NO_RECORDS_FOUND),
            Some(sqlcode) if sqlcode < 0 => failed(tran_type_batch_sql_message(sqlcode)),
            Some(_) => RecordOutcome::Unreported,
            // A store failure with no mapped Db2 condition is a failed
            // statement, `SQLCODE < 0`, so 9999-ABEND runs. Its code slot would
            // carry the PostgreSQL SQLSTATE (E-30), which the port does not
            // report, so the unexpected-error text stands in for a guessed code.
            None => failed(UNEXPECTED_ERROR),
        }
    }
}

/// A record for which `9999-ABEND` is performed with `message`.
fn failed(message: impl Into<String>) -> RecordOutcome {
    RecordOutcome::Failed {
        message: message.into(),
    }
}

/// The state a COBTUPDT run carries from record to record: `RETURN-CODE` and
/// the `WS-RETURN-MSG PIC X(80)` receiver
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L60-62].
///
/// A run starts with return code 0 and a receiver of spaces. The adapter
/// passes every [`RecordOutcome`] to [`MaintRun::record`], in input order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaintRun {
    /// Whether `9999-ABEND` has set `RETURN-CODE` 4. No later paragraph
    /// resets it.
    failed: bool,
    /// The content of `WS-RETURN-MSG`; empty stands for its initial spaces.
    return_msg: String,
}

impl MaintRun {
    /// COBTUPDT 9999-ABEND — P-COBTUPDT-010.
    ///
    /// Adds `outcome` to the run. For a [`RecordOutcome::Failed`] record it
    /// performs `9999-ABEND`
    /// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L230-233]: the
    /// message is laid over `WS-RETURN-MSG`, which is never cleared, so the
    /// receiver past a shorter message keeps the tail of a longer earlier one
    /// ([`cobtupdt_return_msg`]); `RETURN-CODE` becomes 4; and the 80 bytes of
    /// the receiver, trailing spaces included, are returned as the line
    /// `DISPLAY WS-RETURN-MSG` shows. Any other outcome changes nothing and
    /// returns `None`.
    pub fn record(&mut self, outcome: &RecordOutcome) -> Option<&str> {
        let RecordOutcome::Failed { message } = outcome else {
            return None;
        };
        self.failed = true;
        self.return_msg = cobtupdt_return_msg(&self.return_msg, message);
        Some(&self.return_msg)
    }

    /// COBTUPDT 9999-ABEND, 2001-CLOSE-STOP — P-COBTUPDT-010, P-COBTUPDT-012.
    ///
    /// The return code `STOP RUN` ends the job with: 4 when any record
    /// recorded so far failed, else 0. A later success does not reset it.
    #[must_use]
    pub const fn return_code(&self) -> u8 {
        if self.failed {
            ABEND_RETURN_CODE
        } else {
            NORMAL_RETURN_CODE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{INPUT_REC_LEN, MaintRun, RecordOutcome, apply, parse_record};
    use crate::entities::{TranCategoryMaster, TranTypeMaster};
    use crate::messages::{
        ERROR_TYPE_NOT_VALID, IGNORING_COMMENTED_LINE, NO_RECORDS_FOUND,
        RECORD_DELETED_SUCCESSFULLY, RECORD_INSERTED_SUCCESSFULLY, RECORD_UPDATED_SUCCESSFULLY,
        UNEXPECTED_ERROR,
    };
    use crate::numeric::{Alnum, TranCategoryCode, TranTypeCode};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The `WS-RETURN-MSG PIC X(80)` width `9999-ABEND` displays.
    const RETURN_MSG_LEN: usize = 80;

    fn code(text: &str) -> TranTypeCode {
        TranTypeCode::move_from(text)
    }

    fn master(type_code: &str, description: &str) -> TranTypeMaster {
        TranTypeMaster {
            tr_type: code(type_code),
            tr_description: description.to_owned(),
        }
    }

    /// The Db2 masters with types 01 and 02 as `DB2LTTYP` loads them, and one
    /// category of type 01, which makes a delete of 01 restricted.
    fn store() -> FakeStore {
        FakeStore::default()
            .with_tran_type(master("01", "PURCHASE"))
            .with_tran_type(master("02", "PAYMENT"))
            .with_tran_category(TranCategoryMaster {
                trc_type_code: code("01"),
                trc_type_category: TranCategoryCode::new(1).expect("a 4-digit category code"),
                trc_cat_data: Alnum::move_from("REGULAR SALES DRAFT"),
            })
    }

    /// The masters as (code, description) text pairs, in key order.
    fn masters(store: &FakeStore) -> Vec<(String, String)> {
        store
            .tran_types
            .values()
            .map(|row| {
                (
                    row.tr_type.trimmed().into_owned(),
                    row.tr_description.clone(),
                )
            })
            .collect()
    }

    fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|&(type_code, description)| (type_code.to_owned(), description.to_owned()))
            .collect()
    }

    /// What a run of `lines` gives: the outcome and the `9999-ABEND` display
    /// of each record, in input order, and the run after the last one.
    struct Replay {
        outcomes: Vec<RecordOutcome>,
        displays: Vec<Option<String>>,
        run: MaintRun,
    }

    /// Runs `lines` as COBTUPDT does: each record read, applied to `store` and
    /// added to one run, in order. Each line is padded to 53 bytes, as the
    /// fixed records of `INPFILE` are.
    fn run(store: &mut FakeStore, lines: &[&str]) -> Replay {
        let mut run = MaintRun::default();
        let mut outcomes = Vec::new();
        let mut displays = Vec::new();
        for text in lines {
            let record = parse_record(format!("{text:<53}").as_bytes());
            let outcome = block_on(apply(store, &record));
            displays.push(run.record(&outcome).map(str::to_owned));
            outcomes.push(outcome);
        }
        Replay {
            outcomes,
            displays,
            run,
        }
    }

    fn failed(message: &str) -> RecordOutcome {
        RecordOutcome::Failed {
            message: message.to_owned(),
        }
    }

    /// `message` as `DISPLAY WS-RETURN-MSG` shows it on a receiver that held
    /// only spaces before.
    fn displayed(message: &str) -> String {
        format!("{message:<RETURN_MSG_LEN$}")
    }

    // P-COBTUPDT-001, P-COBTUPDT-002, P-COBTUPDT-003
    #[test]
    fn parse_record_moves_the_line_into_ws_input_rec_and_splits_it() {
        let full = format!("{:<53}", "U06REVERSAL");
        let long = format!("A12{}", "X".repeat(60));
        let cases = [
            (
                b"A08BALANCE TRANSFER".as_slice(),
                b"A",
                b"08",
                format!("{:<50}", "BALANCE TRANSFER"),
            ),
            (full.as_bytes(), b"U", b"06", format!("{:<50}", "REVERSAL")),
            (b"D09".as_slice(), b"D", b"09", " ".repeat(50)),
            (b"*".as_slice(), b"*", b"  ", " ".repeat(50)),
            (b"".as_slice(), b" ", b"  ", " ".repeat(50)),
            (long.as_bytes(), b"A", b"12", "X".repeat(50)),
        ];
        for (line, kind, type_code, description) in cases {
            let record = parse_record(line);
            assert_eq!(record.kind.as_bytes(), kind, "{line:?}");
            assert_eq!(record.code.as_bytes(), type_code, "{line:?}");
            assert_eq!(
                record.description.as_bytes().as_slice(),
                description.as_bytes(),
                "{line:?}"
            );
        }
    }

    // P-COBTUPDT-005
    #[test]
    fn processing_message_shows_the_53_record_bytes() {
        let expected = format!("PROCESSING   {:<53}", "A08BALANCE TRANSFER");
        let padded = format!("{:<53}", "A08BALANCE TRANSFER");
        for line in [b"A08BALANCE TRANSFER".as_slice(), padded.as_bytes()] {
            let message = parse_record(line).processing_message();
            assert_eq!(message, expected, "{line:?}");
            assert_eq!(message.len(), 13 + INPUT_REC_LEN);
        }
        let latin1 = parse_record(b"A10CAF\xC9 AU LAIT").processing_message();
        assert!(
            latin1.starts_with("PROCESSING   A10CAF\u{c9} AU LAIT "),
            "{latin1:?}"
        );
    }

    // P-COBTUPDT-006, P-COBTUPDT-007, P-COBTUPDT-008, P-COBTUPDT-009
    #[test]
    fn records_add_update_and_delete_the_master() {
        let mut store = store();
        let result = run(
            &mut store,
            &["A08BALANCE TRANSFER", "U02BILL PAYMENT", "A09ANNUAL FEE"],
        );
        assert_eq!(
            result.outcomes,
            [
                RecordOutcome::Inserted,
                RecordOutcome::Updated,
                RecordOutcome::Inserted
            ]
        );
        assert_eq!(
            masters(&store),
            pairs(&[
                ("01", "PURCHASE"),
                ("02", "BILL PAYMENT"),
                ("08", "BALANCE TRANSFER"),
                ("09", "ANNUAL FEE"),
            ])
        );

        let deleted = run(&mut store, &["D09", "D02"]);
        assert_eq!(
            deleted.outcomes,
            [RecordOutcome::Deleted, RecordOutcome::Deleted]
        );
        assert_eq!(
            masters(&store),
            pairs(&[("01", "PURCHASE"), ("08", "BALANCE TRANSFER")])
        );
        assert_eq!(
            store.calls,
            [
                "insert_tran_type",
                "update_tran_type",
                "insert_tran_type",
                "delete_tran_type",
                "delete_tran_type",
            ]
        );
        for outcome in [result, deleted] {
            assert!(outcome.displays.iter().all(Option::is_none));
            assert_eq!(outcome.run.return_code(), 0);
        }
        assert!(
            store.tran_type_file.is_empty(),
            "the processing copy is untouched"
        );
    }

    // P-COBTUPDT-006, P-COBTUPDT-007, P-COBTUPDT-008, P-COBTUPDT-009
    #[test]
    fn display_texts_are_the_paragraph_displays() {
        let cases = [
            (
                RecordOutcome::Inserted,
                Some(RECORD_INSERTED_SUCCESSFULLY),
                false,
            ),
            (
                RecordOutcome::Updated,
                Some(RECORD_UPDATED_SUCCESSFULLY),
                false,
            ),
            (
                RecordOutcome::Deleted,
                Some(RECORD_DELETED_SUCCESSFULLY),
                false,
            ),
            (RecordOutcome::Ignored, Some(IGNORING_COMMENTED_LINE), false),
            (RecordOutcome::Unreported, None, false),
            (failed(NO_RECORDS_FOUND), None, true),
        ];
        for (outcome, text, is_failed) in cases {
            assert_eq!(outcome.display_text(), text, "{outcome:?}");
            assert_eq!(outcome.is_failed(), is_failed, "{outcome:?}");
        }
    }

    // P-COBTUPDT-003, P-COBTUPDT-007, P-COBTUPDT-008
    #[test]
    fn the_description_is_stored_as_the_right_trimmed_50_input_bytes() {
        let mut store = store();
        let lines = ["A10  LEADING AND INNER  SPACES", "A11", "U02 "];
        let result = run(&mut store, &lines);
        assert!(result.outcomes.iter().all(|outcome| !outcome.is_failed()));
        assert_eq!(
            masters(&store),
            pairs(&[
                ("01", "PURCHASE"),
                ("02", ""),
                ("10", "  LEADING AND INNER  SPACES"),
                ("11", ""),
            ])
        );
        for (text, stored) in lines.iter().zip(["10", "11", "02"]) {
            let record = parse_record(text.as_bytes());
            let row = &store.tran_types[&code(stored)];
            assert_eq!(
                Alnum::<50>::from_trimmed(&row.tr_description),
                Ok(record.description),
                "the stored text restores the 50 input bytes of {text:?}"
            );
        }
    }

    // P-COBTUPDT-006
    #[test]
    fn a_commented_line_changes_nothing() {
        let mut store = store();
        let before = store.clone();
        let result = run(&mut store, &["* COBTUPDT INPFILE: COL 1 TYPE", "*01"]);
        assert_eq!(
            result.outcomes,
            [RecordOutcome::Ignored, RecordOutcome::Ignored]
        );
        assert_eq!(result.displays, [None, None]);
        assert_eq!(result.run.return_code(), 0);
        assert_eq!(store, before, "no statement was issued");
    }

    // P-COBTUPDT-006, P-COBTUPDT-010
    #[test]
    fn an_invalid_type_fails_and_later_records_are_still_applied() {
        for kind in ["u", "a", " ", "X", "\0"] {
            let mut store = store();
            let result = run(&mut store, &[&format!("{kind}03CREDIT"), "A09ANNUAL FEE"]);
            assert_eq!(
                result.outcomes,
                [failed(ERROR_TYPE_NOT_VALID), RecordOutcome::Inserted],
                "{kind:?}"
            );
            assert_eq!(
                result.displays,
                [Some(displayed(ERROR_TYPE_NOT_VALID)), None],
                "{kind:?}"
            );
            assert_eq!(result.run.return_code(), 4, "{kind:?}");
            assert_eq!(store.calls, ["insert_tran_type"], "{kind:?}");
            assert!(store.tran_types.contains_key(&code("09")), "{kind:?}");
        }
    }

    // P-COBTUPDT-008, P-COBTUPDT-009, P-COBTUPDT-010
    #[test]
    fn an_update_or_delete_of_a_missing_type_finds_no_records() {
        for line in ["U99UNKNOWN TRANSACTION TYPE", "D99"] {
            let mut store = store();
            let before = masters(&store);
            let result = run(&mut store, &[line]);
            assert_eq!(result.outcomes, [failed(NO_RECORDS_FOUND)], "{line}");
            assert_eq!(
                result.displays,
                [Some(displayed(NO_RECORDS_FOUND))],
                "{line}"
            );
            assert_eq!(result.run.return_code(), 4, "{line}");
            assert_eq!(masters(&store), before, "{line}");
        }
    }

    // P-COBTUPDT-007, P-COBTUPDT-008, P-COBTUPDT-009 (E-30)
    #[test]
    fn a_negative_sqlcode_is_edited_into_the_access_message() {
        let cases = [
            ("A01PURCHASE DUPLICATE KEY", None, " -803"),
            ("D01", None, " -532"),
            (
                "U02PAYMENT",
                Some(("update_tran_type", PortError::Contention)),
                " -911",
            ),
            (
                "D02",
                Some(("delete_tran_type", PortError::Contention)),
                " -911",
            ),
        ];
        for (line, injected, edited) in cases {
            let mut store = store();
            if let Some((method, error)) = injected {
                store.fail_next(method, error);
            }
            let before = masters(&store);
            let result = run(&mut store, &[line]);
            let message = format!("Error accessing: TRANSACTION_TYPE table. SQLCODE:{edited}");
            assert_eq!(result.outcomes, [failed(&message)], "{line}");
            assert_eq!(result.displays, [Some(displayed(&message))], "{line}");
            assert_eq!(result.run.return_code(), 4, "{line}");
            assert_eq!(
                masters(&store),
                before,
                "a failed statement changes nothing"
            );
        }
    }

    #[test]
    fn a_store_failure_without_a_db2_code_is_an_unexpected_error() {
        for (line, method) in [
            ("A10NEW", "insert_tran_type"),
            ("U01NEW", "update_tran_type"),
            ("D02", "delete_tran_type"),
        ] {
            let mut store = store();
            store.fail_next(
                method,
                PortError::Unavailable("sqlstate 08006: connection lost".to_owned()),
            );
            let result = run(&mut store, &[line]);
            assert_eq!(result.outcomes, [failed(UNEXPECTED_ERROR)], "{line}");
            assert_eq!(result.run.return_code(), 4, "{line}");
        }
    }

    // P-COBTUPDT-007: the insert's EVALUATE tests only 0 and below.
    #[test]
    fn a_positive_sqlcode_on_the_insert_is_not_reported() {
        let mut store = store();
        store.fail_next("insert_tran_type", PortError::NotFound);
        let result = run(&mut store, &["A10NEW"]);
        assert_eq!(result.outcomes, [RecordOutcome::Unreported]);
        assert_eq!(result.displays, [None]);
        assert_eq!(result.run.return_code(), 0);
    }

    // P-COBTUPDT-010
    #[test]
    fn the_return_message_keeps_the_tail_of_a_longer_earlier_one() {
        let mut store = store();
        let result = run(
            &mut store,
            &["A01DUPLICATE", "U99UNKNOWN", "A10NEW", "x10BAD"],
        );
        let access = "Error accessing: TRANSACTION_TYPE table. SQLCODE: -803";
        let overlaid = "No records found.TRANSACTION_TYPE table. SQLCODE: -803";
        let overlaid_again = "ERROR: TYPE NOT VALIDSACTION_TYPE table. SQLCODE: -803";
        assert_eq!(
            result.displays,
            [
                Some(displayed(access)),
                Some(displayed(overlaid)),
                None,
                Some(displayed(overlaid_again)),
            ]
        );
        assert_eq!(result.outcomes[2], RecordOutcome::Inserted);
        assert_eq!(
            result.run.return_code(),
            4,
            "a later success does not reset it"
        );
    }

    #[test]
    fn a_new_run_ends_normally() {
        assert_eq!(MaintRun::default().return_code(), 0);
        assert_eq!(
            MaintRun::default().record(&RecordOutcome::Inserted),
            None,
            "only a failure performs 9999-ABEND"
        );
    }

    #[test]
    fn the_rule_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = store();
        let record = parse_record(b"A10NEW");
        let future = apply(&mut store, &record);
        assert_send(&future);
        assert_eq!(block_on(future), RecordOutcome::Inserted);
    }
}
