//! CTLI (`COTRTLIC`) through the HTTP layer: the list of the Db2 transaction
//! type master `CARDDEMO.TRANSACTION_TYPE` (table `transaction_types`) with its
//! type and description filters and its seven-row pages, the inline
//! description update of a listed row, and the delete of a listed row.
//!
//! | Operation | `COTRTLIC` paragraphs |
//! |---|---|
//! | `GET /admin/transaction-types` | `1220-EDIT-TYPECD`, `1230-EDIT-DESC`, `1290-CROSS-EDITS`, `9100-CHECK-FILTERS`, `8000-READ-FORWARD`, `8100-READ-BACKWARDS`, `2500-SETUP-MESSAGE` |
//! | `PUT /admin/transaction-types/{id}?origin=list` | `1211-EDIT-ARRAY-DESC`, `1240-EDIT-ALPHANUM-REQD`, `9200-UPDATE-RECORD` |
//! | `DELETE /admin/transaction-types/{id}` | `9300-DELETE-RECORD` |
//!
//! Every error answer takes its status and its `code` from its reviewed
//! `V-COTRTLIC` row of `docs/parity/validation-audit.md`, read at run time
//! (the `Status` column and the `kind=` of the trigger cell), and its text
//! from `carddemo_domain::messages`, except a Db2 text, which is the row's
//! audited `Message text` filled with the `PIC ----9` edit of the injected
//! `SQLCODE` and the empty `DSNTIAC` text, then checked to equal the text
//! `messages` composes. Each `P-` row cited is checked to have passed its
//! review in `docs/parity/parity-oracle-audit.md`.
//!
//! The seeded master is the Db2 initial load, decoded at run time from the
//! `DB2LTTYP.ctl` lines P-TRNTYPE-003 cites, and checked against the table
//! and the row count P-TRNTYPE-003 records and the type and the code range
//! P-TRNTYPE-004 records; every expected row of a seeded type, in a list, a
//! filter or a page, is taken from it.
//!
//! The Db2 failures the fixtures cannot reach are injected with test-only SQL
//! on the test's own database: a trigger that fails an `UPDATE` or `DELETE`
//! with a chosen `SQLSTATE`, and views standing in for the master whose rows,
//! all of them or one chosen row, fail to read with a lock timeout, so a
//! cursor fails at its `OPEN` or at the `FETCH` of that row. Their `SQLCODE`
//! texts carry the empty `DSNTIAC` slot of E-30.
//!
//! One seeded `postgres:16.15` container serves the whole sequence, so Docker
//! must be running; the phases run in order because each starts from the
//! state the previous one left. The row-selector check reads only the
//! `OpenAPI` document and needs no container. The filter-count check of
//! V-COTRTLIC-003 runs on a container of its own, where the count also fails
//! with the master taken away.
//!
//! A cursor `CLOSE` fails only after the read has returned every row it asked
//! for, and the cursor is one statement that no test SQL fails after its last
//! row, so the `CLOSE` check drives the list rule over an in-memory store
//! that reports that failure, and needs no container either. The same store
//! fails the forward re-read of F7 on the first page at its `OPEN` and at a
//! `FETCH`, with no container.

pub mod common;

use std::collections::BTreeSet;
use std::fs;
use std::future::ready;

use axum::extract::State;
use carddemo_api::AppState;
use carddemo_api::auth::{AdminUser, AuthUser, UserType};
use carddemo_api::dto::KeySegment;
use carddemo_api::dto::admin_transaction_types::{
    TranTypeDeleteQuery, TranTypeListField, TranTypeListPageQuery, TranTypeListQuery,
    TranTypeOrigin, TranTypeResponse, TranTypeUpdateQuery, TranTypeUpdateRequest,
};
use carddemo_api::error::{ApiError, ApiJson, ApiPath, ApiQuery};
use carddemo_api::handlers::admin_transaction_types::{
    delete_tran_type, list_tran_types, update_tran_type,
};
use carddemo_api::pagination::Page;
use carddemo_audit::{Document, Repo};
use carddemo_domain::entities::TranTypeMaster;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_domain::numeric::TranTypeCode;
use carddemo_domain::ports::{
    Browse, CursorError, CursorPhase, CursorResult, Direction, PortError, PortResult,
    TranTypeFilter, TranTypeMasterStore,
};
use carddemo_domain::rules::tran_type_list::{self, TranTypeListInput};
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;
use carddemo_testkit::fixtures;
use reqwest::StatusCode;
use serde_json::{Value, json};

use common::{Resp, TestApp};

/// The CTLI list route; `PUT` and `DELETE` address `{LIST}/{type code}`.
const LIST: &str = "/admin/transaction-types";

/// The query that selects the `COTRTLIC` inline update on `PUT`.
const ORIGIN_LIST: &str = "?origin=list";

/// The type code filter field `TRTYPE`.
const TRTYPE: &str = TranTypeListField::TrType.as_str();

/// The description filter field `TRDESC`.
const TRDESC: &str = TranTypeListField::TrDesc.as_str();

/// The parity row of the Db2 initial load of the type master: its
/// `Paragraph` cell cites the load's lines, its `Expected result` the table
/// and the row count.
const DB2_LOAD_ROW: &str = "P-TRNTYPE-003";

/// The parity row of the seeded master type: its code, its description and
/// the range of the loaded codes.
const SEEDED_TYPE_ROW: &str = "P-TRNTYPE-004";

/// The parity row of `TR_DESCRIPTION VARCHAR(50)`, stored right-trimmed.
const DESCRIPTION_COLUMN_ROW: &str = "P-TRNTYPE-002";

/// The parity row of the seeded processing copy `TRANTYPE` of one type, whose
/// description differs from the Db2 master's until TRANEXTR runs.
const PROCESSING_TYPE_ROW: &str = "P-CVTRA03Y-005";

/// The control file of the Db2 load among the citations of
/// [`DB2_LOAD_ROW`].
const DB2_LOAD_FILE: &str = "DB2LTTYP.ctl";

/// The table the Db2 load fills, as [`DB2_LOAD_ROW`] names it.
const MASTER_TABLE: &str = "transaction_types";

/// The types the paging phase adds through `POST`, so the master holds a
/// second page. None has a category, and every description holds `FEE`,
/// which no seeded description does.
const ADDED_TYPES: [(&str, &str); 5] = [
    ("08", "ANNUAL FEE"),
    ("09", "LATE FEE"),
    ("10", "CASH ADVANCE FEE"),
    ("11", "FOREIGN TRANSACTION FEE"),
    ("12", "OVERLIMIT FEE"),
];

/// The code no type has ("P-TRNTYPE-005").
const MISSING_TYPE: &str = "99";

/// The rows of a page when the request names none: `WS-MAX-SCREEN-LINES`
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L60].
const SCREEN_ROWS: usize = 7;

/// `SQLCODE` +100: no row.
const SQLCODE_NOT_FOUND: i32 = 100;

/// `SQLCODE` -532: child rows restrict the delete.
const SQLCODE_RESTRICTED: i32 = -532;

/// `SQLCODE` -911: deadlock or lock timeout.
const SQLCODE_CONTENTION: i32 = -911;

/// `SQLCODE` -803: duplicate key.
const SQLCODE_DUPLICATE: i32 = -803;

/// The `DSNTIAC` text of every Db2 message: the slot stays empty (E-30).
const NO_DSNTIAC_TEXT: &str = "";

/// The slot of `WS-DISP-SQLCODE` in a Db2 text of the validation audit.
const SQLCODE_SLOT: &str = "{WS-DISP-SQLCODE}";

/// The slot of the `DSNTIAC` text in a Db2 text of the validation audit.
const DSNTIAC_SLOT: &str = "{WS-DSNTIAC-FMTD-TEXT}";

/// `WS-DISP-SQLCODE PIC ----9` holding +100, the code E-30 maps a missing
/// row to.
const EDITED_NOT_FOUND: &str = "  100";

/// `WS-DISP-SQLCODE PIC ----9` holding -532, the code E-30 maps a restrict
/// violation to.
const EDITED_RESTRICTED: &str = " -532";

/// `WS-DISP-SQLCODE PIC ----9` holding -911, the code E-30 maps a deadlock
/// or a lock timeout to.
const EDITED_CONTENTION: &str = " -911";

/// `WS-DISP-SQLCODE PIC ----9` holding -803, the code E-30 maps a unique
/// violation to.
const EDITED_DUPLICATE: &str = " -803";

/// The media type of every CTLI answer.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The Db2 master as stored, in `TR_TYPE` order.
const MASTER_ROWS: &str = "SELECT tr_type, tr_description FROM transaction_types ORDER BY tr_type";

/// The processing copy `TRANTYPE` the reports read, in key order.
const PROCESSING_ROWS: &str =
    "SELECT tran_type, tran_type_desc FROM trantype_file ORDER BY tran_type";

/// A trigger function that fails its statement with the `SQLSTATE` its
/// trigger passes as the first argument.
const CREATE_FAIL_FUNCTION: &str = r"
    CREATE FUNCTION ctli_test_fail() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
        RAISE EXCEPTION 'CTLI test failure' USING ERRCODE = TG_ARGV[0];
    END
    $$";

/// Drops [`CREATE_FAIL_FUNCTION`] once no trigger uses it.
const DROP_FAIL_FUNCTION: &str = "DROP FUNCTION ctli_test_fail()";

/// Every `UPDATE` of the master fails with `40P01`, a deadlock: `SQLCODE`
/// -911.
const FAIL_UPDATES_WITH_DEADLOCK: &str = "CREATE TRIGGER ctli_test_update \
     BEFORE UPDATE ON transaction_types FOR EACH ROW EXECUTE FUNCTION ctli_test_fail('40P01')";

/// Every `UPDATE` of the master fails with `23505`, a unique violation:
/// `SQLCODE` -803.
const FAIL_UPDATES_WITH_DUPLICATE: &str = "CREATE TRIGGER ctli_test_update \
     BEFORE UPDATE ON transaction_types FOR EACH ROW EXECUTE FUNCTION ctli_test_fail('23505')";

/// Removes the `UPDATE` trigger.
const DROP_UPDATE_TRIGGER: &str = "DROP TRIGGER ctli_test_update ON transaction_types";

/// Every `DELETE` from the master fails with `40P01`: `SQLCODE` -911.
const FAIL_DELETES_WITH_DEADLOCK: &str = "CREATE TRIGGER ctli_test_delete \
     BEFORE DELETE ON transaction_types FOR EACH ROW EXECUTE FUNCTION ctli_test_fail('40P01')";

/// Removes the `DELETE` trigger.
const DROP_DELETE_TRIGGER: &str = "DROP TRIGGER ctli_test_delete ON transaction_types";

/// Puts a view in the master's place whose every row fails to read with
/// `55P03`, a lock timeout (`SQLCODE` -911): the cursors and the filter count
/// then fail as a locked Db2 table fails them. The table keeps its rows,
/// constraints and indexes under another name.
const FAIL_READS_WITH_LOCK_TIMEOUT: &str = r"
    CREATE FUNCTION ctli_test_fail_read() RETURNS boolean LANGUAGE plpgsql AS $$
    BEGIN
        RAISE EXCEPTION 'CTLI test read failure' USING ERRCODE = '55P03';
    END
    $$;
    ALTER TABLE transaction_types RENAME TO ctli_test_transaction_types;
    CREATE VIEW transaction_types AS
        SELECT tr_type, tr_description FROM ctli_test_transaction_types
         WHERE ctli_test_fail_read()";

/// Undoes [`FAIL_READS_WITH_LOCK_TIMEOUT`].
const RESTORE_READS: &str = r"
    DROP VIEW transaction_types;
    ALTER TABLE ctli_test_transaction_types RENAME TO transaction_types;
    DROP FUNCTION ctli_test_fail_read()";

/// Puts a view in the master's place whose row of a code
/// `ctli_test_unreadable` holds fails to read with `55P03`, a lock timeout
/// (`SQLCODE` -911). The check is a `STABLE` function in the select list:
/// `PostgreSQL` merges the view into each cursor statement, which still reads
/// the padded-key index in order, and makes the check for each row the
/// cursor returns, in fetch order, so a cursor returns the rows before that
/// row and then fails. (A `VOLATILE` check would keep the view a subquery
/// whose every row is checked before the first is returned.) The table keeps
/// its rows, constraints and indexes under another name, and the view's
/// columns keep their types.
const FAIL_ROW_READS: &str = r"
    CREATE TABLE ctli_test_unreadable (tr_type varchar(2) PRIMARY KEY);
    CREATE FUNCTION ctli_test_readable(code varchar) RETURNS boolean
        LANGUAGE plpgsql STABLE AS $$
    BEGIN
        IF EXISTS (SELECT 1 FROM ctli_test_unreadable WHERE tr_type = code) THEN
            RAISE EXCEPTION 'CTLI test row read failure' USING ERRCODE = '55P03';
        END IF;
        RETURN true;
    END
    $$;
    ALTER TABLE transaction_types RENAME TO ctli_test_transaction_types;
    CREATE VIEW transaction_types AS
        SELECT tr_type,
               CASE WHEN ctli_test_readable(tr_type) THEN tr_description
                    ELSE tr_description END AS tr_description
          FROM ctli_test_transaction_types";

/// Makes the row of type 05 unreadable, the fifth row of the first page.
const UNREADABLE_05: &str =
    "DELETE FROM ctli_test_unreadable; INSERT INTO ctli_test_unreadable VALUES ('05')";

