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
//! Each container gets 300 seconds to start and become ready, instead of
//! testcontainers' default 60, and each of the stack's own Docker calls, such
//! as the network's creation and removal, gets 300 seconds instead of bollard's
//! default 120: a shared, busy Docker daemon can exceed both defaults.
//!
//! The public API names only `sqlx::PgPool`, `lapin::Connection` and standard
//! library types. No testcontainers or `carddemo-persistence` type crosses it,
//! because the persistence suites reach this crate through a dev-dependency
//! cycle and would otherwise meet a second copy of every persistence type.

use std::collections::{BTreeSet, HashMap};
use std::env;
use std::error::Error;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
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
use testcontainers::bollard::network::{CreateNetworkOptions, InspectNetworkOptions};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tokio::runtime::{Builder, Handle};

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
const CONTAINER_START_TIMEOUT: Duration = Duration::from_secs(300);

/// Client timeout of the stack's own Docker API calls: creating and removing
/// its network, and removing the containers left on it. bollard's default, 120
/// seconds, is shorter than a shared, busy Docker daemon can take for them,
/// and the daemon still completes a call its client gave up on: a network it
/// creates after that has no guard to remove it.
const DOCKER_CALL_TIMEOUT: Duration = Duration::from_secs(300);

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

/// A migrated `PostgreSQL` test database and, from [`TestStack::with_rabbitmq`],
/// a `RabbitMQ` broker, each in its own container. The containers join the
/// stack's own bridge network, so their published ports listen on `127.0.0.1`
/// only.
///
/// Dropping the stack removes its containers, force-removes any other
/// container still on its network, and then removes the network. That works in
/// a plain `#[test]` after [`crate::block_on`], inside `block_on`, and in a
/// `#[tokio::test]` of either runtime flavour.
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
    /// start and become ready within 300 seconds, its port cannot be looked up,
    /// the pool cannot connect, or a migration fails. As the panic unwinds,
    /// every container on the stack network, one whose start failed included,
    /// is force-removed, then the network.
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
    /// become ready within 300 seconds or its AMQP port cannot be looked up.
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
struct StackNetwork {
    name: String,
}

impl StackNetwork {
    /// Creates a uniquely named bridge network whose default host binding is
    /// [`NETWORK_HOST_BINDING`].
    ///
    /// # Panics
    ///
    /// Panics, naming the network and the error, if the Docker daemon cannot be
    /// reached or refuses to create the network.
    async fn create() -> Self {
        let sequence = NETWORK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = format!("{NETWORK_NAME_PREFIX}-{}-{nanos}-{sequence}", process::id());
        if let Err(error) = create_network(&name).await {
            panic!(
                "cannot create the test network {name} with host binding \
                 {NETWORK_HOST_BINDING} (is the Docker daemon running?): {error}"
            );
        }
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
        if let Err(error) = outcome {
            eprintln!(
                "warning: carddemo-testkit left the Docker network {name} behind \
                 (remove it with `docker network rm {name}`): {error}"
            );
        }
    }
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

/// Removes the Docker network `name` through a client and a current-thread
/// runtime of its own, for [`StackNetwork`]'s `Drop`.
///
/// Before the network, it force-removes every container still on it, through
/// [`remove_stack_containers`], unless `TESTCONTAINERS_COMMAND` is `keep`.
/// Then the containers are kept, as testcontainers keeps its own, and the
/// network stays while one of them is attached.
///
/// # Errors
///
/// Returns the error if the runtime cannot be built, the Docker daemon cannot
/// be reached, or it refuses to remove the network, for example because a
/// container kept by `TESTCONTAINERS_COMMAND=keep`, or one that could not be
/// removed, is still attached. A container that could not be looked up or
/// removed is reported on stderr instead.
fn remove_network(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let docker = docker_client()?;
        if !keeps_containers() {
            remove_stack_containers(&docker, name).await;
        }
        docker.remove_network(name).await
    })?;
    Ok(())
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
/// network `name`, for [`remove_network`]. They are the containers attached to
/// it, which Docker requires gone before it removes the network, and those
/// created on it but never started, which a start that timed out leaves
/// behind. After a failed or cut-short start testcontainers keeps no handle on
/// such a container, so nothing else removes it. The network's name is unique
/// to its stack, so no other container is touched.
///
/// A container that is already gone, or whose removal is already in progress,
/// as testcontainers' own drop may have it, counts as removed. A lookup or a
/// removal that fails is reported on stderr, with the command that finishes
/// the job, and the network's removal is still attempted.
async fn remove_stack_containers(docker: &Docker, name: &str) {
    let (attached, listed) = tokio::join!(
        docker.inspect_network(name, None::<InspectNetworkOptions<&str>>),
        docker.list_containers(Some(ListContainersOptions {
            all: true,
            filters: HashMap::from([("network", vec![name])]),
            ..ListContainersOptions::default()
        })),
    );
    let mut ids = BTreeSet::new();
    match attached {
        Ok(network) => ids.extend(network.containers.unwrap_or_default().into_keys()),
        // A network that is gone has no container attached.
        Err(error) if has_status(&error, &[NOT_FOUND]) => {}
        Err(error) => warn_unlisted(name, &error),
    }
    match listed {
        // Docker's `network` filter also matches a prefix of a network ID, so
        // only a container that names this network is kept.
        Ok(containers) => ids.extend(
            containers
                .into_iter()
                .filter(|container| {
                    container
                        .network_settings
                        .as_ref()
                        .and_then(|settings| settings.networks.as_ref())
                        .is_some_and(|networks| networks.contains_key(name))
                })
                .filter_map(|container| container.id),
        ),
        Err(error) => warn_unlisted(name, &error),
    }
    for id in ids {
        let removal = docker
            .remove_container(
                &id,
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
            Err(error) => eprintln!(
                "warning: carddemo-testkit left the Docker container {id} of the network \
                 {name} behind (remove it with `docker rm -f -v {id}`): {error}"
            ),
        }
    }
}

