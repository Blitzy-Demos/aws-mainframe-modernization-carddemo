//! `dusrsecj`: the `DUSRSECJ` job, which builds the `USRSEC` user-security file from the
//! in-stream records of its own deck (`app/jcl/DUSRSECJ.jcl`; `modern/audit/batch/LOADS.md`,
//! LOADS-009).
//!
//! | JCL step | Program | What it does | Here |
//! |---|---|---|---|
//! | `PREDEL` | `IEFBR14` | deletes the staging dataset `USRSEC.PS` | nothing: no staging file exists (LOADS-035) |
//! | `STEP01` | `IEBGENER` | copies the `SYSUT1 DD *` lines 35-44 to the 80-byte `USRSEC.PS` | `decode_usrsec` over the deck read in place (LOADS-011, LOADS-012) |
//! | `STEP02` | `IDCAMS` | deletes and defines the KSDS, `KEYS(8,0)`, `RECORDSIZE(80,80)`, `REUSE` | the delete of the replacement; the table comes from the migrations (LOADS-014, LOADS-036) |
//! | `STEP03` | `IDCAMS` | `REPRO` of the staged records into the KSDS | the inserts of the replacement (LOADS-009, LOADS-014) |
//!
//! The whole job is one replacement of table `users` inside one `PostgreSQL` transaction: every
//! row is deleted, then every in-stream record is inserted, so once it commits the table holds
//! exactly the ten users of the deck (LOADS-012, LOADS-014). An empty table before the run is no
//! error (LOADS-015). An in-stream section that holds no record replaces `users` with no rows,
//! the same delete-then-insert as every other load; whether such a deck should fail instead is
//! candidate X-LOADS-5 of `LOADS.md`, which awaits the reviewer, and no fixture reaches it.
//!
//! # Failures leave the previous users
//!
//! An unreadable deck (LOADS-019), a deck codec A or the in-stream extraction refuses
//! (LOADS-013, LOADS-016), a key out of sequence (LOADS-017) and a failed hash all end the job
//! before the transaction opens. A failed delete, insert or commit drops the uncommitted
//! transaction, which rolls it back (LOADS-018). Either way `users` keeps its previous rows.
//!
//! # Passwords (E25)
//!
//! Codec B has no column for `SEC-USR-PWD`: each record's 8 password bytes are hashed with
//! argon2id by the domain's `hash_user`, and `users.password_hash` stores only that PHC string,
//! so codec B is not byte-identical for that one field (LOADS-033, LOADS-034, E25). This module
//! never reaches the plaintext: the records go from the decoder to the hasher, and the hashes
//! from the hasher to the repository. A hashing failure becomes [`JobError::Password`], which
//! carries no detail, and the only count reported is the number of rows written, so no password
//! or hash reaches the database in clear, an error, a tracing event or standard output
//! (LOADS-039, Cardholder Data Redaction).
//!
//! # No schema work and no output file
//!
//! The job applies no schema (Single Schema Executor): the `users` table and its key come from
//! the migrations `carddemo-migrate` applies, and against an unmigrated database the job fails
//! at its first statement with [`JobError::Persistence`] (LOADS-021). It writes no file, so
//! [`JobOutcome::written`] is empty and `main` prints no path line (LOADS-038).

use std::path::Path;

use domain::batch::loads::{HashedUser, decode_usrsec, hash_user};
use persistence::UserRepository;

use super::{JobContext, JobError, JobOutcome};

/// Replaces `users` with the in-stream records of the `DUSRSECJ` deck at `input`, each stored
/// with the argon2id hash of its `SEC-USR-PWD` in place of the password (LOADS-009, LOADS-012,
/// LOADS-014, LOADS-033).
///
/// The deck is read in place and never copied (LOADS-011). Every record is decoded and hashed
/// before the transaction opens, so the transaction holds only the delete and the inserts.
/// The read, the decoding and the argon2id hashes block, so they run as one step on the
/// blocking pool, awaited before the transaction opens; their errors are those of an inline
/// run, in the same order (W011-F05). The outcome reports the number of rows written as
/// `records`, writes no file and returns 0 (LOADS-038).
///
/// # Errors
///
/// - [`JobError::Io`] when the deck cannot be read (LOADS-019).
/// - [`JobError::Load`] when the deck has no `//SYSUT1   DD *` statement or no closing `/*`
///   line, an in-stream card codec A rejects, or a `SEC-USR-ID` out of sequence (LOADS-013,
///   LOADS-016, LOADS-017).
/// - [`JobError::Password`] when argon2id cannot hash a password (LOADS-033).
/// - [`JobError::Persistence`] when the transaction cannot be opened, the delete or an insert
///   fails, or the commit is refused; nothing is committed then (LOADS-018, LOADS-021).
///
/// Every failure leaves the previous contents of `users`.
pub(crate) async fn run(ctx: &JobContext<'_>, input: &Path) -> Result<JobOutcome, JobError> {
    let input = input.to_path_buf();
    let users = super::blocking(super::READING_INPUT, move || {
        let deck = super::read_input(&input)?;
        // LineFraming describes the deck line and is never stored: only the record goes on.
        decode_usrsec(&deck)?
            .into_iter()
            .map(|framed| hash_user(framed.record).map_err(|_| JobError::Password))
            .collect::<Result<Vec<HashedUser>, JobError>>()
    })
    .await??;

    // Dropping `tx` on an early return rolls the replacement back (LOADS-018).
    let mut tx = ctx.store.begin().await?;
    let written = tx.replace_all(&users).await?;
    tx.commit().await?;

    Ok(JobOutcome {
        written: Vec::new(),
        return_code: 0,
        counts: vec![("records", written)],
    })
}
