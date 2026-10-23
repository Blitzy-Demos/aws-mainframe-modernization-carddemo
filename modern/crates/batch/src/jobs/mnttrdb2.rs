//! MNTTRDB2, the batch maintenance of the Db2 transaction types: `carddemo-batch mnttrdb2
//! --input <file>` drives the rules of COBTUPDT (`domain::batch::cobtupdt`) against
//! `db2.transaction_type` (AAP 0.2.3, 0.3.4.3). The audit is `modern/audit/batch/MNTTRDB2.md`.
//!
//! # Run
//!
//! 1. The `--input` file is read and decoded in full before the database is reached. A line that
//!    is no 53-byte record fails the job with [`JobError::Input`]: no record is processed and no
//!    table changes (COBTUPDT-003, COBTUPDT-004).
//! 2. One transaction holds the run, the single unit of work of a program that issues no `COMMIT`
//!    (COBTUPDT-024). Records are planned and executed one at a time, in file order
//!    (COBTUPDT-006, COBTUPDT-007): `A` inserts, `U` updates and `D` deletes; `*` and any other
//!    action byte run no statement (COBTUPDT-008, COBTUPDT-009).
//! 3. Each statement's result goes back to the domain as a [`SqlOutcome`]: `Ok` for a completed
//!    statement, `NotFound` for an update or delete that changed no row (COBTUPDT-013,
//!    COBTUPDT-016), and `Failed` for any statement error, the duplicate insert (COBTUPDT-011)
//!    and the delete refused by the `ON DELETE RESTRICT` of `TRNTYCAT.ddl:6-7` (COBTUPDT-017)
//!    included. A failed statement is first reported in one `ERROR` event (see Job log). The run
//!    goes on with the next record after every failure (COBTUPDT-022).
//! 4. One commit after the last record; the exit code is then the domain's `RETURN-CODE`, 4 once
//!    any record reached `9999-ABEND` and 0 otherwise (COBTUPDT-023, COBTUPDT-025).
//!
//! The message overlay, the return code and the absence of any field validation are the domain's
//! and are reproduced, never corrected, here (Behaviour constraint).
//!
//! # Savepoints
//!
//! A failed Db2 statement changes nothing and leaves the unit of work open, whereas a failed
//! `PostgreSQL` statement aborts the whole transaction. Each statement therefore runs inside a
//! savepoint of its own, released when the statement completes and rolled back to when it fails,
//! so the earlier records' changes stand and the later records still run (COBTUPDT-024). Two
//! failures are not answered that way:
//!
//! - A deadlock or a lock timeout, SQLSTATE `40P01` or `55P03`, stands for Db2 `SQLCODE` -911,
//!   which rolls back the whole unit of work: the run's transaction is rolled back, every earlier
//!   change of the run with it, and a new transaction holds the later records (COBTUPDT-024).
//! - A lost connection, a savepoint or rollback the server refuses, and a failed commit end the
//!   run at once with [`JobError::Persistence`] and nothing committed, because a transaction whose
//!   connection is gone can be neither continued nor committed. The audit raises this case as a
//!   halt candidate (`X-MNTTRDB2-H2`) and claims no parity for it.
//!
//! # Job log
//!
//! Every domain step returns the `DISPLAY` lines COBTUPDT writes. This job discards them: the
//! batch job contract reports only the files a job wrote, and this job writes none, so it prints
//! nothing (AAP 0.3.4.3). The lines are proved where the domain builds them; the five `SQLCODE`
//! positions their error text ends with are halted under H3 (COBTUPDT-020).
//!
//! The tracing stream is no job log. Besides `main`'s events, it carries one `ERROR` event,
//! `COBTUPDT database call failed`, for each `A`, `U` or `D` statement that failed and was
//! answered with [`SqlOutcome::Failed`]: its fields are `operation`, the COBTUPDT paragraph
//! (`10031-INSERT-DB`, `10032-UPDATE-DB` or `10033-DELETE-DB`), `error`, the
//! [`PersistenceError`] text, and `unit_of_work_rolled_back`, true for `SQLCODE` -911. So a run
//! that ends with return code 4 shows why each statement failed. A record that reached
//! `9999-ABEND` through an invalid action byte or a `+100` had no statement error and adds no
//! event.
//!
//! # Data version (E23)
//!
//! Only `db2.transaction_type` is written, through [`Db2TransactionTypeRepository`];
//! `db2.transaction_type_category` is reached only through the delete restriction. The VSAM data
//! version in `transaction_types`, which `tranrept` reads, is never touched, so a maintenance
//! change never reaches the report lookups (COBTUPDT-026, COBTUPDT-027).
//!
//! The job does no schema work (Single Schema Executor). It runs against the schema and the Db2
//! seed that migration 0006 gave the database once; against a database without them each `A`,
//! `U` and `D` record fails its statement and the run exits 4 (COBTUPDT-026).
//!
//! # Redaction
//!
//! A description is unvalidated free text and may hold a protected value, so no line, record or
//! description reaches an error, a counter or a tracing event: [`JobError`] carries an SQLSTATE
//! and a constraint name at most, the failed-statement event a static paragraph name, the
//! [`PersistenceError`] text and a flag, never the type code or the description, and the
//! counters are plain counts (Cardholder Data Redaction).

