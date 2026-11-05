//! The two versions of the transaction reference data (AAP 0.3.2): an edit of a type in the
//! Db2 master reaches the TRANREPT report only after `batch seed --only tranextr`.
//!
//! CTTU (COTRTUPC) writes the Db2 master `TRANSACTION_TYPE` (P-COTRTUPC-007), while CBTRN03C
//! reads the VSAM processing copies `TRANTYPE` and `TRANCATG` (P-CBTRN03C-001) and moves
//! their descriptions into every detail line (P-CBTRN03C-004, P-CBTRN03C-007). The two
//! versions start different: the Db2 load holds `PURCHASE` for type `01` (P-TRNTYPE-004),
//! the fixture `Purchase` (P-CVTRA03Y-005). TRANEXTR unloads both masters, and the
//! `TRANTYPE` and `TRANCATG` jobs reload the processing copies from that unload
//! [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L62-122], so only then do the reports show
//! the masters' texts.
//!
//! The test posts the fixture day, edits type `01` through `PUT /admin/transaction-types/01`,
//! reports, copies the masters and reports again. The edit goes through the real API: the
//! test has Cargo build the `api` binary of `carddemo-api`, which `carddemo-batch` may not
//! depend on, runs it against the test database on a loopback port the system picks, signs
//! on as `ADMIN001` and sends the PUT without `origin`, so the COTRTUPC path of the handler
//! runs (P-COTRTUPC-007). HTTP/1.1 and JSON are spoken with the standard library alone, and
//! the server is killed once the edit is answered and its handler span is written.

pub mod common;

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::ops::Range;
use std::os::unix::process::CommandExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use carddemo_audit::Document;
use carddemo_runtime::config::keys;
use carddemo_testkit::fixtures;
use common::{
    Harness, TRANREPT_CLOCK, block_on, expected_span, expected_value, parity, split_records,
};

/// The audit rows this test exercises.
///
/// TRANEXTR has no rows of its own: it is a seed job, replaced rather than parity-tested
/// (AAP 0.3.2), whose result is defined as the masters themselves. Its effect is therefore
/// checked against the masters row for row, and the seeded state of both versions is taken
/// from the rows of the Db2 load and the fixture.
const P_IDS: &[&str] = &[
    "P-CBTRN03C-001",
    "P-CBTRN03C-004",
    "P-CBTRN03C-007",
    "P-CBTRN03C-015",
    "P-CBTRN03C-020",
    "P-COTRTUPC-007",
    "P-COTRTUPC-017",
    "P-TRNTYPE-004",
    "P-TRNTYCAT-005",
    "P-CVTRA03Y-005",
];

/// The type the CTTU edit changes: `Purchase`, the type of most fixture records.
const TYPE_CODE: &str = "01";

/// The new description of [`TYPE_CODE`]: 15 characters, the width of `TRAN-REPORT-TYPE-DESC
/// X(15)` [app/cpy/CVTRA07Y.cpy:L22], unlike both `Purchase` and `PURCHASE` in those 15, and
/// only letters and a space, which the COTRTUPC description edit accepts (P-COTRTUPC-017).
const NEW_DESC: &str = "RETAIL PURCHASE";

/// The key of category `0001` of [`TYPE_CODE`], as [`descriptions`] builds category keys.
const CATEGORY_KEY: &str = "010001";

/// The report dataset and its record width, `FD-REPTFILE-REC X(133)`.
const REPORT: &str = "TRANREPT";
const REPORT_WIDTH: usize = 133;

/// The TRANREPT job with the JCL dates, and the TRANEXTR copy.
const TRANREPT: [&str; 1] = ["tranrept"];
const TRANEXTR: [&str; 3] = ["seed", "--only", "tranextr"];

/// The exit code of a POSTTRAN run that rejects records (AAP 0.5.3.5); the fixture day has
/// rejects.
const POSTTRAN_REJECTED: i32 = 4;

/// The `TRANSACTION-DETAIL-REPORT` fields this test reads, as byte offsets of the 133-byte
/// line [app/cpy/CVTRA07Y.cpy:L15-31]: the TRAN-ID, the type code, which the `'-'` filler
/// follows, its description, the category code, which a `'-'` follows too, and its
/// description.
const TRANS_ID: Range<usize> = 0..16;
const TYPE_CD: Range<usize> = 29..31;
const TYPE_DESC: Range<usize> = 32..47;
const CAT_CD: Range<usize> = 48..52;
const CAT_DESC: Range<usize> = 53..82;

/// The processing copy `trantype_file` and the master `transaction_types`, code and
/// description, in key order; columns as `migrations/0001_reference_data.sql` defines them.
const PROCESSING_TYPES_SQL: &str =
    "SELECT tran_type, tran_type_desc FROM trantype_file ORDER BY tran_type COLLATE \"C\"";
const MASTER_TYPES_SQL: &str =
    "SELECT tr_type, tr_description FROM transaction_types ORDER BY tr_type COLLATE \"C\"";

/// The processing copy `trancatg_file` and the master `transaction_categories`, type code,
/// four-digit category code and description, in key order. The copy keeps the category as the
/// `9(04)` number, so it is shown in the master's `CHAR(4)` form.
const PROCESSING_CATEGORIES_SQL: &str = "SELECT tran_type_cd, lpad(tran_cat_cd::text, 4, '0'), \
     tran_cat_type_desc FROM trancatg_file ORDER BY tran_type_cd COLLATE \"C\", tran_cat_cd";
const MASTER_CATEGORIES_SQL: &str = "SELECT trc_type_code, trc_type_category, trc_cat_data \
     FROM transaction_categories ORDER BY trc_type_code COLLATE \"C\", trc_type_category \
     COLLATE \"C\"";

/// Rows in their `PostgreSQL` text form.
type Rows = Vec<Vec<Option<String>>>;

/// The administrator the seed loads from the DUSRSECJ rows, and the password it hashes for
/// that user; only an administrator may call `/admin/**`.
const ADMIN_USER: &str = "ADMIN001";
const ADMIN_PASSWORD: &str = "PASSWORD";

/// The package and binary the test builds and runs as the API server.
const API_PACKAGE: &str = "carddemo-api";
const API_BIN: &str = "api";

/// The listen address the server is given: loopback, on a port the system picks, which its
/// `api listening` event then names.
const API_LISTEN: &str = "127.0.0.1:0";

/// The `api listening` event of the server's start-up, and the CICS transaction its
/// `PUT /admin/transaction-types/{id}` handler records without `origin`.
const API_LISTENING: &str = "api listening";
const CTTU: &str = "CTTU";

/// The bound on the `cargo build` of the `api` binary: 900 seconds. A build whose units are
/// fresh ends in a second, and one that compiles the package and the dependencies the
/// batch tests do not share takes about a minute. The margin is for a shared host running
/// many builds at once, or a Cargo that waits for another build's lock on the target
/// directory.
const API_BUILD_TIMEOUT: Duration = Duration::from_secs(900);

