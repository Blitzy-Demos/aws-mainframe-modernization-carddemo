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
//! Every [`TestStack`] also owns a Docker bridge network of its own, which its
//! containers join. The network's default host binding is `127.0.0.1`, so the
//! random host ports the containers publish listen on loopback only, as the
//! Compose services' publications do, instead of on every host interface as
//! they would on Docker's default bridge. Dropping the stack removes the
//! network after the containers.
//!
//! The network's teardown, which also runs when a constructor panics, first
//! force-removes every container still on the network, then the network. That
//! covers a container whose start timed out or was cut short, which
//! testcontainers no longer tracks and so never removes. With
//! `TESTCONTAINERS_COMMAND=keep`, the containers are kept, as testcontainers
//! keeps its own, and the network stays while one of them is attached.
//!
//! A run that SIGINT (Ctrl-C) or SIGTERM (`kill`, a CI timeout) interrupts
//! unwinds nothing, so no `Drop` runs. The first stack a process creates
//! therefore installs a thread that watches for both signals. On the first
//! one it tears down, concurrently, every stack network the process created
//! and has not removed, with the containers on it. It then waits for every
//! network creation and container start still in flight, for at most 900
//! seconds, three Docker calls' timeouts; when that runs out, it reports how
//! many are left in flight. A network whose creation the teardown overtook is
//! removed by that creation once Docker has created it. A container start may
//! have had Docker create its container after the teardown looked for the
//! network's containers, and Docker removes a network that only created, never
//! started, containers name while it keeps those containers; testcontainers,
//! too, re-creates a network it finds gone and starts its container there. A
//! final sweep therefore tears every one of those networks down again, with any
//! container Docker still lists under the network's name. The process then
//! ends with status 128 plus the signal's number (130 for SIGINT, 143 for
//! SIGTERM), as a shell reports a death by that signal. A second signal cuts
//! the teardown short. Only the networks this process created, and the
//! containers on them, are touched. Once the teardown has begun, no new stack
//! network is created and no new container starts, and a thread that drops a
//! stack waits for the exit instead of returning into its test. SIGKILL cannot
//! be caught, so a killed run still leaves its stacks behind. With
//! `TESTCONTAINERS_COMMAND=keep` no watcher is installed: both signals end the
//! process at once and the containers are kept.
//!
//! Each container gets 300 seconds to start and become ready, instead of
//! testcontainers' default 60, and each of the stack's own Docker calls, such
//! as the network's creation and removal, gets 300 seconds instead of bollard's
//! default 120: a shared, busy Docker daemon can exceed both defaults.
//! testcontainers' own Docker calls keep bollard's 120 seconds, which it does
//! not let a caller change. A container start that one of them runs out of is
//! therefore retried, after a pause of 5 and then 10 seconds, at most three
//! attempts in all, and before each retry the container of that image the
//! failed attempt left on the stack network is force-removed. A start that
//! runs out of its 300 seconds, or fails in any other way, is not retried.
//!
//! The public API names only `sqlx::PgPool`, `lapin::Connection` and standard
//! library types. No testcontainers or `carddemo-persistence` type crosses it,
//! because the persistence suites reach this crate through a dev-dependency
//! cycle and would otherwise meet a second copy of every persistence type.

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::fmt::{self, Display};
use std::future::poll_fn;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::pin::pin;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, Once, PoisonError, mpsc};
use std::task::Poll;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use carddemo_persistence::seed::{SeedMode, SeedSources};
use carddemo_persistence::{Db, migrate, seed};
use lapin::{Connection, ConnectionProperties};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row, ValueRef};
use testcontainers::bollard::Docker;
use testcontainers::bollard::container::{ListContainersOptions, RemoveContainerOptions};
use testcontainers::bollard::errors::Error as DockerError;
use testcontainers::bollard::models::ContainerSummary;
use testcontainers::bollard::network::{CreateNetworkOptions, InspectNetworkOptions};
use testcontainers::core::error::ClientError;
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt, TestcontainersError};
use testcontainers_modules::postgres::Postgres;
use tokio::runtime::{Builder, Handle};
#[cfg(unix)]
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::task::{self, JoinSet};

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

/// Start budget of each test container: testcontainers' startup timeout, which
/// bounds `docker start` plus the image's readiness wait, but not the create
/// or image pull before them. testcontainers' default, 60 seconds, is shorter
/// than a shared, busy Docker daemon can take for that. When the budget runs
/// out, testcontainers keeps no handle on the container it created, and the
/// stack network's teardown removes it.
///
/// It is the budget of one start attempt. A start that runs out of it is not
/// retried ([`is_client_timeout`]), because a retry would multiply the bound
/// that already covers the daemon's slowest start.
const CONTAINER_START_TIMEOUT: Duration = Duration::from_secs(300);

/// The pauses of a test container's start between its attempts, one before
/// each retry: 5 seconds before the second attempt and 10 before the third,
/// so a start makes at most three. Only a start that failed on a Docker client
/// timeout ([`is_client_timeout`]) is retried, by [`start_retrying`]: that
/// timeout, bollard's 120 seconds in testcontainers' own client, is fixed, and
/// a daemon that a burst of other work slowed down for one call often answers
/// the next in time.
const START_BACKOFF: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(10)];

/// Client timeout of the stack's own Docker API calls: creating and removing
/// its network, and removing the containers left on it. bollard's default, 120
/// seconds, is shorter than a shared, busy Docker daemon can take for them,
/// and the daemon still completes a call its client gave up on: a network it
/// creates after that has no guard to remove it.
const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a signal teardown waits, in [`wait_for_creations`], for the network
/// creations ([`StackNetwork::create`]) and container starts
/// ([`ContainerStart`]) still in flight: three times [`DOCKER_CALL_TIMEOUT`],
/// 900 seconds.
///
/// A network creation has at most three Docker calls left, one after another
/// and each under that client timeout: the network's creation, which is under
/// way, and then, because the teardown took its name, the lookup of the
/// containers on the network, whose two requests run side by side, and the
/// network's removal. No container removal follows, because no container can
/// join a network its creation has not handed out.
///
/// A container start has at most testcontainers' network lookup and creation
/// and two container creations left, each under testcontainers' 120-second
/// client timeout, and then the container's start and readiness wait under
/// [`CONTAINER_START_TIMEOUT`]: 780 seconds in all. Between the two
/// creations, which a creation that finds no image or no network makes it
/// retry, testcontainers pulls the image, a download no client timeout bounds.
///
/// That bound is per attempt of a start that [`start_retrying`] retries after
/// a Docker client timeout. Only each attempt is in flight: between attempts,
/// during the pause and the removal of what the failed attempt left, nothing
/// is, and an attempt that would begin once the teardown has begun panics
/// before it creates anything. The teardown therefore waits for one attempt at
/// most.
///
/// A call that is still polled therefore settles in time, unless it pulls an
/// image for longer than the rest of the bound. Only such a pull, and a call
/// that is never polled again, outlast the bound. An example of the latter is
/// one on a current-thread runtime whose only thread waits in
/// [`await_signal_exit`] after dropping a stack. Without the bound, such a
/// call would keep the process from ever exiting.
const CREATIONS_WAIT: Duration = DOCKER_CALL_TIMEOUT.saturating_mul(3);

/// The environment variable through which testcontainers decides whether the
/// containers it started are removed when dropped.
const TESTCONTAINERS_COMMAND: &str = "TESTCONTAINERS_COMMAND";

/// The [`TESTCONTAINERS_COMMAND`] value with which testcontainers keeps them.
const KEEP_COMMAND: &str = "keep";

/// Docker API status of a container or network that does not exist.
const NOT_FOUND: u16 = 404;

/// Docker API status of a container removal that another one, such as
/// testcontainers' own drop, already has in progress.
const CONFLICT: u16 = 409;

/// Connections of a stack's pool: enough for a test and the consumers or
/// handlers it drives, while many stacks run side by side.
const POOL_MAX_CONNECTIONS: u32 = 5;

/// Docker network driver of a stack's network.
const NETWORK_DRIVER: &str = "bridge";

/// Bridge driver option that sets the host address a port published without
/// one binds to. Docker's default bridge sets it to `0.0.0.0`.
const HOST_BINDING_OPTION: &str = "com.docker.network.bridge.host_binding_ipv4";

/// The host address every published port of a stack binds to: loopback, the
/// host of the URLs the stack hands out.
const NETWORK_HOST_BINDING: &str = "127.0.0.1";

/// First part of a stack network's name, which the process id, the creation
/// time and [`NETWORK_SEQUENCE`] make unique.
const NETWORK_NAME_PREFIX: &str = "carddemo-testkit";

/// Label key and value that testcontainers puts on the containers it starts,
/// put on a stack network too so it is found alongside them.
const NETWORK_LABEL: (&str, &str) = ("org.testcontainers.managed-by", "testcontainers");

/// Stack networks this process has created so far.
static NETWORK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The signals the watcher thread tears the stacks down on, as its warnings
/// name them.
#[cfg(unix)]
const STOP_SIGNALS: &str = "SIGINT and SIGTERM";

/// The signal the watcher thread tears the stacks down on, as its warnings
/// name it.
#[cfg(not(unix))]
const STOP_SIGNALS: &str = "Ctrl-C";

/// Exit status after a Ctrl-C teardown off Unix: 128 plus SIGINT's number, 2,
/// the status a Unix process reports for it.
#[cfg(not(unix))]
const CTRL_C_STATUS: i32 = 130;

/// The stack networks of this process, for the teardown on a signal.
static LIVE_NETWORKS: Mutex<LiveNetworks> = Mutex::new(LiveNetworks {
    names: Some(BTreeSet::new()),
    in_flight: 0,
});

/// Notified whenever [`LiveNetworks::in_flight`] drops, for
/// [`wait_for_creations`].
static IN_FLIGHT_SETTLED: Condvar = Condvar::new();