/// Makes the row of type 06 unreadable, the fourth type below 11.
const UNREADABLE_06: &str =
    "DELETE FROM ctli_test_unreadable; INSERT INTO ctli_test_unreadable VALUES ('06')";

/// Makes the row of type 07 unreadable, the first type below 09.
const UNREADABLE_07: &str =
    "DELETE FROM ctli_test_unreadable; INSERT INTO ctli_test_unreadable VALUES ('07')";

/// Makes the row of type 09 unreadable, the eighth type and the second
/// below 11.
const UNREADABLE_09: &str =
    "DELETE FROM ctli_test_unreadable; INSERT INTO ctli_test_unreadable VALUES ('09')";

/// Makes the row of type 10 unreadable, the first type below 11.
const UNREADABLE_10: &str =
    "DELETE FROM ctli_test_unreadable; INSERT INTO ctli_test_unreadable VALUES ('10')";

/// Undoes [`FAIL_ROW_READS`].
const RESTORE_ROW_READS: &str = r"
    DROP VIEW transaction_types;
    ALTER TABLE ctli_test_transaction_types RENAME TO transaction_types;
    DROP FUNCTION ctli_test_readable(varchar);
    DROP TABLE ctli_test_unreadable";

/// The codes the master holds when [`failed_fetches`] places its unreadable
/// rows: the Db2 load ([`db2_load`]) and the added types, less 08, which the
/// delete phase removed.
fn master_after_deletes(audits: &Audits) -> Vec<String> {
    with_added_types(&audits.db2_types)
        .into_iter()
        .map(|(code, _)| code)
        .filter(|code| code != "08")
        .collect()
}

/// F7 from type 11, the first code of the fourth page of three rows: the
/// page reached holds 07, 09 and 10, and the backward cursor reads them,
/// then 06, the look-behind row.
const BACK_FROM_11: &str = "?start_key=11&direction=backward&page_size=3";

/// Empties the master, its categories first, as their foreign key requires.
const EMPTY_MASTER: &str = "DELETE FROM transaction_categories; DELETE FROM transaction_types";

/// The outcome a reviewed `V-COTRTLIC` row assigns.
struct Audited {
    /// The `Status` column.
    status: StatusCode,
    /// The `kind=` of the trigger cell; `None` for `kind=none`, the answers
    /// that are no error.
    kind: Option<ErrorKind>,
}

/// The two audits, parsed once per test, and the Db2 load they direct.
struct Audits {
    validation: Document,
    parity: Document,
    /// The rows of the Db2 initial load of the type master, each its
    /// `TR_TYPE` and its `TR_DESCRIPTION`, in load order, as [`db2_load`]
    /// decodes them.
    db2_types: Vec<(String, String)>,
}

impl Audits {
    fn load() -> Self {
        let parity = common::parity_audit();
        let db2_types = db2_load(&parity);
        Self {
            validation: common::validation_audit(),
            parity,
            db2_types,
        }
    }

    /// The description the Db2 load gives the type `code`.
    fn db2_description(&self, code: &str) -> &str {
        let (_, description) = self
            .db2_types
            .iter()
            .find(|(loaded, _)| loaded == code)
            .unwrap_or_else(|| panic!("the Db2 load holds no type {code}: {:?}", self.db2_types));
        description
    }

    /// The outcome of the validation row `id`, once its review has passed.
    fn row(&self, id: &str) -> Audited {
        assert_review_passed(&self.validation, id);
        let (table, row) = self
            .validation
            .find_row(id)
            .unwrap_or_else(|| panic!("the validation audit defines no {id} row"));
        let status = table.cell(row, "Status").unwrap_or_default();
        let status = status
            .parse()
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {status:?}"));
        let trigger = table.cell(row, "Trigger condition").unwrap_or_default();
        let kind = carddemo_audit::key_values(trigger)
            .into_iter()
            .rev()
            .find(|pair| pair.key == "kind")
            .unwrap_or_else(|| panic!("{id} records no kind: {trigger:?}"));
        Audited {
            status,
            kind: error_kind(id, &kind.value),
        }
    }

    /// Asserts that the validation row `id`, once its review has passed,
    /// assigns 200 and records no error kind: a page or a write that answers
    /// its text in `message`.
    fn ok_row(&self, id: &str) {
        let row = self.row(id);
        assert_eq!(row.status, StatusCode::OK, "status of {id}");
        assert_eq!(row.kind, None, "kind of {id}");
    }

    /// Asserts that each parity row in `ids` exists and passed its review.
    fn reviewed(&self, ids: &[&str]) {
        for id in ids {
            assert!(
                self.parity.find_row(id).is_some(),
                "the parity audit defines no {id} row"
            );
            assert_review_passed(&self.parity, id);
        }
    }
}

/// The [`ErrorKind`] the audit's `kind=` names, `None` for `none`.
fn error_kind(id: &str, name: &str) -> Option<ErrorKind> {
    match name {
        "none" => None,
        "Invalid" => Some(ErrorKind::Invalid),
        "Empty" => Some(ErrorKind::Empty),
        "NotFound" => Some(ErrorKind::NotFound),
        "Conflict" => Some(ErrorKind::Conflict),
        "Unexpected" => Some(ErrorKind::Unexpected),
        other => panic!("{id} records the unknown kind {other:?}"),
    }
}

/// Asserts that the last review verdict `doc` records for `id` is `pass`:
/// only reviewed rows gate a test.
fn assert_review_passed(doc: &Document, id: &str) {
    let verdicts: Vec<&str> = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(|row| table.cell(row, "Row ID") == Some(id))
                .filter_map(|row| table.cell(row, "Verdict"))
        })
        .collect();
    assert_eq!(verdicts.last(), Some(&"pass"), "review verdicts of {id}");
}

/// The exact texts the code spans of the `column` cell of the audit row `id`
/// hold, in order, each without its quotes and with its `\"` and `\\`
/// escapes resolved.
fn audited_texts(doc: &Document, id: &str, column: &str) -> Vec<String> {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the audit defines no {id} row"));
    let cell = table
        .cell(row, column)
        .unwrap_or_else(|| panic!("{id} has no {column} cell"));
    carddemo_audit::cell_code_spans(cell)
        .into_iter()
        .map(|span| unquote(id, &span.text))
        .collect()
}

/// `text`, a quoted exact text of the audit row `id`, without its quotes and
/// with each `\"` and `\\` escape replaced by the character it escapes.
fn unquote(id: &str, text: &str) -> String {
    let inner = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} holds the unquoted text {text:?}"));
    let mut plain = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            plain.extend(chars.next());
        } else {
            plain.push(character);
        }
    }
    plain
}

/// The `key=` value of the `Expected result` cell of the parity row `id`, as
/// written: a quoted value keeps its backticks.
fn expected_entry(parity: &Document, id: &str, key: &str) -> String {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit defines no {id} row"));
    let result = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no Expected result cell"));
    carddemo_audit::key_values(result)
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| panic!("{id} records no {key}=: {result:?}"))
        .value
}

/// The quoted `key=` value of the `Expected result` cell of the parity row
/// `id`: the text of its first code span, without its quotes.
fn expected_text(parity: &Document, id: &str, key: &str) -> String {
    let entry = expected_entry(parity, id, key);
    let span = carddemo_audit::code_span(&entry)
        .unwrap_or_else(|| panic!("{id} quotes no {key}: {entry:?}"));
    unquote(id, &span.text)
}

/// The Db2 initial load of the type master, decoded from the lines of
/// `DB2LTTYP.ctl` that the `Paragraph` cell of P-TRNTYPE-003 cites, once
/// P-TRNTYPE-002, P-TRNTYPE-003 and P-TRNTYPE-004 have passed their review.
///
/// Each cited line that holds SQL string literals holds exactly two, the
/// `TR_TYPE` and the `TR_DESCRIPTION` of one row; a line with none (the
/// `INSERT INTO`, its column list, the `COMMIT`) loads no row. The
/// description is right-trimmed, as `TR_DESCRIPTION VARCHAR(50)` is stored
/// (P-TRNTYPE-002), and otherwise kept as the bytes say, `REVERAL` included.
/// The rows are checked to fill the `table=` P-TRNTYPE-003 names with its
/// `rows=` count, and to hold the type P-TRNTYPE-004 records
/// ([`assert_seeded_type`]).
fn db2_load(parity: &Document) -> Vec<(String, String)> {
    for id in [DESCRIPTION_COLUMN_ROW, DB2_LOAD_ROW, SEEDED_TYPE_ROW] {
        assert_review_passed(parity, id);
    }
    let (table, row) = parity
        .find_row(DB2_LOAD_ROW)
        .unwrap_or_else(|| panic!("the parity audit defines no {DB2_LOAD_ROW} row"));
    let paragraph = table
        .cell(row, "Paragraph")
        .unwrap_or_else(|| panic!("{DB2_LOAD_ROW} has no Paragraph cell"));
    let cited = carddemo_audit::citations(paragraph)
        .into_iter()
        .find(|citation| citation.path.ends_with(DB2_LOAD_FILE))
        .unwrap_or_else(|| panic!("{DB2_LOAD_ROW} cites no {DB2_LOAD_FILE}: {paragraph:?}"));
    let path = Repo::new(fixtures::repo_root())
        .resolve(&cited.path)
        .unwrap_or_else(|| panic!("{} is not in the repository", cited.path));
    let control = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let lines: Vec<&str> = control.lines().collect();
    let cited_lines = cited
        .start
        .checked_sub(1)
        .and_then(|first| lines.get(first..cited.end))
        .unwrap_or_else(|| panic!("{} has no lines L{}-{}", cited.path, cited.start, cited.end));
    let rows: Vec<(String, String)> = cited_lines
        .iter()
        .zip(cited.start..)
        .filter_map(|(line, number)| {
            let at = format!("{}:L{number}", cited.path);
            let literals = sql_literals(line)
                .unwrap_or_else(|| panic!("{at} leaves a string literal open: {line:?}"));
            match literals.as_slice() {
                [] => None,
                [code, description] => {
                    Some((code.clone(), description.trim_end_matches(' ').to_owned()))
                }
                other => panic!(
                    "{at} holds {} literals, not a code and a description: {line:?}",
                    other.len()
                ),
            }
        })
        .collect();
    assert_eq!(
        expected_entry(parity, DB2_LOAD_ROW, "table"),
        MASTER_TABLE,
        "the table {DB2_LOAD_ROW} loads"
    );
    let count = expected_entry(parity, DB2_LOAD_ROW, "rows");
    let count: usize = count
        .parse()
        .unwrap_or_else(|_| panic!("{DB2_LOAD_ROW} counts no rows: {count:?}"));
    assert_eq!(
        rows.len(),
        count,
        "{DB2_LOAD_ROW}: the rows of {}: {rows:?}",
        cited.path
    );
    assert_seeded_type(parity, &rows);
    rows
}

/// The SQL string literals of `line`, in order, each without its quotes and
/// with each doubled `''` read as one `'`; `None` when the last one is not
/// closed.
fn sql_literals(line: &str) -> Option<Vec<String>> {
    let mut literals = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\'' {
            continue;
        }
        let mut literal = String::new();
        loop {
            let next = chars.next()?;
            match next {
                '\'' if chars.next_if_eq(&'\'').is_some() => literal.push('\''),
                '\'' => break,
                other => literal.push(other),
            }
        }
        literals.push(literal);
    }
    Some(literals)
}

/// Asserts that `rows`, the decoded Db2 load, start with the type
/// P-TRNTYPE-004 records (its `value=` and `tr_description=`), and that its
/// `codes=` range, `first-last`, is their codes: every code from the first
/// to the last, in order, each written in the width of the range's bounds.
fn assert_seeded_type(parity: &Document, rows: &[(String, String)]) {
    let code = expected_text(parity, SEEDED_TYPE_ROW, "value");
    let description = expected_text(parity, SEEDED_TYPE_ROW, "tr_description");
    assert_eq!(
        rows.first(),
        Some(&(code, description)),
        "{SEEDED_TYPE_ROW}: the first row of the Db2 load"
    );
    let range = expected_entry(parity, SEEDED_TYPE_ROW, "codes");
    let (first, last) = range
        .split_once('-')
        .unwrap_or_else(|| panic!("{SEEDED_TYPE_ROW}: codes={range} is no range"));
    let bound = |text: &str| -> u8 {
        text.parse()
            .unwrap_or_else(|_| panic!("{SEEDED_TYPE_ROW}: codes={range} has the bound {text:?}"))
    };
    let width = first.len();
    let expected: Vec<String> = (bound(first)..=bound(last))
        .map(|code| format!("{code:0width$}"))
        .collect();
    let loaded: Vec<&str> = rows.iter().map(|(code, _)| code.as_str()).collect();
    assert_eq!(
        loaded, expected,
        "{SEEDED_TYPE_ROW}: the codes {range} of the Db2 load"
    );
}

/// `types`, the Db2 load, followed by [`ADDED_TYPES`]: the master once
/// [`add_types`] has run.
fn with_added_types(types: &[(String, String)]) -> Vec<(String, String)> {
    types.iter().cloned().chain(owned(&ADDED_TYPES)).collect()
}