/// The bound from the start of the server to its `api listening` event: 120 seconds, a wide
/// margin over the connect to the test database and the bind.
const API_START_TIMEOUT: Duration = Duration::from_secs(120);

/// The bound on the connect, each write and each read of one HTTP exchange: 120 seconds.
/// The sign-on verifies an argon2 hash in an unoptimised build, which takes seconds.
const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the server may take, after its answer, to write the span line of the handler
/// that served the PUT: 30 seconds. The span ends as the handler returns, so it is usually
/// written before the answer.
const SPAN_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a child is checked for an exit.
const POLL: Duration = Duration::from_millis(20);

/// How long the output of a child that has ended is waited for: 10 seconds. Its pipes close
/// as it exits, so the wait is long only when a process that inherited them holds them.
const DRAIN_GRACE: Duration = Duration::from_secs(10);

/// How long the shell of [`stopping_a_process_also_kills_the_descendants_holding_its_stdout`]
/// may take to write the pid of its background `sleep`, and that `sleep` to be gone once the
/// shell is stopped: 10 seconds, a wide margin over a start and a `SIGKILL` on a loaded host.
const DESCENDANT_BOUND: Duration = Duration::from_secs(10);

#[test]
fn master_edit_reaches_report_only_after_tranextr() {
    let doc = parity();
    for id in P_IDS {
        assert!(
            doc.find_row(id).is_some(),
            "the parity audit has no row {id}"
        );
    }
    let h = Harness::new();
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), POSTTRAN_REJECTED);

    // The seeded state: the processing copy and the Db2 master differ in case.
    let types_before = rows(&h, PROCESSING_TYPES_SQL);
    let categories_before = rows(&h, PROCESSING_CATEGORIES_SQL);
    let old_desc = description_of(&types_before, TYPE_CODE);
    assert_eq!(old_desc, audited(&doc, "P-CVTRA03Y-005", "tran_type_desc"));
    let masters_before = rows(&h, MASTER_TYPES_SQL);
    assert_eq!(
        description_of(&masters_before, TYPE_CODE),
        audited(&doc, "P-TRNTYPE-004", "tr_description"),
    );

    // CTTU, through the API, updates the existing type in the master, and only there.
    let saved = cttu_put(&h, TYPE_CODE, NEW_DESC);
    assert_eq!(
        (saved.type_code.as_str(), saved.description.as_str()),
        (TYPE_CODE, NEW_DESC)
    );
    assert_eq!(saved.message, audited(&doc, "P-COTRTUPC-007", "info"));
    assert_eq!(saved.info, None, "the COTRTLIC path answered the PUT");
    assert_eq!(
        rows(&h, MASTER_TYPES_SQL),
        redescribed(&masters_before, TYPE_CODE, NEW_DESC),
        "CTTU inserted or removed a master type, or changed more than the description of {TYPE_CODE}"
    );
    assert_eq!(
        rows(&h, PROCESSING_TYPES_SQL),
        types_before,
        "CTTU changed the processing copy `trantype_file`"
    );

    // The first report still shows the descriptions of the processing copies.
    let before = report(&h, &doc);
    assert_eq!(
        before[4],
        moved(&audited(&doc, "P-CBTRN03C-020", "text"), REPORT_WIDTH),
        "report line 5 before TRANEXTR"
    );
    assert_descriptions(&before, &types_before, &categories_before, "report 1");
    assert_eq!(
        type_descs(&before, TYPE_CODE),
        BTreeSet::from([moved(&old_desc, TYPE_DESC.len())])
    );
    assert!(
        before.iter().all(|line| !line.contains(NEW_DESC)),
        "the report before TRANEXTR shows the master's {NEW_DESC:?}"
    );

    // TRANEXTR copies both masters over the processing copies.
    let _ = h.run_expect(&TRANEXTR, Some(TRANREPT_CLOCK), 0);
    let types_after = rows(&h, PROCESSING_TYPES_SQL);
    let categories_after = rows(&h, PROCESSING_CATEGORIES_SQL);
    assert_eq!(
        types_after,
        rows(&h, MASTER_TYPES_SQL),
        "`trantype_file` after TRANEXTR"
    );
    assert_eq!(
        categories_after,
        rows(&h, MASTER_CATEGORIES_SQL),
        "`trancatg_file` after TRANEXTR"
    );
    assert_eq!(
        description_of(&categories_after, CATEGORY_KEY),
        audited(&doc, "P-TRNTYCAT-005", "trc_cat_data"),
    );

    // The second report shows the masters' texts, the edit included, and nothing else changes.
    let after = report(&h, &doc);
    assert_descriptions(&after, &types_after, &categories_after, "report 2");
    assert_eq!(
        type_descs(&after, TYPE_CODE),
        BTreeSet::from([NEW_DESC.to_owned()])
    );
    for (number, (old, new)) in (1..).zip(before.iter().zip(&after)) {
        assert_eq!(
            masked(old),
            masked(new),
            "report line {number} changed outside the descriptions"
        );
    }

    let generations = h.generations(REPORT);
    let names: Vec<_> = generations
        .iter()
        .filter_map(|path| path.file_name())
        .collect();
    assert_eq!(names, ["TRANREPT.G0001V00", "TRANREPT.G0002V00"]);
}

/// Runs TRANREPT under the pinned clock and returns the lines of the newest `TRANREPT`
/// generation, checked against the audited width and line count (P-CBTRN03C-015). The
/// descriptions change no line count.
fn report(h: &Harness, doc: &Document) -> Vec<String> {
    let _ = h.run_expect(&TRANREPT, Some(TRANREPT_CLOCK), 0);
    let id = "P-CBTRN03C-015";
    assert_eq!(expected_value(doc, id, "width"), REPORT_WIDTH.to_string());
    let lines = split_records(&h.newest(REPORT), REPORT_WIDTH);
    assert_eq!(
        lines.len().to_string(),
        expected_value(doc, id, "lines"),
        "TRANREPT line count"
    );
    lines
}