/// The stack networks this process has created, or is creating, and has not
/// removed. Only these are torn down on a signal: a network named after this
/// process's id may belong to another process with the same id in another pid
/// namespace on the same Docker daemon.
struct LiveNetworks {
    /// Their names, from just before each network's creation until its
    /// removal; `None` once a signal teardown has taken them.
    names: Option<BTreeSet<String>>,
    /// The [`InFlight`] counts alive: the network creations ([`Creation`]) and
    /// the container starts ([`ContainerStart`]) that have begun and not yet
    /// ended, each of which may still create a Docker resource on a stack
    /// network.
    in_flight: usize,
}

/// A migrated `PostgreSQL` test database and, from [`TestStack::with_rabbitmq`],
/// a `RabbitMQ` broker, each in its own container. The containers join the
/// stack's own bridge network, so their published ports listen on `127.0.0.1`
/// only.
///
/// Dropping the stack removes its containers, force-removes any other
/// container still on its network, and then removes the network. That works in
/// a plain `#[test]` after [`crate::block_on`], inside `block_on`, and in a
/// `#[tokio::test]` of either runtime flavour.
///
/// When SIGINT or SIGTERM interrupts the process, its signal thread does the
/// same for every stack still alive and then ends the process with status 130
/// or 143 (see the module documentation). A stack dropped once that teardown
/// has begun leaves its network to it, and the dropping thread waits for the
/// exit.
pub struct TestStack {
    pool: PgPool,
    database_url: String,
    amqp_url: Option<String>,
    rabbitmq: Option<ContainerAsync<GenericImage>>,
    /// `Some` from construction until [`Drop::drop`] takes it.
    postgres: Option<ContainerAsync<Postgres>>,
    /// `Some` from construction until [`Drop::drop`] takes it, after the
    /// containers, which Docker must have detached before it removes the
    /// network.
    network: Option<StackNetwork>,
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
        drop(self.network.take());
    }
}