use std::path::Path;

use domain::batch::cobtupdt::{Maintenance, MaintenanceAction, SqlOutcome, decode_input};
use persistence::{Db2TransactionTypeRepository, PersistenceError, PgTx};
use runtime::telemetry::JobCounter;
use tracing::callsite::{Callsite as _, DefaultCallsite};
use tracing::field::{self, Field, Value};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use super::{JobContext, JobError, JobOutcome};

/// The SQLSTATEs standing for Db2 `SQLCODE` -911, after which Db2 has rolled back the unit of
/// work: `deadlock_detected` and `lock_not_available` (COBTUPDT-024).
const UNIT_OF_WORK_ROLLED_BACK: [&str; 2] = ["40P01", "55P03"];

// The COBTUPDT paragraph each statement replaces, named in the event that reports its failure.

/// The `INSERT` of an `A` record (COBTUPDT-010).
const INSERT_DB_OP: &str = "10031-INSERT-DB";
/// The `UPDATE` of a `U` record (COBTUPDT-012).
const UPDATE_DB_OP: &str = "10032-UPDATE-DB";
/// The `DELETE` of a `D` record (COBTUPDT-015).
const DELETE_DB_OP: &str = "10033-DELETE-DB";

/// How one statement ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Statement {
    /// The `SQLCODE` class the domain's `EVALUATE` tests (COBTUPDT-010 to COBTUPDT-019).
    outcome: SqlOutcome,
    /// Whether the failure stands for `SQLCODE` -911, so that the run's transaction must be
    /// rolled back and replaced (COBTUPDT-024).
    unit_of_work_rolled_back: bool,
}

/// What a failed statement means for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// A negative `SQLCODE`: the statement changed nothing and the run goes on (COBTUPDT-019).
    Statement,
    /// `SQLCODE` -911: the statement failed and the whole unit of work is rolled back
    /// (COBTUPDT-024).
    UnitOfWork,
    /// The database cannot be reached: the run cannot go on.
    Connection,
}

/// Runs MNTTRDB2 over the records of the file at `input` and reports COBTUPDT's return code
/// (COBTUPDT-023). It writes no file, so [`JobOutcome::written`] is empty; its counters are
/// `records`, the records decoded, and `failed`, those that reached `9999-ABEND`
/// (COBTUPDT-022).
///
/// # Errors
///
/// - [`JobError::Io`] when the file cannot be read, and [`JobError::Input`] when a line of it is
///   no COBTUPDT record; the database is not reached (COBTUPDT-004).
/// - [`JobError::Persistence`] when the run's transaction cannot be opened, a savepoint cannot be
///   opened, released or rolled back, the connection is lost, or the final commit fails. Nothing
///   is committed then.
///
/// A failed `A`, `U` or `D` statement is no error: it is reported to the domain, which sets
/// return code 4, and the run goes on (COBTUPDT-022).
pub(crate) async fn run(ctx: &JobContext<'_>, input: &Path) -> Result<JobOutcome, JobError> {
    // The read and the decoding block, so they run as one step on the blocking pool, which
    // finishes before the database is reached. Its first failure ends the run there: an
    // unreadable file as `JobError::Io`, else the first line that is no record as
    // `JobError::Input` (COBTUPDT-004).
    let input = input.to_path_buf();
    let records = super::blocking(super::READING_INPUT, move || {
        decode_input(&super::read_input(&input)?).map_err(JobError::from)
    })
    .await??;
    let mut maintenance = Maintenance::new();
    let mut failed = 0_usize;
    let mut tx = ctx.store.begin().await?;
    for record in &records {
        // The job log is discarded here and below: `JobOutcome` carries no job log and a job that
        // writes no file prints nothing, although `MNTTRDB2.md` sends COBTUPDT's `DISPLAY` lines
        // to standard output; the domain's tests prove those lines.
        let (_job_log, action) = maintenance.plan(record);
        let reached_abend = match execute(&mut tx, &action).await? {
            Some(statement) => {
                if statement.unit_of_work_rolled_back {
                    tx.rollback().await?;
                    tx = ctx.store.begin().await?;
                }
                let _job_log = maintenance.complete(&action, statement.outcome);
                statement.outcome != SqlOutcome::Ok
            }
            // No statement ran: a comment changes nothing, while an invalid action byte has
            // already reached `9999-ABEND` in the plan (COBTUPDT-008, COBTUPDT-009).
            None => matches!(action, MaintenanceAction::InvalidType),
        };
        if reached_abend {
            failed += 1;
        }
    }
    tx.commit().await?;
    Ok(JobOutcome {
        written: Vec::new(),
        return_code: maintenance.return_code(),
        counts: vec![
            (JobCounter::Records, super::count(records.len())),
            (JobCounter::Failed, super::count(failed)),
        ],
    })
}