/// Checks that every detail line of `report`, named `label` in a failure, shows the
/// descriptions `types` and `categories` hold for its codes, as `1120-WRITE-DETAIL` moves the
/// `TRAN-TYPE-DESC` and `TRAN-CAT-TYPE-DESC` the lookups read into the line (P-CBTRN03C-004,
/// P-CBTRN03C-007).
fn assert_descriptions(report: &[String], types: &Rows, categories: &Rows, label: &str) {
    let (types, categories) = (descriptions(types), descriptions(categories));
    let lookup = |table: &BTreeMap<String, String>, key: &str, width: usize| {
        let text = table
            .get(key)
            .unwrap_or_else(|| panic!("{label}: no reference row has the key {key:?}"));
        moved(text, width)
    };
    let mut details = 0_usize;
    for line in report.iter().filter(|line| is_detail(line)) {
        let type_cd = field(line, TYPE_CD);
        let category = format!("{type_cd}{}", field(line, CAT_CD));
        let type_desc = lookup(&types, type_cd, TYPE_DESC.len());
        let cat_desc = lookup(&categories, &category, CAT_DESC.len());
        assert_eq!(field(line, TYPE_DESC), type_desc, "{label}: {line:?}");
        assert_eq!(field(line, CAT_DESC), cat_desc, "{label}: {line:?}");
        details += 1;
    }
    assert!(details > 0, "{label} holds no detail line");
}

/// The distinct type descriptions the detail lines of `report` show for the type `code`.
fn type_descs(report: &[String], code: &str) -> BTreeSet<String> {
    report
        .iter()
        .filter(|line| is_detail(line) && field(line, TYPE_CD) == code)
        .map(|line| field(line, TYPE_DESC).to_owned())
        .collect()
}

/// `line` with the type and category descriptions of a detail line blanked, so two reports
/// compare equal when they differ only there.
fn masked(line: &str) -> String {
    let mut bytes = line.as_bytes().to_vec();
    if is_detail(line) {
        bytes[TYPE_DESC].fill(b' ');
        bytes[CAT_DESC].fill(b' ');
    }
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("report line {line:?}: {error}"))
}

/// Whether `line` is a `TRANSACTION-DETAIL-REPORT` line: a TRAN-ID of 16 digits, and the
/// `'-'` fillers after the type and the category codes. No header, separator or total line
/// has them.
fn is_detail(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes[TRANS_ID].iter().all(u8::is_ascii_digit)
        && bytes[TYPE_CD.end] == b'-'
        && bytes[CAT_CD.end] == b'-'
}

/// The text of the detail field at `range` of the 133-byte `line`.
fn field(line: &str, range: Range<usize>) -> &str {
    line.get(range)
        .unwrap_or_else(|| panic!("report line {line:?} is not ASCII at a CVTRA07Y field"))
}

/// `text` as a COBOL `MOVE` stores it in an `X(width)` field: cut on the right, or padded
/// there with spaces.
fn moved(text: &str, width: usize) -> String {
    format!("{text:<width$.width$}")
}

/// The rows `sql` reads from the test database.
fn rows(h: &Harness, sql: &str) -> Rows {
    block_on(h.stack().fetch_text_rows(sql))
}

/// The last column of every reference row, keyed by its other columns run together: the
/// description of a type code (`01`), or of a type and category code ([`CATEGORY_KEY`]).
fn descriptions(rows: &Rows) -> BTreeMap<String, String> {
    rows.iter()
        .map(|row| {
            let text = |cell: Option<&str>| {
                cell.unwrap_or_else(|| panic!("a null in reference row {row:?}"))
                    .to_owned()
            };
            let (description, key) = row
                .split_last()
                .unwrap_or_else(|| panic!("a reference row without columns"));
            let key: String = key.iter().map(|cell| text(cell.as_deref())).collect();
            (key, text(description.as_deref()))
        })
        .collect()
}

/// The description of the reference row whose key is `key`, as [`descriptions`] keys it.
fn description_of(rows: &Rows, key: &str) -> String {
    descriptions(rows)
        .remove(key)
        .unwrap_or_else(|| panic!("no reference row has the key {key:?}"))
}

/// The type rows `rows`, code and description, with the description of type `code` set to
/// `description`: the table as an update of that one type in place leaves it.
fn redescribed(rows: &Rows, code: &str, description: &str) -> Rows {
    let mut edited = rows.clone();
    let row = edited
        .iter_mut()
        .find(|row| row.first().and_then(Option::as_deref) == Some(code))
        .unwrap_or_else(|| panic!("no type row has the code {code:?}"));
    let cell = row
        .last_mut()
        .unwrap_or_else(|| panic!("type row {code:?} has no columns"));
    *cell = Some(description.to_owned());
    edited
}

/// The text of the quoted code span of `key=` in row `id`, without its quotes: the audited
/// value as the database stores it, before any padding to the span's width.
fn audited(doc: &Document, id: &str, key: &str) -> String {
    let span = expected_span(doc, id, key);
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("parity audit row {id}: {key}= holds no quoted text"))
        .to_owned()
}

/// What `PUT /admin/transaction-types/{id}` answered with 200: the type as stored, the
/// program's `message`, and `info`, which only the COTRTLIC path (`origin=list`) sends.
struct TypeSaved {
    type_code: String,
    description: String,
    message: String,
    info: Option<String>,
}

/// CTTU through the API: starts the `api` server on the test database of `h`, signs on as
/// [`ADMIN_USER`], sends `PUT /admin/transaction-types/{code}` with `description` and no
/// `origin`, and returns the 200 answer. The server is stopped on return.
///
/// Before that, the server must write the span line of the handler that served the PUT,
/// with the transaction `CTTU`, the type `code` and the outcome `ok`. The line is printed,
/// so the output of a `--nocapture` run shows the request reached the COTRTUPC handler.
///
/// # Panics
///
/// Panics as [`ApiServer`] does: when the server cannot be built, started or reached, when
/// an answer is not 200 or not the documented JSON, or when the handler span is missing or
/// records another outcome. The message shows the server's stderr and stdout.
fn cttu_put(h: &Harness, code: &str, description: &str) -> TypeSaved {
    let mut api = ApiServer::start(h.stack().database_url());
    let token = api.sign_on(ADMIN_USER, ADMIN_PASSWORD);
    let saved = api.update_type(&token, code, description);
    let span = api.handler_span(CTTU, code);
    println!("handler span the api wrote for the CTTU PUT: {span}");
    saved
}

/// The `api` binary of [`API_PACKAGE`], serving one test database on a loopback port the
/// system picks. Dropping it kills and reaps the process, also when the test panics.
struct ApiServer {
    process: Process,
    addr: SocketAddr,
}