impl TestStack {
    /// Starts a `PostgreSQL` container on [`POSTGRES_IMAGE`]:[`POSTGRES_TAG`]
    /// with database, user and password `carddemo`, opens a pool on it, and
    /// applies every workspace migration (`migrations/0001_reference_data.sql`
    /// to `0005_rewards.sql`). The database holds the schema and no rows; call
    /// [`TestStack::seed`] for the fixture state.
    ///
    /// The container joins a bridge network the stack creates first, whose
    /// default host binding is `127.0.0.1`, so its random published port
    /// listens on loopback only.
    ///
    /// # Panics
    ///
    /// Panics, naming the network, the image, the URL or the error, if Docker is
    /// unavailable, the stack network cannot be created, the container does not
    /// start and become ready within 300 seconds, its start fails on a Docker
    /// client timeout in each of its three attempts, its port cannot be looked
    /// up, the pool cannot connect, or a migration fails. A failed start's panic
    /// also gives the number of attempts made: three after client timeouts, one
    /// after any other error. As the panic unwinds, every container on the stack
    /// network, one whose start failed included, is force-removed, then the
    /// network.
    pub async fn postgres() -> Self {
        // Created before any container, so that on a panic the containers,
        // dropped first, are gone when the network is removed.
        let network = StackNetwork::create().await;
        let (postgres, database_url, pool) = start_postgres(network.name()).await;
        Self {
            pool,
            database_url,
            amqp_url: None,
            rabbitmq: None,
            postgres: Some(postgres),
            network: Some(network),
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
    /// Both containers join the one bridge network the stack creates, whose
    /// default host binding is `127.0.0.1`, so every port the broker and the
    /// database publish listens on loopback only.
    ///
    /// # Panics
    ///
    /// Panics in every case [`TestStack::postgres`] does, if either mounted
    /// file is missing (checked before any Docker network or container is
    /// created) or its path is not UTF-8, or if the broker does not start and
    /// become ready within 300 seconds, its start fails on a Docker client
    /// timeout in each of its three attempts, or its AMQP port cannot be looked
    /// up. A failed start's panic gives the number of attempts made, as for the
    /// database.
    pub async fn with_rabbitmq() -> Self {
        let definitions = mount_source(RABBITMQ_DEFINITIONS_SOURCE);
        let conf = mount_source(RABBITMQ_CONF_SOURCE);
        // Created before any container, so that on a panic the containers,
        // dropped first, are gone when the network is removed.
        let network = StackNetwork::create().await;
        let ((postgres, database_url, pool), (rabbitmq, amqp_url)) = tokio::join!(
            start_postgres(network.name()),
            start_rabbitmq(network.name(), definitions, conf)
        );
        Self {
            pool,
            database_url,
            amqp_url: Some(amqp_url),
            rabbitmq: Some(rabbitmq),
            postgres: Some(postgres),
            network: Some(network),
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
    /// As `batch seed` does, the sources are read, decoded and hashed by
    /// `seed::prepare` on tokio's blocking pool before the transaction opens,
    /// so the argon2id hashing never holds a runtime worker or a connection.
    ///
    /// # Errors
    ///
    /// Returns the seed's error, after rolling its transaction back, when a
    /// fixture cannot be read or decoded, a password cannot be hashed, or a
    /// statement or the commit fails; and the join error, before any
    /// transaction, when the preparation panics or the runtime cancels it.
    ///
    /// # Panics
    ///
    /// Panics, as `tokio::task::spawn_blocking` does, when called outside a
    /// tokio runtime; [`crate::block_on`] and `#[tokio::test]` provide one.
    pub async fn seed(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let sources = SeedSources::with_defaults(&fixtures::repo_root());
        let prepared =
            tokio::task::spawn_blocking(move || seed::prepare(SeedMode::Full, &sources)).await?;
        seed::run(&Db::from_pool(self.pool.clone()), prepared).await?;
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

/// A Docker bridge network that one [`TestStack`] creates and its containers
/// join. Its default host binding is [`NETWORK_HOST_BINDING`], so the ports its
/// containers publish without a host address listen on loopback only, where
/// Docker's default bridge would bind them to every host interface.
///
/// The network exists before testcontainers sees its name, so testcontainers
/// treats it as created elsewhere and never removes it; this guard's `Drop`
/// does. Before that, unless `TESTCONTAINERS_COMMAND` is `keep`, it
/// force-removes every container still on the network: one that testcontainers
/// lost track of, because its start timed out or its drop was cut short, would
/// otherwise outlive the stack and keep the network from being removed.
///
/// The name is in [`LIVE_NETWORKS`] from just before the network's creation
/// until the guard's `Drop` has removed it, so a SIGINT or SIGTERM in between
/// tears the network down through the signal thread instead.
struct StackNetwork {
    name: String,
}

impl StackNetwork {
    /// Creates a uniquely named bridge network whose default host binding is
    /// [`NETWORK_HOST_BINDING`], after installing the signal thread on the
    /// process's first call ([`watch_signals_once`]).
    ///
    /// # Panics
    ///
    /// Panics, naming the network and the error, if the Docker daemon cannot be
    /// reached or refuses to create the network. Panics, naming the network,
    /// once a signal teardown has begun, because the process is exiting: before
    /// creating the network, or after removing it again when the teardown took
    /// the name while Docker was creating it.
    async fn create() -> Self {
        watch_signals_once();
        let sequence = NETWORK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = format!("{NETWORK_NAME_PREFIX}-{}-{nanos}-{sequence}", process::id());
        // Registered before the network exists, so that a signal teardown from
        // here on either removes it or waits, at most `CREATIONS_WAIT`, until
        // this call has.
        let Some(creation) = Creation::register(&name) else {
            panic!(
                "cannot create the test network {name}: the process is removing its \
                 test stacks on a signal and exiting"
            );
        };
        let created = create_network(&name).await;
        let taken = creation.settle(created.is_ok());
        if taken && created.is_ok() {
            // The teardown that took the name may have looked for the network
            // before Docker created it, and it ends the process only once
            // `creation` is dropped, after this removal, or once
            // `CREATIONS_WAIT` has run out.
            teardown_or_warn(&name).await;
        }
        drop(creation);
        if let Err(error) = created {
            panic!(
                "cannot create the test network {name} with host binding \
                 {NETWORK_HOST_BINDING} (is the Docker daemon running?): {error}"
            );
        }
        assert!(
            !taken,
            "removed the test network {name} again: the process is removing its test \
             stacks on a signal and exiting"
        );
        Self { name }
    }

    /// The network's name, which containers join through
    /// [`ImageExt::with_network`].
    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for StackNetwork {
    fn drop(&mut self) {
        // A signal teardown that has taken the names removes this network and
        // then ends the process. Removing it here as well would race that
        // teardown, and returning would let the test harness exit first, with
        // a status of its own.
        if signal_teardown_started() {
            await_signal_exit();
        }
        // Removal takes Docker API calls, which need a tokio runtime. Inside a
        // runtime, as in `block_on` or a `#[tokio::test]` of either flavour, a
        // nested `block_on` panics, so a thread of its own, with its own
        // current-thread runtime and client, runs it alike in every context,
        // outside any runtime included. A failure is reported instead of
        // raised, because a panic in `Drop` during unwinding aborts.
        let name = &self.name;
        let removed = name.clone();
        let removal = thread::Builder::new()
            .name(String::from("testkit-network"))
            .spawn(move || remove_network(&removed));
        let outcome = match removal {
            Ok(handle) => match handle.join() {
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(_) => Err(String::from("its removal thread panicked")),
            },
            Err(error) => Err(format!("its removal thread did not start: {error}")),
        };
        // Deregistered only after the removal, so that a signal during it still
        // tears the network down. A teardown that took the name meanwhile
        // reports its own outcome for it.
        if !deregister(name) {
            await_signal_exit();
        }
        if let Err(error) = outcome {
            warn_network_left(name, &error);
        }
    }
}

/// One count in [`LiveNetworks::in_flight`], held by a [`Creation`] or a
/// [`ContainerStart`]. Dropping it, also when the future that holds it is
/// dropped part-way, ends that call for a signal teardown that waits on it in
/// [`wait_for_creations`].
struct InFlight;

impl InFlight {
    /// Counts one more call in flight in `live`, the locked [`LIVE_NETWORKS`],
    /// and returns that count together with the names, for the caller to
    /// update under the same lock; or returns `None`, counting nothing, once a
    /// signal teardown has taken the names.
    fn begin(live: &mut LiveNetworks) -> Option<(Self, &mut BTreeSet<String>)> {
        let names = live.names.as_mut()?;
        live.in_flight += 1;
        Some((Self, names))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        live_networks().in_flight -= 1;
        IN_FLIGHT_SETTLED.notify_all();
    }
}

/// A stack network name that [`StackNetwork::create`] has registered in
/// [`LIVE_NETWORKS`] and not yet settled, counted in flight until dropped.
struct Creation<'a> {
    name: &'a str,
    _in_flight: InFlight,
}

impl<'a> Creation<'a> {
    /// Registers `name` as a network being created, or returns `None`,
    /// registering nothing, once a signal teardown has taken the names.
    fn register(name: &'a str) -> Option<Self> {
        let mut live = live_networks();
        let (in_flight, names) = InFlight::begin(&mut live)?;
        names.insert(name.to_owned());
        Some(Self {
            name,
            _in_flight: in_flight,
        })
    }

    /// Settles the creation's outcome: deregisters the name unless the network
    /// was `created`, and returns whether a signal teardown has taken the names,
    /// this one included, meanwhile.
    fn settle(&self, created: bool) -> bool {
        match live_networks().names.as_mut() {
            Some(names) => {
                if !created {
                    names.remove(self.name);
                }
                false
            }
            None => true,
        }
    }
}

/// A testcontainers start of one of a stack's containers, counted in flight
/// from just before its `start()` until that call returns, or until its future
/// is dropped. Until then the start may still create a container on the stack
/// network, even after a signal teardown has looked for the network's
/// containers, so that teardown waits for it before its final sweep.
struct ContainerStart {
    _in_flight: InFlight,
}

impl ContainerStart {
    /// Counts the start of the container `image` on the stack network
    /// `network` in flight.
    ///
    /// # Panics
    ///
    /// Panics, naming the image and the network, once a signal teardown has
    /// taken the names, so that no container is created after that teardown
    /// has begun.
    fn begin(image: &str, network: &str) -> Self {
        let begun = InFlight::begin(&mut live_networks()).map(|(in_flight, _)| in_flight);
        let Some(in_flight) = begun else {
            panic!(
                "cannot start the {image} test container on the network {network}: the process \
                 is removing its test stacks on a signal and exiting"
            );
        };
        Self {
            _in_flight: in_flight,
        }
    }
}

/// Locks [`LIVE_NETWORKS`]. Each update under the lock is one insertion,
/// removal, take or count, which leaves the state whole, so a lock that a
/// panicking thread poisoned is used as it is.
fn live_networks() -> MutexGuard<'static, LiveNetworks> {
    LIVE_NETWORKS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Removes `name` from [`LIVE_NETWORKS`], for [`StackNetwork`]'s `Drop`, and
/// returns `false`, removing nothing, once a signal teardown has taken the
/// names.
fn deregister(name: &str) -> bool {
    match live_networks().names.as_mut() {
        Some(names) => {
            names.remove(name);
            true
        }
        None => false,
    }
}

/// Whether a signal teardown has taken the names in [`LIVE_NETWORKS`].
fn signal_teardown_started() -> bool {
    live_networks().names.is_none()
}

/// Blocks the calling thread for good, once a signal teardown has begun: that
/// teardown ends the process with [`process::exit`] when it is over, and this
/// thread neither returns into a test, which would then fail on its removed
/// containers, nor lets the test harness exit first with its own status.
fn await_signal_exit() -> ! {
    loop {
        thread::park();
    }
}

/// Waits until no network creation ([`Creation`]) and no container start
/// ([`ContainerStart`]) is in flight, for at most `bound`, and returns how many
/// still are: 0 once all have ended, otherwise the count still in flight when
/// `bound` ran out.
///
/// The signal teardown waits here, at most [`CREATIONS_WAIT`], between its
/// first pass and its final sweep. A creation that the teardown's take
/// overtook removes its network itself before it ends. A container start may
/// have created its container after the first pass looked for the network's
/// containers. It ends once its start succeeds or fails: early when the first
/// pass removed its container or its network, and otherwise once the
/// container is ready, on a network of that name that testcontainers
/// re-created because it found the network gone. The final sweep then removes
/// that container and network.
fn wait_for_creations(bound: Duration) -> usize {
    let (live, _) = IN_FLIGHT_SETTLED
        .wait_timeout_while(live_networks(), bound, |live| live.in_flight > 0)
        .unwrap_or_else(PoisonError::into_inner);
    live.in_flight
}

/// Installs, on the first call in the process, the thread that tears the
/// process's stack networks down on SIGINT or SIGTERM (Ctrl-C off Unix), and
/// returns once that thread watches for them, so a signal right after the
/// first network exists is handled.
///
/// With `TESTCONTAINERS_COMMAND=keep` it installs nothing: the signals keep
/// their default action, which ends the process at once, and the containers
/// are kept, as testcontainers keeps its own. A thread or runtime that cannot
/// be started, or a signal that cannot be watched, is reported on stderr, and
/// that signal keeps its default action too.
fn watch_signals_once() {
    static WATCHER: Once = Once::new();
    WATCHER.call_once(|| {
        if keeps_containers() {
            return;
        }
        let (ready, watching) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name(String::from("testkit-signals"))
            .spawn(move || watch_signals(&ready));
        match spawned {
            Ok(_) => {
                // The thread answers once its handlers are in place. It ends
                // without answering only by panicking, which the panic hook
                // reports, and the stacks then still start.
                let _ = watching.recv();
            }
            Err(error) => warn_unwatched(STOP_SIGNALS, &error),
        }
    });
}

/// The signal thread. On a current-thread runtime of its own, so that it works
/// whatever runtime the tests use, it installs the handlers, answers `ready`,
/// and waits. On the first signal it tears down every stack network in
/// [`LIVE_NETWORKS`] and ends the process with the signal's status, even when
/// the teardown panics, because the threads in [`await_signal_exit`] wait for
/// that exit.
fn watch_signals(ready: &mpsc::Sender<()>) {
    let runtime = match Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            warn_unwatched(STOP_SIGNALS, &error);
            // `create` waits for this answer, so the send cannot fail.
            let _ = ready.send(());
            return;
        }
    };
    let Some((stop, mut signals)) = runtime.block_on(first_signal(ready)) else {
        return;
    };
    let teardown = panic::catch_unwind(AssertUnwindSafe(|| {
        runtime.block_on(teardown_on_signal(stop, &mut signals));
    }));
    if teardown.is_err() {
        report(format_args!(
            "warning: carddemo-testkit's teardown on {} failed part-way, so test stacks \
             may be left behind (list them with \
             `docker network ls --filter name={NETWORK_NAME_PREFIX}-{}-`)",
            stop.name,
            process::id()
        ));
    }
    process::exit(stop.status);
}

/// Installs the signal handlers, answers `ready` once they are in place, and
/// waits for the first signal. `None` when no handler could be installed.
async fn first_signal(ready: &mpsc::Sender<()>) -> Option<(Stop, StopSignals)> {
    let mut signals = StopSignals::install();
    let stop = {
        let mut first = pin!(signals.next());
        // One poll installs the handlers that install only when first polled,
        // such as Ctrl-C's off Unix, before `create` proceeds.
        let polled = poll_fn(|context| Poll::Ready(first.as_mut().poll(context))).await;
        // `create` waits for this answer, so the send cannot fail.
        let _ = ready.send(());
        match polled {
            Poll::Ready(stop) => stop,
            Poll::Pending => first.await,
        }
    }?;
    Some((stop, signals))
}

/// Takes the names in [`LIVE_NETWORKS`], so that no new stack network is
/// created and no new container starts, and tears those stack networks down
/// through [`remove_live_networks`], unless a second signal cuts that short:
/// then every network whose final sweep has not finished is reported.
async fn teardown_on_signal(stop: Stop, signals: &mut StopSignals) {
    let mut left = live_networks().names.take().unwrap_or_default();
    report(format_args!(
        "carddemo-testkit: {} received; removing the {} test stack network(s) of this \
         process, with their containers, before it exits with status {}",
        stop.name,
        left.len(),
        stop.status
    ));
    let second = tokio::select! {
        () = remove_live_networks(&mut left) => None,
        Some(second) = signals.next() => Some(second),
    };
    if let Some(second) = second {
        report(format_args!(
            "carddemo-testkit: {} received during the teardown; exiting at once",
            second.name
        ));
        let reason = format!("a second signal, {}, cut its teardown short", second.name);
        for name in &left {
            warn_network_left(name, &reason);
        }
    }
}

/// Tears down the stack networks in `left`, the names the signal teardown
/// took, in three steps:
///
/// 1. A first pass tears every one of them down concurrently, through
///    [`teardown_network`]. Its outcomes are not reported, because the final
///    sweep retries every name.
/// 2. A wait, in [`wait_for_creations`], until every network creation and
///    container start in flight has ended. A creation that created its network
///    after the names were taken removes that network itself. A container start
///    may have had Docker create its container after the first pass looked for
///    the network's containers. Once the first pass has removed its network or
///    container, the start fails early, unless testcontainers, finding the
///    network gone, re-creates it and takes the container to readiness there.
///    The wait lasts at most [`CREATIONS_WAIT`]: when it runs out, the calls
///    still in flight are reported on stderr, with the commands that list what
///    they may leave behind, and the teardown goes on.
/// 3. The final sweep, [`sweep_networks`], tears every name down again,
///    including a network of that name that testcontainers re-created. It
///    exists because Docker removes a network that only created, never
///    started, containers name and keeps those containers, so a container
///    created after the first pass's lookup survives that pass. Docker still
///    lists such a container under its network's name, so the sweep finds it.
///    Each name leaves `left` once its sweep is over, and a failure is reported
///    as it comes.
async fn remove_live_networks(left: &mut BTreeSet<String>) {
    teardown_networks(left.iter().cloned(), |_, _| {}).await;
    match task::spawn_blocking(|| wait_for_creations(CREATIONS_WAIT)).await {
        Ok(0) => {}
        Ok(unsettled) => report(format_args!(
            "warning: carddemo-testkit stopped waiting, after {CREATIONS_WAIT:?}, for the \
             {unsettled} test network creation(s) and container start(s) still in flight, so \
             their networks and containers may be left behind (list the networks with \
             `docker network ls --filter name={NETWORK_NAME_PREFIX}-{}-` and the containers \
             on each with `docker ps -a --filter network=<name>`)",
            process::id()
        )),
        Err(error) => report(format_args!(
            "warning: carddemo-testkit cannot wait for the test network creations and \
             container starts still in flight, so their networks and containers may be left \
             behind: {error}"
        )),
    }
    sweep_networks(left).await;
}

/// The final sweep of a signal teardown: tears every stack network in `left`
/// down concurrently, through [`teardown_network`], taking each name out of
/// `left` once its teardown is over and reporting a failure on stderr as it
/// comes. A network or container that is already gone counts as removed.
///
/// It works on `left` alone and never on [`LIVE_NETWORKS`], so it serves any
/// set of stack network names.
async fn sweep_networks(left: &mut BTreeSet<String>) {
    let names = left.clone();
    teardown_networks(names, |name, outcome| {
        left.remove(&name);
        if let Err(error) = outcome {
            warn_network_left(&name, &error);
        }
    })
    .await;
}

/// Tears every stack network in `names` down concurrently, through
/// [`teardown_network`] on one client, and hands each name with its outcome to
/// `settled` as its teardown ends. Without a Docker client, every name is
/// handed over at once with that error.
async fn teardown_networks(
    names: impl IntoIterator<Item = String>,
    mut settled: impl FnMut(String, Result<(), String>),
) {
    let docker = match docker_client() {
        Ok(docker) => docker,
        Err(error) => {
            let error = error.to_string();
            for name in names {
                settled(name, Err(error.clone()));
            }
            return;
        }
    };
    let mut teardowns = JoinSet::new();
    let mut tasks = HashMap::new();
    for name in names {
        let (docker, removed) = (docker.clone(), name.clone());
        let task = teardowns.spawn(async move { teardown_network(&docker, &removed).await });
        tasks.insert(task.id(), name);
    }
    while let Some(joined) = teardowns.join_next_with_id().await {
        let (id, outcome) = match joined {
            Ok((id, removed)) => (id, removed.map_err(|error| error.to_string())),
            Err(error) => (
                error.id(),
                Err(format!("its teardown task failed: {error}")),
            ),
        };
        if let Some(name) = tasks.remove(&id) {
            settled(name, outcome);
        }
    }
}

/// A signal that ends the process once the stacks are torn down.
#[derive(Clone, Copy)]
struct Stop {
    /// The signal's name, for the notices.
    name: &'static str,
    /// The process's exit status for it: 128 plus the signal's number.
    status: i32,
}

/// The handlers of SIGINT and SIGTERM, each `None` when it could not be
/// installed, which leaves that signal its default action.
#[cfg(unix)]
struct StopSignals {
    interrupt: Option<Signal>,
    terminate: Option<Signal>,
}

#[cfg(unix)]
impl StopSignals {
    /// Installs both handlers, reporting on stderr one that cannot be
    /// installed. Needs a current tokio runtime with its I/O driver enabled.
    fn install() -> Self {
        Self {
            interrupt: install_handler(SignalKind::interrupt(), "SIGINT"),
            terminate: install_handler(SignalKind::terminate(), "SIGTERM"),
        }
    }

    /// The next signal the process receives, or `None` when no handler is
    /// installed.
    async fn next(&mut self) -> Option<Stop> {
        tokio::select! {
            Some(()) = received(self.interrupt.as_mut()) => Some(Stop {
                name: "SIGINT",
                status: 128 + SignalKind::interrupt().as_raw_value(),
            }),
            Some(()) = received(self.terminate.as_mut()) => Some(Stop {
                name: "SIGTERM",
                status: 128 + SignalKind::terminate().as_raw_value(),
            }),
            else => None,
        }
    }
}

/// Installs the handler of `kind`, named `name`, or reports on stderr that it
/// cannot be installed.
#[cfg(unix)]
fn install_handler(kind: SignalKind, name: &str) -> Option<Signal> {
    signal(kind)
        .inspect_err(|error| warn_unwatched(name, error))
        .ok()
}

/// The next delivery of `handler`, or `None` at once without one.
#[cfg(unix)]
async fn received(handler: Option<&mut Signal>) -> Option<()> {
    match handler {
        Some(handler) => handler.recv().await,
        None => None,
    }
}

/// The Ctrl-C handler, which `tokio::signal::ctrl_c` installs when first
/// polled.
#[cfg(not(unix))]
struct StopSignals {
    /// Whether the handler could not be installed, which
    /// [`StopSignals::next`] reports once.
    failed: bool,
}

#[cfg(not(unix))]
impl StopSignals {
    /// The handler, installed on the first poll of [`StopSignals::next`].
    fn install() -> Self {
        Self { failed: false }
    }

    /// The next Ctrl-C, or `None` when its handler cannot be installed, which
    /// is reported on stderr the first time.
    async fn next(&mut self) -> Option<Stop> {
        if self.failed {
            return None;
        }
        match tokio::signal::ctrl_c().await {
            Ok(()) => Some(Stop {
                name: "Ctrl-C",
                status: CTRL_C_STATUS,
            }),
            Err(error) => {
                self.failed = true;
                warn_unwatched(STOP_SIGNALS, &error);
                None
            }
        }
    }
}

/// Reports that `signal` cannot be watched, so it still ends the process at
/// once and leaves the process's test stacks behind.
fn warn_unwatched(signal: &str, error: &dyn Display) {
    report(format_args!(
        "warning: carddemo-testkit cannot watch for {signal}, so a run it interrupts \
         leaves its Docker networks and containers behind: {error}"
    ));
}

/// Reports that the stack network `name` could not be removed, with the
/// command that finishes the job.
fn warn_network_left(name: &str, error: &dyn Display) {
    report(format_args!(
        "warning: carddemo-testkit left the Docker network {name} behind \
         (remove it with `docker network rm {name}`): {error}"
    ));
}

/// Writes `message`, and a newline, to the process's stderr: every notice and
/// warning of this module goes through it.
///
/// It writes to [`io::stderr`] itself, because `eprintln!` and the other
/// print macros honour the test harness's output capture, which every thread a
/// test spawns inherits. The signal thread, spawned inside the first test that
/// creates a stack, would write into that test's capture buffer, which the
/// teardown's [`process::exit`] discards unprinted. A [`StackNetwork`]'s
/// `Drop`, on the test's thread and on its removal thread, would write into a
/// buffer that a passing test never prints. A Docker resource left behind
/// outlives the run, so no test outcome may hide it.
fn report(message: fmt::Arguments<'_>) {
    // A failed write is ignored: a report must never panic, because it runs
    // in `Drop` too, where a panic during unwinding aborts the process.
    let _ = writeln!(io::stderr().lock(), "{message}");
}

/// Creates the bridge network `name`, labelled [`NETWORK_LABEL`], with
/// [`HOST_BINDING_OPTION`] set to [`NETWORK_HOST_BINDING`], for
/// [`StackNetwork::create`].
///
/// # Errors
///
/// Returns the error if the Docker daemon cannot be reached or refuses the
/// network, for example because one of that name exists.
async fn create_network(name: &str) -> Result<(), DockerError> {
    docker_client()?
        .create_network(CreateNetworkOptions {
            name,
            check_duplicate: true,
            driver: NETWORK_DRIVER,
            options: HashMap::from([(HOST_BINDING_OPTION, NETWORK_HOST_BINDING)]),
            labels: HashMap::from([NETWORK_LABEL]),
            ..CreateNetworkOptions::default()
        })
        .await?;
    Ok(())
}

/// Removes the Docker network `name` through [`teardown_network`], on a client
/// and a current-thread runtime of its own, for [`StackNetwork`]'s `Drop`.
///
/// # Errors
///
/// Returns the error if the runtime cannot be built, the Docker daemon cannot
/// be reached, or [`teardown_network`] fails.
fn remove_network(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async { teardown_network(&docker_client()?, name).await })?;
    Ok(())
}

/// Tears the stack network `name` down, for [`StackNetwork`]'s `Drop` and the
/// signal teardown alike: it force-removes every container still on the
/// network, through [`remove_stack_containers`], unless
/// `TESTCONTAINERS_COMMAND` is `keep`, and then removes the network. Kept
/// containers are kept as testcontainers keeps its own, and the network stays
/// while one of them is attached. A network that is already gone, because the
/// other of those two removed it first, counts as removed.
///
/// # Errors
///
/// Returns the error if the Docker daemon cannot be reached or refuses to
/// remove the network, for example because a container kept by
/// `TESTCONTAINERS_COMMAND=keep`, or one that could not be removed, is still
/// attached. A container that could not be looked up or removed is reported on
/// stderr instead.
async fn teardown_network(docker: &Docker, name: &str) -> Result<(), DockerError> {
    if !keeps_containers() {
        remove_stack_containers(docker, name).await;
    }
    match docker.remove_network(name).await {
        Err(error) if has_status(&error, &[NOT_FOUND]) => Ok(()),
        removed => removed,
    }
}

/// Tears the stack network `name` down through [`teardown_network`], on a
/// client of its own, and reports a failure on stderr.
async fn teardown_or_warn(name: &str) {
    let removed = match docker_client() {
        Ok(docker) => teardown_network(&docker, name).await,
        Err(error) => Err(error),
    };
    if let Err(error) = removed {
        warn_network_left(name, &error);
    }
}

/// A Docker client for the stack's own API calls, on the daemon `DOCKER_HOST`
/// names (the local socket by default), with [`DOCKER_CALL_TIMEOUT`].
///
/// # Errors
///
/// Returns the error if no client can be configured, for example because
/// `DOCKER_HOST` names an unsupported scheme.
fn docker_client() -> Result<Docker, DockerError> {
    Ok(Docker::connect_with_defaults()?.with_timeout(DOCKER_CALL_TIMEOUT))
}

/// Whether testcontainers keeps the containers it starts instead of removing
/// them on drop, as it does when [`TESTCONTAINERS_COMMAND`] is
/// [`KEEP_COMMAND`].
fn keeps_containers() -> bool {
    env::var_os(TESTCONTAINERS_COMMAND).is_some_and(|command| command == KEEP_COMMAND)
}

/// Force-removes, with their anonymous volumes, the containers on the stack
/// network `name`, for [`teardown_network`]. They are the containers attached to
/// it, which Docker requires gone before it removes the network, and those
/// created on it but never started, which a start that timed out leaves
/// behind, as does one whose container Docker created after a signal
/// teardown's first pass. Docker lists those under the network's name even
/// once the network is gone, so the final sweep of that teardown finds them.
/// After a failed or cut-short start testcontainers keeps no handle on such a
/// container, so nothing else removes it. The network's name is unique to its
/// stack, so no other container is touched.
///
/// A container that is already gone, or whose removal is already in progress,
/// as testcontainers' own drop may have it, counts as removed. A lookup or a
/// removal that fails is reported on stderr, with the command that finishes
/// the job, and the network's removal is still attempted.
async fn remove_stack_containers(docker: &Docker, name: &str) {
    let (attached, listed) = tokio::join!(
        docker.inspect_network(name, None::<InspectNetworkOptions<&str>>),
        stack_containers(docker, name),
    );
    let mut ids = BTreeSet::new();
    match attached {
        Ok(network) => ids.extend(network.containers.unwrap_or_default().into_keys()),
        // A network that is gone has no container attached.
        Err(error) if has_status(&error, &[NOT_FOUND]) => {}
        Err(error) => warn_unlisted(name, &error),
    }
    match listed {
        Ok(containers) => ids.extend(containers.into_iter().filter_map(|container| container.id)),
        Err(error) => warn_unlisted(name, &error),
    }
    for id in ids {
        force_remove(docker, &id, name).await;
    }
}

/// The containers, in any state, that Docker lists on the stack network
/// `name`, for [`remove_stack_containers`] and [`remove_failed_start`].
/// Docker's `network` filter also matches a prefix of a network ID, so only a
/// container that names this network is kept.
///
/// # Errors
///
/// Returns the error if the Docker daemon cannot be reached or refuses the
/// listing.
async fn stack_containers(
    docker: &Docker,
    name: &str,
) -> Result<Vec<ContainerSummary>, DockerError> {
    let listed = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters: HashMap::from([("network", vec![name])]),
            ..ListContainersOptions::default()
        }))
        .await?;
    Ok(listed
        .into_iter()
        .filter(|container| {
            container
                .network_settings
                .as_ref()
                .and_then(|settings| settings.networks.as_ref())
                .is_some_and(|networks| networks.contains_key(name))
        })
        .collect())
}