/// The rows of `types` the cursors' conditions pass (P-COTRTLIC-003):
/// `TR_TYPE = type_code` when one is given, and `TR_DESCRIPTION LIKE` `%`,
/// the trimmed `description`, `%` when that is not blank, matched by
/// [`like`]; checked to hold a row, so no case built on them is vacuous.
fn filtered_rows(
    types: &[(String, String)],
    type_code: Option<&str>,
    description: &str,
) -> Vec<(String, String)> {
    let text = description.trim_matches(' ');
    let pattern: Vec<char> = format!("%{text}%").chars().collect();
    let rows: Vec<(String, String)> = types
        .iter()
        .filter(|(code, _)| type_code.is_none_or(|wanted| code == wanted))
        .filter(|(_, stored)| {
            let stored: Vec<char> = stored.chars().collect();
            text.is_empty() || like(&pattern, &stored)
        })
        .cloned()
        .collect();
    assert!(
        !rows.is_empty(),
        "no type of {types:?} passes the type {type_code:?} and the description {description:?}"
    );
    rows
}

/// The Db2 text the validation row `id` records, with `edited`, the
/// `PIC ----9` text of a mapped `SQLCODE`, in its `SQLCODE` slot and the
/// empty `DSNTIAC` text of E-30 in its last slot.
fn audited_db2_text(audits: &Audits, id: &str, edited: &str) -> String {
    let texts = audited_texts(&audits.validation, id, "Message text");
    let template = texts
        .first()
        .unwrap_or_else(|| panic!("{id} records no message text"));
    assert!(
        template.contains(SQLCODE_SLOT) && template.ends_with(DSNTIAC_SLOT),
        "{id} records no Db2 text: {template:?}"
    );
    template
        .replace(SQLCODE_SLOT, edited)
        .replace(DSNTIAC_SLOT, NO_DSNTIAC_TEXT)
}

/// `9999-FORMAT-DB2-MESSAGE` with the mapped codes of E-30: the text of the
/// validation row `id` for `sqlcode`, whose `PIC ----9` edit is `edited`,
/// read from the audit and checked to be the text `messages` composes for
/// `action`.
fn mapped_db2_text(audits: &Audits, id: &str, action: &str, sqlcode: i32, edited: &str) -> String {
    let audited = audited_db2_text(audits, id, edited);
    assert_eq!(
        db2_text(action, sqlcode),
        audited,
        "the {id} text of SQLCODE {sqlcode}"
    );
    audited
}

/// The members of the JSON object `value`.
fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{value} is no JSON object"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Asserts that the page answer `page` carries only the rows, each with its
/// type code and description alone, the two neighbouring keys and, when the
/// program shows one, the page text: no member holds a screen header, a
/// field attribute or a page number.
fn assert_page_members(page: &Value) {
    let mut expected = BTreeSet::from(["items", "next_key", "prev_key"]);
    if page.get("message").is_some() {
        expected.insert("message");
    }
    assert_eq!(members(page), expected, "members of the page {page}");
    let items = page["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the page {page} holds no item array"));
    for item in items {
        assert_eq!(
            members(item),
            BTreeSet::from(["description", "type_code"]),
            "members of the item {item}"
        );
    }
}

/// Asserts that `resp` is the error answer of the validation row `id`: its
/// audited status and kind, the text `message` and the `(field, message)`
/// pairs `fields`.
fn assert_row_error(
    audits: &Audits,
    resp: &Resp,
    id: &str,
    message: &str,
    fields: &[(&str, &str)],
) {
    let row = audits.row(id);
    let kind = row
        .kind
        .unwrap_or_else(|| panic!("{id} records no error kind"));
    common::assert_error(resp, row.status, kind.code(), message, fields);
}

/// The `9999-FORMAT-DB2-MESSAGE` text of `action` with `sqlcode` and the empty
/// `DSNTIAC` slot (E-30), checked to start with the trimmed action, the label
/// and the `----9` code.
fn db2_text(action: &str, sqlcode: i32) -> String {
    let text = messages::tran_type_list_db2_message(action, sqlcode, NO_DSNTIAC_TEXT);
    let head = messages::with_sqlcode(
        &[action.trim_end(), messages::SQLCODE_LABEL].concat(),
        sqlcode,
    );
    assert!(text.starts_with(&head), "{text:?} starts with {head:?}");
    text
}

/// One listed type as the API answers it.
fn item(type_code: &str, description: &str) -> Value {
    json!({ "type_code": type_code, "description": description })
}

/// The page body of `items` with its neighbouring keys and, when given, the
/// page text.
fn page<S: AsRef<str>>(
    items: &[(S, S)],
    next_key: Option<&str>,
    prev_key: Option<&str>,
    message: Option<&str>,
) -> Value {
    let items: Vec<Value> = items
        .iter()
        .map(|(code, description)| item(code.as_ref(), description.as_ref()))
        .collect();
    let mut page = json!({ "items": items, "next_key": next_key, "prev_key": prev_key });
    if let Some(message) = message {
        page["message"] = json!(message);
    }
    page
}

/// `GET {LIST}{query}` answered 200: the page.
async fn list(app: &TestApp, token: &str, query: &str) -> Value {
    let path = format!("{LIST}{query}");
    let (status, body, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "status of GET {path}: {raw}");
    body
}

/// Asserts that `GET {LIST}{query}` answers 200 with exactly `expected`.
async fn assert_page(app: &TestApp, token: &str, query: &str, expected: &Value) {
    let body = list(app, token, query).await;
    assert_eq!(&body, expected, "body of GET {LIST}{query}");
}

/// `GET {LIST}{query}`, for an error answer.
async fn list_error(app: &TestApp, token: &str, query: &str) -> Resp {
    app.get(&format!("{LIST}{query}"), Some(token)).await
}

/// The `Content-Type` of the answer to `GET {LIST}{query}`, once its status
/// is checked to be 200.
async fn list_content_type(app: &TestApp, token: &str, query: &str) -> String {
    let url = app.url(&format!("{LIST}{query}"));
    let response = app
        .client()
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET {url} failed: {error}"));
    assert_eq!(response.status(), StatusCode::OK, "status of GET {url}");
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("GET {url} answered no textual Content-Type"))
        .to_owned()
}

/// `PUT {LIST}/{code}?origin=list` with the row description `body`.
async fn put_row(app: &TestApp, token: &str, code: &str, body: &Value) -> Resp {
    app.put(&format!("{LIST}/{code}{ORIGIN_LIST}"), Some(token), body)
        .await
}

/// `DELETE {LIST}/{code}`.
async fn delete_row(app: &TestApp, token: &str, code: &str) -> Resp {
    app.delete(&format!("{LIST}/{code}"), Some(token)).await
}

/// Asserts that `resp` is the 200 of a stored inline update of `code` to
/// `description`, answered with `message` and the info line of
/// `9200-UPDATE-RECORD`.
fn assert_updated(resp: &Resp, code: &str, description: &str, message: &str) {
    let (status, body, raw) = resp;
    assert_eq!(
        *status,
        StatusCode::OK,
        "status of the update of {code}: {raw}"
    );
    let expected = json!({
        "transaction_type": item(code, description),
        "message": message,
        "info": messages::HIGHLIGHTED_ROW_WAS_UPDATED,
    });
    assert_eq!(*body, expected, "body of the update of {code}: {raw}");
}

/// The two-column rows `sql` reads, as `(code, description)` pairs.
async fn rows(app: &TestApp, sql: &str) -> Vec<(String, String)> {
    app.stack()
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .map(|row| match row.as_slice() {
            [Some(code), Some(description)] => (code.clone(), description.clone()),
            other => panic!("{sql} read the row {other:?}"),
        })
        .collect()
}

/// `pairs` as owned `(code, description)` rows.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(code, description)| ((*code).to_owned(), (*description).to_owned()))
        .collect()
}

/// The first page of the master as it is stored now: seven rows and the
/// eighth code, fetched ahead, as the next key.
async fn stored_first_page(app: &TestApp) -> Value {
    let master = rows(app, MASTER_ROWS).await;
    let shown = master.len().min(SCREEN_ROWS);
    let next_key = master.get(shown).map(|(code, _)| code.as_str());
    page(&master[..shown], next_key, None, None)
}

/// The whole CTLI sequence on one seeded database: the seeded list and its
/// filters, the pages once five types are added, the inline update and its
/// failures, the delete and its failures, the failed reads, the empty
/// master, and last, with the pool closed, the database that cannot be
/// reached.
#[test]
fn ctli_lists_updates_and_deletes_the_type_master() {
    let audits = Audits::load();
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let token = app.admin_token().await;
        seeded_first_page(&app, &token, &audits).await;
        other_keys_act_as_enter(&app, &token, &audits).await;
        type_code_filters(&app, &token, &audits).await;
        description_filters(&app, &token, &audits).await;
        filters_matching_nothing(&app, &token, &audits).await;
        add_types(&app, &token, &audits).await;
        pages(&app, &token, &audits).await;
        filtered_pages(&app, &token, &audits).await;
        inline_updates(&app, &token, &audits).await;
        inline_update_failures(&app, &token, &audits).await;
        deletes(&app, &token, &audits).await;
        failed_reads(&app, &token, &audits).await;
        failed_fetches(&app, &token, &audits).await;
        empty_master(&app, &token, &audits).await;
        unreachable_database(&app, &token, &audits).await;
    });
}

/// The seeded master, unfiltered: the rows of the Db2 load ([`db2_load`],
/// P-TRNTYPE-003 and P-TRNTYPE-004) in `TR_TYPE` order on one page, with no
/// page before or after it and no page text.
///
/// The master is a version of its own: the processing copy `TRANTYPE`, which
/// the seed loads from `trantype.txt`, holds the description P-CVTRA03Y-005
/// records for its type, which differs from the Db2 load's until TRANEXTR
/// runs, so the expected master is the Db2 load and never the copy.
///
/// The first request of a session is the entry from the admin menu: no
/// filter applies and the list starts at the first type. The page is the
/// JSON answer of the HTTP response, which carries the rows and the page
/// keys and none of the `CTRTLIA` header fields or the page number.
async fn seeded_first_page(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        DB2_LOAD_ROW,
        SEEDED_TYPE_ROW,
        "P-COTRTLIC-003",
        "P-COTRTLIC-030",
    ]);
    assert_eq!(
        rows(app, MASTER_ROWS).await,
        audits.db2_types,
        "the seeded Db2 master"
    );

    audits.reviewed(&[PROCESSING_TYPE_ROW]);
    let code = expected_text(&audits.parity, PROCESSING_TYPE_ROW, "value");
    let copy = expected_text(&audits.parity, PROCESSING_TYPE_ROW, "tran_type_desc");
    let processing = rows(app, PROCESSING_ROWS).await;
    let stored = processing
        .iter()
        .find(|(stored, _)| *stored == code)
        .map(|(_, description)| description.as_str());
    assert_eq!(
        stored,
        Some(copy.as_str()),
        "{PROCESSING_TYPE_ROW}: the processing copy of type {code}"
    );
    assert_ne!(
        audits.db2_description(&code),
        copy,
        "{PROCESSING_TYPE_ROW}: the Db2 master of type {code} is not its processing copy"
    );

    let page = list(app, token, "").await;
    insta::assert_yaml_snapshot!("first_page", page);
    assert_eq!(
        page,
        stored_first_page(app).await,
        "the first page is the stored master"
    );

    audits.reviewed(&["P-COTRTLIC-012"]);
    let (first_code, first_description) = &audits.db2_types[0];
    assert_eq!(
        page["items"][0],
        item(first_code, first_description),
        "the entry lists from the first type"
    );
    assert_eq!(
        page["prev_key"],
        Value::Null,
        "the entry shows the first page"
    );

    audits.reviewed(&["P-COTRTLIC-023", "P-COTRTLIC-024"]);
    assert_page_members(&page);

    audits.reviewed(&["P-COTRTLIC-029"]);
    assert_eq!(
        list_content_type(app, token, "").await,
        JSON_MEDIA_TYPE,
        "the HTTP response replaces the map sent"
    );
}

/// `0000-MAIN` sets any key other than ENTER, F2, F3, F7, F8 and F10 with a
/// pending action to ENTER, with no message. The list has no key input: a
/// request naming another key answers the ENTER result, the first page.
async fn other_keys_act_as_enter(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-001"]);
    let enter = list(app, token, "").await;
    for query in ["?aid=PF12", "?aid=PA1&key=PF5", "?aid=PF10"] {
        assert_page(app, token, query, &enter).await;
    }
}

/// `1220-EDIT-TYPECD`: blank or `00` sets no type condition, two digits select
/// that type, and anything else is refused on `TRTYPE`. The rows expected
/// are those of the Db2 load ([`db2_load`]) the condition passes.
async fn type_code_filters(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-018"]);
    let types = &audits.db2_types;
    let all = page(&filtered_rows(types, None, ""), None, None, None);
    for query in ["?type_code=", "?type_code=%20%20", "?type_code=00"] {
        assert_page(app, token, query, &all).await;
    }
    let credit = page(&filtered_rows(types, Some("03"), ""), None, None, None);
    assert_page(app, token, "?type_code=03", &credit).await;

    // The query of every request is edited as `1200-EDIT-INPUTS` edits the
    // map, and the rejected filter is named in `fields` by its BMS field.
    audits.reviewed(&["P-COTRTLIC-012", "P-COTRTLIC-027"]);
    let text = messages::TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT;
    for query in ["?type_code=A1", "?type_code=1", "?type_code=0A"] {
        let resp = list_error(app, token, query).await;
        assert_row_error(audits, &resp, "V-COTRTLIC-001", text, &[(TRTYPE, text)]);
    }
}