/// Reports that the containers on the stack network `name` could not be looked
/// up, so any of them may outlive the stack.
fn warn_unlisted(name: &str, error: &DockerError) {
    eprintln!(
        "warning: carddemo-testkit cannot look up the containers on the Docker network \
         {name}, so any of them may be left behind (list them with \
         `docker ps -a --filter network={name}`): {error}"
    );
}

/// Whether `error` is a Docker API response with one of `statuses`.
fn has_status(error: &DockerError, statuses: &[u16]) -> bool {
    matches!(
        error,
        DockerError::DockerResponseServerError { status_code, .. }
            if statuses.contains(status_code)
    )
}

/// Starts the `PostgreSQL` container on `network`, connects a pool and migrates
/// it, returning the container, the database URL and the pool.
async fn start_postgres(network: &str) -> (ContainerAsync<Postgres>, String, PgPool) {
    let container = Postgres::default()
        .with_db_name(POSTGRES_DB)
        .with_user(POSTGRES_USER)
        .with_password(POSTGRES_PASSWORD)
        .with_tag(POSTGRES_TAG)
        .with_network(network)
        .with_startup_timeout(CONTAINER_START_TIMEOUT)
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

/// Starts the `RabbitMQ` container on `network` with the two repository files
/// mounted read-only, returning the container and its AMQP URL.
async fn start_rabbitmq(
    network: &str,
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
        .with_network(network)
        .with_startup_timeout(CONTAINER_START_TIMEOUT)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use testcontainers::bollard::Docker;
    use testcontainers::bollard::container::{
        Config, CreateContainerOptions, InspectContainerOptions, ListContainersOptions,
        StartContainerOptions,
    };
    use testcontainers::bollard::errors::Error as DockerError;
    use testcontainers::bollard::models::HostConfig;
    use testcontainers::bollard::network::InspectNetworkOptions;
    use testcontainers::core::error::WaitContainerError;
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{GenericImage, ImageExt, TestcontainersError};

    use super::{
        AMQP_PORT, POSTGRES_IMAGE, POSTGRES_PORT, POSTGRES_TAG, StackNetwork, TestStack,
        docker_client,
    };
    use crate::block_on;

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

    /// Starts a container on `network` through the Docker API alone, so that no
    /// testcontainers handle removes it, as after a drop cut short, and returns
    /// its ID. It keeps running, so it holds an endpoint on the network.
    async fn start_untracked(docker: &Docker, network: &str) -> String {
        let created = docker
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
            .unwrap_or_else(|error| panic!("cannot create a container on {network}: {error}"));
        docker
            .start_container(&created.id, None::<StartContainerOptions<String>>)
            .await
            .unwrap_or_else(|error| panic!("cannot start the container {}: {error}", created.id));
        created.id
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
}
