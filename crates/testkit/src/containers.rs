//! [`TestStack`]: one freshly migrated, isolated `PostgreSQL` database per
//! stack and, on request, a `RabbitMQ` broker loaded with the repository's
//! `infra/rabbitmq` definitions.
//!
//! The containers run on the image tags of the `docker-compose.yml` services
//! `postgres` and `rabbitmq` (AAP 0.8.4), with the same database name,
//! credentials and broker definitions, so a test meets the schema and the
//! queues the local runtime has. `tests/compose_tags.rs` compares the public
//! constants of this module with `docker-compose.yml`.
//!
//! Every [`TestStack`] owns its containers: each constructor starts new ones,
//! so tests never share state, and dropping the stack removes them.
//!
//! The public API names only `sqlx::PgPool`, `lapin::Connection` and standard
//! library types. No testcontainers or `carddemo-persistence` type crosses it,
//! because the persistence suites reach this crate through a dev-dependency
//! cycle and would otherwise meet a second copy of every persistence type.

use std::error::Error;

use carddemo_persistence::seed::{SeedMode, SeedSources};
use carddemo_persistence::{Db, migrate, seed};
use lapin::{Connection, ConnectionProperties};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row, ValueRef};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tokio::runtime::Handle;

use crate::{fixtures, runtime};

/// Image of the `postgres` service in `docker-compose.yml`.
pub const POSTGRES_IMAGE: &str = "postgres";

/// Tag of the `postgres` service image in `docker-compose.yml`.
pub const POSTGRES_TAG: &str = "16.15";

/// Image of the `rabbitmq` service in `docker-compose.yml`.
pub const RABBITMQ_IMAGE: &str = "rabbitmq";

/// Tag of the `rabbitmq` service image in `docker-compose.yml`.
pub const RABBITMQ_TAG: &str = "3.13.7-management";

/// Mount target of `infra/rabbitmq/definitions.json` in `docker-compose.yml`,
/// and the `load_definitions` path in `infra/rabbitmq/rabbitmq.conf`.
pub const RABBITMQ_DEFINITIONS_TARGET: &str = "/etc/rabbitmq/definitions.json";

/// Mount target of `infra/rabbitmq/rabbitmq.conf` in `docker-compose.yml`.
pub const RABBITMQ_CONF_TARGET: &str = "/etc/rabbitmq/rabbitmq.conf";

/// `POSTGRES_DB` of the `postgres` service in `docker-compose.yml`, the
/// database of `CARDDEMO_DATABASE_URL` in `.env.example`.
pub const POSTGRES_DB: &str = "carddemo";

/// `POSTGRES_USER` of the `postgres` service in `docker-compose.yml`, the user
/// of `CARDDEMO_DATABASE_URL` in `.env.example`.
pub const POSTGRES_USER: &str = "carddemo";

/// `POSTGRES_PASSWORD` of the `postgres` service in `docker-compose.yml`, the
/// password of `CARDDEMO_DATABASE_URL` in `.env.example`.
pub const POSTGRES_PASSWORD: &str = "carddemo";

/// The user `infra/rabbitmq/definitions.json` declares, the user of
/// `CARDDEMO_AMQP_URL` in `.env.example`.
pub const RABBITMQ_USER: &str = "carddemo";

/// The password whose hash `infra/rabbitmq/definitions.json` holds, the
/// password of `CARDDEMO_AMQP_URL` in `.env.example`.
pub const RABBITMQ_PASSWORD: &str = "carddemo";

/// Container port of `PostgreSQL`, published as `5432` in `docker-compose.yml`.
const POSTGRES_PORT: u16 = 5432;

/// Container port of AMQP, published as `5672` in `docker-compose.yml`.
const AMQP_PORT: u16 = 5672;

/// Repository-relative source of the definitions mount in `docker-compose.yml`.
const RABBITMQ_DEFINITIONS_SOURCE: &str = "infra/rabbitmq/definitions.json";

/// Repository-relative source of the configuration mount in `docker-compose.yml`.
const RABBITMQ_CONF_SOURCE: &str = "infra/rabbitmq/rabbitmq.conf";