/// `1230-EDIT-DESC`: the filter is searched as `TR_DESCRIPTION LIKE` `%`, the
/// trimmed text, `%`, case-sensitive and with `_` matching any one
/// character; blank sets no condition. It combines with a type filter. The
/// rows expected are those of the Db2 load ([`db2_load`]) the conditions
/// pass.
async fn description_filters(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-003", "P-COTRTLIC-020"]);
    let types = &audits.db2_types;
    let cases = [
        ("?description=PAY", None, "PAY"),
        ("?description=%20PAY%20", None, " PAY "),
        ("?description=P_Y", None, "P_Y"),
        ("?type_code=02&description=PAY", Some("02"), "PAY"),
        ("?description=RE", None, "RE"),
        ("?description=%20%20", None, "  "),
    ];
    for (query, type_code, description) in cases {
        let expected = page(
            &filtered_rows(types, type_code, description),
            None,
            None,
            None,
        );
        assert_page(app, token, query, &expected).await;
    }
}

/// `1290-CROSS-EDITS`: valid filters that no type passes are refused, the
/// cursor on each filter supplied, type filter first. The no-records text is
/// moved last, so it also replaces the type code text when a rejected type
/// filter comes with a description no type holds; with one some type holds,
/// the type code text stands.
///
/// The refusal is the 404 its audit row assigns, not an empty 200 page:
/// `1290-CROSS-EDITS` sets `INPUT-ERROR`, so no cursor is opened.
async fn filters_matching_nothing(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        "P-TRNTYPE-005",
        "P-COTRTLIC-014",
        "P-COTRTLIC-022",
        "P-COTRTLIC-027",
    ]);
    let text = messages::NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS;
    let missing = format!("?type_code={MISSING_TYPE}");
    let cases: [(&str, &[(&str, &str)]); 4] = [
        (missing.as_str(), &[(TRTYPE, text)]),
        ("?description=pay", &[(TRDESC, text)]),
        (
            "?type_code=03&description=PAY",
            &[(TRTYPE, text), (TRDESC, text)],
        ),
        (
            "?type_code=A1&description=ZZZ",
            &[(TRTYPE, text), (TRDESC, text)],
        ),
    ];
    for (query, fields) in cases {
        let resp = list_error(app, token, query).await;
        assert_row_error(audits, &resp, "V-COTRTLIC-002", text, fields);
    }

    let type_text = messages::TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT;
    let resp = list_error(app, token, "?type_code=A1&description=PAY").await;
    assert_row_error(
        audits,
        &resp,
        "V-COTRTLIC-001",
        type_text,
        &[(TRTYPE, type_text)],
    );
}

/// Adds [`ADDED_TYPES`] through `POST`, the `COTRTUPC` add, so the master
/// holds twelve types; each is committed with the `COTRTUPC` text.
///
/// F2 on the list transfers to `COTRTUPC` to add a type; its replacement is
/// the link to the add form, whose save is this `POST`.
async fn add_types(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-002"]);
    for (code, description) in ADDED_TYPES {
        let body = json!({ "type_code": code, "description": description });
        let (status, answer, raw) = app.post(LIST, Some(token), &body).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "status of the add of {code}: {raw}"
        );
        let expected = json!({
            "transaction_type": item(code, description),
            "message": messages::CHANGES_COMMITTED_TO_DATABASE,
        });
        assert_eq!(answer, expected, "body of the add of {code}: {raw}");
    }
}

/// The browse over twelve types: ENTER and F8 forward with the look-ahead
/// row as the next key, F7 backward with the page shown ascending, and the
/// page-boundary texts in `message`.
async fn pages(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        "P-COTRTLIC-004",
        "P-COTRTLIC-030",
        "P-COTRTLIC-031",
        "P-COTRTLIC-033",
    ]);
    let all = with_added_types(&audits.db2_types);
    let (first, second) = all.split_at(SCREEN_ROWS);
    assert_page(app, token, "", &page(first, Some("08"), None, None)).await;

    audits.ok_row("V-COTRTLIC-005");
    let last = page(
        second,
        None,
        Some("08"),
        Some(messages::NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS),
    );
    assert_page(app, token, "?start_key=08", &last).await;

    audits.ok_row("V-COTRTLIC-008");
    let past_the_end = page::<&str>(
        &[],
        None,
        None,
        Some(messages::NO_MORE_PAGES_TO_DISPLAY_COTRTLIC),
    );
    assert_page(app, token, "?start_key=13", &past_the_end).await;

    // F7 on the first page re-reads it; F7 from the second page reaches it.
    audits.ok_row("V-COTRTLIC-009");
    let no_previous = Some(messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
    let first_again = page(first, Some("08"), None, no_previous);
    assert_page(app, token, "?direction=backward", &first_again).await;
    assert_page(app, token, "?start_key=08&direction=backward", &first_again).await;

    // Three-row pages backward from 08: each starts F8 at the code it came
    // from, and the page with no type before it is the first.
    let code = |at: usize| all[at].0.as_str();
    let backward = [
        ("08", &all[4..7], Some(code(4)), None),
        (code(4), &all[1..4], Some(code(1)), None),
        (code(1), &all[0..1], None, no_previous),
    ];
    for (start_key, items, prev_key, message) in backward {
        let query = format!("?start_key={start_key}&direction=backward&page_size=3");
        let expected = page(items, Some(start_key), prev_key, message);
        assert_page(app, token, &query, &expected).await;
    }

    // Selecting a row for a delete or an update prompts in the SPA and sends
    // no request; the list read that ENTER sends writes nothing.
    audits.reviewed(&["P-COTRTLIC-006"]);
    assert_eq!(
        rows(app, MASTER_ROWS).await,
        all,
        "the browse writes nothing"
    );
}

/// Pages under a description filter: the stateless request carries the
/// filter with the start key and the direction the previous page gave.
async fn filtered_pages(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-011", "P-COTRTLIC-031", "P-COTRTLIC-033"]);
    let filtered = "?description=FEE&page_size=2";
    let no_more = Some(messages::NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS);
    let no_previous = Some(messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
    let cases = [
        ("", &ADDED_TYPES[0..2], Some("10"), None, None),
        (
            "&start_key=10",
            &ADDED_TYPES[2..4],
            Some("12"),
            Some("10"),
            None,
        ),
        (
            "&start_key=12",
            &ADDED_TYPES[4..5],
            None,
            Some("12"),
            no_more,
        ),
        (
            "&start_key=10&direction=backward",
            &ADDED_TYPES[0..2],
            Some("10"),
            None,
            no_previous,
        ),
    ];
    for (position, items, next_key, prev_key, message) in cases {
        let query = format!("{filtered}{position}");
        let expected = page(items, next_key, prev_key, message);
        assert_page(app, token, &query, &expected).await;
    }

    // A changed filter resets the paging: the request that carries it reads
    // from the first type it matches, the first page.
    audits.reviewed(&["P-COTRTLIC-019"]);
    let master = with_added_types(&audits.db2_types);
    let payment = page(&filtered_rows(&master, None, "PAY"), None, None, None);
    assert_page(app, token, "?description=PAY&page_size=2", &payment).await;
}

/// `PUT {LIST}/{id}?origin=list`: `1211-EDIT-ARRAY-DESC` compares the entered
/// description with the listed one, `1240-EDIT-ALPHANUM-REQD` edits a
/// changed one, and `9200-UPDATE-RECORD` stores it trimmed in the Db2 master
/// only, never in the processing copy the reports read.
async fn inline_updates(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        "P-COTRTLIC-009",
        "P-COTRTLIC-013",
        "P-COTRTLIC-017",
        "P-COTRTLIC-021",
        "P-COTRTLIC-028",
        "P-COTRTLIC-045",
    ]);
    let processing = rows(app, PROCESSING_ROWS).await;
    let no_change = messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES;

    // The listed description, and one that differs from it only in case:
    // both answer the no-change text, yet the save stores the entered text,
    // so a change of case alone is written (E-73).
    audits.ok_row("V-COTRTLIC-012");
    let listed = audits.db2_description("02");
    let resp = put_row(app, token, "02", &json!({ "description": listed })).await;
    assert_updated(&resp, "02", listed, no_change);
    let listed = audits.db2_description("01");
    let recased = listed.to_ascii_lowercase();
    assert_ne!(recased, listed, "the case of type 01 of the Db2 load");
    let resp = put_row(app, token, "01", &json!({ "description": recased })).await;
    assert_updated(&resp, "01", &recased, no_change);

    // Absent, empty, spaces and `*`, which the screen shows for a blank
    // description, are not supplied. The edits name no field: the
    // description edited is the row field of whichever row was selected.
    let empty =
        messages::field_edit_message(messages::FIELD_TRANSACTION_DESC, messages::MUST_BE_SUPPLIED);
    for body in [
        json!({}),
        json!({ "description": "" }),
        json!({ "description": "   " }),
        json!({ "description": "*" }),
    ] {
        let resp = put_row(app, token, "02", &body).await;
        assert_row_error(audits, &resp, "V-COTRTLIC-013", &empty, &[]);
    }
    let not_alphanumeric = messages::field_edit_message(
        messages::FIELD_TRANSACTION_DESC,
        messages::CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    );
    let body = json!({ "description": "ELECTRONIC-PAYMENT" });
    let resp = put_row(app, token, "02", &body).await;
    assert_row_error(audits, &resp, "V-COTRTLIC-014", &not_alphanumeric, &[]);

    let body = json!({ "description": "  ELECTRONIC PAYMENT  " });
    let resp = put_row(app, token, "02", &body).await;
    let updated = messages::HIGHLIGHTED_ROW_WAS_UPDATED;
    assert_updated(&resp, "02", "ELECTRONIC PAYMENT", updated);

    let expected: Vec<(String, String)> = with_added_types(&audits.db2_types)
        .into_iter()
        .map(|(code, description)| match code.as_str() {
            "01" => (code, recased.clone()),
            "02" => (code, "ELECTRONIC PAYMENT".to_owned()),
            _ => (code, description),
        })
        .collect();
    assert_eq!(rows(app, MASTER_ROWS).await, expected, "the Db2 master");
    assert_eq!(
        rows(app, PROCESSING_ROWS).await,
        processing,
        "the processing copy is not written"
    );

    // The type is gone since it was listed: the UPDATE finds no row, and the
    // action text is formatted with its `SQLCODE` like every Db2 text.
    let not_found = mapped_db2_text(
        audits,
        "V-COTRTLIC-016",
        messages::RECORD_NOT_FOUND_DELETED_BY_OTHERS,
        SQLCODE_NOT_FOUND,
        EDITED_NOT_FOUND,
    );
    assert!(
        not_found.starts_with(messages::RECORD_NOT_FOUND_DELETED_BY_OTHERS),
        "{not_found:?} keeps the action text and its trailing space"
    );
    let body = json!({ "description": "NEW TYPE" });
    let resp = put_row(app, token, MISSING_TYPE, &body).await;
    assert_row_error(audits, &resp, "V-COTRTLIC-016", &not_found, &[]);
}

/// `9200-UPDATE-RECORD` when the `UPDATE` fails: the Db2 text of the lock
/// failure or of any other code, 500, and nothing written. After the
/// no-change text, the failure keeps that text and takes its own status
/// (V-COTRTLIC-012).
async fn inline_update_failures(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-010", "P-COTRTLIC-043"]);
    let before = rows(app, MASTER_ROWS).await;
    app.stack().exec_sql(CREATE_FAIL_FUNCTION).await;

    app.stack().exec_sql(FAIL_UPDATES_WITH_DEADLOCK).await;
    let deadlock = mapped_db2_text(
        audits,
        "V-COTRTLIC-017",
        messages::DEADLOCK_SOMEONE_ELSE_UPDATING,
        SQLCODE_CONTENTION,
        EDITED_CONTENTION,
    );
    let body = json!({ "description": "CREDIT TO ACCOUNT" });
    let resp = put_row(app, token, "03", &body).await;
    assert_row_error(audits, &resp, "V-COTRTLIC-017", &deadlock, &[]);
    let no_change = messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES;
    let recased = audits.db2_description("03").to_ascii_lowercase();
    let resp = put_row(app, token, "03", &json!({ "description": recased })).await;
    assert_row_error(audits, &resp, "V-COTRTLIC-017", no_change, &[]);
    app.stack().exec_sql(DROP_UPDATE_TRIGGER).await;

    app.stack().exec_sql(FAIL_UPDATES_WITH_DUPLICATE).await;
    let failed = mapped_db2_text(
        audits,
        "V-COTRTLIC-018",
        messages::UPDATE_FAILED_WITH,
        SQLCODE_DUPLICATE,
        EDITED_DUPLICATE,
    );
    let resp = put_row(app, token, "03", &body).await;
    assert_row_error(audits, &resp, "V-COTRTLIC-018", &failed, &[]);
    app.stack().exec_sql(DROP_UPDATE_TRIGGER).await;

    assert_eq!(
        rows(app, MASTER_ROWS).await,
        before,
        "a failed update writes nothing"
    );
}