impl ApiServer {
    /// Builds the binary ([`api_binary`]), starts it on `database_url`, and waits up to
    /// [`API_START_TIMEOUT`] for its `api listening` event, which names the bound address.
    ///
    /// Every setting the binary reads, [`keys::ALL`], is removed from its environment, so a
    /// value the shell exports, such as a `CARDDEMO_HTTP_ADDR` of a running stack, cannot
    /// point it elsewhere. It gets `CARDDEMO_DATABASE_URL`, `CARDDEMO_HTTP_ADDR`
    /// [`API_LISTEN`] and `RUST_LOG` `info`, at which the handler spans are written, and the
    /// defaults otherwise, the system clock included.
    ///
    /// The server is one process that starts no other, so it stays in the test's process
    /// group ([`Group::Inherited`]): killing it alone ends all of it, and the Ctrl-C of an
    /// interrupted run still reaches it.
    fn start(database_url: &str) -> Self {
        let mut command = Command::new(api_binary());
        command.current_dir(fixtures::repo_root());
        for key in keys::ALL {
            command.env_remove(key);
        }
        command
            .env(keys::DATABASE_URL, database_url)
            .env(keys::HTTP_ADDR, API_LISTEN)
            .env(keys::RUST_LOG, "info");
        let mut process = Process::spawn("api server", &mut command, Group::Inherited);
        let addr = process.wait_for(
            &format!("{API_LISTENING:?} event"),
            API_START_TIMEOUT,
            listening_addr,
        );
        Self { process, addr }
    }

    /// `POST /auth/login` as `user` with `password`: the bearer token of the 200 answer.
    fn sign_on(&mut self, user: &str, password: &str) -> String {
        let body = format!(
            "{{\"user_id\":{},\"password\":{}}}",
            json_string(user),
            json_string(password)
        );
        let (answer, _) = self.call("POST", "/auth/login", None, &body);
        match answer.member("token").and_then(Json::text) {
            Some(token) => token.to_owned(),
            None => self
                .process
                .fail("answered POST /auth/login with 200 but no token"),
        }
    }

    /// `PUT /admin/transaction-types/{code}` with `description`, the bearer `token` and no
    /// `origin`: the COTRTUPC update. Returns the 200 answer.
    fn update_type(&mut self, token: &str, code: &str, description: &str) -> TypeSaved {
        assert!(
            code.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "type code {code:?} would need percent-encoding in the path"
        );
        let path = format!("/admin/transaction-types/{code}");
        let body = format!("{{\"description\":{}}}", json_string(description));
        let (answer, text) = self.call("PUT", &path, Some(token), &body);
        let string = |value: Option<&Json>| value.and_then(Json::text).map(str::to_owned);
        let stored = answer.member("transaction_type");
        let info = match answer.member("info") {
            None => Ok(None),
            Some(Json::Text(info)) => Ok(Some(info.clone())),
            Some(_) => Err(()),
        };
        let fields = (
            string(stored.and_then(|stored| stored.member("type_code"))),
            string(stored.and_then(|stored| stored.member("description"))),
            string(answer.member("message")),
            info,
        );
        let (Some(type_code), Some(description), Some(message), Ok(info)) = fields else {
            self.process.fail(&format!(
                "answered PUT {path} with 200 but not a type, a message and an optional \
                 info text: {text}"
            ))
        };
        TypeSaved {
            type_code,
            description,
            message,
            info,
        }
    }

    /// Waits up to [`SPAN_TIMEOUT`] for the span line of the handler that served
    /// `transaction` for the type `code`, and returns it once it records the outcome `ok`.
    fn handler_span(&mut self, transaction: &str, code: &str) -> String {
        let what = format!("{transaction} handler span for type {code}");
        let (line, outcome) = self.process.wait_for(&what, SPAN_TIMEOUT, |line| {
            let span = Json::parse(line).ok()?;
            let attributes = span.member("attributes")?;
            let served = span.member("type")?.text()? == "span"
                && span.member("name")?.text()? == "handler"
                && attributes.member("transaction")?.text()? == transaction
                && attributes.member("type_code")?.text()? == code;
            let outcome = attributes.member("outcome").and_then(Json::text);
            served.then(|| (line.to_owned(), outcome.map(str::to_owned)))
        });
        if outcome.as_deref() != Some("ok") {
            self.process.fail(&format!(
                "recorded the outcome {outcome:?} on the {what}: {line}"
            ));
        }
        line
    }

    /// One HTTP/1.1 exchange on a connection of its own, which `Connection: close` ends
    /// after the answer: `method` `path` with the JSON `body`, and `Authorization: Bearer`
    /// `token` when one is given. Returns the parsed JSON of a 200 answer and its text.
    ///
    /// A failed exchange, a status other than 200 or a body that is not JSON fails the test
    /// through [`Process::fail`]. The message never shows the request, which carries the
    /// password or the token.
    fn call(
        &mut self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> (Json, String) {
        let authorization = token.map_or_else(String::new, |token| {
            format!("Authorization: Bearer {token}\r\n")
        });
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{authorization}Accept: \
             application/json\r\nContent-Type: application/json\r\nContent-Length: \
             {length}\r\nConnection: close\r\n\r\n{body}",
            addr = self.addr,
            length = body.len(),
        );
        let answer = exchange(self.addr, request.as_bytes())
            .map_err(|error| error.to_string())
            .and_then(|bytes| response(&bytes));
        let (status, text) = match answer {
            Ok(answer) => answer,
            Err(problem) => self
                .process
                .fail(&format!("did not answer {method} {path}: {problem}")),
        };
        if status != 200 {
            self.process
                .fail(&format!("answered {method} {path} with {status}: {text}"));
        }
        match Json::parse(&text) {
            Ok(json) => (json, text),
            Err(problem) => self.process.fail(&format!(
                "answered {method} {path} with a body that is not JSON ({problem}): {text}"
            )),
        }
    }
}

/// The address the `api listening` event line `line` names in its `addr` field; `None` for
/// any other line.
fn listening_addr(line: &str) -> Option<SocketAddr> {
    let event = Json::parse(line).ok()?;
    let fields = event.member("fields")?;
    if fields.member("message")?.text()? != API_LISTENING {
        return None;
    }
    fields.member("addr")?.text()?.parse().ok()
}

/// The `api` binary of [`API_PACKAGE`], built for this test by the Cargo that runs it.
///
/// `carddemo-batch` may not depend on `carddemo-api`, so `cargo test -p carddemo-batch`
/// builds no `api`. The test runs `cargo build --locked -p carddemo-api --bin api` on the
/// workspace manifest, into the target directory of this test, the parent of
/// `CARGO_TARGET_TMPDIR`, and with the test's environment, so the compiler flags, the
/// offline query data and the instrumentation of a coverage run are those of the test's own
/// build, and units already built are reused. Cargo releases its lock on the target
/// directory before it runs tests, so the nested build does not wait for the outer one. It
/// must end within [`API_BUILD_TIMEOUT`].
///
/// Cargo leads a process group of its own ([`Group::Own`]), which its `rustc` and linker
/// processes join, so a build stopped on a timeout or a panic ends with all of them.
///
/// The executable is the one the build's `compiler-artifact` message names for the `api`
/// bin target.
fn api_binary() -> PathBuf {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let target_dir = tmp
        .parent()
        .unwrap_or_else(|| panic!("CARGO_TARGET_TMPDIR {} has no parent", tmp.display()));
    let root = fixtures::repo_root();
    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "build",
            "--locked",
            "--package",
            API_PACKAGE,
            "--bin",
            API_BIN,
        ])
        .arg("--message-format=json-render-diagnostics")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(&root);
    let mut build = Process::spawn("cargo build of the api binary", &mut command, Group::Own);
    let status = build.wait_exit(API_BUILD_TIMEOUT);
    if !status.success() {
        build.fail(&format!("exited with {status}"));
    }
    let executables: Vec<PathBuf> = build
        .stdout
        .iter()
        .filter_map(|line| artifact_executable(line))
        .collect();
    match executables.as_slice() {
        [executable] => executable.clone(),
        _ => build.fail(&format!(
            "named {} executables of the {API_BIN} bin target instead of one",
            executables.len()
        )),
    }
}

