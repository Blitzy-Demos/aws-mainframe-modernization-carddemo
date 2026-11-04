//! `batch migrate`: the schema step of CREADB21 [app/app-transaction-type-db2/jcl/CREADB21.jcl].
//!
//! CREADB21 creates the Db2 objects in its `CRCRDDB` step, `DSNTIAD` over the `DB2CREAT`
//! statements [app/app-transaction-type-db2/jcl/CREADB21.jcl:L52-59], then loads the transaction
//! type and category tables through `DSNTEP4` with the `DB2LTTYP` and `DB2LTCAT` statements
//! [app/app-transaction-type-db2/jcl/CREADB21.jcl:L64-84]. This job replaces only the schema
//! step: it applies the migrations of `migrations/` that `carddemo_persistence::migrate` embeds,
//! which also define the tables the IDCAMS `DEFINE` steps of the VSAM file jobs create. The rows
//! are loaded by the first `batch seed`, never here. The `FREEPLN` step, which frees Db2 plans
//! and packages [app/app-transaction-type-db2/jcl/CREADB21.jcl:L36-46], has no counterpart.
//!
//! The job is idempotent: on a migrated database every version is already recorded, nothing
//! changes, and the job ends with exit 0 again.

use carddemo_persistence::migrate;
use carddemo_persistence::tx::Db;

use super::{JobContext, setup_failure};
use crate::exit::ExitStatus;

/// Applies every migration the database of `db` has not applied yet, in version order, and
/// returns how the job ended.
///
/// [`ExitStatus::Success`] (exit 0) once the schema is current, whether this run applied
/// versions or found them all applied. A migration failure has no COBOL counterpart: it is
/// logged and ends the job as a setup failure (exit 16) reporting the migrator's error text on
/// one stderr line. The versions applied before a failing one stay applied, because each runs
/// in its own database transaction.
///
/// The job sets no count, so the job span holds `read`, `posted` and `rejected` as 0 whatever
/// the outcome: the schema step reads, loads and turns away no record.
///
/// `_ctx` is unused, because the schema depends on no setting, clock or generation and the job
/// counts nothing. The parameter keeps the `run(ctx, &db)` shape [`super::dispatch`] calls every
/// job with.
pub(crate) async fn run(_ctx: &JobContext, db: &Db) -> ExitStatus {
    match migrate::run(db).await {
        Ok(()) => {
            tracing::info!(job = "migrate", "migrations applied");
            ExitStatus::Success
        }
        Err(err) => setup_failure("schema migration failed", err),
    }
}