/// `DELETE {LIST}/{id}`: `9300-DELETE-RECORD` deletes a type no category
/// references and answers the deleted-row text; categories restrict the
/// delete (`ON DELETE RESTRICT`, Db2 -532), a missing row is +100, and any
/// other failure answers the delete-failed text.
async fn deletes(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        "P-TRNTYCAT-005",
        "P-COTRTLIC-007",
        "P-COTRTLIC-008",
        "P-COTRTLIC-028",
        "P-COTRTLIC-043",
    ]);
    let (status, body, raw) = delete_row(app, token, "08").await;
    assert_eq!(status, StatusCode::OK, "status of the delete of 08: {raw}");
    let deleted = json!({
        "transaction_type": null,
        "message": messages::HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
    });
    assert_eq!(body, deleted, "body of the delete of 08: {raw}");
    let master = rows(app, MASTER_ROWS).await;
    assert!(
        master.iter().all(|(code, _)| code != "08"),
        "08 is deleted: {master:?}"
    );

    // Deleted since it was listed, and never stored.
    let not_found = mapped_db2_text(
        audits,
        "V-COTRTLIC-022",
        messages::DELETE_FAILED_WITH_MESSAGE,
        SQLCODE_NOT_FOUND,
        EDITED_NOT_FOUND,
    );
    for code in ["08", MISSING_TYPE] {
        let resp = delete_row(app, token, code).await;
        assert_row_error(audits, &resp, "V-COTRTLIC-022", &not_found, &[]);
    }

    let restricted = mapped_db2_text(
        audits,
        "V-COTRTLIC-021",
        messages::PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST,
        SQLCODE_RESTRICTED,
        EDITED_RESTRICTED,
    );
    let resp = delete_row(app, token, "01").await;
    assert_row_error(audits, &resp, "V-COTRTLIC-021", &restricted, &[]);

    app.stack().exec_sql(FAIL_DELETES_WITH_DEADLOCK).await;
    let failed = mapped_db2_text(
        audits,
        "V-COTRTLIC-023",
        messages::DELETE_FAILED_WITH_MESSAGE,
        SQLCODE_CONTENTION,
        EDITED_CONTENTION,
    );
    let resp = delete_row(app, token, "09").await;
    assert_row_error(audits, &resp, "V-COTRTLIC-023", &failed, &[]);
    app.stack().exec_sql(DROP_DELETE_TRIGGER).await;
    app.stack().exec_sql(DROP_FAIL_FUNCTION).await;

    assert_eq!(
        rows(app, MASTER_ROWS).await,
        master,
        "a failed delete removes nothing"
    );
}

/// The cursors and the filter count when the master cannot be read: the
/// forward and backward `OPEN` texts on `TRTYPE`, and for a failed count the
/// no-records text, as the count it leaves is 0. Once the master reads
/// again, so does the list.
async fn failed_reads(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[
        "P-COTRTLIC-036",
        "P-COTRTLIC-037",
        "P-COTRTLIC-039",
        "P-COTRTLIC-045",
    ]);
    app.stack().exec_sql(FAIL_READS_WITH_LOCK_TIMEOUT).await;

    let forward = mapped_db2_text(
        audits,
        "V-COTRTLIC-024",
        messages::C_TR_TYPE_FORWARD_OPEN,
        SQLCODE_CONTENTION,
        EDITED_CONTENTION,
    );
    let resp = list_error(app, token, "").await;
    assert_row_error(
        audits,
        &resp,
        "V-COTRTLIC-024",
        &forward,
        &[(TRTYPE, &forward)],
    );

    // F7 from a page after the first: the types before it cannot be counted
    // either, so the page reached is not shown to be the first. The close of
    // the backward cursor that ends every backward read keeps the text of
    // the failed open.
    audits.reviewed(&["P-COTRTLIC-035"]);
    let backward = mapped_db2_text(
        audits,
        "V-COTRTLIC-026",
        messages::C_TR_TYPE_BACKWARD_OPEN,
        SQLCODE_CONTENTION,
        EDITED_CONTENTION,
    );
    let resp = list_error(app, token, "?start_key=09&direction=backward").await;
    assert_row_error(
        audits,
        &resp,
        "V-COTRTLIC-026",
        &backward,
        &[(TRTYPE, &backward)],
    );

    let no_records = messages::NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS;
    let resp = list_error(app, token, "?type_code=03").await;
    assert_row_error(
        audits,
        &resp,
        "V-COTRTLIC-028",
        no_records,
        &[(TRTYPE, no_records)],
    );

    app.stack().exec_sql(RESTORE_READS).await;
    let expected = stored_first_page(app).await;
    assert_page(app, token, "", &expected).await;
}

/// The cursors when one row of the master cannot be read: the cursor
/// returns the rows before it and fails at it, and the list answers the Db2
/// text of the cursor step that failed, on `TRTYPE`. A failed `FETCH` of a
/// row of the forward page names the close (E-73), the failed look-ahead
/// `FETCH` after a full page the fetch, a failed backward `FETCH` on a
/// page-back that does not reach the first page the backward fetch, and on
/// such a page-back a backward cursor that fails before its first row its
/// `OPEN`, whose text the `FETCH` and `CLOSE` of the unopened cursor keep. On
/// a page-back that reaches the first page both give way to the first-page
/// text, with the rows read. So does a failed forward `FETCH` on F7 from the
/// first page, which re-reads it forward: the rows before a failed `FETCH`
/// of a page row with no next key (E-86), the page before a failed
/// look-ahead `FETCH` with its last code as the next key. The look-behind
/// row past the backward page, which the program never fetches, fails
/// nothing the program reads: the page is answered as read. Once the master
/// reads again, so does the list.
async fn failed_fetches(app: &TestApp, token: &str, audits: &Audits) {
    let master = rows(app, MASTER_ROWS).await;
    let codes: Vec<String> = master.iter().map(|(code, _)| code.clone()).collect();
    assert_eq!(
        codes,
        master_after_deletes(audits),
        "the master the failures read"
    );
    let page_back = list(app, token, BACK_FROM_11).await;
    app.stack().exec_sql(FAIL_ROW_READS).await;

    // 01 to 04 are fetched, and the FETCH of 05 fails; from 03 too.
    audits.reviewed(&["P-COTRTLIC-032", "P-COTRTLIC-044"]);
    app.stack().exec_sql(UNREADABLE_05).await;
    let page_fetch = messages::C_TR_TYPE_FORWARD_CLOSE;
    for query in ["", "?start_key=03"] {
        assert_cursor_error(app, token, audits, query, "V-COTRTLIC-007", page_fetch).await;
    }

    // F7 on the first page re-reads it forward, from the start of the table
    // and from its first code, before which no type stands: the first-page
    // text replaces the cursor's, and the rows 01 to 04 fetched before the
    // failed FETCH of 05 stay shown, with no next key (E-86).
    audits.reviewed(&["P-COTRTLIC-004"]);
    audits.ok_row("V-COTRTLIC-009");
    let first_page = audited_message(audits, "V-COTRTLIC-009");
    let first_page_back = [
        "?direction=backward".to_owned(),
        format!("?start_key={}&direction=backward", master[0].0),
    ];
    let before_05: Vec<(String, String)> = master
        .iter()
        .take_while(|(code, _)| code.as_str() < "05")
        .cloned()
        .collect();
    let kept = page(&before_05, None, None, Some(&first_page));
    for query in &first_page_back {
        assert_page(app, token, query, &kept).await;
    }

    // The seven rows of the first page are fetched, and the look-ahead
    // FETCH of 09 fails.
    audits.reviewed(&["P-COTRTLIC-031"]);
    app.stack().exec_sql(UNREADABLE_09).await;
    let look_ahead = messages::C_TR_TYPE_FORWARD_FETCH;
    assert_cursor_error(app, token, audits, "", "V-COTRTLIC-006", look_ahead).await;

    // F7 on the first page: the seven rows fetched stay shown, and the
    // seventh code, which the program moves to WS-CA-LAST-TR-CODE before the
    // look-ahead FETCH, is the next key.
    let shown = &master[..SCREEN_ROWS];
    let last_shown = shown.last().map(|(code, _)| code.as_str());
    let kept = page(shown, last_shown, None, Some(&first_page));
    for query in &first_page_back {
        assert_page(app, token, query, &kept).await;
    }

    // Backward from 11, 10 is fetched and the FETCH of 09 fails: more than a
    // page of types precede 11, so the page reached is not the first.
    audits.reviewed(&["P-COTRTLIC-034"]);
    let back_fetch = messages::ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD;
    assert_cursor_error(
        app,
        token,
        audits,
        BACK_FROM_11,
        "V-COTRTLIC-010",
        back_fetch,
    )
    .await;

    // Backward from 11, the first row, 10, fails: the OPEN's text stays.
    audits.reviewed(&["P-COTRTLIC-035", "P-COTRTLIC-039"]);
    app.stack().exec_sql(UNREADABLE_10).await;
    let back_open = messages::C_TR_TYPE_BACKWARD_OPEN;
    assert_cursor_error(
        app,
        token,
        audits,
        BACK_FROM_11,
        "V-COTRTLIC-026",
        back_open,
    )
    .await;

    // The look-behind row 06 past the page of 07, 09 and 10 fails: the
    // program's backward loop ends at the page's last row.
    audits.reviewed(&["P-COTRTLIC-033"]);
    app.stack().exec_sql(UNREADABLE_06).await;
    assert_page(app, token, BACK_FROM_11, &page_back).await;

    failed_fetch_texts_give_way_on_the_first_page(app, token, audits, &master).await;
    app.stack().exec_sql(RESTORE_ROW_READS).await;
    let expected = stored_first_page(app).await;
    assert_page(app, token, "", &expected).await;
}

/// P-COTRTLIC-044 records the three cursor texts that contradict their
/// steps' names (E-73), and that each gives way to the first-page text on a
/// page-back that reaches the first page. The stateless request judges the
/// page reached from the data (E-54), so a backward cursor that answers +100
/// within the page, fewer than a page of types preceding the start key, has
/// reached the first page: F7 from 02 in three-row pages reads the stored
/// row of 01 of `master` alone and answers the first-page text, with the
/// row 06 still unreadable.
///
/// F7 from 09, which the seven types 01 to 07 precede, is the page-back from
/// the second page, so the backward cursor's own failure gives way to the
/// first-page text too, 200 (V-COTRTLIC-009). With 06 unreadable the cursor
/// fetches 07 and its `FETCH` of 06 fails: the row fetched stays shown, the
/// rows above it blank (P-COTRTLIC-034). With 07 unreadable its first row
/// fails, the `OPEN`, and the list is empty (P-COTRTLIC-039). F8 from either
/// starts at 09 again (P-COTRTLIC-040).
async fn failed_fetch_texts_give_way_on_the_first_page(
    app: &TestApp,
    token: &str,
    audits: &Audits,
    master: &[(String, String)],
) {
    audits.reviewed(&["P-COTRTLIC-044"]);
    assert_eq!(
        audited_texts(&audits.parity, "P-COTRTLIC-044", "Expected result"),
        [
            messages::ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD,
            messages::C_TR_TYPE_FORWARD_CLOSE,
            messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC,
        ],
        "the texts of P-COTRTLIC-044"
    );
    audits.ok_row("V-COTRTLIC-009");
    let first_page = messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC;
    assert_eq!(
        audited_message(audits, "V-COTRTLIC-009"),
        first_page,
        "the text of V-COTRTLIC-009"
    );
    let expected = page(&master[..1], Some("02"), None, Some(first_page));
    let query = "?start_key=02&direction=backward&page_size=3";
    assert_page(app, token, query, &expected).await;

    audits.reviewed(&["P-COTRTLIC-034", "P-COTRTLIC-039", "P-COTRTLIC-040"]);
    let before_09: Vec<&str> = master
        .iter()
        .map(|(code, _)| code.as_str())
        .take_while(|code| *code < "09")
        .collect();
    assert_eq!(
        before_09.len(),
        SCREEN_ROWS,
        "the types before 09: {before_09:?}"
    );
    let seventh: Vec<(String, String)> = master
        .iter()
        .filter(|(code, _)| code == "07")
        .cloned()
        .collect();
    let query = "?start_key=09&direction=backward";
    let fetched = page(&seventh, Some("09"), None, Some(first_page));
    assert_page(app, token, query, &fetched).await;

    app.stack().exec_sql(UNREADABLE_07).await;
    let opened = page::<&str>(&[], Some("09"), None, Some(first_page));
    assert_page(app, token, query, &opened).await;
}

/// Asserts that `GET {LIST}{query}` answers the error of the validation row
/// `id`, a failed cursor step: its audited status and kind, the text the row
/// records, checked to be the `9999-FORMAT-DB2-MESSAGE` text of `action`, with
/// the `SQLCODE` -911 of the lock timeout the test view raises, and that text
/// on `TRTYPE`, the field the row names.
async fn assert_cursor_error(
    app: &TestApp,
    token: &str,
    audits: &Audits,
    query: &str,
    id: &str,
    action: &str,
) {
    let resp = list_error(app, token, query).await;
    assert_cursor_answer(audits, &resp, id, action);
}

/// Asserts that `resp` is the error answer of the validation row `id`, a
/// failed cursor step: its audited status and kind, the text the row
/// records, checked to be the `9999-FORMAT-DB2-MESSAGE` text of `action`,
/// with the `SQLCODE` -911 of a lock timeout, and that text on `TRTYPE`, the
/// field the row names.
fn assert_cursor_answer(audits: &Audits, resp: &Resp, id: &str, action: &str) {
    let text = mapped_db2_text(audits, id, action, SQLCODE_CONTENTION, EDITED_CONTENTION);
    let (table, row) = audits
        .validation
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit defines no {id} row"));
    assert_eq!(
        table.cell(row, "BMS field"),
        Some(TRTYPE),
        "the field of {id}"
    );
    assert_row_error(audits, resp, id, &text, &[(TRTYPE, &text)]);
}