/// The broker's stdout line once it has booted, which follows the boot-time
/// definitions import. Only its common prefix is matched: the suffix counts the
/// enabled plugins, which is 5 on the management image, so the
/// `testcontainers-modules` `RabbitMq` image, which waits for "4 plugins
/// started.", never becomes ready on this tag.
const RABBITMQ_READY: &str = "Server startup complete";

/// Erlang scheduler threads of the test broker. Unbounded, the VM starts one per
/// CPU of the Docker host, which on a large shared host bursts to many cores at
/// boot; two serve one test's traffic.
const RABBITMQ_ERL_ARGS: &str = "+S 2:2";

/// Connections of a stack's pool: enough for a test and the consumers or
/// handlers it drives, while many stacks run side by side.
const POOL_MAX_CONNECTIONS: u32 = 5;

/// A migrated `PostgreSQL` test database and, from [`TestStack::with_rabbitmq`],
/// a `RabbitMQ` broker, each in its own container.
///
/// Dropping the stack removes its containers. That works in a plain `#[test]`
/// after [`crate::block_on`], inside `block_on`, and in a `#[tokio::test]` of
/// either runtime flavour.
pub struct TestStack {
    pool: PgPool,
    database_url: String,
    amqp_url: Option<String>,
    rabbitmq: Option<ContainerAsync<GenericImage>>,
    /// `Some` from construction until [`Drop::drop`] takes it.
    postgres: Option<ContainerAsync<Postgres>>,
}

impl Drop for TestStack {
    fn drop(&mut self) {
        // A container removes itself in its own `Drop`, which needs a current
        // tokio runtime. Outside every runtime the shared one is entered for
        // that. Inside a runtime its own context is kept, because testcontainers
        // removes containers differently per flavour and blocking in place, its
        // multi-thread path, is not possible on a current-thread runtime.
        let _entered = Handle::try_current().is_err().then(|| runtime().enter());
        drop(self.rabbitmq.take());
        drop(self.postgres.take());
    }
}

impl TestStack {
    /// Starts a `PostgreSQL` container on [`POSTGRES_IMAGE`]:[`POSTGRES_TAG`]
    /// with database, user and password `carddemo`, opens a pool on it, and
    /// applies every workspace migration (`migrations/0001_reference_data.sql`
    /// to `0005_rewards.sql`). The database holds the schema and no rows; call
    /// [`TestStack::seed`] for the fixture state.
    ///
    /// # Panics
    ///
    /// Panics, naming the image, the URL or the error, if Docker is unavailable,
    /// the container does not start or become ready, its port cannot be looked
    /// up, the pool cannot connect, or a migration fails.
    pub async fn postgres() -> Self {
        let (postgres, database_url, pool) = start_postgres().await;
        Self {
            pool,
            database_url,
            amqp_url: None,
            rabbitmq: None,
            postgres: Some(postgres),
        }
    }

    /// Starts everything [`TestStack::postgres`] starts and, alongside it, a
    /// `RabbitMQ` container on [`RABBITMQ_IMAGE`]:[`RABBITMQ_TAG`].
    ///
    /// The broker mounts `infra/rabbitmq/definitions.json` at
    /// [`RABBITMQ_DEFINITIONS_TARGET`] and `infra/rabbitmq/rabbitmq.conf` at
    /// [`RABBITMQ_CONF_TARGET`], both read-only, as the Compose service does.
    /// The configuration's `load_definitions` imports at boot, before the stack
    /// is returned:
    ///
    /// - user `carddemo` with password `carddemo`, which replaces the default
    ///   `guest` user;
    /// - vhost `/`, with full permissions for `carddemo`;
    /// - six durable classic queues: the request queues
    ///   `CARD.DEMO.REQUEST.DATE`, `CARD.DEMO.REQUEST.ACCT` and
    ///   `CARD.DEMO.REQUEST.PAUTH`; the reply queues `CARD.DEMO.REPLY.DATE`
    ///   [app/app-vsam-mq/cbl/CODATE01.cbl:147] and `CARD.DEMO.REPLY.ACCT`
    ///   [app/app-vsam-mq/cbl/COACCT01.cbl:198]; and the error queue
    ///   `CARD.DEMO.ERROR` [app/app-vsam-mq/cbl/CODATE01.cbl:243]
    ///   [app/app-vsam-mq/cbl/COACCT01.cbl:294].
    ///
    /// # Panics
    ///
    /// Panics in every case [`TestStack::postgres`] does, if either mounted
    /// file is missing (checked before any container starts) or its path is not
    /// UTF-8, or if the broker does not start or become ready or its AMQP port
    /// cannot be looked up.
    pub async fn with_rabbitmq() -> Self {
        let definitions = mount_source(RABBITMQ_DEFINITIONS_SOURCE);
        let conf = mount_source(RABBITMQ_CONF_SOURCE);
        let ((postgres, database_url, pool), (rabbitmq, amqp_url)) =
            tokio::join!(start_postgres(), start_rabbitmq(definitions, conf));
        Self {
            pool,
            database_url,
            amqp_url: Some(amqp_url),
            rabbitmq: Some(rabbitmq),
            postgres: Some(postgres),
        }
    }