/// The executable a Cargo `compiler-artifact` message line names for the [`API_BIN`] bin
/// target; `None` for any other line.
fn artifact_executable(line: &str) -> Option<PathBuf> {
    let message = Json::parse(line).ok()?;
    let target = message.member("target")?;
    let api_bin = message.member("reason")?.text()? == "compiler-artifact"
        && target.member("name")?.text()? == API_BIN
        && target
            .member("kind")?
            .items()?
            .iter()
            .any(|kind| kind.text() == Some("bin"));
    if !api_bin {
        return None;
    }
    message.member("executable")?.text().map(PathBuf::from)
}

/// Sends `request` to `addr` on a new connection, bounded by [`HTTP_TIMEOUT`] for the
/// connect, each write and each read, and reads the answer until the server closes it.
fn exchange(addr: SocketAddr, request: &[u8]) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect_timeout(&addr, HTTP_TIMEOUT)?;
    stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT))?;
    stream.write_all(request)?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The status code and the body text of the HTTP/1.1 response `bytes`, read to the close of
/// its connection.
///
/// The body is what follows the header section, and must be exactly `Content-Length` bytes
/// when that header is sent. A response with a `Transfer-Encoding`, chunked or another, is
/// refused rather than decoded: the API sends its JSON answers with their length.
fn response(bytes: &[u8]) -> Result<(u16, String), String> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("{} bytes hold no end of a header section", bytes.len()))?;
    let (head, body) = (&bytes[..split], &bytes[split + 4..]);
    let head = std::str::from_utf8(head)
        .map_err(|error| format!("the header section is not UTF-8: {error}"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .strip_prefix("HTTP/1.1 ")
        .and_then(|rest| rest.split(' ').next())
        .filter(|code| code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("{status_line:?} is not an HTTP/1.1 status line"))?;
    let mut length = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("{line:?} is not a header field"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(format!(
                "the body is sent with Transfer-Encoding {value:?}, which this client does \
                 not decode"
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value
                .parse::<usize>()
                .map_err(|error| format!("Content-Length {value:?}: {error}"))?;
            length = Some(parsed);
        }
    }
    if let Some(length) = length.filter(|&length| length != body.len()) {
        return Err(format!(
            "the body holds {} bytes, but Content-Length is {length}",
            body.len()
        ));
    }
    let text = String::from_utf8(body.to_vec())
        .map_err(|error| format!("the body is not UTF-8: {error}"))?;
    Ok((status, text))
}

/// A child process of the test, its stdout read line by line and its stderr to the end,
/// each on a thread of its own, so the child never blocks on a full pipe. The stdout lines
/// received so far are kept for checks and failure messages.
///
/// The child is stopped once, by [`Process::end`]: a `SIGKILL` to its process group when it
/// leads one ([`Group::Own`]), then one to the child, then the reap. Dropping a `Process`
/// stops a child that has not been stopped yet. The group ID equals the child's pid, and an
/// unreaped child keeps its pid even after it has exited, so no other process or group can
/// hold that ID while the child is unreaped. The child is therefore never reaped before its
/// group is signalled: [`Process::wait_exit`] sees an exit in `/proc` without reaping it,
/// and only [`kill_and_reap`] reaps, after the signals. A group ID a reap may have freed is
/// never signalled.
struct Process {
    /// What the child is, in messages.
    label: &'static str,
    child: Child,
    /// Whether the child leads a process group of its own.
    group: Group,
    /// When the child was started.
    started: Instant,
    /// The stdout lines from the reader thread, then the error that ended it, if any.
    lines: Receiver<io::Result<String>>,
    /// The stdout lines received so far, without their line endings.
    stdout: Vec<String>,
    /// The whole of stderr, or the error that ended its reading.
    stderr: Receiver<io::Result<Vec<u8>>>,
    /// How the stop of the child went, once [`Process::end`] has run: the reaped status, or
    /// what failed.
    ended: Option<Result<ExitStatus, String>>,
}

/// Whether the child of a [`Process`] leads a process group of its own.
///
/// A group lets one signal reach the processes the child starts, which outlive a child killed
/// alone and keep its inherited pipes open. Its cost: a Ctrl-C sends `SIGINT` to the
/// terminal's foreground process group, the test's, which ends the test without running any
/// `Drop`, and misses a child in a group of its own, which then runs on until it ends by
/// itself.
#[derive(Clone, Copy)]
enum Group {
    /// The child leads a new process group (`process_group(0)`), and its group is killed with
    /// it. For the `cargo build`, whose `rustc` and linker processes would outlive a Cargo
    /// killed alone; an interrupted run leaves the build to end by itself.
    Own,
    /// The child stays in the test's process group, still gets the Ctrl-C of an interrupted
    /// run, and is killed alone. For the API server, a single process that starts no other.
    Inherited,
}

impl Group {
    /// How a child stopped by [`kill_and_reap`] in this group mode ended, given the `status`
    /// it was reaped with.
    fn stopped(self, status: ExitStatus) -> String {
        match self {
            Self::Own => {
                format!("its process group was sent SIGKILL, and it was reaped with {status}")
            }
            Self::Inherited => format!("it was sent SIGKILL and reaped with {status}"),
        }
    }
}

impl Process {
    /// Starts `command`, named `label` in messages, in its own process group when `group` is
    /// [`Group::Own`], with an empty stdin and both outputs piped to their reader threads.
    fn spawn(label: &'static str, command: &mut Command, group: Group) -> Self {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Group::Own = group {
            command.process_group(0);
        }
        let started = Instant::now();
        let mut child = command.spawn().unwrap_or_else(|error| {
            panic!(
                "cannot start the {label} {}: {error}",
                Path::new(command.get_program()).display()
            )
        });
        let readers = match (child.stdout.take(), child.stderr.take()) {
            (Some(stdout), Some(stderr)) => lines_on_thread(stdout, label)
                .and_then(|lines| Ok((lines, bytes_on_thread(stderr, label)?))),
            _ => Err(io::Error::other("the child has no stdout or stderr pipe")),
        };
        match readers {
            Ok((lines, stderr)) => Self {
                label,
                child,
                group,
                started,
                lines,
                stdout: Vec::new(),
                stderr,
                ended: None,
            },
            Err(error) => {
                let pid = child.id();
                let ended = kill_and_reap(&mut child, group)
                    .map_or_else(|problem| problem, |status| group.stopped(status));
                panic!("cannot read the output of the {label} (pid {pid}): {error}; {ended}")
            }
        }
    }

    /// The first value `found` takes from a stdout line received within `bound` of now;
    /// `what` names the awaited line in a failure. Every line received is kept.
    fn wait_for<T>(
        &mut self,
        what: &str,
        bound: Duration,
        mut found: impl FnMut(&str) -> Option<T>,
    ) -> T {
        let until = Instant::now() + bound;
        loop {
            match self
                .lines
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(line)) => {
                    let value = found(&line);
                    self.stdout.push(line);
                    if let Some(value) = value {
                        return value;
                    }
                }
                Ok(Err(error)) => self.fail(&format!(
                    "could not be read while the test waited for the {what}: {error}"
                )),
                Err(RecvTimeoutError::Timeout) => {
                    self.fail(&format!("wrote no {what} within {bound:?}"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.fail(&format!("closed its stdout without writing the {what}"));
                }
            }
        }
    }

    /// Waits for the child to exit within `bound` of its start, checking every [`POLL`],
    /// then for the end of its stdout, then stops it ([`Process::end`]), and returns its
    /// exit status.
    ///
    /// The exit is seen without a reap ([`Process::exited`]), so a process of the child's
    /// group that still holds the stdout [`DRAIN_GRACE`] later, or runs on without it, is
    /// killed with the group while the unreaped child still reserves the group ID.
    fn wait_exit(&mut self, bound: Duration) -> ExitStatus {
        loop {
            match self.exited() {
                Ok(true) => break,
                Ok(false) if self.started.elapsed() >= bound => {
                    self.fail(&format!("did not exit within {bound:?}"));
                }
                Ok(false) => thread::sleep(POLL),
                Err(error) => self.fail(&format!("could not be checked for an exit: {error}")),
            }
        }
        let until = Instant::now() + DRAIN_GRACE;
        loop {
            match self
                .lines
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(line)) => self.stdout.push(line),
                Ok(Err(error)) => {
                    self.fail(&format!(
                        "exited, but its stdout could not be read: {error}"
                    ));
                }
                Err(RecvTimeoutError::Timeout) => self.fail(&format!(
                    "exited, but its stdout was still open {DRAIN_GRACE:?} later"
                )),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        match self.end() {
            Ok(status) => status,
            Err(_) => self.fail("exited, but could not be stopped"),
        }
    }

    /// Whether the child has exited, seen without reaping it: an exited child that is not
    /// reaped is a zombie, state `Z` in its `/proc/<pid>/stat` (Linux). A stopped child has
    /// exited.
    fn exited(&self) -> io::Result<bool> {
        if self.ended.is_some() {
            return Ok(true);
        }
        let pid = self.child.id();
        match proc_stat(pid)? {
            Some(stat) => Ok(stat.state == 'Z'),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the unreaped child {pid} has no /proc entry"),
            )),
        }
    }

    /// Stops the child ([`Process::stop`]), collects what it wrote within [`DRAIN_GRACE`],
    /// and panics with `problem`, the pid, how the child ended, its stderr and its stdout.
    fn fail(&mut self, problem: &str) -> ! {
        let (Ok(ended) | Err(ended)) = self.stop();
        let until = Instant::now() + DRAIN_GRACE;
        while let Ok(Ok(line)) = self
            .lines
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            self.stdout.push(line);
        }
        let stderr = match self
            .stderr
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Err(error)) => format!("<stderr could not be read: {error}>"),
            Err(RecvTimeoutError::Timeout) => {
                format!("<stderr was still open {DRAIN_GRACE:?} after the stop>")
            }
            Err(RecvTimeoutError::Disconnected) => {
                "<the stderr reader ended without a result>".to_owned()
            }
        };
        panic!(
            "the {} (pid {}) {problem}; {ended}\n--- stderr ---\n{stderr}\n--- stdout ---\n{}",
            self.label,
            self.child.id(),
            self.stdout.join("\n"),
        )
    }

    /// Stops the child ([`Process::end`]) and says how it ended: `Err` when a signal or the
    /// reap failed.
    fn stop(&mut self) -> Result<String, String> {
        let group = self.group;
        self.end().map(|status| group.stopped(status))
    }

    /// Stops the child with [`kill_and_reap`] on the first call, whether it is still running
    /// or has exited, which ends the processes left in its group too, and returns the reaped
    /// status or what failed. A later call returns the same without a signal: once the child
    /// is reaped, its group ID may be freed and taken by another process.
    fn end(&mut self) -> Result<ExitStatus, String> {
        self.ended
            .get_or_insert_with(|| kill_and_reap(&mut self.child, self.group))
            .clone()
    }
}