/// An unfiltered first page with no type at all: 200, no rows, and the
/// no-records text of the forward read.
async fn empty_master(app: &TestApp, token: &str, audits: &Audits) {
    app.stack().exec_sql(EMPTY_MASTER).await;
    audits.ok_row("V-COTRTLIC-004");
    let expected = page::<&str>(
        &[],
        None,
        None,
        Some(messages::NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC),
    );
    assert_page(app, token, "", &expected).await;
}

/// The parity row of `9998-PRIMING-QUERY`, the Db2 check every `COTRTLIC`
/// task makes before any list action.
const PRIMING_ROW: &str = "P-COTRTLIC-042";

/// The disposition [`PRIMING_ROW`] records for its replacement.
const PRIMING_NOT_REPRODUCED: &str = "replacement: not reproduced (E-83)";

/// The admin of the seed, the signed-on user the `AdminUser` extractor gives
/// for the token of [`TestApp::admin_token`].
const SIGNED_ON_ADMIN: &str = "ADMIN001";

/// The type the update and the delete of [`unreachable_database`] address
/// with its description in the Db2 load ([`db2_load`]): valid input, so each
/// request gets as far as the database, whose content no request then reads.
const UNREACHED_TYPE: &str = "02";

/// COTRTLIC 9998-PRIMING-QUERY, SEND-LONG-TEXT — P-COTRTLIC-042 (E-83): with
/// the database out of reach, the list, the inline update and the row
/// delete answer 500 with `Unexpected error` and no fields, never the
/// `Db2 access failure.` text: over HTTP the unit of work of the subject
/// check fails before the handler's, and called directly, past that check,
/// each handler's own begin fails with no Db2 code for the text. The pool
/// is closed for good, so this phase comes last.
async fn unreachable_database(app: &TestApp, token: &str, audits: &Audits) {
    audits.reviewed(&[PRIMING_ROW]);
    let unexpected = messages::UNEXPECTED_ERROR;
    assert_eq!(
        audited_texts(&audits.parity, PRIMING_ROW, "Expected result"),
        [
            messages::DB2_ACCESS_FAILURE,
            messages::SQLCODE_LABEL,
            DB2_TEXT_SEPARATOR,
            unexpected,
        ],
        "the texts of {PRIMING_ROW}"
    );
    let (table, row) = audits
        .parity
        .find_row(PRIMING_ROW)
        .unwrap_or_else(|| panic!("the parity audit defines no {PRIMING_ROW} row"));
    let expected = table.cell(row, "Expected result").unwrap_or_default();
    assert!(
        expected.contains(PRIMING_NOT_REPRODUCED),
        "{PRIMING_ROW} records {PRIMING_NOT_REPRODUCED:?}: {expected}"
    );

    let state = AppState::new(
        Db::from_pool(app.stack().pool().clone()),
        Settings::default(),
    )
    .unwrap_or_else(|error| panic!("the default settings were rejected: {error}"));
    state.db().close().await;
    let (code, description) = (UNREACHED_TYPE, audits.db2_description(UNREACHED_TYPE));
    let body = json!({ "description": description });
    let mut answers = vec![
        list_error(app, token, "").await,
        put_row(app, token, code, &body).await,
        delete_row(app, token, code).await,
    ];

    let admin = || {
        AdminUser(AuthUser {
            user_id: SIGNED_ON_ADMIN.to_owned(),
            usr_type: UserType::Admin,
        })
    };
    let filters = ApiQuery(TranTypeListQuery::default());
    let pages = ApiQuery(TranTypeListPageQuery::default());
    let list_answer = list_tran_types(State(state.clone()), admin(), filters, pages).await;
    answers.push(handler_answer(list_answer));
    let origin = ApiQuery(TranTypeUpdateQuery {
        origin: Some(TranTypeOrigin::List),
    });
    let row_description = ApiJson(TranTypeUpdateRequest {
        description: Some(description.to_owned()),
    });
    let path = || ApiPath(KeySegment(code.to_owned()));
    let update_answer = update_tran_type(
        State(state.clone()),
        admin(),
        path(),
        origin,
        row_description,
    )
    .await;
    answers.push(handler_answer(update_answer));
    let row_origin = ApiQuery(TranTypeDeleteQuery::default());
    let delete_answer = delete_tran_type(State(state), admin(), path(), row_origin).await;
    answers.push(handler_answer(delete_answer));

    let db2_access = messages::DB2_ACCESS_FAILURE.trim_end();
    for resp in &answers {
        common::assert_error(
            resp,
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::Unexpected.code(),
            unexpected,
            &[],
        );
        assert!(!resp.2.contains(db2_access), "{PRIMING_ROW}: {}", resp.2);
    }
}

/// The answer a handler called directly gives: its error's status and body.
fn handler_answer<T>(result: Result<T, ApiError>) -> Resp {
    let Err(error) = result else {
        panic!("the handler answered 2xx with the database out of reach");
    };
    let body = serde_json::to_value(error.body())
        .unwrap_or_else(|failure| panic!("the error body does not serialize: {failure}"));
    let raw = body.to_string();
    (error.status(), body, raw)
}

/// Stores the types `.` and `..`: `MNTTRDB2` adds any two characters as a
/// type code, while the CTTU entry form takes only digits, so SQL stands in
/// for the batch add.
const ADD_DOT_TYPES: &str = "INSERT INTO transaction_types (tr_type, tr_description) \
                             VALUES ('.', 'DOT TYPE'), ('..', 'DOTS TYPE')";

/// The `{id}` segments the types `.` and `..` travel as, behind one more `~`
/// (`carddemo_api::dto::KeySegment`), since a URL drops dot segments.
const DOT_TYPE_SEGMENTS: [(&str, &str); 2] = [(".", "~."), ("..", "~..")];

/// The row description the inline update of type `.` stores.
const RENAMED_DOT_TYPE: &str = "DOT TYPE RENAMED";

/// The listed types `.` and `..` reach the CTLI row actions through their
/// escaped `{id}` segments: the inline update of `.` (`origin=list`) and the
/// delete of `..` (no `origin`) act on those stored codes, never on the list
/// route, and leave every other type as it was.
#[test]
fn dot_segment_type_codes_are_updated_and_deleted_through_their_escaped_segments() {
    let [(updated, update_segment), (deleted, delete_segment)] = DOT_TYPE_SEGMENTS;
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let token = app.admin_token().await;
        app.stack().exec_sql(ADD_DOT_TYPES).await;
        let before = rows(&app, MASTER_ROWS).await;
        for (code, _) in DOT_TYPE_SEGMENTS {
            assert!(
                before.iter().any(|(stored, _)| stored == code),
                "{ADD_DOT_TYPES} stores {code}: {before:?}"
            );
        }

        let body = json!({ "description": RENAMED_DOT_TYPE });
        let resp = put_row(&app, &token, update_segment, &body).await;
        assert_updated(
            &resp,
            updated,
            RENAMED_DOT_TYPE,
            messages::HIGHLIGHTED_ROW_WAS_UPDATED,
        );

        let (status, body, raw) = delete_row(&app, &token, delete_segment).await;
        assert_eq!(status, StatusCode::OK, "the delete of {deleted}: {raw}");
        let answer = json!({
            "transaction_type": null,
            "message": messages::HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
        });
        assert_eq!(body, answer, "the delete of {deleted}: {raw}");

        let expected: Vec<(String, String)> = before
            .into_iter()
            .filter(|(code, _)| code != deleted)
            .map(|(code, description)| {
                if code == updated {
                    (code, RENAMED_DOT_TYPE.to_owned())
                } else {
                    (code, description)
                }
            })
            .collect();
        assert_eq!(rows(&app, MASTER_ROWS).await, expected, "the Db2 master");
    });
}

/// The `COTRTLIC` row selectors have no REST parameter (E-54): each row action
/// is one request for one row, `PUT` for `U` and `DELETE` for `D`, so the
/// texts of an invalid selector and of more than one selected row cannot be
/// reached. The `OpenAPI` document proves it: the list takes only the filters
/// and the page position, the update only the code, the origin and the
/// description, the delete only the code and the origin that selects the
/// `COTRTUPC` delete instead (E-23), and the list's error fields are the two
/// filters.
///
/// The same document shows that the list has no key input, that no
/// operation keeps a pending row action, and that every answer is JSON.
#[test]
fn row_selectors_have_no_rest_parameter() {
    let audits = Audits::load();
    for id in [
        "V-COTRTLIC-011",
        "V-COTRTLIC-015",
        "V-COTRTLIC-019",
        "V-COTRTLIC-020",
    ] {
        let row = audits.row(id);
        assert_eq!(row.kind, Some(ErrorKind::Invalid), "kind of {id}");
    }
    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let row_path = format!("{LIST}/{{id}}");
    assert_selector_rows(&audits, &spec, &row_path);

    // Any key the list does not serve acts as ENTER: the list has no key
    // input, only the filters and the page position.
    audits.reviewed(&["P-COTRTLIC-001"]);
    let operations = [
        (
            LIST,
            "get",
            &[
                "type_code",
                "description",
                "start_key",
                "direction",
                "page_size",
            ][..],
        ),
        (row_path.as_str(), "put", &["id", "origin"][..]),
        (row_path.as_str(), "delete", &["id", "origin"][..]),
    ];
    for (path, method, expected) in operations {
        let operation = &spec["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} is declared");
        assert_eq!(
            names(&operation["parameters"], "name"),
            expected,
            "parameters of {method} {path}"
        );
    }
    let list = &spec["paths"][LIST]["get"];
    let delete = &spec["paths"][&row_path]["delete"];
    assert!(
        list.get("requestBody").is_none(),
        "get {LIST} takes no body"
    );
    assert!(
        delete.get("requestBody").is_none(),
        "delete {row_path} takes no body"
    );

    let reference = spec["paths"][&row_path]["put"]["requestBody"]["content"]["application/json"]
        ["schema"]["$ref"]
        .as_str()
        .unwrap_or_else(|| panic!("put {row_path} names no body schema"));
    let schema = reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or_else(|| panic!("{reference} is no component schema"));
    let properties = spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("{schema} declares no properties"));
    assert_eq!(
        properties.keys().map(String::as_str).collect::<Vec<_>>(),
        ["description"],
        "members of {schema}"
    );
    assert_eq!(
        spec["components"]["schemas"]["TranTypeListField"]["enum"],
        json!([TRTYPE, TRDESC]),
        "the error fields of the list"
    );

    // `SEND-PLAIN-TEXT` is never performed and has no replacement: no
    // answer of the three operations is anything but JSON.
    audits.reviewed(&["P-COTRTLIC-041"]);
    for (path, method) in [
        (LIST, "get"),
        (row_path.as_str(), "put"),
        (row_path.as_str(), "delete"),
    ] {
        assert_json_answers(&spec, path, method);
    }
}

/// The selector edits and texts, the prompts of a pending action, the row
/// attributes and the pending flags: filters and row actions travel in
/// separate requests, each action names its one row in the path, and the
/// pending delete and update are the SPA's dialog and inline edit (E-54).
///
/// Asserts that the selector texts of the audit are those of `messages`, and
/// that the type master is served by exactly the two paths of the list and
/// of one row, `row_path`, with the list read and the add on the first and
/// the update and the delete on the second of `spec`: no operation records
/// or confirms a pending row action, which the server never keeps.
fn assert_selector_rows(audits: &Audits, spec: &Value, row_path: &str) {
    audits.reviewed(&[
        "P-COTRTLIC-005",
        "P-COTRTLIC-006",
        "P-COTRTLIC-015",
        "P-COTRTLIC-016",
        "P-COTRTLIC-025",
        "P-COTRTLIC-026",
    ]);
    assert_eq!(
        audited_texts(&audits.parity, "P-COTRTLIC-005", "Expected result"),
        [
            messages::PLEASE_SELECT_ONLY_1_ACTION,
            messages::ACTION_CODE_SELECTED_IS_INVALID,
        ],
        "the selector texts of the audit"
    );
    let paths: Vec<&str> = members(&spec["paths"])
        .into_iter()
        .filter(|path| path.starts_with(LIST))
        .collect();
    assert_eq!(paths, [LIST, row_path], "the type master paths");
    assert_eq!(
        members(&spec["paths"][LIST]),
        BTreeSet::from(["get", "post"]),
        "the operations of {LIST}"
    );
    assert_eq!(
        members(&spec["paths"][row_path]),
        BTreeSet::from(["delete", "put"]),
        "the operations of {row_path}"
    );
}

/// Asserts that every answer the document declares for `method` on `path`
/// is JSON alone.
fn assert_json_answers(spec: &Value, path: &str, method: &str) {
    let responses = spec["paths"][path][method]["responses"]
        .as_object()
        .unwrap_or_else(|| panic!("{method} {path} declares no responses"));
    assert!(
        !responses.is_empty(),
        "{method} {path} declares no responses"
    );
    for (status, response) in responses {
        assert_eq!(
            members(&response["content"]),
            BTreeSet::from([JSON_MEDIA_TYPE]),
            "media types of the {status} answer of {method} {path}"
        );
    }
}

