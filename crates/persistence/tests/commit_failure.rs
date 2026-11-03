//! A `COMMIT` the server fails, against a migrated `PostgreSQL` 16.15
//! (`carddemo_persistence::tx`, `UnitOfWork::commit`, and
//! `carddemo_persistence::seed::run`; AAP 0.6.4).
//!
//! CP00 writes the pending-authorization summary and detail in a unit of work
//! of its own after the request is acknowledged, and logs `I003` when its
//! `COMMIT` fails (AAP 0.3.3). A `COMMIT` that fails on the server has already
//! rolled the transaction back, so a `ROLLBACK` sent after it draws only the
//! warning `25P01` "there is no transaction in progress", which sqlx logs as a
//! `sqlx::postgres::notice` event. A seed run, `batch seed`, commits its own
//! transaction the same way.
//!
//! The case installs a deferrable constraint trigger, initially deferred, that
//! raises for the details of one account, so the summary and detail inserts
//! succeed and the `COMMIT` fails. It asserts that the commit reports the
//! trigger's error, nothing of that unit of work persisted and the pool's only
//! connection serves a later unit of work that commits. A second such trigger
//! on the processing copy of the transaction types then fails the `COMMIT` of
//! a `TRANEXTR` seed run, which must report the trigger's error and persist
//! nothing, and once it is dropped the same connection serves a seed run that
//! commits. Last, no session is left in a transaction, and the server sent no
//! notice at all. A `ROLLBACK` sent outside any transaction then shows that
//! the capture sees that notice.
//!
//! Notices are captured by a subscriber installed as the process-wide
//! default, because sqlx flushes a connection it takes back on a task of the
//! shared test runtime, on another thread. This file holds that one test, so
//! no other test's events reach it. The test starts its own container through
//! `carddemo_testkit::TestStack`, so it needs a running Docker daemon
//! (AAP 0.10.4). The trigger and the committed state go through the stack's
//! text-typed SQL helpers, so the test adds no compile-time query.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use carddemo_domain::entities::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, CardNumber, CustomerId, Money, NumericCode,
};
use carddemo_domain::ports::AuthorizationStore;
use carddemo_persistence::seed::{self, SeedMode, SeedReport, SeedSources};
use carddemo_persistence::{Db, PersistenceError};
use carddemo_testkit::{TestStack, block_on, fixtures};
use sqlx::postgres::PgPoolOptions;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// The account whose detail the deferred trigger refuses at `COMMIT`.
const REFUSED: u64 = 201;

/// The account the later unit of work writes.
const LATER: u64 = 202;

/// A constraint trigger that runs at `COMMIT` and raises for every detail of
/// account 201, [`REFUSED`].
const DEFERRED_TRIGGER: &str = "\
    CREATE FUNCTION refuse_detail_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
    BEGIN RAISE EXCEPTION 'detail refused at commit'; END $$; \
    CREATE CONSTRAINT TRIGGER refuse_detail_at_commit \
    AFTER INSERT ON pending_auth_details DEFERRABLE INITIALLY DEFERRED \
    FOR EACH ROW WHEN (NEW.pa_acct_id = 201) \
    EXECUTE FUNCTION refuse_detail_at_commit()";

/// One Db2 master type for the `TRANEXTR` copy, and a constraint trigger
/// that runs at `COMMIT` and raises for every processing-copy type it inserts.
const DEFERRED_COPY_TRIGGER: &str = "\
    INSERT INTO transaction_types (tr_type, tr_description) VALUES ('01', 'PURCHASE'); \
    CREATE FUNCTION refuse_type_copy_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
    BEGIN RAISE EXCEPTION 'type copy refused at commit'; END $$; \
    CREATE CONSTRAINT TRIGGER refuse_type_copy_at_commit \
    AFTER INSERT ON trantype_file DEFERRABLE INITIALLY DEFERRED \
    FOR EACH ROW EXECUTE FUNCTION refuse_type_copy_at_commit()";

/// Removes the trigger of [`DEFERRED_COPY_TRIGGER`].
const DROP_DEFERRED_COPY_TRIGGER: &str = "\
    DROP TRIGGER refuse_type_copy_at_commit ON trantype_file; \
    DROP FUNCTION refuse_type_copy_at_commit()";

