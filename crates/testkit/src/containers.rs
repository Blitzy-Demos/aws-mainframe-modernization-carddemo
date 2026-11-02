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
//! The public API names only `sqlx::PgPool`, `lapin::Connection` and standard
//! library types. No testcontainers or `carddemo-persistence` type crosses it,
//! because the persistence suites reach this crate through a dev-dependency
//! cycle and would otherwise meet a second copy of every persistence type.

use std::collections::HashMap;
use std::error::Error;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use carddemo_persistence::seed::{SeedMode, SeedSources};
use carddemo_persistence::{Db, migrate, seed};
use lapin::{Connection, ConnectionProperties};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row, ValueRef};
use testcontainers::bollard::Docker;
use testcontainers::bollard::errors::Error as DockerError;
use testcontainers::bollard::network::CreateNetworkOptions;
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
/// Dropping the stack removes its containers and then its network. That works
/// in a plain `#[test]` after [`crate::block_on`], inside `block_on`, and in a
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
    /// start or become ready, its port cannot be looked up, the pool cannot
    /// connect, or a migration fails. As the panic unwinds, a container already
    /// started is removed, then the stack network.
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
    /// created) or its path is not UTF-8, or if the broker does not start or
    /// become ready or its AMQP port cannot be looked up.
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

/// A Docker bridge network that one [`TestStack`] creates and its containers
/// join. Its default host binding is [`NETWORK_HOST_BINDING`], so the ports its
/// containers publish without a host address listen on loopback only, where
/// Docker's default bridge would bind them to every host interface.
///
/// The network exists before testcontainers sees its name, so testcontainers
/// treats it as created elsewhere and never removes it; this guard's `Drop`
/// does.
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
        // Removal is a Docker API call, which needs a tokio runtime. Inside a
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
    Docker::connect_with_defaults()?
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
/// # Errors
///
/// Returns the error if the runtime cannot be built, the Docker daemon cannot
/// be reached, or it refuses the removal, for example because a container kept
/// by `TESTCONTAINERS_COMMAND=keep` is still attached.
fn remove_network(name: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime = Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async { Docker::connect_with_defaults()?.remove_network(name).await })?;
    Ok(())
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
    use testcontainers::bollard::Docker;
    use testcontainers::bollard::container::InspectContainerOptions;
    use testcontainers::bollard::errors::Error as DockerError;
    use testcontainers::bollard::network::InspectNetworkOptions;

    use super::{AMQP_PORT, POSTGRES_PORT, TestStack};
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
}