/// The `key` member of every object in the array `values`, in order; none
/// when `values` is absent.
fn names<'a>(values: &'a Value, key: &str) -> Vec<&'a str> {
    values
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value[key]
                        .as_str()
                        .unwrap_or_else(|| panic!("{value} has no string {key}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `9100-CHECK-FILTERS` ending with a Db2 warning that still assigns its
/// count: the text `1290-CROSS-EDITS` then keeps.
const COUNT_WARNING_ROW: &str = "V-COTRTLIC-003";

/// The disposition [`COUNT_WARNING_ROW`] records: no warning reaches the count.
const COUNT_WARNING_NOT_REPRODUCED: &str = "not reproduced (E-82)";

/// One filter as `9100-CHECK-FILTERS` counts it: a value some seeded type
/// passes, the rows that pass it, a value no type passes, and the filter's
/// BMS field.
type CountedFilter = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static str,
);

/// The filters counted one at a time, each a [`CountedFilter`] whose rows
/// are those of the Db2 load `types` ([`db2_load`]) its value passes.
fn counted_filters(types: &[(String, String)]) -> [CountedFilter; 2] {
    [
        (
            "?type_code=03",
            filtered_rows(types, Some("03"), ""),
            "?type_code=99",
            TRTYPE,
        ),
        (
            "?description=PAY",
            filtered_rows(types, None, "PAY"),
            "?description=pay",
            TRDESC,
        ),
    ]
}

/// Takes the master away, so the count fails with no Db2 code.
const HIDE_COUNTED_MASTER: &str = "ALTER TABLE transaction_types RENAME TO ctli_test_counted_types";

/// Undoes [`HIDE_COUNTED_MASTER`].
const RESTORE_COUNTED_MASTER: &str =
    "ALTER TABLE ctli_test_counted_types RENAME TO transaction_types";

/// The two ways the count fails: a lock timeout (`SQLCODE` -911) and a
/// master taken away.
const COUNT_FAULTS: [(&str, &str); 2] = [
    (FAIL_READS_WITH_LOCK_TIMEOUT, RESTORE_READS),
    (HIDE_COUNTED_MASTER, RESTORE_COUNTED_MASTER),
];

/// COTRTLIC 9100-CHECK-FILTERS, 1290-CROSS-EDITS — V-COTRTLIC-003 (E-82): the
/// filter count answers a count or a failure, never a Db2 warning, so the
/// `Error reading TRANSACTION_TYPE table` text that only a warning with a
/// non-zero count leaves standing is never answered. A filter some type
/// passes answers its page, one no type passes the 404 of V-COTRTLIC-002, and
/// a count that fails, however it fails, the 500 of V-COTRTLIC-028.
#[test]
fn the_filter_count_answers_a_count_or_a_failure_never_the_warning_text() {
    let audits = Audits::load();
    let recorded = audits.row(COUNT_WARNING_ROW);
    assert_eq!(
        (recorded.status, recorded.kind),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(ErrorKind::Unexpected)
        ),
        "the outcome {COUNT_WARNING_ROW} records for the COBOL warning"
    );
    let (table, cells) = audits
        .validation
        .find_row(COUNT_WARNING_ROW)
        .unwrap_or_else(|| panic!("the validation audit defines no {COUNT_WARNING_ROW} row"));
    let trigger = table.cell(cells, "Trigger condition").unwrap_or_default();
    assert!(
        trigger.contains(COUNT_WARNING_NOT_REPRODUCED),
        "{COUNT_WARNING_ROW} records no {COUNT_WARNING_NOT_REPRODUCED:?}: {trigger}"
    );
    let warning = messages::ERROR_READING_TRANSACTION_TYPE_TABLE.trim_end();
    let head = [warning, messages::SQLCODE_LABEL].concat();
    let texts = audited_texts(&audits.validation, COUNT_WARNING_ROW, "Message text");
    assert!(
        texts.first().is_some_and(|text| text.starts_with(&head)),
        "{COUNT_WARNING_ROW} records the {head:?} text: {texts:?}"
    );
    let no_records = messages::NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS;
    let app = TestApp::start();
    carddemo_testkit::block_on(async {
        let token = app.admin_token().await;
        let mut answers = Vec::new();
        for (passing, passed, passing_none, field) in counted_filters(&audits.db2_types) {
            let counted = page(&passed, None, None, None);
            let resp = list_error(&app, &token, passing).await;
            assert_eq!((resp.0, &resp.1), (StatusCode::OK, &counted), "{passing}");
            answers.push(resp);
            let resp = list_error(&app, &token, passing_none).await;
            let fields = [(field, no_records)];
            assert_row_error(&audits, &resp, "V-COTRTLIC-002", no_records, &fields);
            answers.push(resp);
            for (fail, restore) in COUNT_FAULTS {
                app.stack().exec_sql(fail).await;
                let resp = list_error(&app, &token, passing).await;
                app.stack().exec_sql(restore).await;
                assert_row_error(&audits, &resp, "V-COTRTLIC-028", no_records, &fields);
                answers.push(resp);
            }
        }
        for (status, _, raw) in &answers {
            assert!(
                !raw.contains(warning),
                "{COUNT_WARNING_ROW}: a {status} answer carries the warning text: {raw}"
            );
        }
    });
}

/// The rows a cursor read returns before its `CLOSE`: the page and the
/// look-ahead or look-behind row after it, the limit of every list read
/// ([`CursorError::after`] names a failure after them the `CLOSE`'s).
const CLOSE_AFTER: usize = SCREEN_ROWS + 1;

/// The `' '` that `9999-FORMAT-DB2-MESSAGE` strings between the edited
/// `SQLCODE` and the `DSNTIAC` text.
const DB2_TEXT_SEPARATOR: &str = " ";

/// A description filter that eleven of the twelve types of a
/// [`CursorStore`] pass: every description holds an `E` but `AUTHORIZATION`.
const DESCRIPTION_OF_ELEVEN: &str = "E";

/// The type master at the rule boundary, for the cursor `CLOSE` failures of
/// P-COTRTLIC-038 and P-COTRTLIC-040, and for each failed step of the
/// first-page re-read (E-86).
///
/// The adapter reads a cursor as one statement whose rows all arrive before
/// it completes, and no test SQL fails that statement only after its last
/// row, so no request provokes the `CLOSE` step. This store answers the
/// rows of the `COTRTLIC` cursors on the Db2 load it is given
/// ([`db2_load`]) and the added types, with the cursors' type and
/// description conditions, up to the read's limit; the next read that
/// returns at least the count `failure` names fails right after that many
/// rows, at the step [`CursorError::after`] gives.
struct CursorStore {
    /// The master, in `TR_TYPE` order.
    types: Vec<TranTypeMaster>,
    /// The rows a read returns before it fails, and the outcome it fails
    /// with; consumed by the read that fails.
    failure: Option<(usize, PortError)>,
    /// The direction and start key of every read, in call order.
    reads: Vec<(Direction, Option<TranTypeCode>)>,
    /// The key of every count of the types before it, in call order.
    counted: Vec<TranTypeCode>,
    /// The cursor step of every failure answered, in call order.
    failed: Vec<CursorPhase>,
}

impl CursorStore {
    /// The store of `db2_types`, the Db2 load, and [`ADDED_TYPES`], with
    /// `failure` queued.
    fn new(db2_types: &[(String, String)], failure: Option<(usize, PortError)>) -> Self {
        let types = with_added_types(db2_types)
            .into_iter()
            .map(|(code, description)| {
                TranTypeMaster::try_new(TranTypeCode::move_from(&code), description)
                    .unwrap_or_else(|error| panic!("type {code} is no master row: {error}"))
            })
            .collect();
        Self {
            types,
            failure,
            reads: Vec::new(),
            counted: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// One cursor read: forward the codes from the start key on, ascending
    /// (P-COTRTLIC-030); backward the codes below it, descending
    /// (P-COTRTLIC-033).
    fn read(
        &mut self,
        browse: &Browse<TranTypeCode>,
        filter: &TranTypeFilter,
    ) -> CursorResult<TranTypeMaster> {
        self.reads.push((browse.direction, browse.start_key));
        let limit = usize::from(browse.limit);
        let passing = self.types.iter().filter(|row| passes(filter, row));
        let mut rows: Vec<TranTypeMaster> = match browse.direction {
            Direction::Forward => passing
                .filter(|row| browse.start_key.is_none_or(|start| row.tr_type >= start))
                .take(limit)
                .cloned()
                .collect(),
            Direction::Backward => passing
                .rev()
                .filter(|row| browse.start_key.is_none_or(|start| row.tr_type < start))
                .take(limit)
                .cloned()
                .collect(),
        };
        match self.failure.take() {
            Some((fetched, error)) if rows.len() >= fetched => {
                rows.truncate(fetched);
                let failure = CursorError::after(rows, browse.limit, error);
                self.failed.push(failure.phase);
                Err(failure)
            }
            pending => {
                self.failure = pending;
                Ok(rows)
            }
        }
    }
}

/// Whether `row` passes the cursors' conditions (P-COTRTLIC-003):
/// `TR_TYPE = code` and `TR_DESCRIPTION LIKE pattern`, each when given.
fn passes(filter: &TranTypeFilter, row: &TranTypeMaster) -> bool {
    let description: Vec<char> = row.tr_description.chars().collect();
    filter.type_code.is_none_or(|code| row.tr_type == code)
        && filter.description.as_deref().is_none_or(|pattern| {
            let pattern: Vec<char> = pattern.chars().collect();
            like(&pattern, &description)
        })
}

/// `text LIKE pattern` with no escape character, as the cursors match: `%`
/// matches any run of characters, `_` any one, and every other character
/// itself.
fn like(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((&'%', rest)) => (0..=text.len())
            .filter_map(|skipped| text.get(skipped..))
            .any(|tail| like(rest, tail)),
        Some((&wanted, rest)) => text
            .split_first()
            .is_some_and(|(&found, tail)| (wanted == '_' || wanted == found) && like(rest, tail)),
    }
}

impl TranTypeMasterStore for CursorStore {
    fn read_tran_type(
        &mut self,
        code: TranTypeCode,
    ) -> impl Future<Output = PortResult<TranTypeMaster>> + Send + '_ {
        let found = self.types.iter().find(|row| row.tr_type == code).cloned();
        ready(found.ok_or(PortError::NotFound))
    }

    fn browse_tran_types(
        &mut self,
        browse: Browse<TranTypeCode>,
        filter: TranTypeFilter,
    ) -> impl Future<Output = CursorResult<TranTypeMaster>> + Send + '_ {
        ready(self.read(&browse, &filter))
    }

    fn count_tran_types(
        &mut self,
        filter: TranTypeFilter,
    ) -> impl Future<Output = PortResult<u64>> + Send + '_ {
        let count = self.types.iter().filter(|row| passes(&filter, row)).count();
        ready(u64::try_from(count).map_err(|_| PortError::Unavailable(format!("{count} rows"))))
    }

    /// The types below `key` that pass the cursors' conditions: the rows a
    /// backward read from `key` returns with no limit (P-COTRTLIC-033).
    fn count_tran_types_before(
        &mut self,
        filter: TranTypeFilter,
        key: TranTypeCode,
    ) -> impl Future<Output = PortResult<u64>> + Send + '_ {
        self.counted.push(key);
        let count = self
            .types
            .iter()
            .filter(|row| row.tr_type < key && passes(&filter, row))
            .count();
        ready(u64::try_from(count).map_err(|_| PortError::Unavailable(format!("{count} rows"))))
    }

    fn insert_tran_type(
        &mut self,
        rec: TranTypeMaster,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        let at = self.types.partition_point(|row| row.tr_type < rec.tr_type);
        let outcome = if self
            .types
            .get(at)
            .is_some_and(|row| row.tr_type == rec.tr_type)
        {
            Err(PortError::Duplicate)
        } else {
            self.types.insert(at, rec);
            Ok(())
        };
        ready(outcome)
    }

    fn update_tran_type(
        &mut self,
        rec: TranTypeMaster,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        let outcome = match self.types.iter_mut().find(|row| row.tr_type == rec.tr_type) {
            Some(row) => {
                *row = rec;
                Ok(())
            }
            None => Err(PortError::NotFound),
        };
        ready(outcome)
    }

    fn delete_tran_type(
        &mut self,
        code: TranTypeCode,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        let stored = self.types.len();
        self.types.retain(|row| row.tr_type != code);
        let outcome = if self.types.len() < stored {
            Ok(())
        } else {
            Err(PortError::NotFound)
        };
        ready(outcome)
    }
}

/// The list request for one unfiltered page from `start`, a type code or
/// empty for none, in `direction`.
fn read_from(start: &str, direction: Direction) -> TranTypeListInput {
    TranTypeListInput {
        start_key: start_key(start),
        direction,
        ..TranTypeListInput::default()
    }
}

/// The type code `text` names; the empty text names none.
fn start_key(text: &str) -> Option<TranTypeCode> {
    (!text.is_empty()).then(|| TranTypeCode::move_from(text))
}

/// A store failure with no Db2 code: the `Unavailable` outcome of any
/// failure but a lock timeout.
fn unavailable() -> PortError {
    PortError::Unavailable("connection reset".to_owned())
}