/// Force-removes the container `id` of the stack network `name`, with its
/// anonymous volumes. A container that is already gone, or whose removal is
/// already in progress, as testcontainers' own drop may have it, counts as
/// removed. A removal that fails is reported on stderr, with the command that
/// finishes the job.
async fn force_remove(docker: &Docker, id: &str, name: &str) {
    let removal = docker
        .remove_container(
            id,
            Some(RemoveContainerOptions {
                force: true,
                v: true,
                ..RemoveContainerOptions::default()
            }),
        )
        .await;
    match removal {
        Ok(()) => {}
        Err(error) if has_status(&error, &[NOT_FOUND, CONFLICT]) => {}
        Err(error) => report(format_args!(
            "warning: carddemo-testkit left the Docker container {id} of the network \
             {name} behind (remove it with `docker rm -f -v {id}`): {error}"
        )),
    }
}

/// Reports that the containers on the stack network `name` could not be looked
/// up, so any of them may outlive the stack.
fn warn_unlisted(name: &str, error: &DockerError) {
    report(format_args!(
        "warning: carddemo-testkit cannot look up the containers on the Docker network \
         {name}, so any of them may be left behind (list them with \
         `docker ps -a --filter network={name}`): {error}"
    ));
}

/// Whether `error` is a Docker API response with one of `statuses`.
fn has_status(error: &DockerError, statuses: &[u16]) -> bool {
    matches!(
        error,
        DockerError::DockerResponseServerError { status_code, .. }
            if statuses.contains(status_code)
    )
}