impl Drop for Process {
    /// Stops a child that has not been stopped yet ([`Process::stop`]), with its process
    /// group when it leads one, so none of its processes outlives the test, whether it passed
    /// or panicked; a process that has left the group (`setsid`, `setpgid`) is out of reach.
    /// A failed stop is reported on stderr; it changes no outcome.
    fn drop(&mut self) {
        if self.ended.is_none()
            && let Err(problem) = self.stop()
        {
            eprintln!(
                "cannot stop the {} (pid {}): {problem}",
                self.label,
                self.child.id()
            );
        }
    }
}

/// Kills `child` with `SIGKILL`, after its whole process group when it leads one
/// ([`Group::Own`]), and waits for it, so it leaves no zombie. `Ok` is the reaped status;
/// `Err` names the step that failed.
///
/// `child` must not have been reaped: an unreaped child, running or exited, keeps its pid,
/// so neither that pid nor the group ID equal to it can name another process while the
/// signals are sent. The child is also killed on its own, so a group signal that failed
/// cannot leave it running and the reap waiting for it. A `SIGKILL` to a child that has
/// already exited changes nothing about it: it is reaped with the status it exited with.
fn kill_and_reap(child: &mut Child, group: Group) -> Result<ExitStatus, String> {
    let grouped = match group {
        Group::Own => Some(kill_group(child.id())),
        Group::Inherited => None,
    };
    let killed = child.kill();
    match (grouped, killed, child.wait()) {
        (None | Some(Ok(())), Ok(()), Ok(status)) => Ok(status),
        (grouped, killed, reaped) => {
            let grouped = grouped.map_or_else(String::new, |grouped| {
                format!("the kill of its process group returned {grouped:?}, ")
            });
            Err(format!(
                "{grouped}kill returned {killed:?}, reap {reaped:?}"
            ))
        }
    }
}