/// The answer the router gives the `list` rule's outcome on `store` for
/// `input`: the page with 200, or the error with the status, code and
/// fields `carddemo_api::error` maps it to.
fn listed(store: &mut CursorStore, input: TranTypeListInput) -> Resp {
    let (status, body) = match carddemo_testkit::block_on(tran_type_list::list(store, input)) {
        Ok(shown) => (
            StatusCode::OK,
            serde_json::to_value(Page::<TranTypeResponse>::from(shown)),
        ),
        Err(error) => {
            let error = ApiError::from(error);
            (error.status(), serde_json::to_value(error.body()))
        }
    };
    let body = body.unwrap_or_else(|error| panic!("the answer does not serialize: {error}"));
    let raw = body.to_string();
    (status, body, raw)
}

/// The message text the validation row `id` records, without its quotes.
fn audited_message(audits: &Audits, id: &str) -> String {
    audited_texts(&audits.validation, id, "Message text")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{id} records no message text"))
}

/// COTRTLIC 9450-CLOSE-FORWARD-CURSOR, 9550-CLOSE-BACK-CURSOR —
/// P-COTRTLIC-038, P-COTRTLIC-040, V-COTRTLIC-025, V-COTRTLIC-027: the list
/// rule over a [`CursorStore`] whose cursor fails at its `CLOSE`, after the
/// page and the row after it, answered as the router answers the rule's
/// outcome. Needs no Docker.
#[test]
fn a_failed_cursor_close_answers_the_close_texts() {
    let audits = Audits::load();
    forward_close_failures(&audits);
    first_page_close_failures(&audits);
    backward_close_failures(&audits);
}

/// A forward read that fetched the page and its look-ahead row fails at its
/// `CLOSE`: 500 with the `C-TR-TYPE-FORWARD close` text on `TRTYPE`
/// (V-COTRTLIC-025), the text a failed `FETCH` of the page gives too
/// (P-COTRTLIC-032, V-COTRTLIC-007), for ENTER, F8 and a list whose valid
/// filter sets no earlier text; a failure with no Db2 code answers the
/// unexpected-error text. A `FETCH` that fails first gives its own text,
/// which the `CLOSE` keeps, and F8 past the last page reads no row, so no
/// `CLOSE` fails and V-COTRTLIC-008 answers.
fn forward_close_failures(audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-032", "P-COTRTLIC-038"]);
    let action = messages::C_TR_TYPE_FORWARD_CLOSE;
    assert_eq!(
        audited_texts(&audits.parity, "P-COTRTLIC-038", "Expected result"),
        [action, messages::SQLCODE_LABEL, DB2_TEXT_SEPARATOR],
        "the text of P-COTRTLIC-038"
    );
    assert_eq!(
        audited_db2_text(audits, "V-COTRTLIC-025", EDITED_CONTENTION),
        audited_db2_text(audits, "V-COTRTLIC-007", EDITED_CONTENTION),
        "a failed CLOSE and a failed FETCH of the page show one text"
    );
    let filtered = TranTypeListInput {
        desc_filter: Some(DESCRIPTION_OF_ELEVEN.to_owned()),
        ..TranTypeListInput::default()
    };
    let types = &audits.db2_types;
    let unexpected = messages::UNEXPECTED_ERROR;
    for input in [
        read_from("", Direction::Forward),
        read_from("05", Direction::Forward),
        filtered,
    ] {
        let case = format!("{input:?}");
        let mut store = CursorStore::new(types, Some((CLOSE_AFTER, PortError::Contention)));
        let resp = listed(&mut store, input.clone());
        assert_cursor_answer(audits, &resp, "V-COTRTLIC-025", action);
        assert_eq!(store.failed, [CursorPhase::Close], "{case}");
        let reads = [(Direction::Forward, input.start_key)];
        assert_eq!(store.reads, reads, "{case}: the CLOSE ends the read");

        let mut store = CursorStore::new(types, Some((CLOSE_AFTER, unavailable())));
        let resp = listed(&mut store, input);
        assert_row_error(
            audits,
            &resp,
            "V-COTRTLIC-025",
            unexpected,
            &[(TRTYPE, unexpected)],
        );
        assert_eq!(store.failed, [CursorPhase::Close], "{case}");
    }

    let first_fetches = [
        (
            SCREEN_ROWS,
            "V-COTRTLIC-006",
            messages::C_TR_TYPE_FORWARD_FETCH,
        ),
        (3, "V-COTRTLIC-007", action),
    ];
    for (fetched, id, step) in first_fetches {
        let mut store = CursorStore::new(types, Some((fetched, PortError::Contention)));
        let resp = listed(&mut store, read_from("", Direction::Forward));
        assert_cursor_answer(audits, &resp, id, step);
        assert_eq!(store.failed, [CursorPhase::Fetch], "{id}");
    }

    audits.ok_row("V-COTRTLIC-008");
    let no_more_pages = audited_message(audits, "V-COTRTLIC-008");
    let mut store = CursorStore::new(types, Some((CLOSE_AFTER, PortError::Contention)));
    let resp = listed(&mut store, read_from("13", Direction::Forward));
    let expected = page::<&str>(&[], None, None, Some(&no_more_pages));
    assert_eq!((resp.0, &resp.1), (StatusCode::OK, &expected), "{}", resp.2);
    assert!(
        store.failed.is_empty(),
        "F8 past the last page reads no row: {:?}",
        store.failed
    );
}

/// F7 on the first page re-reads it forward (P-COTRTLIC-004): from the start
/// of the table, and from the first code of the Db2 load ([`db2_load`]),
/// before which no type stands. The `CLOSE` fails after the page and the
/// look-ahead row, which stay shown (P-COTRTLIC-038), and
/// `2500-SETUP-MESSAGE` replaces the close text with that of
/// V-COTRTLIC-009: 200, the rows of the Db2 load and the next key 08.
fn first_page_close_failures(audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-004", "P-COTRTLIC-038"]);
    audits.ok_row("V-COTRTLIC-009");
    let first_page = audited_message(audits, "V-COTRTLIC-009");
    let types = &audits.db2_types;
    let expected = page(types, Some(ADDED_TYPES[0].0), None, Some(&first_page));
    for start in ["", types[0].0.as_str()] {
        let mut store = CursorStore::new(types, Some((CLOSE_AFTER, PortError::Contention)));
        let resp = listed(&mut store, read_from(start, Direction::Backward));
        assert_eq!(
            (resp.0, &resp.1),
            (StatusCode::OK, &expected),
            "F7 from {start:?}: {}",
            resp.2
        );
        assert_eq!(store.failed, [CursorPhase::Close], "F7 from {start:?}");
        let last = store.reads.last().copied();
        let reread = Some((Direction::Forward, start_key(start)));
        assert_eq!(last, reread, "F7 from {start:?} re-reads forward");
    }
}

/// COTRTLIC 0000-MAIN F7 with CA-FIRST-PAGE, 8000-READ-FORWARD,
/// 9400-OPEN-FORWARD-CURSOR, 2500-SETUP-MESSAGE — P-COTRTLIC-004,
/// P-COTRTLIC-031, P-COTRTLIC-032, P-COTRTLIC-037, V-COTRTLIC-009: F7 on the
/// first page over a [`CursorStore`] whose forward re-read fails before its
/// `CLOSE`, answered as the router answers the rule's outcome. The text of
/// V-COTRTLIC-009 replaces the cursor's, 200, and the rows fetched before
/// the failure stay shown, the rows of the Db2 load ([`db2_load`]) in order:
///
/// - the look-ahead `FETCH` fails after the page: the page, with its last
///   code as the next key, the code F8 reads from (P-COTRTLIC-031);
/// - a `FETCH` of a page row fails: the rows before it, with no next key, as
///   the halt E-86 proposes (P-COTRTLIC-032);
/// - the `OPEN` fails: no row and no next key (P-COTRTLIC-037, E-86).
///
/// The re-read runs from the start of the table and from the first code of
/// the Db2 load, before which no type stands. The `OPEN` case runs from the
/// start of the table only: from the first code, the backward read, which
/// returns no row, would take a failure queued after none. Needs no Docker.
#[test]
fn a_failed_first_page_reread_keeps_the_rows_fetched() {
    let audits = Audits::load();
    audits.reviewed(&[
        "P-COTRTLIC-004",
        "P-COTRTLIC-031",
        "P-COTRTLIC-032",
        "P-COTRTLIC-037",
    ]);
    audits.ok_row("V-COTRTLIC-009");
    let first_page = audited_message(&audits, "V-COTRTLIC-009");
    let types = &audits.db2_types;
    assert!(
        types.len() >= SCREEN_ROWS,
        "the Db2 load fills a page: {types:?}"
    );
    let first_code = types[0].0.as_str();
    let cases = [
        ("", SCREEN_ROWS, CursorPhase::Fetch),
        (first_code, SCREEN_ROWS, CursorPhase::Fetch),
        ("", 4, CursorPhase::Fetch),
        (first_code, 4, CursorPhase::Fetch),
        ("", 1, CursorPhase::Fetch),
        ("", 0, CursorPhase::Open),
    ];
    for (start, fetched, phase) in cases {
        let case = format!("F7 from {start:?} failing after {fetched} rows");
        let next_key = (fetched == SCREEN_ROWS).then(|| types[SCREEN_ROWS - 1].0.as_str());
        let expected = page(&types[..fetched], next_key, None, Some(&first_page));
        for error in [PortError::Contention, unavailable()] {
            let mut store = CursorStore::new(types, Some((fetched, error)));
            let resp = listed(&mut store, read_from(start, Direction::Backward));
            assert_eq!(
                (resp.0, &resp.1),
                (StatusCode::OK, &expected),
                "{case}: {}",
                resp.2
            );
            assert_eq!(store.failed, [phase], "{case}");
            let last = store.reads.last().copied();
            let reread = Some((Direction::Forward, start_key(start)));
            assert_eq!(last, reread, "{case} re-reads forward");
        }
    }
}

/// F7 from 10, which nine types precede: the backward read fetches the page
/// and the row before it, so the page reached is not the first (F7 from the
/// third page or later), and its `CLOSE` fails: 500 with the
/// `C-TR-TYPE-BACKWARD close` text on `TRTYPE` (V-COTRTLIC-027); a failure
/// with no Db2 code answers the unexpected-error text. An `OPEN` or a
/// `FETCH` that fails first gives its own text, which the `CLOSE` keeps
/// (P-COTRTLIC-040, V-COTRTLIC-026, V-COTRTLIC-010).
///
/// F7 from 08, which seven types precede, is the page-back from the second
/// page: the program fetches the seven rows, then closes the cursor, and the
/// read here fails right after the same seven rows, before the row it asks
/// for beyond them (P-COTRTLIC-033). The page reached is the first, whose
/// text replaces the cursor's (V-COTRTLIC-009), and the rows read stay
/// shown.
fn backward_close_failures(audits: &Audits) {
    audits.reviewed(&["P-COTRTLIC-033", "P-COTRTLIC-040"]);
    let action = messages::C_TR_TYPE_BACKWARD_CLOSE;
    assert_eq!(
        audited_texts(&audits.parity, "P-COTRTLIC-040", "Expected result"),
        [action, messages::SQLCODE_LABEL, DB2_TEXT_SEPARATOR],
        "the text of P-COTRTLIC-040"
    );
    let from_10 = || read_from("10", Direction::Backward);
    let types = &audits.db2_types;
    let mut store = CursorStore::new(types, Some((CLOSE_AFTER, PortError::Contention)));
    let resp = listed(&mut store, from_10());
    assert_cursor_answer(audits, &resp, "V-COTRTLIC-027", action);
    assert_eq!(store.failed, [CursorPhase::Close]);
    let reads = [(Direction::Backward, start_key("10"))];
    assert_eq!(store.reads, reads, "the CLOSE ends the read");
    assert!(store.counted.is_empty(), "no count follows a CLOSE failure");

    let unexpected = messages::UNEXPECTED_ERROR;
    let mut store = CursorStore::new(types, Some((CLOSE_AFTER, unavailable())));
    let resp = listed(&mut store, from_10());
    assert_row_error(
        audits,
        &resp,
        "V-COTRTLIC-027",
        unexpected,
        &[(TRTYPE, unexpected)],
    );

    let first_steps = [
        (
            0,
            CursorPhase::Open,
            "V-COTRTLIC-026",
            messages::C_TR_TYPE_BACKWARD_OPEN,
        ),
        (
            3,
            CursorPhase::Fetch,
            "V-COTRTLIC-010",
            messages::ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD,
        ),
    ];
    for (fetched, phase, id, step) in first_steps {
        let mut store = CursorStore::new(types, Some((fetched, PortError::Contention)));
        let resp = listed(&mut store, from_10());
        assert_cursor_answer(audits, &resp, id, step);
        assert_eq!(store.failed, [phase], "{id}");
        let counted = [TranTypeCode::move_from("10")];
        assert_eq!(
            store.counted, counted,
            "{id}: the types before 10 are counted"
        );
    }

    audits.ok_row("V-COTRTLIC-009");
    let first_page = audited_message(audits, "V-COTRTLIC-009");
    let next = ADDED_TYPES[0].0;
    let mut store = CursorStore::new(types, Some((SCREEN_ROWS, PortError::Contention)));
    let resp = listed(&mut store, read_from(next, Direction::Backward));
    let expected = page(types, Some(next), None, Some(&first_page));
    assert_eq!((resp.0, &resp.1), (StatusCode::OK, &expected), "{}", resp.2);
    assert_eq!(store.failed, [CursorPhase::Fetch]);
}
