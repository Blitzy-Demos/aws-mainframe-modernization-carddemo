//! Plans of the paginated queries and of the previous release's query set.
//!
//! - Bounded Pagination: every file of `queries/`, which the repositories read through
//!   `sqlx::query_file_as!`, is served by an index. With `enable_seqscan` off, the planner picks an
//!   index whenever one can serve a statement, even on the near-empty tables of a freshly migrated
//!   container, so a plan holding a sequential scan names a query that no index serves.
//! - Single Schema Executor: every query the previous release shipped, kept as its
//!   `.sqlx/query-*.json` copies in `compat/previous/`, still plans against the schema that the
//!   migrations directory produces. The first release has no predecessor, so `BASELINE.md`
//!   declares the baseline commit `59cc6c2` with an empty query set instead.
//!
//! Each container's schema comes from the `test-support` Postgres harness, never from this file.
//! `EXPLAIN (GENERIC_PLAN)` plans a statement whose `$n` placeholders stay unbound. The extended
//! protocol would take them as parameters of the `EXPLAIN` itself and demand values, so the
//! statements go through the simple-query protocol of `sqlx::raw_sql`. That makes this file the
//! one exemption from the workspace scan for non-macro SQL calls, and it issues only `SET` and
//! `EXPLAIN` statements built from committed files. `EXPLAIN` without `ANALYZE` executes nothing,
//! so even a previous-release `INSERT`, `UPDATE` or `DELETE` stays read-only.
//!
//! Failure messages carry file names, plan node types and the server's planning errors, which
//! name schema objects only: planning reads no row (Cardholder Data Redaction).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sqlx::{PgConnection, Row};

/// Every paginated query of `queries/`, sorted. A new paginated query is added here deliberately,
/// so that it cannot ship without its plan being checked.
const QUERY_FILES: [&str; 9] = [
    "cards_page_backward.sql",
    "cards_page_forward.sql",
    "db2_transaction_types_page_backward.sql",
    "db2_transaction_types_page_forward.sql",
    "pending_auth_details_page_forward.sql",
    "transactions_page_backward.sql",
    "transactions_page_forward.sql",
    "users_page_backward.sql",
    "users_page_forward.sql",
];

/// The line of `compat/previous/BASELINE.md` that declares the first release's empty
/// previous-release query set.
const BASELINE_DECLARATION: &str = "Previous release: 59cc6c2 (empty query set)";

/// Plan nodes that read through an index. A backward scan is an `Index Scan` whose
/// `Scan Direction` is `Backward`.
const INDEX_NODES: [&str; 3] = ["Index Scan", "Index Only Scan", "Bitmap Index Scan"];

/// The plan node of a sequential scan, which no paginated query may contain.
const SEQ_SCAN: &str = "Seq Scan";