/// Sends `SIGKILL` to every process of the process group `group` with the `kill` utility,
/// `kill -s KILL -- -<group>`. The standard library signals only a child of its own, and a
/// direct `kill(2)` call needs code the workspace lints forbid. `Err` describes a `kill`
/// that could not be run or failed.
fn kill_group(group: u32) -> Result<(), String> {
    let target = format!("-{group}");
    let output = Command::new("kill")
        .args(["-s", "KILL", "--", &target])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run kill: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "kill -s KILL -- {target} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim_end()
        ))
    }
}

/// The fields of a `/proc/<pid>/stat` line (Linux) that [`Process::exited`] and the tests
/// read.
#[derive(Debug)]
struct ProcStat {
    /// The state, field 3: `Z` for a zombie, an exited process that is not reaped yet.
    state: char,
    /// The start time in clock ticks after boot, field 22, which tells the process from a
    /// later one given the same pid.
    start: u64,
}

/// The [`ProcStat`] of the process `pid`; `None` when it has no `/proc` entry, as a reaped
/// process has not.
fn proc_stat(pid: u32) -> io::Result<Option<ProcStat>> {
    let path = format!("/proc/{pid}/stat");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // The command name, field 2, is in parentheses and may hold any byte, `)` and spaces
    // included, so the fields are counted after the last `)`, from field 3.
    let fields: Vec<&[u8]> = bytes
        .iter()
        .rposition(|&byte| byte == b')')
        .map(|end| {
            bytes[end + 1..]
                .split(u8::is_ascii_whitespace)
                .filter(|field| !field.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let text = |index: usize| {
        fields
            .get(index)
            .and_then(|field| std::str::from_utf8(field).ok())
    };
    let state = text(0).and_then(|state| {
        let mut chars = state.chars();
        chars.next().filter(|_| chars.next().is_none())
    });
    let start = text(19).and_then(|start| start.parse().ok());
    match (state, start) {
        (Some(state), Some(start)) => Ok(Some(ProcStat { state, start })),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{path} is not a stat line: {}",
                String::from_utf8_lossy(&bytes)
            ),
        )),
    }
}

/// A [`Group::Own`] shell whose background `sleep` holds the inherited stdout is stopped
/// together with that `sleep`, on both ways [`Process::wait_exit`] fails: the shell runs past
/// its bound, or it exits while the `sleep` keeps the stdout open past [`DRAIN_GRACE`], so it
/// is stopped after its exit. Each time the stop ends the `sleep`, and the stdout reader
/// disconnects within [`DRAIN_GRACE`]; a stop that killed the shell alone would leave the
/// `sleep` holding the pipe for ten minutes. It needs `sh`, `sleep`, `kill` and `/proc`, and
/// neither Docker nor a build.
#[test]
fn stopping_a_process_also_kills_the_descendants_holding_its_stdout() {
    // The script, the bound given to `wait_exit`, and the problem its failure names. The
    // shell that exits gets a bound far beyond the time it takes to exit.
    let running = Duration::from_secs(2);
    let exiting = Duration::from_secs(60);
    let cases = [
        (
            "sleep 600 & echo $!; wait",
            running,
            format!("did not exit within {running:?}"),
        ),
        (
            "sleep 600 & echo $!",
            exiting,
            format!("but its stdout was still open {DRAIN_GRACE:?} later"),
        ),
    ];
    for (script, bound, problem) in cases {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        let mut shell = Process::spawn("shell", &mut command, Group::Own);
        let sleep: u32 = shell.wait_for("pid of its sleep", DESCENDANT_BOUND, |line| {
            line.parse().ok()
        });
        let before = match proc_stat(sleep) {
            Ok(Some(stat)) if stat.state != 'Z' => stat,
            other => panic!("the sleep {sleep} of {script:?} is not running: {other:?}"),
        };

        let outcome = panic::catch_unwind(AssertUnwindSafe(|| shell.wait_exit(bound)));
        let message = match &outcome {
            Ok(status) => panic!("wait_exit returned {status} for {script:?} instead of failing"),
            Err(payload) => panic_text(payload.as_ref()),
        };
        assert!(
            message.contains(&problem),
            "the failure of {script:?} does not name {problem:?}: {message}"
        );
        assert!(
            !message.contains("after the stop"),
            "the stop of {script:?} left the stderr open: {message}"
        );
        assert!(
            matches!(shell.lines.try_recv(), Err(TryRecvError::Disconnected)),
            "the stop of {script:?} left the stdout open {DRAIN_GRACE:?} later: {message}"
        );

        let until = Instant::now() + DESCENDANT_BOUND;
        loop {
            match proc_stat(sleep) {
                Ok(None) => break,
                Ok(Some(stat)) if stat.state == 'Z' || stat.start != before.start => break,
                last if Instant::now() >= until => panic!(
                    "the sleep {sleep} of {script:?} still runs {DESCENDANT_BOUND:?} after the \
                     stop: {last:?}"
                ),
                _ => thread::sleep(POLL),
            }
        }
    }
}

/// The text of a panic payload: the message of a formatted `panic!`, or its literal.
fn panic_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("<a panic payload that is not text>")
}

/// Reads `pipe`, the stdout of the child `label`, line by line on a new thread, which sends
/// each line without its line ending, then the read error that ends it, if any.
///
/// # Errors
///
/// The thread cannot be started.
fn lines_on_thread<R: Read + Send + 'static>(
    pipe: R,
    label: &str,
) -> io::Result<Receiver<io::Result<String>>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(format!("{label} stdout"))
        .spawn(move || {
            let mut reader = BufReader::new(pipe);
            loop {
                let mut line = Vec::new();
                let read = match reader.read_until(b'\n', &mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        let text = line.strip_suffix(b"\n").unwrap_or(&line);
                        let text = text.strip_suffix(b"\r").unwrap_or(text);
                        Ok(String::from_utf8_lossy(text).into_owned())
                    }
                    Err(error) => Err(error),
                };
                let failed = read.is_err();
                // The receiver is gone only when the test has already failed and stopped
                // reading; the error, if any, is the last thing sent.
                if sender.send(read).is_err() || failed {
                    return;
                }
            }
        })?;
    Ok(receiver)
}

/// Reads `pipe`, the stderr of the child `label`, to its end on a new thread, which sends the
/// bytes or the read error.
///
/// # Errors
///
/// The thread cannot be started.
fn bytes_on_thread<R: Read + Send + 'static>(
    mut pipe: R,
    label: &str,
) -> io::Result<Receiver<io::Result<Vec<u8>>>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(format!("{label} stderr"))
        .spawn(move || {
            let mut bytes = Vec::new();
            let read = pipe.read_to_end(&mut bytes).map(|_| bytes);
            // The receiver is gone only when the test has already failed and stopped waiting.
            let _ = sender.send(read);
        })?;
    Ok(receiver)
}