/// The SQLSTATE of a `RAISE EXCEPTION` that names none.
const RAISE_EXCEPTION: &str = "P0001";

/// The target sqlx logs every server notice under.
const NOTICE_TARGET: &str = "sqlx::postgres::notice";

/// The warning `PostgreSQL` sends for a `ROLLBACK` outside any transaction.
const NO_TRANSACTION: &str = "there is no transaction in progress";

/// One event the capture kept.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Captured {
    target: String,
    level: Level,
    message: String,
}

/// A subscriber that keeps every `WARN` and `ERROR` event, from any thread,
/// and enables no span.
#[derive(Debug, Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<Captured>>>,
    /// The last span ID handed out; spans are disabled, so none is in
    /// practice.
    spans: Arc<AtomicU64>,
}

impl Capture {
    /// Installs a capture as the process-wide default subscriber.
    fn install() -> Self {
        let capture = Self::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("no other subscriber is the process default");
        capture
    }

    fn events(&self) -> MutexGuard<'_, Vec<Captured>> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The server notices sqlx logged so far.
    fn notices(&self) -> Vec<Captured> {
        self.events()
            .iter()
            .filter(|event| event.target == NOTICE_TARGET)
            .cloned()
            .collect()
    }
}

impl Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // A `tracing::enabled!` probe, as sqlx makes before each notice, has
        // neither the event nor the span kind, so only spans are refused.
        !metadata.is_span() && *metadata.level() <= Level::WARN
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::WARN)
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.spans.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut message = Message::default();
        event.record(&mut message);
        let metadata = event.metadata();
        self.events().push(Captured {
            target: metadata.target().to_owned(),
            level: *metadata.level(),
            message: message.0,
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// The `message` field of an event.
#[derive(Debug, Default)]
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            value.clone_into(&mut self.0);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn cents<const D: u8>(value: i64) -> Money<D> {
    Money::from_cents(value).expect("an amount within the field")
}

/// The summary of `account` with one approved authorization.
fn summary(account_id: u64) -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: account(account_id),
        pa_cust_id: CustomerId::new(1).expect("a customer number"),
        pa_auth_status: Alnum::move_from("A"),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: cents(2_020_000),
        pa_cash_limit: cents(1_020_000),
        pa_credit_balance: cents(3_000),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::new(1).expect("a count of at most four digits"),
        pa_declined_auth_cnt: AuthCount::new(0).expect("a count of at most four digits"),
        pa_approved_auth_amt: cents(3_000),
        pa_declined_auth_amt: Money::ZERO,
    }
}