/// Whether `error` reports a Docker API call of a testcontainers start that ran
/// out of its client timeout, bollard's fixed 120 seconds: the lookup or
/// creation of the network, or the creation, start or inspection of the
/// container. Such a call failed on the daemon's load rather than on the
/// request, so [`start_retrying`] retries the start.
///
/// A start that ran out of [`CONTAINER_START_TIMEOUT`] is not such a call, and
/// neither is a Docker error response or any other failure: a retry would
/// multiply the start budget, or fail again the same way.
fn is_client_timeout(error: &TestcontainersError) -> bool {
    matches!(
        error,
        TestcontainersError::Client(
            ClientError::ListNetworks(DockerError::RequestTimeoutError)
                | ClientError::InspectNetwork(DockerError::RequestTimeoutError)
                | ClientError::CreateNetwork(DockerError::RequestTimeoutError)
                | ClientError::CreateContainer(DockerError::RequestTimeoutError)
                | ClientError::StartContainer(DockerError::RequestTimeoutError)
                | ClientError::InspectContainer(DockerError::RequestTimeoutError)
        )
    )
}

/// A container start that [`start_retrying`] gave up on.
struct StartFailure<E> {
    /// The error of the last attempt.
    error: E,
    /// The attempts made, the last one included.
    attempts: usize,
}

/// Runs `start`, one attempt of the start of the container `image`, until an
/// attempt succeeds, fails with an error that `retried` rejects, or fails once
/// more than `backoff` has pauses, and returns that attempt's outcome.
///
/// After each failure that `retried` accepts, short of the last attempt, it
/// reports the failure on stderr, naming `image` and the attempt, waits that
/// retry's pause from `backoff`, and runs `cleanup`, which removes what the
/// failed attempt left behind, before the next attempt. The cleanup runs after
/// the pause, so it also finds a container that the daemon created late, after
/// its client had given up on the call. Neither the pause nor the cleanup is
/// part of an attempt, so a [`ContainerStart`] that `start` holds is in flight
/// during the attempts only.
///
/// # Errors
///
/// Returns the last attempt's error and the number of attempts made.
///
/// # Panics
///
/// Panics, as [`tokio::time::sleep`] does, if a pause runs outside a tokio
/// runtime whose time driver is enabled.
async fn start_retrying<T, E, Start, Started, Cleanup, Cleaned>(
    image: &str,
    backoff: &[Duration],
    mut start: Start,
    retried: impl Fn(&E) -> bool,
    mut cleanup: Cleanup,
) -> Result<T, StartFailure<E>>
where
    E: Display,
    Start: FnMut() -> Started,
    Started: Future<Output = Result<T, E>>,
    Cleanup: FnMut() -> Cleaned,
    Cleaned: Future<Output = ()>,
{
    let mut pauses = backoff.iter();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let error = match start().await {
            Ok(started) => return Ok(started),
            Err(error) => error,
        };
        let pause = match pauses.next() {
            Some(pause) if retried(&error) => *pause,
            _ => return Err(StartFailure { error, attempts }),
        };
        report(format_args!(
            "warning: carddemo-testkit's start of the {image} test container failed on a \
             Docker client timeout in attempt {attempts} of {}, so it is retried in \
             {pause:?}: {error}",
            backoff.len() + 1
        ));
        tokio::time::sleep(pause).await;
        cleanup().await;
    }
}

/// Force-removes, with their anonymous volumes, the containers of `image`
/// that Docker lists on the stack network `network`, before [`start_retrying`]
/// retries a start of that image: a timed-out attempt may have left one there,
/// created and never started, or started and no longer tracked. Only
/// containers of `image` are touched, because the stack's other container may
/// be starting on the same network meanwhile, as in
/// [`TestStack::with_rabbitmq`].
///
/// Nothing is removed with `TESTCONTAINERS_COMMAND=keep`, which keeps every
/// container, or once a signal teardown has begun, which removes the network
/// with every container on it. A container that is already gone, or whose
/// removal is already in progress, counts as removed. A lookup or removal that
/// fails is reported on stderr and the retry still goes ahead. A container that
/// the daemon creates only after the lookup is left to the stack network's
/// teardown.
async fn remove_failed_start(network: &str, image: &str) {
    if keeps_containers() || signal_teardown_started() {
        return;
    }
    let listed = match docker_client() {
        Ok(docker) => stack_containers(&docker, network)
            .await
            .map(|containers| (docker, containers)),
        Err(error) => Err(error),
    };
    let (docker, containers) = match listed {
        Ok(listed) => listed,
        Err(error) => {
            warn_unlisted(network, &error);
            return;
        }
    };
    let left = containers
        .into_iter()
        .filter(|container| container.image.as_deref() == Some(image))
        .filter_map(|container| container.id);
    for id in left {
        force_remove(&docker, &id, network).await;
    }
}