#[tokio::test]
async fn paginated_queries_are_served_by_an_index() {
    let dir = queries_dir();
    assert_eq!(
        file_names(&dir, is_sql_file),
        QUERY_FILES,
        "queries/ and QUERY_FILES list different paginated queries"
    );

    let guard = test_support::postgres::start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let mut conn = guard
        .pool()
        .acquire()
        .await
        .expect("acquire a connection on the migrated database");
    // A session setting: every plan below runs on this one connection.
    sqlx::raw_sql("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .expect("disable sequential scans for this session");

    for name in QUERY_FILES {
        let text = fs::read_to_string(dir.join(name))
            .unwrap_or_else(|err| panic!("read queries/{name}: {err}"));
        let types = plan_node_types(&mut conn, name, &text).await;
        assert!(
            types
                .iter()
                .any(|node| INDEX_NODES.contains(&node.as_str())),
            "{name}: no index scan in plan {types:?}"
        );
        assert!(
            types.iter().all(|node| node != SEQ_SCAN),
            "{name}: sequential scan in plan {types:?}"
        );
    }
}

#[tokio::test]
async fn previous_release_queries_plan_against_the_migrated_schema() {
    let dir = previous_dir();
    assert!(
        dir.is_dir(),
        "compat/previous/ is missing: it must hold the previous release's query-*.json copies \
         or BASELINE.md"
    );
    let copies = file_names(&dir, is_query_copy);

    let guard = test_support::postgres::start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let mut conn = guard
        .pool()
        .acquire()
        .await
        .expect("acquire a connection on the migrated database");

    if copies.is_empty() {
        assert_baseline_declared(&dir);
    } else {
        for name in &copies {
            plan_previous_query(&mut conn, &dir, name).await;
        }
    }
}

/// Plans `text`, the committed file `name`, with `EXPLAIN (GENERIC_PLAN, FORMAT JSON)` and returns
/// every node type of its plan tree, outermost first.
async fn plan_node_types(conn: &mut PgConnection, name: &str, text: &str) -> Vec<String> {
    let sql = format!("EXPLAIN (GENERIC_PLAN, FORMAT JSON)\n{text}");
    let row = sqlx::raw_sql(&sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap_or_else(|err| panic!("{name}: EXPLAIN failed: {err}"));
    // The column is `json`, which `String` decodes only unchecked: the simple-query protocol
    // returns it as text.
    let json = row
        .try_get_unchecked::<String, _>(0)
        .unwrap_or_else(|err| panic!("{name}: read the plan: {err}"));
    let document: Value =
        serde_json::from_str(&json).unwrap_or_else(|err| panic!("{name}: parse the plan: {err}"));
    let plan = document
        .as_array()
        .and_then(|statements| statements.first())
        .unwrap_or_else(|| panic!("{name}: the plan document is not a non-empty array"));
    let mut types = Vec::new();
    node_types(plan, &mut types);
    types
}

/// Appends the `Node Type` of `plan` and of every node beneath it, reached through `Plan` and
/// `Plans`, to `out` in depth-first order.
fn node_types(plan: &Value, out: &mut Vec<String>) {
    if let Some(node_type) = plan.get("Node Type").and_then(Value::as_str) {
        out.push(node_type.to_owned());
    }
    if let Some(inner) = plan.get("Plan") {
        node_types(inner, out);
    }
    if let Some(children) = plan.get("Plans").and_then(Value::as_array) {
        for child in children {
            node_types(child, out);
        }
    }
}

/// The first-release case: `BASELINE.md` declares the baseline commit with an empty query set.
fn assert_baseline_declared(dir: &Path) {
    let baseline = fs::read_to_string(dir.join("BASELINE.md")).unwrap_or_else(|err| {
        panic!("compat/previous/ holds no query-*.json copy, and BASELINE.md cannot be read: {err}")
    });
    assert!(
        baseline
            .lines()
            .any(|line| line.trim_end() == BASELINE_DECLARATION),
        "compat/previous/BASELINE.md lacks the line {BASELINE_DECLARATION:?}"
    );
}

/// Plans the `query` text of the previous release's sqlx metadata file `name` against the
/// migrated schema; a column, table or type it relies on that the migrations no longer create
/// fails the plan.
async fn plan_previous_query(conn: &mut PgConnection, dir: &Path, name: &str) {
    let text = fs::read_to_string(dir.join(name))
        .unwrap_or_else(|err| panic!("read compat/previous/{name}: {err}"));
    let metadata: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("compat/previous/{name}: parse the metadata: {err}"));
    let query = metadata
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("compat/previous/{name}: no top-level \"query\" string"));
    let sql = format!("EXPLAIN (GENERIC_PLAN)\n{query}");
    let plan = sqlx::raw_sql(&sql)
        .fetch_all(&mut *conn)
        .await
        .unwrap_or_else(|err| panic!("compat/previous/{name}: EXPLAIN failed: {err}"));
    assert!(
        !plan.is_empty(),
        "compat/previous/{name}: EXPLAIN returned no plan"
    );
}

/// The `queries/` directory of this crate.
fn queries_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("queries")
}

/// The `compat/previous/` directory of this crate.
fn previous_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("compat")
        .join("previous")
}

/// A paginated query file: `*.sql`.
fn is_sql_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "sql")
}

/// A copy of the previous release's sqlx offline metadata: `query-*.json`.
fn is_query_copy(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "json")
        && path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|file| file.starts_with("query-"))
}

/// The names of the regular files of `dir` that `keep` accepts, sorted.
fn file_names(dir: &Path, keep: fn(&Path) -> bool) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("list {}: {err}", dir.display()));
    let mut names = Vec::new();
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| panic!("list {}: {err}", dir.display()))
            .path();
        if path.is_file() && keep(&path) {
            let name = path
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_else(|| panic!("{}: the file name is not UTF-8", path.display()));
            names.push(name.to_owned());
        }
    }
    names.sort();
    names
}