/// Executes the statement `action` names inside a savepoint of `tx`: the insert of
/// `10031-INSERT-DB`, the update of `10032-UPDATE-DB` or the delete of `10033-DELETE-DB`
/// (COBTUPDT-010, COBTUPDT-012, COBTUPDT-015). `None` for a comment or an invalid action byte,
/// which run no statement.
///
/// # Errors
///
/// [`JobError::Persistence`] when the savepoint cannot be opened, released or rolled back, or the
/// statement lost the connection (see [`settle`]).
async fn execute(
    tx: &mut PgTx<'_>,
    action: &MaintenanceAction,
) -> Result<Option<Statement>, JobError> {
    let (operation, savepoint, result) = match action {
        MaintenanceAction::Insert(row) => {
            let mut savepoint = tx.savepoint().await?;
            let result = Db2TransactionTypeRepository::insert(&mut savepoint, row)
                .await
                .map(|()| SqlOutcome::Ok);
            (INSERT_DB_OP, savepoint, result)
        }
        MaintenanceAction::Update(row) => {
            let mut savepoint = tx.savepoint().await?;
            let result = Db2TransactionTypeRepository::update(&mut savepoint, row)
                .await
                .map(row_found);
            (UPDATE_DB_OP, savepoint, result)
        }
        MaintenanceAction::Delete(code) => {
            let mut savepoint = tx.savepoint().await?;
            let result = Db2TransactionTypeRepository::delete(&mut savepoint, code)
                .await
                .map(row_found);
            (DELETE_DB_OP, savepoint, result)
        }
        MaintenanceAction::Ignore | MaintenanceAction::InvalidType => return Ok(None),
    };
    settle(operation, savepoint, result).await.map(Some)
}

/// The outcome of an update or a delete: `SQLCODE` 0 when it changed a row, `+100` when no row
/// had the code (COBTUPDT-013, COBTUPDT-016).
const fn row_found(changed: bool) -> SqlOutcome {
    if changed {
        SqlOutcome::Ok
    } else {
        SqlOutcome::NotFound
    }
}

/// Ends the savepoint the statement of paragraph `operation` ran in and names how the statement
/// ended (COBTUPDT-024).
///
/// A statement that completed, whether or not it changed a row, keeps its work in the run's
/// transaction and its savepoint is released. A failed statement is first reported in one
/// `ERROR` event (see [`report`]), then undone by rolling back to its savepoint, and is returned
/// as [`SqlOutcome::Failed`], flagged when it stands for `SQLCODE` -911. The event comes before
/// the rollback, so the statement's cause is on record even when the rollback fails too.
///
/// # Errors
///
/// [`JobError::Persistence`] when the savepoint cannot be released or rolled back, or when the
/// statement failed because the database cannot be reached; the run's transaction is then lost.
/// A lost connection raises no event here: the error itself ends the job, and `main` reports it.
async fn settle(
    operation: &'static str,
    savepoint: PgTx<'_>,
    result: Result<SqlOutcome, PersistenceError>,
) -> Result<Statement, JobError> {
    match result {
        Ok(outcome) => {
            savepoint.commit().await?;
            Ok(Statement {
                outcome,
                unit_of_work_rolled_back: false,
            })
        }
        Err(error) => {
            let unit_of_work_rolled_back = match classify(&error) {
                Failure::Statement => false,
                Failure::UnitOfWork => true,
                // Not a failed statement: with the connection gone there is no transaction to go
                // on with or to commit, so the run ends here with nothing committed, the
                // replacement's answer to the audit's halt candidate rather than a parity result.
                Failure::Connection => return Err(JobError::Persistence(error)),
            };
            report(operation, &error, unit_of_work_rolled_back);
            savepoint.rollback().await?;
            Ok(Statement {
                outcome: SqlOutcome::Failed,
                unit_of_work_rolled_back,
            })
        }
    }
}