/// The approved detail of `account`.
fn detail(account_id: u64) -> NewPendingAuthDetail {
    NewPendingAuthDetail {
        pa_acct_id: account(account_id),
        pa_auth_date_9c: 99_999 - 22_182,
        pa_auth_time_9c: 1,
        pa_auth_orig_date: Alnum::move_from("220701"),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from("4111000000000001"),
        pa_auth_type: Alnum::move_from("SALE"),
        pa_card_expiry_date: Alnum::move_from("0525"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from("00"),
        pa_auth_resp_reason: Alnum::move_from("0000"),
        pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
        pa_transaction_amt: cents(3_000),
        pa_approved_amt: cents(3_000),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
        pa_merchant_id: Alnum::move_from("800000000"),
        pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
        pa_merchant_city: Alnum::move_from("Enoshaven"),
        pa_merchant_state: Alnum::move_from("NC"),
        pa_merchant_zip: Alnum::move_from("72112"),
        pa_transaction_id: Alnum::move_from("000000000000001"),
        pa_match_status: MatchStatus::Pending,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// Writes the summary and then the detail of `account` in one unit of work,
/// as CP00 does, and returns the outcome of its commit.
async fn write_and_commit(db: &Db, account_id: u64) -> Result<(), PersistenceError> {
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    store
        .insert_summary(summary(account_id))
        .await
        .expect("the summary is inserted");
    store
        .insert_detail(detail(account_id))
        .await
        .expect("the detail is inserted, its trigger deferred to the commit");
    uow.commit().await
}

/// Copies the Db2 master types over the processing copy in one seed run, as
/// `batch seed --only tranextr` does, and returns the run's outcome.
async fn copy_masters(db: &Db) -> Result<SeedReport, PersistenceError> {
    let sources = SeedSources::with_defaults(&fixtures::repo_root());
    seed::run(db, seed::prepare(SeedMode::OnlyTranextr, &sources)).await
}

/// Asserts that `outcome` is the error of a trigger's `RAISE EXCEPTION`.
fn assert_raised<T: fmt::Debug>(outcome: &Result<T, PersistenceError>, what: &str) {
    assert!(
        matches!(
            outcome,
            Err(PersistenceError::Sqlx(sqlx::Error::Database(error)))
                if error.code().as_deref() == Some(RAISE_EXCEPTION)
        ),
        "{what} did not fail with the trigger's error: {outcome:?}"
    );
}

/// The first column of the first row `sql` reads, as text.
async fn text(stack: &TestStack, sql: &str) -> Option<String> {
    stack
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .flatten()
}

/// The second column of the row of `account` among the rows `sql` reads,
/// whose first column is the account number, or `0` without such a row.
async fn of_account(stack: &TestStack, sql: &str, account_id: u64) -> String {
    let key = account_id.to_string();
    stack
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .find(|row| row.first().and_then(Option::as_deref) == Some(key.as_str()))
        .and_then(|row| row.into_iter().nth(1))
        .flatten()
        .unwrap_or_else(|| "0".to_owned())
}

/// How many summaries and details of `account` are committed.
async fn committed(stack: &TestStack, account_id: u64) -> (String, String) {
    (
        of_account(
            stack,
            "SELECT pa_acct_id, count(*) FROM pending_auth_summaries GROUP BY pa_acct_id",
            account_id,
        )
        .await,
        of_account(
            stack,
            "SELECT pa_acct_id, count(*) FROM pending_auth_details GROUP BY pa_acct_id",
            account_id,
        )
        .await,
    )
}

#[test]
fn a_commit_the_server_fails_sends_no_rollback_and_frees_its_connection() {
    let capture = Capture::install();
    block_on(async {
        let stack = TestStack::postgres().await;
        stack.exec_sql(DEFERRED_TRIGGER).await;
        // One connection: each later unit of work or seed run waits until
        // sqlx has taken back, and flushed, the connection a failed commit
        // used.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(stack.database_url())
            .await
            .expect("the pool connects");
        let db = Db::from_pool(pool);

        assert_raised(&write_and_commit(&db, REFUSED).await, "the commit");
        write_and_commit(&db, LATER)
            .await
            .expect("the pool's connection serves a later unit of work that commits");

        assert_eq!(
            committed(&stack, REFUSED).await,
            ("0".to_owned(), "0".to_owned()),
            "a write of the failed commit persisted"
        );
        assert_eq!(
            committed(&stack, LATER).await,
            ("1".to_owned(), "1".to_owned()),
            "the later unit of work did not persist"
        );

        // A seed run commits through the same path.
        stack.exec_sql(DEFERRED_COPY_TRIGGER).await;
        assert_raised(&copy_masters(&db).await, "the seed run's commit");
        stack.exec_sql(DROP_DEFERRED_COPY_TRIGGER).await;
        assert_eq!(
            text(&stack, "SELECT count(*) FROM trantype_file")
                .await
                .as_deref(),
            Some("0"),
            "a write of the failed seed run persisted"
        );
        assert_eq!(
            copy_masters(&db)
                .await
                .expect("the pool's connection serves a later seed run that commits"),
            SeedReport {
                trantype_file: 1,
                ..SeedReport::default()
            },
            "the later seed run did not copy the master type"
        );
        assert_eq!(
            text(
                &stack,
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE datname = current_database() AND state LIKE 'idle in transaction%'"
            )
            .await
            .as_deref(),
            Some("0"),
            "a session was left in a transaction"
        );
        let notices = capture.notices();
        assert!(
            notices.is_empty(),
            "the server sent notices around the failed commit: {notices:?}"
        );

        stack.exec_sql("ROLLBACK").await;
        assert_eq!(
            capture.notices(),
            [Captured {
                target: NOTICE_TARGET.to_owned(),
                level: Level::WARN,
                message: NO_TRANSACTION.to_owned(),
            }],
            "the capture missed the notice of a ROLLBACK outside any transaction"
        );
    });
}