/// Starts the `PostgreSQL` container on `network`, connects a pool and migrates
/// it, returning the container, the database URL and the pool.
///
/// The testcontainers start runs through [`start_retrying`], so a start that a
/// Docker client timeout fails is retried after each pause of
/// [`START_BACKOFF`], three attempts at most. Each attempt is a
/// [`ContainerStart`] in flight until it returns, so a signal teardown waits
/// for it before its final sweep removes a container that Docker created after
/// the teardown's first pass. Once that teardown has begun, the next attempt
/// panics before it creates anything.
async fn start_postgres(network: &str) -> (ContainerAsync<Postgres>, String, PgPool) {
    let descriptor = format!("{POSTGRES_IMAGE}:{POSTGRES_TAG}");
    let image = descriptor.as_str();
    let started = start_retrying(
        image,
        &START_BACKOFF,
        // A request is consumed by its start, so each attempt builds its own.
        move || async move {
            let request = Postgres::default()
                .with_db_name(POSTGRES_DB)
                .with_user(POSTGRES_USER)
                .with_password(POSTGRES_PASSWORD)
                .with_tag(POSTGRES_TAG)
                .with_network(network)
                .with_startup_timeout(CONTAINER_START_TIMEOUT);
            let start = ContainerStart::begin(image, network);
            let started = request.start().await;
            drop(start);
            started
        },
        is_client_timeout,
        move || remove_failed_start(network, image),
    )
    .await;
    let container = started.unwrap_or_else(|failure| {
        panic!(
            "cannot start the {image} test container in {} attempt(s) \
             (is the Docker daemon running?): {}",
            failure.attempts, failure.error
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

/// Starts the `RabbitMQ` container on `network` with the two repository files
/// mounted read-only, returning the container and its AMQP URL.
///
/// The testcontainers start is retried after a Docker client timeout, and each
/// attempt is a [`ContainerStart`] in flight until it returns, as in
/// [`start_postgres`].
async fn start_rabbitmq(
    network: &str,
    definitions: String,
    conf: String,
) -> (ContainerAsync<GenericImage>, String) {
    let descriptor = format!("{RABBITMQ_IMAGE}:{RABBITMQ_TAG}");
    let image = descriptor.as_str();
    let (definitions, conf) = (definitions.as_str(), conf.as_str());
    let started = start_retrying(
        image,
        &START_BACKOFF,
        // A request is consumed by its start, so each attempt builds its own.
        move || async move {
            let request = GenericImage::new(RABBITMQ_IMAGE, RABBITMQ_TAG)
                .with_exposed_port(AMQP_PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout(RABBITMQ_READY))
                .with_mount(
                    Mount::bind_mount(definitions, RABBITMQ_DEFINITIONS_TARGET)
                        .with_access_mode(AccessMode::ReadOnly),
                )
                .with_mount(
                    Mount::bind_mount(conf, RABBITMQ_CONF_TARGET)
                        .with_access_mode(AccessMode::ReadOnly),
                )
                .with_env_var("RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS", RABBITMQ_ERL_ARGS)
                .with_network(network)
                .with_startup_timeout(CONTAINER_START_TIMEOUT);
            let start = ContainerStart::begin(image, network);
            let started = request.start().await;
            drop(start);
            started
        },
        is_client_timeout,
        move || remove_failed_start(network, image),
    )
    .await;
    let container = started.unwrap_or_else(|failure| {
        panic!(
            "cannot start the {image} test container in {} attempt(s) \
             (is the Docker daemon running?): {}",
            failure.attempts, failure.error
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::env;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::process::{self, Child, Command, ExitStatus, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use testcontainers::bollard::Docker;
    use testcontainers::bollard::container::{
        Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions,
        RemoveContainerOptions, StartContainerOptions,
    };
    use testcontainers::bollard::errors::Error as DockerError;
    use testcontainers::bollard::models::{ContainerStateStatusEnum, HostConfig};
    use testcontainers::bollard::network::InspectNetworkOptions;
    use testcontainers::core::error::{ClientError, WaitContainerError};
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{GenericImage, ImageExt, TestcontainersError};

    use super::{
        AMQP_PORT, CREATIONS_WAIT, ContainerStart, Creation, NETWORK_NAME_PREFIX, NOT_FOUND,
        POSTGRES_IMAGE, POSTGRES_PORT, POSTGRES_TAG, StackNetwork, TESTCONTAINERS_COMMAND,
        TestStack, create_network, docker_client, has_status, is_client_timeout, start_retrying,
        sweep_networks, teardown_network, wait_for_creations,
    };
    use crate::block_on;

    /// The bound of the creation waits that must run out: long enough to tell
    /// a wait that ran out from one that returned at once, short enough not to
    /// slow the suite.
    const SHORT_WAIT: Duration = Duration::from_millis(200);

    /// Asserts that `in_flight`, an open call of the kind `kind`, keeps
    /// [`wait_for_creations`] waiting until its bound, and that dropping it
    /// while a wait on the full [`CREATIONS_WAIT`] runs ends that wait long
    /// before its bound: with the last call in flight, other tests' included.
    fn assert_wait_follows(kind: &str, in_flight: impl Send) {
        let started = Instant::now();
        let unsettled = wait_for_creations(SHORT_WAIT);
        let waited = started.elapsed();
        assert!(
            unsettled >= 1,
            "the wait reported {unsettled} calls in flight while a {kind} was open"
        );
        assert!(
            waited >= SHORT_WAIT,
            "with a {kind} open, the wait returned after {waited:?}, before its bound of \
             {SHORT_WAIT:?}"
        );

        let started = Instant::now();
        let unsettled = thread::scope(|scope| {
            scope.spawn(move || {
                thread::sleep(SHORT_WAIT);
                drop(in_flight);
            });
            wait_for_creations(CREATIONS_WAIT)
        });
        let waited = started.elapsed();
        assert_eq!(
            unsettled, 0,
            "calls were still in flight after the wait on a {kind}"
        );
        assert!(
            waited < CREATIONS_WAIT,
            "the wait returned only at its bound of {CREATIONS_WAIT:?}, not once the {kind} \
             ended"
        );
    }

    #[test]
    fn the_creation_wait_ends_at_its_bound_or_once_the_creations_settle() {
        // Neither call reaches Docker: each stays in flight while its waits run.
        let name = format!("{NETWORK_NAME_PREFIX}-{}-creation-wait", process::id());
        let creation =
            Creation::register(&name).expect("no signal teardown runs in the test process");
        // Settled as a failed creation, which deregisters the name; the creation
        // stays in flight until it is dropped.
        assert!(
            !creation.settle(false),
            "no signal teardown runs in the test process"
        );
        assert_wait_follows("network creation", creation);
        let start = ContainerStart::begin(&format!("{POSTGRES_IMAGE}:{POSTGRES_TAG}"), &name);
        assert_wait_follows("container start", start);
    }

    /// testcontainers' error for a call of `kind` that ran out of its Docker
    /// client timeout, as a busy daemon makes it.
    fn timed_out(kind: fn(DockerError) -> ClientError) -> TestcontainersError {
        TestcontainersError::Client(kind(DockerError::RequestTimeoutError))
    }

    /// testcontainers' error for a container creation that ran out of its
    /// Docker client timeout, the failure the retry exists for.
    fn create_timed_out() -> TestcontainersError {
        timed_out(ClientError::CreateContainer)
    }

    /// testcontainers' error for a start that ran out of its start budget.
    fn startup_timed_out() -> TestcontainersError {
        TestcontainersError::WaitContainer(WaitContainerError::StartupTimeout)
    }

    #[test]
    fn only_a_docker_client_timeout_makes_a_start_retry() {
        let cases = [
            (
                "network list timeout",
                timed_out(ClientError::ListNetworks),
                true,
            ),
            (
                "network inspection timeout",
                timed_out(ClientError::InspectNetwork),
                true,
            ),
            (
                "network creation timeout",
                timed_out(ClientError::CreateNetwork),
                true,
            ),
            ("container creation timeout", create_timed_out(), true),
            (
                "container start timeout",
                timed_out(ClientError::StartContainer),
                true,
            ),
            (
                "container inspection timeout",
                timed_out(ClientError::InspectContainer),
                true,
            ),
            (
                "container creation refused",
                TestcontainersError::Client(ClientError::CreateContainer(
                    DockerError::DockerResponseServerError {
                        status_code: 500,
                        message: String::from("the daemon failed the creation"),
                    },
                )),
                false,
            ),
            ("start budget run out", startup_timed_out(), false),
        ];
        for (case, error, retried) in &cases {
            assert_eq!(is_client_timeout(error), *retried, "{case}: {error}");
        }
    }

    /// A pause of zero before each of the two retries of a fake start, so the
    /// retry tests run at once.
    const NO_BACKOFF: [Duration; 2] = [Duration::ZERO; 2];

    /// The error of each attempt of a fake start, by the attempt's number from
    /// 1, or `None` for an attempt that succeeds.
    type FakeAttempts = fn(usize) -> Option<TestcontainersError>;

    #[test]
    fn a_start_is_retried_after_client_timeouts_only_and_at_most_three_times() {
        // Each case: the attempts' errors; the outcome, the attempt that
        // succeeded or the attempts made with the last error; and the attempts
        // made.
        let cases: [(&str, FakeAttempts, _, usize); 4] = [
            (
                "two client timeouts, then a start",
                |attempt| (attempt < 3).then(create_timed_out),
                Ok(3),
                3,
            ),
            (
                "client timeouts only",
                |_| Some(create_timed_out()),
                Err((3, create_timed_out().to_string())),
                3,
            ),
            (
                "a run-out start budget",
                |_| Some(startup_timed_out()),
                Err((1, startup_timed_out().to_string())),
                1,
            ),
            (
                "a client timeout, then a run-out start budget",
                |attempt| {
                    Some(if attempt < 2 {
                        create_timed_out()
                    } else {
                        startup_timed_out()
                    })
                },
                Err((2, startup_timed_out().to_string())),
                2,
            ),
        ];
        for (case, fails, expected, attempts) in cases {
            let (mut made, mut cleanups) = (0, 0);
            let outcome = block_on(start_retrying(
                "fake:retry",
                &NO_BACKOFF,
                || {
                    made += 1;
                    let attempt = made;
                    async move { fails(attempt).map_or(Ok(attempt), Err) }
                },
                is_client_timeout,
                || {
                    cleanups += 1;
                    async {}
                },
            ))
            .map_err(|failure| (failure.attempts, failure.error.to_string()));
            assert_eq!(outcome, expected, "{case}");
            assert_eq!(made, attempts, "{case}: attempts made");
            assert_eq!(
                cleanups,
                attempts - 1,
                "{case}: a cleanup runs before each retry and at no other time"
            );
        }
    }

    /// The only host address a stack's containers may publish on. Written out
    /// rather than taken from the module, so that a changed binding fails here.
    const LOOPBACK: &str = "127.0.0.1";

    /// The Docker bridge driver option that sets the default host binding.
    const HOST_BINDING_OPTION: &str = "com.docker.network.bridge.host_binding_ipv4";

    /// Asserts that container `id` runs on `network`, publishes `port`, and binds
    /// every published port to [`LOOPBACK`] only.
    fn assert_loopback_only(docker: &Docker, id: &str, network: &str, port: u16) {
        let inspected = block_on(docker.inspect_container(id, None::<InspectContainerOptions>))
            .unwrap_or_else(|error| panic!("cannot inspect the test container {id}: {error}"));
        let network_mode = inspected.host_config.and_then(|config| config.network_mode);
        assert_eq!(
            network_mode.as_deref(),
            Some(network),
            "container {id} is not on the stack network"
        );
        let ports = inspected
            .network_settings
            .and_then(|settings| settings.ports)
            .unwrap_or_default();
        let published = format!("{port}/tcp");
        assert!(
            ports
                .get(&published)
                .and_then(Option::as_ref)
                .is_some_and(|bindings| !bindings.is_empty()),
            "container {id} does not publish {published}: {ports:?}"
        );
        for (container_port, bindings) in &ports {
            for binding in bindings.iter().flatten() {
                assert_eq!(
                    binding.host_ip.as_deref(),
                    Some(LOOPBACK),
                    "container {id} publishes {container_port} beyond loopback: {binding:?}"
                );
            }
        }
    }

    #[test]
    fn stack_publishes_on_loopback_only_and_removes_its_network() {
        let docker = Docker::connect_with_defaults()
            .unwrap_or_else(|error| panic!("cannot reach the Docker daemon: {error}"));
        let stack = block_on(TestStack::with_rabbitmq());
        let network = stack
            .network
            .as_ref()
            .expect("a new stack owns its network")
            .name()
            .to_owned();
        let postgres = stack
            .postgres
            .as_ref()
            .expect("a new stack holds its PostgreSQL container")
            .id()
            .to_owned();
        let rabbitmq = stack
            .rabbitmq
            .as_ref()
            .expect("a stack from with_rabbitmq holds its broker container")
            .id()
            .to_owned();

        assert_loopback_only(&docker, &postgres, &network, POSTGRES_PORT);
        assert_loopback_only(&docker, &rabbitmq, &network, AMQP_PORT);
        let created =
            block_on(docker.inspect_network(&network, None::<InspectNetworkOptions<&str>>))
                .unwrap_or_else(|error| {
                    panic!("cannot inspect the stack network {network}: {error}")
                });
        assert_eq!(created.driver.as_deref(), Some("bridge"));
        assert_eq!(
            created
                .options
                .as_ref()
                .and_then(|options| options.get(HOST_BINDING_OPTION))
                .map(String::as_str),
            Some(LOOPBACK),
            "the stack network {network} lacks the loopback host binding"
        );

        drop(stack);
        let removed =
            block_on(docker.inspect_network(&network, None::<InspectNetworkOptions<&str>>));
        assert!(
            matches!(
                removed,
                Err(DockerError::DockerResponseServerError {
                    status_code: 404,
                    ..
                })
            ),
            "the stack network {network} outlived the stack: {removed:?}"
        );
    }

    /// Whether `result` is the Docker API's answer for a container or network
    /// that does not exist.
    fn is_not_found<T>(result: &Result<T, DockerError>) -> bool {
        matches!(
            result,
            Err(DockerError::DockerResponseServerError {
                status_code: 404,
                ..
            })
        )
    }

    /// The IDs of the containers, in any state, that Docker lists on `network`.
    fn containers_on(docker: &Docker, network: &str) -> Vec<String> {
        let listed = block_on(docker.list_containers(Some(ListContainersOptions {
            all: true,
            filters: HashMap::from([("network", vec![network])]),
            ..ListContainersOptions::default()
        })))
        .unwrap_or_else(|error| panic!("cannot list the containers on {network}: {error}"));
        listed
            .into_iter()
            .filter_map(|container| container.id)
            .collect()
    }

    /// Creates a container on `network` through the Docker API alone, without
    /// starting it, so that no testcontainers handle removes it, and returns its
    /// ID.
    async fn create_untracked(docker: &Docker, network: &str) -> String {
        docker
            .create_container(
                None::<CreateContainerOptions<String>>,
                Config {
                    image: Some(format!("{POSTGRES_IMAGE}:{POSTGRES_TAG}")),
                    cmd: Some(vec![String::from("sleep"), String::from("infinity")]),
                    host_config: Some(HostConfig {
                        network_mode: Some(network.to_owned()),
                        ..HostConfig::default()
                    }),
                    ..Config::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("cannot create a container on {network}: {error}"))
            .id
    }

    /// Starts a container on `network` through the Docker API alone, so that no
    /// testcontainers handle removes it, as after a drop cut short, and returns
    /// its ID. It keeps running, so it holds an endpoint on the network.
    async fn start_untracked(docker: &Docker, network: &str) -> String {
        let id = create_untracked(docker, network).await;
        docker
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .unwrap_or_else(|error| panic!("cannot start the container {id}: {error}"));
        id
    }

    /// A Docker network and the containers a test created on it by hand. Its
    /// `Drop` force-removes those containers, with their anonymous volumes, and
    /// then the network, so that a failed check leaks nothing; one already gone
    /// counts as removed.
    struct Leftovers {
        docker: Docker,
        network: String,
        containers: Vec<String>,
    }

    impl Drop for Leftovers {
        fn drop(&mut self) {
            block_on(async {
                for id in &self.containers {
                    let removed = self
                        .docker
                        .remove_container(
                            id,
                            Some(RemoveContainerOptions {
                                force: true,
                                v: true,
                                ..RemoveContainerOptions::default()
                            }),
                        )
                        .await;
                    match removed {
                        Ok(()) => {}
                        Err(error) if has_status(&error, &[NOT_FOUND]) => {}
                        Err(error) => eprintln!("cannot remove the test container {id}: {error}"),
                    }
                }
                match self.docker.remove_network(&self.network).await {
                    Ok(()) => {}
                    Err(error) if has_status(&error, &[NOT_FOUND]) => {}
                    Err(error) => {
                        eprintln!("cannot remove the test network {}: {error}", self.network);
                    }
                }
            });
        }
    }

    #[test]
    fn the_final_sweep_removes_a_container_its_removed_network_left_behind() {
        let docker = docker_client()
            .unwrap_or_else(|error| panic!("cannot reach the Docker daemon: {error}"));
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        // Named as a stack network, and never registered in `LIVE_NETWORKS`.
        let name = format!("{NETWORK_NAME_PREFIX}-{}-{nanos}-sweep", process::id());
        let mut leftovers = Leftovers {
            docker: docker.clone(),
            network: name.clone(),
            containers: Vec::new(),
        };
        block_on(create_network(&name))
            .unwrap_or_else(|error| panic!("cannot create the test network {name}: {error}"));
        let created = block_on(create_untracked(&docker, &name));
        leftovers.containers.push(created.clone());

        // Docker removes a network that only a created, never started,
        // container names and keeps that container: the state a container start
        // that outran a signal teardown's first pass leaves behind.
        block_on(docker.remove_network(&name)).unwrap_or_else(|error| {
            panic!("cannot remove the test network {name} under its created container: {error}")
        });
        let kept = block_on(docker.inspect_container(&created, None::<InspectContainerOptions>))
            .unwrap_or_else(|error| {
                panic!("the created container {created} went with its network {name}: {error}")
            });
        assert_eq!(
            kept.state.and_then(|state| state.status),
            Some(ContainerStateStatusEnum::CREATED),
            "the container {created} on {name} is not merely created"
        );

        let mut left = BTreeSet::from([name.clone()]);
        block_on(sweep_networks(&mut left));
        assert!(left.is_empty(), "the sweep kept {left:?} as not torn down");
        let swept = block_on(docker.inspect_container(&created, None::<InspectContainerOptions>));
        assert!(
            is_not_found(&swept),
            "the container {created} outlived the final sweep of {name}: {swept:?}"
        );
    }

    #[test]
    fn network_teardown_removes_the_containers_left_on_it() {
        let docker = docker_client()
            .unwrap_or_else(|error| panic!("cannot reach the Docker daemon: {error}"));
        // Created first, so that a panic below still sweeps the network.
        let network = block_on(StackNetwork::create());
        let name = network.name().to_owned();

        // A start budget of zero runs out before `docker start` answers, so
        // testcontainers fails as on a saturated daemon: after creating the
        // container, and without a handle that would remove it.
        let failed = block_on(
            GenericImage::new(POSTGRES_IMAGE, POSTGRES_TAG)
                .with_network(name.as_str())
                .with_startup_timeout(Duration::ZERO)
                .start(),
        );
        assert!(
            matches!(
                failed,
                Err(TestcontainersError::WaitContainer(
                    WaitContainerError::StartupTimeout
                ))
            ),
            "a zero start budget did not time out: {failed:?}"
        );
        let running = block_on(start_untracked(&docker, &name));
        let attached = block_on(docker.inspect_network(&name, None::<InspectNetworkOptions<&str>>))
            .unwrap_or_else(|error| panic!("cannot inspect the stack network {name}: {error}"));
        assert!(
            attached
                .containers
                .unwrap_or_default()
                .contains_key(&running),
            "the running container {running} is not attached to {name}"
        );
        let left = containers_on(&docker, &name);
        assert_eq!(
            left.len(),
            2,
            "{name} should hold the failed start's container and {running}: {left:?}"
        );
        assert!(left.contains(&running), "{running} is not listed on {name}");

        drop(network);
        for id in &left {
            let inspected = block_on(docker.inspect_container(id, None::<InspectContainerOptions>));
            assert!(
                is_not_found(&inspected),
                "the container {id} outlived the stack network {name}: {inspected:?}"
            );
        }
        let removed = block_on(docker.inspect_network(&name, None::<InspectNetworkOptions<&str>>));
        assert!(
            is_not_found(&removed),
            "the stack network {name} outlived its teardown: {removed:?}"
        );
    }

    /// The test that re-executes this test binary as its own child.
    const SIGNAL_TEST: &str = "a_signal_removes_the_live_stack_and_exits_with_its_status";

    /// Set, through `Command::env`, only in that child, which then holds a
    /// stack until a signal ends it.
    const SIGNAL_CHILD: &str = "CARDDEMO_TESTKIT_SIGNAL_CHILD";

    /// The text before the network and the container in the child's stdout
    /// line that reports its stack.
    const STACK_MARKER: &str = "carddemo-testkit signal child stack:";

    /// How long the child holds its stack before it stops waiting for the
    /// signal and returns, so that it never runs forever.
    const CHILD_LIFETIME: Duration = Duration::from_secs(300);

    /// How long the parent waits for the child's stack: the network's creation
    /// and the container's start, 300 seconds each at most, plus the migrations.
    const STACK_WAIT: Duration = Duration::from_secs(660);

    /// How long the parent waits for the child to exit, which covers the
    /// teardown's Docker calls after the signal.
    const EXIT_WAIT: Duration = Duration::from_secs(300);

    /// Interval of the parent's checks for the child's exit.
    const EXIT_POLL: Duration = Duration::from_millis(100);

    /// A run of this test binary as the child of [`SIGNAL_TEST`], holding one
    /// stack until a signal ends it. Its `Drop` kills a child that still runs
    /// and removes whatever is left of the stack, so a failed check leaks
    /// nothing.
    struct SignalChild {
        process: Child,
        /// The child's stdout lines, read on a thread of their own so that the
        /// child never blocks on a full pipe.
        lines: mpsc::Receiver<String>,
        /// The thread that collects the child's stderr until the child exits;
        /// `None` once [`SignalChild::finish`] has joined it.
        stderr: Option<JoinHandle<String>>,
        /// The child's stack network and `PostgreSQL` container, once reported.
        stack: Option<(String, String)>,
    }

    impl SignalChild {
        /// Starts this test binary on [`SIGNAL_TEST`] alone, as the child, with
        /// `TESTCONTAINERS_COMMAND` unset so that its stack is not kept. The
        /// child keeps the harness's output capture, as a normal run does, so
        /// its teardown notice on stderr shows that the notices bypass it.
        fn spawn() -> Self {
            let exe = env::current_exe()
                .unwrap_or_else(|error| panic!("cannot locate the test binary: {error}"));
            let tests = module_path!()
                .split_once("::")
                .map_or(module_path!(), |(_, path)| path);
            let filter = format!("{tests}::{SIGNAL_TEST}");
            let mut process = Command::new(&exe)
                .args([filter.as_str(), "--exact", "--test-threads=1"])
                .env(SIGNAL_CHILD, "1")
                .env_remove(TESTCONTAINERS_COMMAND)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|error| panic!("cannot start {}: {error}", exe.display()));
            let stdout = process.stdout.take().expect("the child's stdout is piped");
            let stderr = process.stderr.take().expect("the child's stderr is piped");
            let (sender, lines) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    // The receiver lives until the child is gone.
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
            let stderr = thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = BufReader::new(stderr).read_to_end(&mut bytes);
                let text = String::from_utf8_lossy(&bytes);
                match read {
                    Ok(_) => text.into_owned(),
                    Err(error) => format!("{text}\n(the rest of stderr is unreadable: {error})"),
                }
            });
            Self {
                process,
                lines,
                stderr: Some(stderr),
                stack: None,
            }
        }

        /// Waits, within [`STACK_WAIT`], for the child's stack, and returns its
        /// network and container.
        fn stack(&mut self) -> (String, String) {
            let deadline = Instant::now() + STACK_WAIT;
            loop {
                let line = match self
                    .lines
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(line) => line,
                    Err(RecvTimeoutError::Timeout) => {
                        panic!("the child reported no stack within {STACK_WAIT:?}")
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        let (status, stderr) = self.finish();
                        panic!("the child ended without reporting a stack ({status:?}):\n{stderr}")
                    }
                };
                // The harness may print the test's name on the same line.
                let Some((_, reported)) = line.split_once(STACK_MARKER) else {
                    continue;
                };
                let mut words = reported.split_whitespace().map(str::to_owned);
                let (Some(network), Some(container)) = (words.next(), words.next()) else {
                    panic!("the child's stack line is malformed: {line}");
                };
                self.stack = Some((network.clone(), container.clone()));
                return (network, container);
            }
        }

        /// Sends `signal` (`TERM` or `INT`) to the child with `kill`, then
        /// returns what [`SignalChild::finish`] does.
        fn signal(&mut self, signal: &str) -> (Option<ExitStatus>, String) {
            let pid = self.process.id().to_string();
            let sent = Command::new("kill")
                .args([format!("-{signal}").as_str(), pid.as_str()])
                .status()
                .unwrap_or_else(|error| panic!("cannot run kill: {error}"));
            assert!(sent.success(), "kill -{signal} {pid} failed: {sent}");
            self.finish()
        }

        /// Waits, within [`EXIT_WAIT`], for the child to exit, killing it if it
        /// does not, and returns its exit status, `None` when it was killed,
        /// with everything it wrote to stderr.
        fn finish(&mut self) -> (Option<ExitStatus>, String) {
            let deadline = Instant::now() + EXIT_WAIT;
            let status = loop {
                match self.process.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if Instant::now() < deadline => thread::sleep(EXIT_POLL),
                    Ok(None) => {
                        self.kill();
                        break None;
                    }
                    Err(error) => panic!("cannot wait for the child: {error}"),
                }
            };
            let stderr = self.stderr.take().map_or_else(String::new, |reader| {
                reader
                    .join()
                    .unwrap_or_else(|_| String::from("(the child's stderr reader panicked)"))
            });
            (status, stderr)
        }

        /// Kills and reaps the child, reporting a failure on stderr.
        fn kill(&mut self) {
            if let Err(error) = self.process.kill() {
                eprintln!("cannot kill the child test {}: {error}", self.process.id());
            }
            if let Err(error) = self.process.wait() {
                eprintln!("cannot reap the child test {}: {error}", self.process.id());
            }
        }
    }

    impl Drop for SignalChild {
        fn drop(&mut self) {
            if matches!(self.process.try_wait(), Ok(None)) {
                self.kill();
            }
            let Some((network, container)) = self.stack.take() else {
                return;
            };
            let docker = match docker_client() {
                Ok(docker) => docker,
                Err(error) => {
                    eprintln!(
                        "cannot reach Docker to remove the child's container {container} and \
                         network {network}: {error}"
                    );
                    return;
                }
            };
            block_on(async {
                let removed = docker
                    .remove_container(
                        &container,
                        Some(RemoveContainerOptions {
                            force: true,
                            v: true,
                            ..RemoveContainerOptions::default()
                        }),
                    )
                    .await;
                match removed {
                    Ok(()) => {}
                    Err(error) if has_status(&error, &[NOT_FOUND]) => {}
                    Err(error) => {
                        eprintln!("cannot remove the child's container {container}: {error}");
                    }
                }
                if let Err(error) = teardown_network(&docker, &network).await {
                    eprintln!("cannot remove the child's network {network}: {error}");
                }
            });
        }
    }

    /// The child's part of [`SIGNAL_TEST`]: starts a `PostgreSQL` stack,
    /// reports its network and container on stdout, and holds it until the
    /// signal ends the process, or at most [`CHILD_LIFETIME`].
    fn hold_a_stack() {
        let stack = block_on(TestStack::postgres());
        let network = stack
            .network
            .as_ref()
            .expect("a new stack owns its network")
            .name();
        let container = stack
            .postgres
            .as_ref()
            .expect("a new stack holds its PostgreSQL container")
            .id();
        // Written to stdout itself, past the harness's output capture, which
        // the child runs under as a normal run does.
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{STACK_MARKER} {network} {container}")
            .and_then(|()| stdout.flush())
            .unwrap_or_else(|error| panic!("cannot report the stack on stdout: {error}"));
        drop(stdout);
        thread::sleep(CHILD_LIFETIME);
    }

    #[test]
    fn a_signal_removes_the_live_stack_and_exits_with_its_status() {
        if env::var_os(SIGNAL_CHILD).is_some() {
            hold_a_stack();
            return;
        }
        let docker = docker_client()
            .unwrap_or_else(|error| panic!("cannot reach the Docker daemon: {error}"));
        for (signal, status) in [("TERM", 143), ("INT", 130)] {
            let mut child = SignalChild::spawn();
            let (network, container) = child.stack();
            let (exited, stderr) = child.signal(signal);
            assert_eq!(
                exited.as_ref().and_then(ExitStatus::code),
                Some(status),
                "SIG{signal} did not end the child with status {status} ({exited:?}):\n{stderr}"
            );
            assert!(
                stderr.contains(&format!("SIG{signal} received")),
                "the child did not report its SIG{signal} teardown:\n{stderr}"
            );
            let left =
                block_on(docker.inspect_network(&network, None::<InspectNetworkOptions<&str>>));
            assert!(
                is_not_found(&left),
                "the stack network {network} outlived SIG{signal}: {left:?}"
            );
            let left =
                block_on(docker.inspect_container(&container, None::<InspectContainerOptions>));
            assert!(
                is_not_found(&left),
                "the container {container} outlived SIG{signal}: {left:?}"
            );
        }
    }
}