/// The message of the event [`report`] emits for a failed statement, and the name of its
/// callsite.
const STATEMENT_FAILED: &str = "COBTUPDT database call failed";

/// The fields of the [`STATEMENT_FAILED`] event, in the order [`report`] gives their values.
const STATEMENT_FAILED_FIELDS: &[&str] =
    &["message", "operation", "error", "unit_of_work_rolled_back"];

// Written out: `tracing`'s `error!` expands to an `allow(unused_imports)` lint suppression.
/// The `ERROR` callsite of the [`STATEMENT_FAILED`] event.
static STATEMENT_FAILED_SITE: DefaultCallsite = DefaultCallsite::new(&STATEMENT_FAILED_METADATA);

/// The metadata of [`STATEMENT_FAILED_SITE`].
static STATEMENT_FAILED_METADATA: Metadata<'static> = tracing::metadata! {
    name: STATEMENT_FAILED,
    target: module_path!(),
    level: Level::ERROR,
    fields: STATEMENT_FAILED_FIELDS,
    callsite: &STATEMENT_FAILED_SITE,
    kind: Kind::EVENT,
};

/// Reports why the statement of paragraph `operation` failed, for the operator: the paragraph,
/// the error's text, which names an SQLSTATE, a constraint or a column only, and whether the
/// failure stands for `SQLCODE` -911, so that the run's unit of work is rolled back. No
/// description, type code, record or job-log line reaches it (Cardholder Data Redaction).
///
/// The report is one `ERROR` event in the current span, with target
/// `carddemo_batch::jobs::mnttrdb2` and, in this order, the fields `message`
/// ([`STATEMENT_FAILED`]), `operation`, `error` and `unit_of_work_rolled_back`. It decides
/// through `runtime::telemetry::event_enabled` and dispatches as `tracing`'s event macros do, so
/// the event is the line a macro would write.
fn report(operation: &'static str, error: &PersistenceError, unit_of_work_rolled_back: bool) {
    if !runtime::telemetry::event_enabled(&STATEMENT_FAILED_SITE) {
        return;
    }
    let metadata = STATEMENT_FAILED_SITE.metadata();
    let fields = metadata.fields();
    // The callsite declares every name of `STATEMENT_FAILED_FIELDS`, so each lookup finds its
    // field.
    let (Some(message_field), Some(operation_field), Some(error_field), Some(rolled_back_field)) = (
        fields.field("message"),
        fields.field("operation"),
        fields.field("error"),
        fields.field("unit_of_work_rolled_back"),
    ) else {
        return;
    };
    // The message and the error are recorded through their `Display`, as the macro records its
    // formatted message and a `%` field.
    let message = field::display(STATEMENT_FAILED);
    let error = field::display(error);
    let values: [(&Field, Option<&dyn Value>); 4] = [
        (&message_field, Some(&message)),
        (&operation_field, Some(&operation)),
        (&error_field, Some(&error)),
        (&rolled_back_field, Some(&unit_of_work_rolled_back)),
    ];
    Event::dispatch(metadata, &fields.value_set(&values));
}

/// What the failure of an `A`, `U` or `D` statement stands for, by the `SQLCODE` mapping of
/// `MNTTRDB2.md`: a duplicate key (`-803`, COBTUPDT-011), a restricted delete (`-532`,
/// COBTUPDT-017) and every other statement error are a negative `SQLCODE` (COBTUPDT-014,
/// COBTUPDT-019); a deadlock or lock timeout is `-911` (COBTUPDT-024); a lost connection ends
/// the run.
fn classify(error: &PersistenceError) -> Failure {
    match error {
        PersistenceError::Unavailable => Failure::Connection,
        PersistenceError::Database {
            sqlstate: Some(sqlstate),
            ..
        } if UNIT_OF_WORK_ROLLED_BACK.contains(&sqlstate.as_str()) => Failure::UnitOfWork,
        PersistenceError::NotFound
        | PersistenceError::Duplicate { .. }
        | PersistenceError::Restrict { .. }
        | PersistenceError::Check { .. }
        | PersistenceError::Database { .. }
        | PersistenceError::Decode { .. } => Failure::Statement,
    }
}