/// `text` as a JSON string literal: quoted, with `"`, `\` and every control character
/// escaped.
fn json_string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for character in text.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            control if control < ' ' => {
                let code = u32::from(control);
                literal.push_str("\\u00");
                // Both nibbles are below 16, so each is one hexadecimal digit.
                for nibble in [code >> 4, code & 0xF] {
                    literal.extend(char::from_digit(nibble, 16));
                }
            }
            other => literal.push(other),
        }
    }
    literal.push('"');
    literal
}

/// A JSON value (RFC 8259), as [`Json::parse`] reads the API's answers, the server's event
/// and span lines and Cargo's messages.
#[derive(Debug)]
enum Json {
    /// `true`, `false`, `null` or a number, whose value no check reads.
    Scalar,
    Text(String),
    List(Vec<Json>),
    /// The members in their order; [`Json::member`] finds the first of a name.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parses `text`, which must hold exactly one JSON value, with whitespace around it.
    fn parse(text: &str) -> Result<Self, String> {
        let mut reader = JsonReader { text, at: 0 };
        let value = reader.value()?;
        reader.skip_whitespace();
        if reader.at == text.len() {
            Ok(value)
        } else {
            Err(reader.unexpected("the end of the text"))
        }
    }

    /// The value of the first member `key` of an object; `None` for any other value.
    fn member(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            Self::Scalar | Self::Text(_) | Self::List(_) => None,
        }
    }

    /// The text of a string; `None` for any other value.
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Scalar | Self::List(_) | Self::Object(_) => None,
        }
    }

    /// The items of an array; `None` for any other value.
    fn items(&self) -> Option<&[Self]> {
        match self {
            Self::List(items) => Some(items),
            Self::Scalar | Self::Text(_) | Self::Object(_) => None,
        }
    }
}

/// The cursor of [`Json::parse`]: the text and the byte offset reached, always at a
/// character boundary, as the reader steps over ASCII bytes and whole runs of string text.
struct JsonReader<'t> {
    text: &'t str,
    at: usize,
}

impl JsonReader<'_> {
    /// The byte at the cursor.
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    /// Steps over the JSON whitespace: space, tab, line feed and carriage return.
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// Steps over `byte`, which must be at the cursor.
    fn eat(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.unexpected(&format!("{:?}", char::from(byte))))
        }
    }

    /// The error of a text that holds something else than `wanted` at the cursor.
    fn unexpected(&self, wanted: &str) -> String {
        match self.peek() {
            Some(byte) => format!(
                "{wanted} expected at byte {}, found {:?}",
                self.at,
                char::from(byte)
            ),
            None => format!("{wanted} expected at the end of the text"),
        }
    }

    /// The value at the cursor, after whitespace.
    fn value(&mut self) -> Result<Json, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.list(),
            Some(b'"') => self.string().map(Json::Text),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't' | b'f' | b'n') => self.literal(),
            _ => Err(self.unexpected("a JSON value")),
        }
    }

    /// An object: members `"name": value` between braces, separated by commas.
    fn object(&mut self) -> Result<Json, String> {
        self.eat(b'{')?;
        let mut members = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_whitespace();
            let name = self.string()?;
            self.skip_whitespace();
            self.eat(b':')?;
            members.push((name, self.value()?));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.unexpected("',' or '}'")),
            }
        }
    }

    /// An array: values between brackets, separated by commas.
    fn list(&mut self) -> Result<Json, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::List(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::List(items));
                }
                _ => return Err(self.unexpected("',' or ']'")),
            }
        }
    }

    /// `true`, `false` or `null`.
    fn literal(&mut self) -> Result<Json, String> {
        let rest = self.text.get(self.at..).unwrap_or_default();
        for word in ["true", "false", "null"] {
            if rest.starts_with(word) {
                self.at += word.len();
                return Ok(Json::Scalar);
            }
        }
        Err(self.unexpected("true, false or null"))
    }

    /// A number: an optional `-`, an integer part without a leading zero, an optional
    /// fraction and an optional exponent.
    fn number(&mut self) -> Result<Json, String> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => self.digits()?,
            _ => return Err(self.unexpected("a digit")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            self.digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            self.digits()?;
        }
        Ok(Json::Scalar)
    }

    /// Steps over one or more decimal digits.
    fn digits(&mut self) -> Result<(), String> {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        if self.at == start {
            Err(self.unexpected("a digit"))
        } else {
            Ok(())
        }
    }

    /// A string, its escapes decoded. A control character must be escaped.
    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let text = self.text;
        let mut decoded = String::new();
        loop {
            let start = self.at;
            while matches!(self.peek(), Some(byte) if byte != b'"' && byte != b'\\' && byte >= 0x20)
            {
                self.at += 1;
            }
            // The run ends at an ASCII byte or at the end, so both its ends are character
            // boundaries.
            let run = text
                .get(start..self.at)
                .ok_or_else(|| format!("bytes {start} to {} split a character", self.at))?;
            decoded.push_str(run);
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(decoded);
                }
                Some(b'\\') => {
                    self.at += 1;
                    decoded.push(self.escape()?);
                }
                Some(_) => return Err(self.unexpected("an escaped control character")),
                None => return Err(self.unexpected("'\"'")),
            }
        }
    }

    /// The character of the escape after a `\`: one of `"\/bfnrt`, or `u` and four
    /// hexadecimal digits, a high surrogate taking the `\u` low surrogate that must follow.
    fn escape(&mut self) -> Result<char, String> {
        let decoded = match self.peek() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.at += 1;
                return self.unicode_escape();
            }
            _ => return Err(self.unexpected("an escape character")),
        };
        self.at += 1;
        Ok(decoded)
    }

    /// The character of `\uXXXX` after its `u`, with the low surrogate `\uXXXX` that a high
    /// one takes.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let unit = self.hex4()?;
        let scalar = if (0xD800..0xDC00).contains(&unit) {
            self.eat(b'\\')?;
            self.eat(b'u')?;
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(format!("\\u{unit:04X} is not followed by a low surrogate"));
            }
            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
        } else {
            unit
        };
        char::from_u32(scalar).ok_or_else(|| format!("\\u{unit:04X} is a lone surrogate"))
    }

    /// Four hexadecimal digits at the cursor, as a number.
    fn hex4(&mut self) -> Result<u32, String> {
        let text = self.text;
        let digits = text
            .get(self.at..self.at + 4)
            .filter(|digits| digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| self.unexpected("four hexadecimal digits"))?;
        let unit = u32::from_str_radix(digits, 16).map_err(|error| error.to_string())?;
        self.at += 4;
        Ok(unit)
    }
}