    /// The `postgres://` URL of the stack's database, on `127.0.0.1` and the
    /// container's published port.
    #[must_use]
    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// The pool on the stack's database. Persistence suites wrap a clone in
    /// their own `Db::from_pool`.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The `amqp://` URL of the broker, user `carddemo` and vhost `/`, or
    /// `None` for a stack from [`TestStack::postgres`].
    #[must_use]
    pub fn amqp_url(&self) -> Option<&str> {
        self.amqp_url.as_deref()
    }

    /// Opens a connection to the broker on lapin's built-in executor and
    /// reactor. Tests that need the tokio executor and reactor traits build their
    /// own connection from [`TestStack::amqp_url`].
    ///
    /// # Panics
    ///
    /// Panics if the stack has no broker, because it was built by
    /// [`TestStack::postgres`], or if the connection fails.
    pub async fn amqp_connection(&self) -> Connection {
        let url = self
            .amqp_url
            .as_deref()
            .expect("TestStack::amqp_connection needs a stack from TestStack::with_rabbitmq");
        Connection::connect(url, ConnectionProperties::default())
            .await
            .unwrap_or_else(|error| panic!("cannot connect to the test broker at {url}: {error}"))
    }

    /// Loads the seeded state, as `batch seed` does on an empty database, in
    /// one database transaction: the fixture files of `app/data/ASCII` except
    /// `dailytran.txt` (50 accounts, cards, cross-references, customers and
    /// category balances, 51 disclosure groups, 7 transaction types and 18
    /// categories), the Db2 masters from `DB2LTTYP.ctl` and `DB2LTCAT.ctl`, the
    /// 10 `DUSRSECJ` users and the 2 reward seed rules. The transaction master
    /// stays empty (E-07), as do the reward accruals and balances.
    ///
    /// # Errors
    ///
    /// Returns the seed's error, after rolling its transaction back, when a
    /// fixture cannot be read or decoded, a password cannot be hashed, or a
    /// statement or the commit fails.
    pub async fn seed(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let sources = SeedSources::with_defaults(&fixtures::repo_root());
        seed::run(&Db::from_pool(self.pool.clone()), SeedMode::Full, &sources).await?;
        Ok(())
    }

    /// Runs `sql`, one or more statements separated by semicolons, through the
    /// simple-query protocol. Tests use it to set up state the API cannot
    /// reach and to inject failures, such as the trigger that fails the
    /// customer update in `crates/api/tests/caup_account_update.rs`.
    ///
    /// # Panics
    ///
    /// Panics with the error and `sql` if any statement fails. Statements
    /// without their own transaction control run as one implicit transaction,
    /// so after a failure none of them persists.
    pub async fn exec_sql(&self, sql: &str) {
        if let Err(error) = sqlx::raw_sql(sql).execute(&self.pool).await {
            panic!("test statements failed: {error}\n{sql}");
        }
    }

    /// Runs `sql` through the simple-query protocol and returns every row as
    /// its columns in `PostgreSQL` text form, `None` for a null. Any column type
    /// reads this way, so tests need no compile-time query metadata for ad-hoc
    /// reads. With several statements, their rows are concatenated.
    ///
    /// # Panics
    ///
    /// Panics with the error and `sql` if the statements fail, or with the
    /// column index too if a value is not UTF-8 text.
    pub async fn fetch_text_rows(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        let rows = sqlx::raw_sql(sql)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_else(|error| panic!("test read failed: {error}\n{sql}"));
        rows.iter()
            .map(|row| {
                (0..row.len())
                    .map(|index| text_value(row, index, sql))
                    .collect()
            })
            .collect()
    }
}