#[cfg(test)]
mod tests {
    use domain::batch::cobtupdt::SqlOutcome;
    use persistence::{DecodeKind, PersistenceError};
    use test_support::trace_capture;

    use super::{DELETE_DB_OP, Failure, INSERT_DB_OP, UPDATE_DB_OP, classify, report, row_found};

    /// A server error with SQLSTATE `code` and no constraint.
    fn database(code: &str) -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some(code.to_owned()),
            constraint: None,
        }
    }

    /// Every class of repository failure, by the `SQLCODE` it stands for.
    #[test]
    fn failures_follow_the_sqlcode_mapping() {
        let cases = [
            // audit: COBTUPDT-011
            (
                PersistenceError::Duplicate {
                    constraint: Some("xtran_type".to_owned()),
                },
                Failure::Statement,
            ),
            // audit: COBTUPDT-017
            (
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
                Failure::Statement,
            ),
            // audit: COBTUPDT-019
            (database("42P01"), Failure::Statement),
            (database("40001"), Failure::Statement),
            (
                PersistenceError::Database {
                    sqlstate: None,
                    constraint: None,
                },
                Failure::Statement,
            ),
            (
                PersistenceError::Check { constraint: None },
                Failure::Statement,
            ),
            (PersistenceError::NotFound, Failure::Statement),
            (
                PersistenceError::decode("tr_type", DecodeKind::InvalidByte),
                Failure::Statement,
            ),
            // audit: COBTUPDT-024
            (database("40P01"), Failure::UnitOfWork),
            (database("55P03"), Failure::UnitOfWork),
            (PersistenceError::Unavailable, Failure::Connection),
        ];
        for (error, expected) in cases {
            assert_eq!(classify(&error), expected, "{error}");
        }
    }

    /// An update or delete that changed a row is `SQLCODE` 0, one that changed none `+100`.
    #[test]
    fn changed_rows_decide_the_outcome() {
        // audit: COBTUPDT-012
        assert_eq!(row_found(true), SqlOutcome::Ok);
        // audit: COBTUPDT-013
        assert_eq!(row_found(false), SqlOutcome::NotFound);
    }

    /// A statement failure the run goes on after is reported in one `ERROR` event: the paragraph,
    /// the error's SQLSTATE or constraint text and whether it stands for `SQLCODE` -911, by the
    /// classification `settle` reports it under.
    #[test]
    fn a_failed_statement_is_reported_with_its_paragraph_and_cause() {
        let cases = [
            (
                INSERT_DB_OP,
                PersistenceError::Duplicate {
                    constraint: Some("xtran_type".to_owned()),
                },
                "duplicate key (constraint xtran_type)",
                Failure::Statement,
            ),
            (
                DELETE_DB_OP,
                PersistenceError::Restrict {
                    constraint: Some("trc_type_code".to_owned()),
                },
                "restricted by foreign key (constraint trc_type_code)",
                Failure::Statement,
            ),
            (
                UPDATE_DB_OP,
                database("42P01"),
                "database error (SQLSTATE 42P01)",
                Failure::Statement,
            ),
            (
                UPDATE_DB_OP,
                database("40P01"),
                "database error (SQLSTATE 40P01)",
                Failure::UnitOfWork,
            ),
            (
                DELETE_DB_OP,
                database("55P03"),
                "database error (SQLSTATE 55P03)",
                Failure::UnitOfWork,
            ),
        ];
        for (operation, error, text, failure) in cases {
            assert_eq!(classify(&error), failure, "{error}");
            let rolled_back = failure == Failure::UnitOfWork;
            let capture = trace_capture::capture();
            report(operation, &error, rolled_back);

            let lines = capture.lines();
            let [event] = lines.as_slice() else {
                panic!("expected one event for {error}, got {lines:?}");
            };
            for expected in [
                r#""level":"ERROR""#.to_owned(),
                r#""message":"COBTUPDT database call failed""#.to_owned(),
                format!(r#""operation":"{operation}""#),
                format!(r#""error":"{text}""#),
                format!(r#""unit_of_work_rolled_back":{rolled_back}"#),
            ] {
                assert!(event.contains(&expected), "{expected} missing from {event}");
            }
        }
    }
}