/// Starts the `PostgreSQL` container, connects a pool and migrates it, returning
/// the container, the database URL and the pool.
async fn start_postgres() -> (ContainerAsync<Postgres>, String, PgPool) {
    let container = Postgres::default()
        .with_db_name(POSTGRES_DB)
        .with_user(POSTGRES_USER)
        .with_password(POSTGRES_PASSWORD)
        .with_tag(POSTGRES_TAG)
        .start()
        .await
        .unwrap_or_else(|error| {
            panic!(
                "cannot start the {POSTGRES_IMAGE}:{POSTGRES_TAG} test container \
                 (is the Docker daemon running?): {error}"
            )
        });
    let port = container
        .get_host_port_ipv4(POSTGRES_PORT)
        .await
        .unwrap_or_else(|error| {
            panic!("cannot read the published port of {POSTGRES_IMAGE}:{POSTGRES_TAG}: {error}")
        });
    let url =
        format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@127.0.0.1:{port}/{POSTGRES_DB}");
    let pool = PgPoolOptions::new()
        .max_connections(POOL_MAX_CONNECTIONS)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("cannot connect to the test database at {url}: {error}"));
    migrate::run_on(&pool)
        .await
        .unwrap_or_else(|error| panic!("cannot migrate the test database at {url}: {error}"));
    (container, url, pool)
}

/// Starts the `RabbitMQ` container with the two repository files mounted
/// read-only, returning the container and its AMQP URL.
async fn start_rabbitmq(
    definitions: String,
    conf: String,
) -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new(RABBITMQ_IMAGE, RABBITMQ_TAG)
        .with_exposed_port(AMQP_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stdout(RABBITMQ_READY))
        .with_mount(
            Mount::bind_mount(definitions, RABBITMQ_DEFINITIONS_TARGET)
                .with_access_mode(AccessMode::ReadOnly),
        )
        .with_mount(
            Mount::bind_mount(conf, RABBITMQ_CONF_TARGET).with_access_mode(AccessMode::ReadOnly),
        )
        .with_env_var("RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS", RABBITMQ_ERL_ARGS)
        .start()
        .await
        .unwrap_or_else(|error| {
            panic!(
                "cannot start the {RABBITMQ_IMAGE}:{RABBITMQ_TAG} test container \
                 (is the Docker daemon running?): {error}"
            )
        });
    let port = container
        .get_host_port_ipv4(AMQP_PORT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "cannot read the published AMQP port of {RABBITMQ_IMAGE}:{RABBITMQ_TAG}: {error}"
            )
        });
    let url = format!("amqp://{RABBITMQ_USER}:{RABBITMQ_PASSWORD}@127.0.0.1:{port}/%2f");
    (container, url)
}

/// The absolute path of the repository file `relative`, as the `String` a bind
/// mount takes.
///
/// # Panics
///
/// Panics, naming the path, if the file does not exist or its path is not UTF-8.
fn mount_source(relative: &str) -> String {
    let path = fixtures::repo_root().join(relative);
    assert!(
        path.is_file(),
        "the RabbitMQ test container mounts {}, which does not exist",
        path.display()
    );
    path.to_str()
        .unwrap_or_else(|| panic!("the mount source {} is not UTF-8", path.display()))
        .to_owned()
}

/// Column `index` of `row` as text, `None` for a null.
///
/// # Panics
///
/// Panics with the column index, the error and `sql` if the value cannot be
/// read or is not UTF-8.
fn text_value(row: &PgRow, index: usize, sql: &str) -> Option<String> {
    let value = row
        .try_get_raw(index)
        .unwrap_or_else(|error| panic!("cannot read column {index}: {error}\n{sql}"));
    if value.is_null() {
        return None;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|error| panic!("column {index} is not UTF-8 text: {error}\n{sql}"));
    Some(text.to_owned())
}
