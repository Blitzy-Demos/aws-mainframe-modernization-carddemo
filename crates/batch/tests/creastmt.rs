//! CREASTMT (`batch creastmt`): the account statements CBSTM03A writes with CBSTM03B, as text
//! (`STATEMNT.PS`, 80-byte records [app/cbl/CBSTM03A.CBL:L44-45]) and HTML (`STATEMNT.HTML`,
//! 100-byte records [app/cbl/CBSTM03A.CBL:L46-47]), each record followed by LF, and the PDF of
//! the text statement (`STATEMNT.PS.PDF`) that TXT2PDF1 makes [app/jcl/TXT2PDF1.JCL:L24-40].
//!
//! The expected records come from section D.5 of the parity audit and are never written here.
//! Its template rows give the lines every statement holds, its card summaries the statements in
//! cross-reference order with the transaction count `n` of each, and its variable rows each
//! card's own lines. Each line of each statement must come from exactly one row: a line no row
//! gives fails the test with its card and line number, to be escalated, never guessed.
//!
//! - `statements_match_audit_text_html_and_pdf` runs the README cycle (POSTTRAN, INTCALC, TRANBKP,
//!   COMBTRAN [README.md:L225-228]) and `creastmt`. Both files must equal the audit record for
//!   record and their insta snapshots, and the text extracted from the PDF must equal the
//!   `STMTFILE` records, untrimmed (AAP 0.3.4, 0.9.4). PDF layout is not compared.
//! - `more_than_ten_transactions_on_a_card_halts_with_exit_16` fills the 10 transaction slots of
//!   one card, which still fit the CBSTM03A tables, then adds an 11th, where the COBOL subscripts
//!   run past the table: the run stops with exit 16 and names the halt escalation (E-33), and the
//!   statements of the run before are deleted and none written (AAP 0.9.6). The COBOL result is
//!   undefined there, so the stop's message is the replacement's own: the test spells it out,
//!   with the slots its scenario needs and the table capacities the audit gives.
//! - `empty_live_master_abends_reading_trnxfile`: an empty transaction file abends.
//! - `p_ids_are_every_statement_row` and `audit_describes_every_statement_line` read repository
//!   files only and need no Docker.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::iter::Peekable;
use std::path::Path;
use std::process::Output;
use std::str::Chars;

use carddemo_audit::{
    CodeSpan, Document, ESCALATION_COLUMNS, KeyValue, PARITY_COMMON_COLUMNS, Repo, Row, Table,
};
use carddemo_domain::messages::{ABENDING_PROGRAM, ERROR_READING_TRNXFILE, RETURN_CODE};
use carddemo_domain::numeric::TranId;
use carddemo_persistence::Db;
use carddemo_testkit::fixtures;
use common::{
    Harness, LIVE_MASTER_COLUMNS, TRANREPT_CLOCK, ct02_add, ct02_input, expected_value,
    fixture_lines, parity, parity_cell, parity_row, split_records,
};

/// Normal end, condition code 0: CBSTM03A 9999-GOBACK sets no `RETURN-CODE` (P-CBSTM03A-1264),
/// nor does CBACT04C, and the TRANBKP backup of a live master that holds records ends with
/// condition code 0 (P-TRANBKP-001).
const SUCCESS: i32 = 0;

/// Abend (AAP 0.5.3.5): CBSTM03A 9999-ABEND-PROGRAM calls `CEE3ABD` (P-CBSTM03A-011), and the
/// stop beyond the CBSTM03A tables ends the same way (P-CBSTM03A-1256).
const ABEND: i32 = 16;

/// `STMTFILE`, `DSN=AWS.M2.CARDDEMO.STATEMNT.PS` [app/jcl/CREASTMT.JCL:L87-91].
const STMT_FILE: &str = "STATEMNT.PS";

/// `HTMLFILE`, `DSN=AWS.M2.CARDDEMO.STATEMNT.HTML` [app/jcl/CREASTMT.JCL:L92-96].
const HTML_FILE: &str = "STATEMNT.HTML";

/// The `OUT` dataset of the `%TXT2PDF` conversion of `STATEMNT.PS` [app/jcl/TXT2PDF1.JCL:L38-39].
const PDF_FILE: &str = "STATEMNT.PS.PDF";

/// `FD-STMTFILE-REC PIC X(80)` [app/cbl/CBSTM03A.CBL:L44-45].
const STMT_WIDTH: usize = 80;

/// `FD-HTMLFILE-REC PIC X(100)` [app/cbl/CBSTM03A.CBL:L46-47].
const HTML_WIDTH: usize = 100;

/// The DD name the audit rows' `file=` gives the text statement.
const STMT_DD: &str = "STMTFILE";

/// The DD name the audit rows' `file=` gives the HTML statement.
const HTML_DD: &str = "HTMLFILE";

/// The programs whose parity rows this file proves.
const STATEMENT_PROGRAMS: [&str; 3] = ["CBSTM03A", "CBSTM03B", "CREASTMT"];

/// The INTCALC command of the JCL: `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22].
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", "2022071800"];

/// POSTTRAN over the fixture day, whose `exit=` is the condition code of the run.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// COMBTRAN's load into the live master TRANBKP emptied, the README case, with its `exit=`.
const COMBTRAN_README_LOAD: &str = "P-COMBTRAN-005";

/// The section B row of the CBSTM03A table load, whose copybook field cell gives the `OCCURS`
/// count of each table and whose expected result states the capacity they make.
const TABLES_ROW: &str = "P-CBSTM03A-004";

/// The card table, `WS-CARD-TBL` [app/cbl/CBSTM03A.CBL:L226-230].
const CARD_TABLE: &str = "WS-CARD-TBL";

/// The transaction table of one card, `WS-TRAN-TBL` [app/cbl/CBSTM03A.CBL:L228-230].
const TRANSACTION_TABLE: &str = "WS-TRAN-TBL";

/// The D.5 capacity confirmation, which the D.5 table is found by.
const CAPACITY_ROW: &str = "P-CBSTM03A-014";

/// The D.5 row of the two files: names, record counts, widths and records per statement.
const FILES_ROW: &str = "P-CBSTM03A-015";

/// The section E row of the input class beyond the CBSTM03A tables, with its `escalation=`.
const HALT_ROW: &str = "P-CBSTM03A-1256";

/// The transaction slots of one card, `WS-TRAN-TBL OCCURS 10 TIMES`
/// [app/cbl/CBSTM03A.CBL:L228-230] (P-CBSTM03A-004).
const CARD_TRANSACTION_SLOTS: usize = 10;

/// The CBSTM03B status of the first `TRNXFILE` read of an empty file, the end of file
/// (P-CBSTM03A-003).
const END_OF_FILE_STATUS: &str = "10";

/// The amount of each CT02 add of the capacity test, in the `COTRN2A` screen format.
const CT02_AMOUNT: &str = "+00000050.47";

/// The description of each CT02 add of the capacity test.
const CT02_DESCRIPTION: &str = "Purchase at Abshire-Lowe";

/// Ends every record of the snapshot views and of the mismatch messages, so trailing spaces stay
/// visible.
const RECORD_END: char = '|';

/// Stands for each trailing space of a record in a mismatch message.
const TRAILING_SPACE: char = '\u{b7}';

#[test]
fn statements_match_audit_text_html_and_pdf() {
    let doc = parity();
    let audited = Audited::read(&doc);
    let h = Harness::new();
    run_readme_cycle(&h, &doc);
    assert_live_master_matches_card_summaries(&h, &audited);
    let _ = h.run_expect(&["creastmt"], Some(TRANREPT_CLOCK), SUCCESS);

    let stmt = read_records(&h, &audited.stmt);
    assert_same_lines(STMT_FILE, &stmt, "audit D.5", &audited.stmt_records);
    let html = read_records(&h, &audited.html);
    assert_same_lines(HTML_FILE, &html, "audit D.5", &audited.html_records);

    let stmt_view = snapshot_view(&stmt);
    insta::assert_snapshot!("stmtfile", stmt_view);
    let html_view = snapshot_view(&html);
    insta::assert_snapshot!("htmlfile", html_view);

    let pdf = read_file(&h.statement_dir().join(PDF_FILE));
    assert_same_lines(PDF_FILE, &pdf_text_lines(&pdf), STMT_FILE, &stmt);
}

#[test]
fn more_than_ten_transactions_on_a_card_halts_with_exit_16() {
    let doc = parity();
    let audited = Audited::read(&doc);
    let h = Harness::new();
    let db = h.db();
    let xref = xref_records();
    let card = &xref
        .first()
        .unwrap_or_else(|| panic!("cardxref.txt holds no record"))
        .card_number;
    let (card_capacity, per_card) = table_capacities(&doc);
    assert_eq!(
        per_card, CARD_TRANSACTION_SLOTS,
        "{TABLES_ROW}: the boundary run must fill every {TRANSACTION_TABLE} slot of the card"
    );

    let mut tran_id = TranId::zeros();
    for _ in 0..CARD_TRANSACTION_SLOTS {
        tran_id = add_transaction(&db, card, tran_id);
    }
    // At the boundary every card fits its slots: one statement per cross-reference, this card's
    // with 10 transactions and every other card's with none.
    let _ = h.run_expect(&["creastmt"], Some(TRANREPT_CLOCK), SUCCESS);
    for file in [&audited.stmt, &audited.html] {
        let others = (xref.len() - 1) * file.records_for(0);
        assert_eq!(
            read_records(&h, file).len(),
            file.records_for(CARD_TRANSACTION_SLOTS) + others,
            "{} after {CARD_TRANSACTION_SLOTS} transactions on card {card}",
            file.name
        );
    }
    assert!(h.statement_dir().join(PDF_FILE).is_file(), "no {PDF_FILE}");

    let _ = add_transaction(&db, card, tran_id);
    let output = h.run_expect(&["creastmt"], Some(TRANREPT_CLOCK), ABEND);
    // The COBOL result is undefined past the tables (E-33), so the stop's message is the
    // replacement's own, written out in full. Its counts are this scenario's: the boundary run
    // found every transaction on the one card, which fills one card slot and needs a transaction
    // slot for each transaction added. Its capacities are those of the audited tables.
    let card_slots = 1;
    let transaction_slots = CARD_TRANSACTION_SLOTS + 1;
    let escalation = expected_value(&doc, HALT_ROW, "escalation");
    assert_eq!(
        stderr_lines(&output),
        [
            format!(
                "TRNXFILE needs at least {card_slots} card slots and at least {transaction_slots} \
                 transaction slots for one card; the CBSTM03A tables hold {card_capacity} cards \
                 and {per_card} transactions per card"
            ),
            format!("CAPACITY STOP: halt escalation {escalation}"),
        ],
        "capacity stop stderr"
    );
    // STEP030 deleted the statements of the boundary run, and the stop wrote none.
    assert_no_statement_files(&h);
}

#[test]
fn empty_live_master_abends_reading_trnxfile() {
    let h = Harness::new();
    assert!(
        h.live_master_rows().is_empty(),
        "the seeded state holds a live transaction"
    );

    let output = h.run_expect(&["creastmt"], Some(TRANREPT_CLOCK), ABEND);

    // CEE3ABD is called without parameters, so no `ABEND <code>` line follows.
    assert_eq!(
        stderr_lines(&output),
        [
            ERROR_READING_TRNXFILE.to_owned(),
            format!("{RETURN_CODE}{END_OF_FILE_STATUS}"),
            ABENDING_PROGRAM.to_owned(),
        ],
        "empty TRNXFILE stderr"
    );
    assert_no_statement_files(&h);
}

#[test]
fn p_ids_are_every_statement_row() {
    let doc = parity();
    let audited: BTreeSet<&str> = doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(|row| {
                    table
                        .cell(row, PARITY_COMMON_COLUMNS[1])
                        .is_some_and(|program| STATEMENT_PROGRAMS.contains(&program))
                })
                .filter_map(|row| table.cell(row, PARITY_COMMON_COLUMNS[0]))
        })
        .filter(|id| id.starts_with("P-"))
        .collect();
    let cited: BTreeSet<&str> = P_IDS.iter().copied().collect();

    assert_eq!(cited.len(), P_IDS.len(), "P_IDS lists an ID twice");
    let unlisted: Vec<&&str> = audited.difference(&cited).collect();
    let unknown: Vec<&&str> = cited.difference(&audited).collect();
    assert!(
        unlisted.is_empty() && unknown.is_empty(),
        "P_IDS must be the {STATEMENT_PROGRAMS:?} rows of the parity audit\n  rows not listed: \
         {unlisted:?}\n  listed, not such a row: {unknown:?}"
    );
}

#[test]
fn audit_describes_every_statement_line() {
    let doc = parity();
    let audited = Audited::read(&doc);

    for (file, name, width, records) in [
        (&audited.stmt, STMT_FILE, STMT_WIDTH, &audited.stmt_records),
        (&audited.html, HTML_FILE, HTML_WIDTH, &audited.html_records),
    ] {
        assert_eq!(
            (file.name.as_str(), file.width, file.records),
            (name, width, records.len()),
            "{FILES_ROW} {}: name, width and records, against the lines the rows give",
            file.dd
        );
    }

    let summaries: Vec<XrefRecord> = audited
        .cards
        .iter()
        .map(|card| XrefRecord {
            card_number: card.card_number.clone(),
            customer_id: card.customer_id.clone(),
            account_id: card.account_id.clone(),
        })
        .collect();
    assert_eq!(
        summaries,
        xref_records(),
        "the card summaries must follow the cross-reference file, card for card"
    );

    let counts: Vec<usize> = audited.cards.iter().map(|card| card.n).collect();
    let capacity = |key: &str| expected_number(&expected_value(&doc, CAPACITY_ROW, key), key);
    assert_eq!(counts.len(), capacity("cards"), "{CAPACITY_ROW} cards");
    assert_eq!(
        counts.iter().max().copied().unwrap_or(0),
        capacity("max_transactions_per_card"),
        "{CAPACITY_ROW} max_transactions_per_card"
    );
    assert_eq!(
        counts.iter().sum::<usize>(),
        capacity("transactions"),
        "{CAPACITY_ROW} transactions"
    );
    assert!(
        expected_value(&doc, CAPACITY_ROW, "within_capacity").starts_with("yes"),
        "{CAPACITY_ROW}: the fixture corpus must fit the CBSTM03A tables"
    );

    assert_halt_is_escalated(&doc);
}

/// The README cycle up to CREASTMT [README.md:L225-228] from the seeded state: POSTTRAN,
/// INTCALC, TRANBKP and COMBTRAN, each expected to end with its audited condition code, all
/// under the processing clock the audit D expectations are derived under.
fn run_readme_cycle(h: &Harness, doc: &Document) {
    let steps: [(&[&str], i32); 4] = [
        (&["posttran"], exit_code(doc, POSTTRAN_COUNTS)),
        (&INTCALC, SUCCESS),
        (&["tranbkp"], SUCCESS),
        (&["combtran"], exit_code(doc, COMBTRAN_README_LOAD)),
    ];
    for (args, code) in steps {
        let _ = h.run_expect(args, Some(TRANREPT_CLOCK), code);
    }
}

/// The `exit=` of audit row `id`.
fn exit_code(doc: &Document, id: &str) -> i32 {
    let exit = expected_value(doc, id, "exit");
    exit.parse()
        .unwrap_or_else(|error| panic!("audit row {id}: exit={exit} is no exit code: {error}"))
}

/// The live master CBSTM03A reads after COMBTRAN holds, card for card, the transactions the card
/// summaries count, which the capacity confirmation finds within the tables.
fn assert_live_master_matches_card_summaries(h: &Harness, audited: &Audited) {
    let column = LIVE_MASTER_COLUMNS
        .iter()
        .position(|&name| name == "tran_card_num")
        .unwrap_or_else(|| panic!("the live master rows hold no tran_card_num"));
    let mut live: BTreeMap<String, usize> = BTreeMap::new();
    for row in h.live_master_rows() {
        let card = row
            .into_iter()
            .nth(column)
            .flatten()
            .unwrap_or_else(|| panic!("a live transaction has no card number"));
        *live.entry(card).or_default() += 1;
    }
    let summarised: BTreeMap<String, usize> = audited
        .cards
        .iter()
        .filter(|card| card.n > 0)
        .map(|card| (card.card_number.clone(), card.n))
        .collect();
    assert_eq!(
        live, summarised,
        "live transactions per card after COMBTRAN, against the card summaries of {CAPACITY_ROW}"
    );
}

/// Adds one CT02 transaction on `card`, as the CT02 endpoint writes it with its reward accrual at
/// the processing clock, under the TRAN-ID after `previous`, and returns that TRAN-ID.
///
/// The TRAN-ID is given rather than allocated: on an empty live master the allocation browse of
/// COTRN02C finds no record and adds nothing (E-27).
fn add_transaction(db: &Db, card: &str, previous: TranId) -> TranId {
    let tran_id = previous
        .next()
        .unwrap_or_else(|error| panic!("no TRAN-ID follows the last one added: {error}"));
    ct02_add(
        db,
        ct02_input(card, CT02_AMOUNT, CT02_DESCRIPTION),
        Some(tran_id),
        TRANREPT_CLOCK,
    )
}

/// None of the three statement files exists in the statement directory.
fn assert_no_statement_files(h: &Harness) {
    for name in [STMT_FILE, HTML_FILE, PDF_FILE] {
        let path = h.statement_dir().join(name);
        assert!(
            !path.exists(),
            "{} exists after the job stopped",
            path.display()
        );
    }
}

/// The lines the run wrote to stderr.
fn stderr_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The bytes of `path`.
fn read_file(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The records of `file` in the statement directory, each its width in bytes followed by LF.
fn read_records(h: &Harness, file: &AuditedFile) -> Vec<String> {
    split_records(&read_file(&h.statement_dir().join(&file.name)), file.width)
}

/// A statement file as its snapshot holds it: one record per line, each followed by
/// [`RECORD_END`].
fn snapshot_view(records: &[String]) -> String {
    records
        .iter()
        .map(|record| format!("{record}{RECORD_END}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text lopdf extracts from every page of `pdf`, in page order, split at each LF. Only the
/// empty piece after the final LF is dropped: nothing is trimmed.
fn pdf_text_lines(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf)
        .unwrap_or_else(|error| panic!("{PDF_FILE} does not parse: {error}"));
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    let text = doc
        .extract_text(&pages)
        .unwrap_or_else(|error| panic!("{PDF_FILE} text does not extract: {error}"));
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// `actual`, the lines of `what`, equal `expected`, the lines of `source`, in count, order and
/// bytes. A difference is reported at its first line, both texts shown by [`visible`].
fn assert_same_lines(what: &str, actual: &[String], source: &str, expected: &[String]) {
    if let Some(index) = actual
        .iter()
        .zip(expected)
        .position(|(got, wanted)| got != wanted)
    {
        panic!(
            "{what} line {} differs from {source}\n  {what}: {}\n  {source}: {}",
            index + 1,
            visible(&actual[index]),
            visible(&expected[index]),
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "{what} has {} lines, {source} {}",
        actual.len(),
        expected.len()
    );
}

/// `line` for a message: each trailing space shown as [`TRAILING_SPACE`], the end marked by
/// [`RECORD_END`], then the byte length.
fn visible(line: &str) -> String {
    let kept = line.trim_end_matches(' ');
    let trailing = TRAILING_SPACE.to_string().repeat(line.len() - kept.len());
    format!("{kept}{trailing}{RECORD_END} ({} bytes)", line.len())
}

/// One record of the card cross-reference fixture: `XREF-CARD-NUM X(16)`, `XREF-CUST-ID 9(09)`
/// and `XREF-ACCT-ID 9(11)` [app/cpy/CVACT03Y.cpy:L5-7], stored as 36 of its 50 bytes (E-04).
#[derive(Debug, PartialEq, Eq)]
struct XrefRecord {
    card_number: String,
    customer_id: String,
    account_id: String,
}

/// The records of `app/data/ASCII/cardxref.txt` in file order, the card-number order in which
/// CBSTM03A reads them.
fn xref_records() -> Vec<XrefRecord> {
    fixture_lines("cardxref.txt")
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let field = |start: usize, end: usize| {
                record
                    .get(start..end)
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .map_or_else(
                        || {
                            panic!(
                                "cardxref.txt record {}: no text at {start}..{end}",
                                index + 1
                            )
                        },
                        str::to_owned,
                    )
            };
            XrefRecord {
                card_number: field(0, 16),
                customer_id: field(16, 25),
                account_id: field(25, 36),
            }
        })
        .collect()
}

/// The capacities of the CBSTM03A tables, cards and transactions per card: the `OCCURS` counts
/// the copybook field cell of [`TABLES_ROW`] gives [`CARD_TABLE`] and [`TRANSACTION_TABLE`],
/// which the row's expected result must state as the capacity of the load.
fn table_capacities(doc: &Document) -> (usize, usize) {
    let clauses = parity_cell(doc, TABLES_ROW, PARITY_COMMON_COLUMNS[3]);
    let occurs = |name: &str| {
        let count = clauses
            .split(',')
            .find_map(|clause| {
                let words: Vec<&str> = clause.split_whitespace().collect();
                match words[..] {
                    [table, "OCCURS", count] if table == name => Some(count),
                    _ => None,
                }
            })
            .unwrap_or_else(|| panic!("{TABLES_ROW}: {clauses:?} gives no {name} OCCURS clause"));
        expected_number(count, &format!("{TABLES_ROW} {name} OCCURS"))
    };
    let (cards, per_card) = (occurs(CARD_TABLE), occurs(TRANSACTION_TABLE));
    let rule = parity_cell(doc, TABLES_ROW, PARITY_COMMON_COLUMNS[5]);
    let stated = format!("capacity {cards} cards and {per_card} transactions per card");
    assert!(
        rule.contains(&stated),
        "{TABLES_ROW}: the expected result does not state {stated:?}: {rule}"
    );
    (cards, per_card)
}

/// P-CBSTM03A-1256 gives the input class beyond the CBSTM03A tables the disposition `halt`, and
/// its escalation is a `halt` entry of the escalation log (AAP 0.9.6).
fn assert_halt_is_escalated(doc: &Document) {
    assert_eq!(
        expected_value(doc, HALT_ROW, "disposition"),
        "halt",
        "{HALT_ROW} disposition"
    );
    let escalation = expected_value(doc, HALT_ROW, "escalation");
    let log = Repo::new(fixtures::repo_root())
        .escalations()
        .unwrap_or_else(|error| panic!("cannot load the escalation log: {error}"));
    let (table, row) = log
        .find_row(&escalation)
        .unwrap_or_else(|| panic!("the escalation log has no {escalation}"));
    let status = cell(table, row, ESCALATION_COLUMNS[6]);
    assert!(
        status.starts_with("halt"),
        "{escalation} has status {status:?}, not halt"
    );
}

/// Section D.5 of the parity audit, read: the two statement files, the card statements and every
/// record both files must hold.
struct Audited {
    /// `STMTFILE`.
    stmt: AuditedFile,
    /// `HTMLFILE`.
    html: AuditedFile,
    /// The card statements, in the order they are written.
    cards: Vec<CardSummary>,
    /// Every `STMTFILE` record, statement after statement.
    stmt_records: Vec<String>,
    /// Every `HTMLFILE` record, statement after statement.
    html_records: Vec<String>,
}

impl Audited {
    /// Reads the D.5 table, the one holding [`CAPACITY_ROW`], and builds every statement.
    fn read(doc: &Document) -> Self {
        let (table, _) = parity_row(doc, CAPACITY_ROW);
        let mut files = Vec::new();
        let mut cards = Vec::new();
        let mut lines = Vec::new();
        for row in &table.rows {
            let id = cell(table, row, PARITY_COMMON_COLUMNS[0]);
            let pairs = carddemo_audit::key_values(cell(table, row, PARITY_COMMON_COLUMNS[5]));
            match value(&pairs, "kind") {
                Some("template" | "variable") => lines.push(LineRow::read(id, &pairs)),
                Some("card_summary") => cards.push(CardSummary::read(id, &pairs)),
                None if id == FILES_ROW => files = AuditedFile::read_all(&pairs),
                None if id == CAPACITY_ROW => {}
                kind => panic!("audit row {id}: kind {kind:?} is no CREASTMT row kind"),
            }
        }
        let Ok([stmt, html]) = <[AuditedFile; 2]>::try_from(files) else {
            panic!("{FILES_ROW} must record exactly {STMT_DD} and {HTML_DD}");
        };
        assert_eq!(
            [stmt.dd.as_str(), html.dd.as_str()],
            [STMT_DD, HTML_DD],
            "{FILES_ROW} file order"
        );
        check_line_rows(&lines, &cards);
        let mut stmt_records = Vec::new();
        let mut html_records = Vec::new();
        for card in &cards {
            let stmt_lines = statement_lines(&stmt, card, &lines, &[]);
            html_records.extend(statement_lines(&html, card, &lines, &stmt_lines));
            stmt_records.extend(stmt_lines);
        }
        Self {
            stmt,
            html,
            cards,
            stmt_records,
            html_records,
        }
    }
}

/// A statement file as [`FILES_ROW`] records it.
struct AuditedFile {
    /// The DD name: [`STMT_DD`] or [`HTML_DD`].
    dd: String,
    /// The file name in the statement directory.
    name: String,
    /// The records of the whole file.
    records: usize,
    /// The bytes of each record, which LF follows.
    width: usize,
    /// The records of one statement, an expression in `n`, the transactions of its card.
    per_card: String,
}

impl AuditedFile {
    /// The files of [`FILES_ROW`]: each `file=` starts one, which its `name=`, `records=`,
    /// `width=` and `terminator=` describe, and `per_card_records=` gives the records of one
    /// statement of each.
    fn read_all(pairs: &[KeyValue]) -> Vec<Self> {
        let formulas: BTreeMap<&str, &str> = required(pairs, "per_card_records", FILES_ROW)
            .split(", ")
            .filter_map(|part| part.split_once(' '))
            .filter(|(dd, _)| [STMT_DD, HTML_DD].contains(dd))
            .collect();
        let starts: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, pair)| pair.key == "file")
            .map(|(at, _)| at)
            .collect();
        starts
            .iter()
            .enumerate()
            .map(|(index, &start)| {
                let end = starts.get(index + 1).copied().unwrap_or(pairs.len());
                let group = &pairs[start..end];
                let dd = group[0].value.clone();
                assert_eq!(
                    required(group, "terminator", FILES_ROW),
                    "LF",
                    "{FILES_ROW} {dd} terminator"
                );
                let per_card = formulas.get(dd.as_str()).map_or_else(
                    || panic!("{FILES_ROW} gives no per_card_records for {dd}"),
                    |formula| (*formula).to_owned(),
                );
                Self {
                    name: required(group, "name", FILES_ROW).to_owned(),
                    records: number(group, "records", FILES_ROW),
                    width: number(group, "width", FILES_ROW),
                    per_card,
                    dd,
                }
            })
            .collect()
    }

    /// The records of one statement of a card with `n` transactions.
    fn records_for(&self, n: usize) -> usize {
        evaluate(&self.per_card, n, 0, FILES_ROW)
    }
}

/// A `kind=card_summary` row: one statement, of the card `card_number`, which `n` transactions
/// make.
struct CardSummary {
    id: String,
    card_number: String,
    account_id: String,
    customer_id: String,
    n: usize,
}

impl CardSummary {
    /// Reads the card summary row `id`.
    fn read(id: &str, pairs: &[KeyValue]) -> Self {
        Self {
            id: id.to_owned(),
            card_number: required(pairs, "card_number", id).to_owned(),
            account_id: required(pairs, "account_id", id).to_owned(),
            customer_id: required(pairs, "customer_id", id).to_owned(),
            n: number(pairs, "n", id),
        }
    }
}

/// A row that gives statement records: `kind=template`, a line of every statement, or
/// `kind=variable`, a line of the statement of `card_number` only.
struct LineRow {
    id: String,
    /// The DD name of the file the line belongs to.
    dd: String,
    /// The card of a variable row; `None` for a template row.
    card_number: Option<String>,
    /// The line number within the statement: a number, or for a template row an expression in
    /// `n` and, for a line repeated per transaction, in `i`, the transaction from 1 to `n`.
    line_no: String,
    /// The record text: a double-quoted string and the width it is right-padded to.
    text: CodeSpan,
    /// The `{name}` placeholders of the text, besides `{UNDEF:k}`.
    fields: Vec<FieldRef>,
}

impl LineRow {
    /// Reads the template or variable row `id`.
    fn read(id: &str, pairs: &[KeyValue]) -> Self {
        let kind = required(pairs, "kind", id);
        let card_number = value(pairs, "card_number").map(str::to_owned);
        assert_eq!(
            card_number.is_some(),
            kind == "variable",
            "audit row {id}: a variable row, and only a variable row, names its card"
        );
        let text = required(pairs, "text", id);
        Self {
            id: id.to_owned(),
            dd: required(pairs, "file", id).to_owned(),
            card_number,
            line_no: required(pairs, "line_no", id).to_owned(),
            text: carddemo_audit::code_span(text)
                .unwrap_or_else(|| panic!("audit row {id}: text={text} holds no code span")),
            fields: pairs
                .iter()
                .filter(|pair| pair.key == "field")
                .map(|pair| FieldRef::read(&pair.value, id))
                .collect(),
        }
    }

    /// The transactions `i` the row gives a line for in a statement of `n` transactions: each
    /// one when its line number names `i`, otherwise a single line (`i` unused).
    fn positions(&self, n: usize) -> Vec<usize> {
        if self.line_no.contains('i') {
            (1..=n).collect()
        } else {
            vec![0]
        }
    }

    /// The record of transaction `i` in a statement of `n` transactions, right-padded with spaces
    /// to `width`. `stmt` holds the `STMTFILE` records of the same statement, which the
    /// placeholders of an `HTMLFILE` row are taken from.
    fn record(&self, width: usize, n: usize, i: usize, stmt: &[String]) -> String {
        assert_eq!(
            self.text.width,
            Some(width),
            "audit row {}: the text is not given at its file's width={width}",
            self.id
        );
        let text = expand(&unquote(&self.text.text, &self.id), &self.id, |name| {
            self.field(name, n, i, stmt)
        });
        assert!(
            text.len() <= width,
            "audit row {}: {text:?} is {} bytes, more than width={width}",
            self.id,
            text.len()
        );
        format!("{text:<width$}")
    }

    /// The content of the placeholder `{name}` at transaction `i` of `n`.
    fn field(&self, name: &str, n: usize, i: usize, stmt: &[String]) -> String {
        let field = self
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap_or_else(|| panic!("audit row {}: no field= defines {{{name}}}", self.id));
        let line_no = evaluate(&field.line_no, n, i, &self.id);
        line_no
            .checked_sub(1)
            .and_then(|index| stmt.get(index))
            .and_then(|line| line.get(field.from - 1..field.to))
            .map_or_else(
                || {
                    panic!(
                        "audit row {}: {name} is bytes {}-{} of {STMT_DD} line {line_no}, which \
                         the statement does not hold",
                        self.id, field.from, field.to
                    )
                },
                str::to_owned,
            )
    }
}

/// A `field=` of a template row: `{name}` in its text stands for bytes `from` to `to`, counted
/// from 1, of the `STMTFILE` line `line_no` of the same statement.
struct FieldRef {
    name: String,
    from: usize,
    to: usize,
    line_no: String,
}

impl FieldRef {
    /// Reads `<name> is bytes <from>-<to> of STMTFILE line <line_no> of the same card`.
    fn read(text: &str, id: &str) -> Self {
        let words: Vec<&str> = text.split_whitespace().collect();
        let [
            name,
            "is",
            "bytes",
            bytes,
            "of",
            STMT_DD,
            "line",
            line_no,
            "of",
            "the",
            "same",
            "card",
        ] = words.as_slice()
        else {
            panic!(
                "audit row {id}: field={text} is not `<name> is bytes <from>-<to> of {STMT_DD} \
                 line <line> of the same card`"
            );
        };
        let range = bytes
            .split_once('-')
            .and_then(|(from, to)| Some((from.parse::<usize>().ok()?, to.parse::<usize>().ok()?)))
            .filter(|&(from, to)| (1..=to).contains(&from));
        let Some((from, to)) = range else {
            panic!("audit row {id}: field={text} names no byte range");
        };
        Self {
            name: (*name).to_owned(),
            from,
            to,
            line_no: (*line_no).to_owned(),
        }
    }
}

/// Checks that every line row names a statement file and every variable row a card with a
/// summary, so that building the statements places each variable row exactly once.
fn check_line_rows(lines: &[LineRow], cards: &[CardSummary]) {
    let numbers: BTreeSet<&str> = cards.iter().map(|card| card.card_number.as_str()).collect();
    for line in lines {
        assert!(
            [STMT_DD, HTML_DD].contains(&line.dd.as_str()),
            "audit row {}: file={} is no statement file",
            line.id,
            line.dd
        );
        if let Some(number) = line.card_number.as_deref() {
            assert!(
                numbers.contains(number),
                "audit row {}: card {number} has no card_summary row",
                line.id
            );
        }
    }
}

/// The records of the statement of `card` in `file`: each template row of the file at each of
/// its positions, and the card's own variable rows. `stmt` holds the card's `STMTFILE` records,
/// empty while those are built.
///
/// A line two rows give, a line past the statement's record count ([`FILES_ROW`]) and a line no
/// row gives each fail with the rows, the card and the line: such a statement cannot be built
/// from the audit, so the missing or conflicting row is escalated, never guessed.
fn statement_lines(
    file: &AuditedFile,
    card: &CardSummary,
    rows: &[LineRow],
    stmt: &[String],
) -> Vec<String> {
    let mut slots: Vec<Option<(&str, String)>> = vec![None; file.records_for(card.n)];
    let card_rows = rows.iter().filter(|row| {
        row.dd == file.dd
            && row
                .card_number
                .as_deref()
                .is_none_or(|number| number == card.card_number)
    });
    for row in card_rows {
        for i in row.positions(card.n) {
            let line_no = evaluate(&row.line_no, card.n, i, &row.id);
            let lines = slots.len();
            let slot = line_no
                .checked_sub(1)
                .and_then(|index| slots.get_mut(index))
                .unwrap_or_else(|| {
                    panic!(
                        "audit row {}: line {line_no} lies past the {lines} {} lines of card {}",
                        row.id, file.dd, card.card_number
                    )
                });
            if let Some((other, _)) = slot {
                panic!(
                    "audit rows {other} and {} both give {} line {line_no} of card {}",
                    row.id, file.dd, card.card_number
                );
            }
            *slot = Some((row.id.as_str(), row.record(file.width, card.n, i, stmt)));
        }
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(index, slot)| {
            let Some((_, record)) = slot else {
                panic!(
                    "audit D.5 gives no {} line {} of the statement of card {} ({}): escalate the \
                     missing row; the test never guesses a line",
                    file.dd,
                    index + 1,
                    card.card_number,
                    card.id
                );
            };
            record
        })
        .collect()
}

/// `text` with each `{UNDEF:k}` replaced by k spaces, the bytes the COBOL never sets, which the
/// replacement writes as spaces (E-25), and each other `{name}` by `field(name)`.
fn expand(text: &str, id: &str, field: impl Fn(&str) -> String) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once('{') {
        let (name, tail) = after.split_once('}').unwrap_or_else(|| {
            panic!("audit row {id}: {text:?} opens a placeholder it never closes")
        });
        expanded.push_str(before);
        if let Some(count) = name.strip_prefix("UNDEF:") {
            let count: usize = count.parse().unwrap_or_else(|error| {
                panic!("audit row {id}: {{{name}}} counts no bytes: {error}")
            });
            expanded.push_str(&" ".repeat(count));
        } else {
            expanded.push_str(&field(name));
        }
        rest = tail;
    }
    expanded.push_str(rest);
    expanded
}

/// The value of the double-quoted string `content`, whose only escapes are `\"` and `\\`.
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("audit row {id}: {content:?} is not a double-quoted text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!("audit row {id}: {content:?} holds a backslash that escapes nothing"),
            },
            '"' => panic!("audit row {id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}

/// The value of the line-number expression `expr` of audit row `id` in a statement of `n`
/// transactions at transaction `i`: decimal numbers, `n`, `i`, `+`, `-`, `*` or juxtaposition
/// (`11n`), and parentheses.
fn evaluate(expr: &str, n: usize, i: usize, id: &str) -> usize {
    let (Ok(count), Ok(position)) = (i64::try_from(n), i64::try_from(i)) else {
        panic!("audit row {id}: n={n} or i={i} is too large");
    };
    let mut parser = LineExpr {
        chars: expr.chars().peekable(),
        n: count,
        i: position,
    };
    let value = parser.sum();
    let ended = parser.chars.peek().is_none();
    value
        .filter(|_| ended)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_else(|| {
            panic!("audit row {id}: line expression {expr:?} has no value at n={n}, i={i}")
        })
}

/// A recursive-descent reader of one line-number expression.
struct LineExpr<'e> {
    chars: Peekable<Chars<'e>>,
    n: i64,
    i: i64,
}

impl LineExpr<'_> {
    /// Terms joined by `+` and `-`.
    fn sum(&mut self) -> Option<i64> {
        let mut value = self.product()?;
        loop {
            match self.chars.peek() {
                Some('+') => {
                    self.chars.next();
                    value = value.checked_add(self.product()?)?;
                }
                Some('-') => {
                    self.chars.next();
                    value = value.checked_sub(self.product()?)?;
                }
                _ => return Some(value),
            }
        }
    }

    /// Factors joined by `*` or written side by side.
    fn product(&mut self) -> Option<i64> {
        let mut value = self.factor()?;
        loop {
            match self.chars.peek() {
                Some('*') => {
                    self.chars.next();
                }
                Some('n' | 'i' | '(') => {}
                _ => return Some(value),
            }
            value = value.checked_mul(self.factor()?)?;
        }
    }

    /// A decimal number, `n`, `i` or a parenthesised sum.
    fn factor(&mut self) -> Option<i64> {
        match self.chars.next()? {
            'n' => Some(self.n),
            'i' => Some(self.i),
            '(' => {
                let value = self.sum()?;
                (self.chars.next()? == ')').then_some(value)
            }
            first => {
                let mut value = i64::from(first.to_digit(10)?);
                while let Some(digit) = self.chars.peek().and_then(|ch| ch.to_digit(10)) {
                    self.chars.next();
                    value = value.checked_mul(10)?.checked_add(i64::from(digit))?;
                }
                Some(value)
            }
        }
    }
}

/// The first `key=` value of `pairs`.
fn value<'p>(pairs: &'p [KeyValue], key: &str) -> Option<&'p str> {
    pairs
        .iter()
        .find(|pair| pair.key == key)
        .map(|pair| pair.value.as_str())
}

/// The first `key=` value of `pairs`, which audit row `id` must give.
fn required<'p>(pairs: &'p [KeyValue], key: &str, id: &str) -> &'p str {
    value(pairs, key).unwrap_or_else(|| panic!("audit row {id} gives no {key}="))
}

/// The count the first `key=` of audit row `id` gives.
fn number(pairs: &[KeyValue], key: &str, id: &str) -> usize {
    expected_number(required(pairs, key, id), &format!("{id} {key}"))
}

/// `value` as a count, `what` naming it in the failure message.
fn expected_number(value: &str, what: &str) -> usize {
    value
        .parse()
        .unwrap_or_else(|error| panic!("{what}={value} is no count: {error}"))
}

/// The cell of `row` under `column`.
fn cell<'t>(table: &'t Table, row: &'t Row, column: &str) -> &'t str {
    table.cell(row, column).unwrap_or_else(|| {
        panic!(
            "the audit row at line {} has no {column:?} cell (table at line {})",
            row.line, table.line
        )
    })
}

/// Every parity audit row of CBSTM03A, CBSTM03B and CREASTMT, in audit order, each proven here;
/// `p_ids_are_every_statement_row` holds the list to the audit.
///
/// - `statements_match_audit_text_html_and_pdf` builds every record of both files from the D.5
///   rows and compares the files with them, which also proves the statement rows of sections B
///   and C, the job steps, the control flow, the table load, the reads and closes on their success
///   branches and the end with condition code 0 (P-CBSTM03A-1264).
/// - `empty_live_master_abends_reading_trnxfile` proves P-CBSTM03A-003 and P-CBSTM03A-011.
/// - `more_than_ten_transactions_on_a_card_halts_with_exit_16` proves the capacity of
///   P-CBSTM03A-004, the halt of P-CBSTM03A-1256 and the STEP030 delete of P-CREASTMT-004.
const P_IDS: &[&str] = &[
    // Section B, CBSTM03A: the files, the control flow, the table load, the mainline with its
    // reads and closes on their success branches, the statement lines, the end of the run, the
    // empty transaction file and the abend.
    "P-CBSTM03A-001",
    "P-CBSTM03A-002",
    "P-CBSTM03A-003",
    "P-CBSTM03A-004",
    "P-CBSTM03A-005",
    "P-CBSTM03A-1257",
    "P-CBSTM03A-1258",
    "P-CBSTM03A-1259",
    "P-CBSTM03A-006",
    "P-CBSTM03A-007",
    "P-CBSTM03A-008",
    "P-CBSTM03A-009",
    "P-CBSTM03A-010",
    "P-CBSTM03A-1260",
    "P-CBSTM03A-1261",
    "P-CBSTM03A-1262",
    "P-CBSTM03A-1263",
    "P-CBSTM03A-1264",
    "P-CBSTM03A-011",
    // Section B, CBSTM03B: the dispatch and the sequential and keyed reads behind every statement.
    "P-CBSTM03B-001",
    "P-CBSTM03B-002",
    "P-CBSTM03B-003",
    "P-CBSTM03B-004",
    "P-CBSTM03B-005",
    // Section B, CREASTMT: the job steps, the STEP030 delete included.
    "P-CREASTMT-001",
    "P-CREASTMT-002",
    "P-CREASTMT-003",
    "P-CREASTMT-004",
    "P-CREASTMT-005",
    // Section C: the statement total and the edited amounts.
    "P-CBSTM03A-012",
    "P-CBSTM03A-013",
    // Section D.5: the capacity confirmation, the files written, the template lines, and each
    // card's summary and variable lines.
    "P-CBSTM03A-014",
    "P-CBSTM03A-015",
    "P-CBSTM03A-016",
    "P-CBSTM03A-017",
    "P-CBSTM03A-018",
    "P-CBSTM03A-019",
    "P-CBSTM03A-020",
    "P-CBSTM03A-021",
    "P-CBSTM03A-022",
    "P-CBSTM03A-023",
    "P-CBSTM03A-024",
    "P-CBSTM03A-025",
    "P-CBSTM03A-026",
    "P-CBSTM03A-027",
    "P-CBSTM03A-028",
    "P-CBSTM03A-029",
    "P-CBSTM03A-030",
    "P-CBSTM03A-031",
    "P-CBSTM03A-032",
    "P-CBSTM03A-033",
    "P-CBSTM03A-034",
    "P-CBSTM03A-035",
    "P-CBSTM03A-036",
    "P-CBSTM03A-037",
    "P-CBSTM03A-038",
    "P-CBSTM03A-039",
    "P-CBSTM03A-040",
    "P-CBSTM03A-041",
    "P-CBSTM03A-042",
    "P-CBSTM03A-043",
    "P-CBSTM03A-044",
    "P-CBSTM03A-045",
    "P-CBSTM03A-046",
    "P-CBSTM03A-047",
    "P-CBSTM03A-048",
    "P-CBSTM03A-049",
    "P-CBSTM03A-050",
    "P-CBSTM03A-051",
    "P-CBSTM03A-052",
    "P-CBSTM03A-053",
    "P-CBSTM03A-054",
    "P-CBSTM03A-055",
    "P-CBSTM03A-056",
    "P-CBSTM03A-057",
    "P-CBSTM03A-058",
    "P-CBSTM03A-059",
    "P-CBSTM03A-060",
    "P-CBSTM03A-061",
    "P-CBSTM03A-062",
    "P-CBSTM03A-063",
    "P-CBSTM03A-064",
    "P-CBSTM03A-065",
    "P-CBSTM03A-066",
    "P-CBSTM03A-067",
    "P-CBSTM03A-068",
    "P-CBSTM03A-069",
    "P-CBSTM03A-070",
    "P-CBSTM03A-071",
    "P-CBSTM03A-072",
    "P-CBSTM03A-073",
    "P-CBSTM03A-074",
    "P-CBSTM03A-075",
    "P-CBSTM03A-076",
    "P-CBSTM03A-077",
    "P-CBSTM03A-078",
    "P-CBSTM03A-079",
    "P-CBSTM03A-080",
    "P-CBSTM03A-081",
    "P-CBSTM03A-082",
    "P-CBSTM03A-083",
    "P-CBSTM03A-084",
    "P-CBSTM03A-085",
    "P-CBSTM03A-086",
    "P-CBSTM03A-087",
    "P-CBSTM03A-088",
    "P-CBSTM03A-089",
    "P-CBSTM03A-090",
    "P-CBSTM03A-091",
    "P-CBSTM03A-092",
    "P-CBSTM03A-093",
    "P-CBSTM03A-094",
    "P-CBSTM03A-095",
    "P-CBSTM03A-096",
    "P-CBSTM03A-097",
    "P-CBSTM03A-098",
    "P-CBSTM03A-099",
    "P-CBSTM03A-100",
    "P-CBSTM03A-101",
    "P-CBSTM03A-102",
    "P-CBSTM03A-103",
    "P-CBSTM03A-104",
    "P-CBSTM03A-105",
    "P-CBSTM03A-106",
    "P-CBSTM03A-107",
    "P-CBSTM03A-108",
    "P-CBSTM03A-109",
    "P-CBSTM03A-110",
    "P-CBSTM03A-111",
    "P-CBSTM03A-112",
    "P-CBSTM03A-113",
    "P-CBSTM03A-114",
    "P-CBSTM03A-115",
    "P-CBSTM03A-116",
    "P-CBSTM03A-117",
    "P-CBSTM03A-118",
    "P-CBSTM03A-119",
    "P-CBSTM03A-120",
    "P-CBSTM03A-121",
    "P-CBSTM03A-122",
    "P-CBSTM03A-123",
    "P-CBSTM03A-124",
    "P-CBSTM03A-125",
    "P-CBSTM03A-126",
    "P-CBSTM03A-127",
    "P-CBSTM03A-128",
    "P-CBSTM03A-129",
    "P-CBSTM03A-130",
    "P-CBSTM03A-131",
    "P-CBSTM03A-132",
    "P-CBSTM03A-133",
    "P-CBSTM03A-134",
    "P-CBSTM03A-135",
    "P-CBSTM03A-136",
    "P-CBSTM03A-137",
    "P-CBSTM03A-138",
    "P-CBSTM03A-139",
    "P-CBSTM03A-140",
    "P-CBSTM03A-141",
    "P-CBSTM03A-142",
    "P-CBSTM03A-143",
    "P-CBSTM03A-144",
    "P-CBSTM03A-145",
    "P-CBSTM03A-146",
    "P-CBSTM03A-147",
    "P-CBSTM03A-148",
    "P-CBSTM03A-149",
    "P-CBSTM03A-150",
    "P-CBSTM03A-151",
    "P-CBSTM03A-152",
    "P-CBSTM03A-153",
    "P-CBSTM03A-154",
    "P-CBSTM03A-155",
    "P-CBSTM03A-156",
    "P-CBSTM03A-157",
    "P-CBSTM03A-158",
    "P-CBSTM03A-159",
    "P-CBSTM03A-160",
    "P-CBSTM03A-161",
    "P-CBSTM03A-162",
    "P-CBSTM03A-163",
    "P-CBSTM03A-164",
    "P-CBSTM03A-165",
    "P-CBSTM03A-166",
    "P-CBSTM03A-167",
    "P-CBSTM03A-168",
    "P-CBSTM03A-169",
    "P-CBSTM03A-170",
    "P-CBSTM03A-171",
    "P-CBSTM03A-172",
    "P-CBSTM03A-173",
    "P-CBSTM03A-174",
    "P-CBSTM03A-175",
    "P-CBSTM03A-176",
    "P-CBSTM03A-177",
    "P-CBSTM03A-178",
    "P-CBSTM03A-179",
    "P-CBSTM03A-180",
    "P-CBSTM03A-181",
    "P-CBSTM03A-182",
    "P-CBSTM03A-183",
    "P-CBSTM03A-184",
    "P-CBSTM03A-185",
    "P-CBSTM03A-186",
    "P-CBSTM03A-187",
    "P-CBSTM03A-188",
    "P-CBSTM03A-189",
    "P-CBSTM03A-190",
    "P-CBSTM03A-191",
    "P-CBSTM03A-192",
    "P-CBSTM03A-193",
    "P-CBSTM03A-194",
    "P-CBSTM03A-195",
    "P-CBSTM03A-196",
    "P-CBSTM03A-197",
    "P-CBSTM03A-198",
    "P-CBSTM03A-199",
    "P-CBSTM03A-200",
    "P-CBSTM03A-201",
    "P-CBSTM03A-202",
    "P-CBSTM03A-203",
    "P-CBSTM03A-204",
    "P-CBSTM03A-205",
    "P-CBSTM03A-206",
    "P-CBSTM03A-207",
    "P-CBSTM03A-208",
    "P-CBSTM03A-209",
    "P-CBSTM03A-210",
    "P-CBSTM03A-211",
    "P-CBSTM03A-212",
    "P-CBSTM03A-213",
    "P-CBSTM03A-214",
    "P-CBSTM03A-215",
    "P-CBSTM03A-216",
    "P-CBSTM03A-217",
    "P-CBSTM03A-218",
    "P-CBSTM03A-219",
    "P-CBSTM03A-220",
    "P-CBSTM03A-221",
    "P-CBSTM03A-222",
    "P-CBSTM03A-223",
    "P-CBSTM03A-224",
    "P-CBSTM03A-225",
    "P-CBSTM03A-226",
    "P-CBSTM03A-227",
    "P-CBSTM03A-228",
    "P-CBSTM03A-229",
    "P-CBSTM03A-230",
    "P-CBSTM03A-231",
    "P-CBSTM03A-232",
    "P-CBSTM03A-233",
    "P-CBSTM03A-234",
    "P-CBSTM03A-235",
    "P-CBSTM03A-236",
    "P-CBSTM03A-237",
    "P-CBSTM03A-238",
    "P-CBSTM03A-239",
    "P-CBSTM03A-240",
    "P-CBSTM03A-241",
    "P-CBSTM03A-242",
    "P-CBSTM03A-243",
    "P-CBSTM03A-244",
    "P-CBSTM03A-245",
    "P-CBSTM03A-246",
    "P-CBSTM03A-247",
    "P-CBSTM03A-248",
    "P-CBSTM03A-249",
    "P-CBSTM03A-250",
    "P-CBSTM03A-251",
    "P-CBSTM03A-252",
    "P-CBSTM03A-253",
    "P-CBSTM03A-254",
    "P-CBSTM03A-255",
    "P-CBSTM03A-256",
    "P-CBSTM03A-257",
    "P-CBSTM03A-258",
    "P-CBSTM03A-259",
    "P-CBSTM03A-260",
    "P-CBSTM03A-261",
    "P-CBSTM03A-262",
    "P-CBSTM03A-263",
    "P-CBSTM03A-264",
    "P-CBSTM03A-265",
    "P-CBSTM03A-266",
    "P-CBSTM03A-267",
    "P-CBSTM03A-268",
    "P-CBSTM03A-269",
    "P-CBSTM03A-270",
    "P-CBSTM03A-271",
    "P-CBSTM03A-272",
    "P-CBSTM03A-273",
    "P-CBSTM03A-274",
    "P-CBSTM03A-275",
    "P-CBSTM03A-276",
    "P-CBSTM03A-277",
    "P-CBSTM03A-278",
    "P-CBSTM03A-279",
    "P-CBSTM03A-280",
    "P-CBSTM03A-281",
    "P-CBSTM03A-282",
    "P-CBSTM03A-283",
    "P-CBSTM03A-284",
    "P-CBSTM03A-285",
    "P-CBSTM03A-286",
    "P-CBSTM03A-287",
    "P-CBSTM03A-288",
    "P-CBSTM03A-289",
    "P-CBSTM03A-290",
    "P-CBSTM03A-291",
    "P-CBSTM03A-292",
    "P-CBSTM03A-293",
    "P-CBSTM03A-294",
    "P-CBSTM03A-295",
    "P-CBSTM03A-296",
    "P-CBSTM03A-297",
    "P-CBSTM03A-298",
    "P-CBSTM03A-299",
    "P-CBSTM03A-300",
    "P-CBSTM03A-301",
    "P-CBSTM03A-302",
    "P-CBSTM03A-303",
    "P-CBSTM03A-304",
    "P-CBSTM03A-305",
    "P-CBSTM03A-306",
    "P-CBSTM03A-307",
    "P-CBSTM03A-308",
    "P-CBSTM03A-309",
    "P-CBSTM03A-310",
    "P-CBSTM03A-311",
    "P-CBSTM03A-312",
    "P-CBSTM03A-313",
    "P-CBSTM03A-314",
    "P-CBSTM03A-315",
    "P-CBSTM03A-316",
    "P-CBSTM03A-317",
    "P-CBSTM03A-318",
    "P-CBSTM03A-319",
    "P-CBSTM03A-320",
    "P-CBSTM03A-321",
    "P-CBSTM03A-322",
    "P-CBSTM03A-323",
    "P-CBSTM03A-324",
    "P-CBSTM03A-325",
    "P-CBSTM03A-326",
    "P-CBSTM03A-327",
    "P-CBSTM03A-328",
    "P-CBSTM03A-329",
    "P-CBSTM03A-330",
    "P-CBSTM03A-331",
    "P-CBSTM03A-332",
    "P-CBSTM03A-333",
    "P-CBSTM03A-334",
    "P-CBSTM03A-335",
    "P-CBSTM03A-336",
    "P-CBSTM03A-337",
    "P-CBSTM03A-338",
    "P-CBSTM03A-339",
    "P-CBSTM03A-340",
    "P-CBSTM03A-341",
    "P-CBSTM03A-342",
    "P-CBSTM03A-343",
    "P-CBSTM03A-344",
    "P-CBSTM03A-345",
    "P-CBSTM03A-346",
    "P-CBSTM03A-347",
    "P-CBSTM03A-348",
    "P-CBSTM03A-349",
    "P-CBSTM03A-350",
    "P-CBSTM03A-351",
    "P-CBSTM03A-352",
    "P-CBSTM03A-353",
    "P-CBSTM03A-354",
    "P-CBSTM03A-355",
    "P-CBSTM03A-356",
    "P-CBSTM03A-357",
    "P-CBSTM03A-358",
    "P-CBSTM03A-359",
    "P-CBSTM03A-360",
    "P-CBSTM03A-361",
    "P-CBSTM03A-362",
    "P-CBSTM03A-363",
    "P-CBSTM03A-364",
    "P-CBSTM03A-365",
    "P-CBSTM03A-366",
    "P-CBSTM03A-367",
    "P-CBSTM03A-368",
    "P-CBSTM03A-369",
    "P-CBSTM03A-370",
    "P-CBSTM03A-371",
    "P-CBSTM03A-372",
    "P-CBSTM03A-373",
    "P-CBSTM03A-374",
    "P-CBSTM03A-375",
    "P-CBSTM03A-376",
    "P-CBSTM03A-377",
    "P-CBSTM03A-378",
    "P-CBSTM03A-379",
    "P-CBSTM03A-380",
    "P-CBSTM03A-381",
    "P-CBSTM03A-382",
    "P-CBSTM03A-383",
    "P-CBSTM03A-384",
    "P-CBSTM03A-385",
    "P-CBSTM03A-386",
    "P-CBSTM03A-387",
    "P-CBSTM03A-388",
    "P-CBSTM03A-389",
    "P-CBSTM03A-390",
    "P-CBSTM03A-391",
    "P-CBSTM03A-392",
    "P-CBSTM03A-393",
    "P-CBSTM03A-394",
    "P-CBSTM03A-395",
    "P-CBSTM03A-396",
    "P-CBSTM03A-397",
    "P-CBSTM03A-398",
    "P-CBSTM03A-399",
    "P-CBSTM03A-400",
    "P-CBSTM03A-401",
    "P-CBSTM03A-402",
    "P-CBSTM03A-403",
    "P-CBSTM03A-404",
    "P-CBSTM03A-405",
    "P-CBSTM03A-406",
    "P-CBSTM03A-407",
    "P-CBSTM03A-408",
    "P-CBSTM03A-409",
    "P-CBSTM03A-410",
    "P-CBSTM03A-411",
    "P-CBSTM03A-412",
    "P-CBSTM03A-413",
    "P-CBSTM03A-414",
    "P-CBSTM03A-415",
    "P-CBSTM03A-416",
    "P-CBSTM03A-417",
    "P-CBSTM03A-418",
    "P-CBSTM03A-419",
    "P-CBSTM03A-420",
    "P-CBSTM03A-421",
    "P-CBSTM03A-422",
    "P-CBSTM03A-423",
    "P-CBSTM03A-424",
    "P-CBSTM03A-425",
    "P-CBSTM03A-426",
    "P-CBSTM03A-427",
    "P-CBSTM03A-428",
    "P-CBSTM03A-429",
    "P-CBSTM03A-430",
    "P-CBSTM03A-431",
    "P-CBSTM03A-432",
    "P-CBSTM03A-433",
    "P-CBSTM03A-434",
    "P-CBSTM03A-435",
    "P-CBSTM03A-436",
    "P-CBSTM03A-437",
    "P-CBSTM03A-438",
    "P-CBSTM03A-439",
    "P-CBSTM03A-440",
    "P-CBSTM03A-441",
    "P-CBSTM03A-442",
    "P-CBSTM03A-443",
    "P-CBSTM03A-444",
    "P-CBSTM03A-445",
    "P-CBSTM03A-446",
    "P-CBSTM03A-447",
    "P-CBSTM03A-448",
    "P-CBSTM03A-449",
    "P-CBSTM03A-450",
    "P-CBSTM03A-451",
    "P-CBSTM03A-452",
    "P-CBSTM03A-453",
    "P-CBSTM03A-454",
    "P-CBSTM03A-455",
    "P-CBSTM03A-456",
    "P-CBSTM03A-457",
    "P-CBSTM03A-458",
    "P-CBSTM03A-459",
    "P-CBSTM03A-460",
    "P-CBSTM03A-461",
    "P-CBSTM03A-462",
    "P-CBSTM03A-463",
    "P-CBSTM03A-464",
    "P-CBSTM03A-465",
    "P-CBSTM03A-466",
    "P-CBSTM03A-467",
    "P-CBSTM03A-468",
    "P-CBSTM03A-469",
    "P-CBSTM03A-470",
    "P-CBSTM03A-471",
    "P-CBSTM03A-472",
    "P-CBSTM03A-473",
    "P-CBSTM03A-474",
    "P-CBSTM03A-475",
    "P-CBSTM03A-476",
    "P-CBSTM03A-477",
    "P-CBSTM03A-478",
    "P-CBSTM03A-479",
    "P-CBSTM03A-480",
    "P-CBSTM03A-481",
    "P-CBSTM03A-482",
    "P-CBSTM03A-483",
    "P-CBSTM03A-484",
    "P-CBSTM03A-485",
    "P-CBSTM03A-486",
    "P-CBSTM03A-487",
    "P-CBSTM03A-488",
    "P-CBSTM03A-489",
    "P-CBSTM03A-490",
    "P-CBSTM03A-491",
    "P-CBSTM03A-492",
    "P-CBSTM03A-493",
    "P-CBSTM03A-494",
    "P-CBSTM03A-495",
    "P-CBSTM03A-496",
    "P-CBSTM03A-497",
    "P-CBSTM03A-498",
    "P-CBSTM03A-499",
    "P-CBSTM03A-500",
    "P-CBSTM03A-501",
    "P-CBSTM03A-502",
    "P-CBSTM03A-503",
    "P-CBSTM03A-504",
    "P-CBSTM03A-505",
    "P-CBSTM03A-506",
    "P-CBSTM03A-507",
    "P-CBSTM03A-508",
    "P-CBSTM03A-509",
    "P-CBSTM03A-510",
    "P-CBSTM03A-511",
    "P-CBSTM03A-512",
    "P-CBSTM03A-513",
    "P-CBSTM03A-514",
    "P-CBSTM03A-515",
    "P-CBSTM03A-516",
    "P-CBSTM03A-517",
    "P-CBSTM03A-518",
    "P-CBSTM03A-519",
    "P-CBSTM03A-520",
    "P-CBSTM03A-521",
    "P-CBSTM03A-522",
    "P-CBSTM03A-523",
    "P-CBSTM03A-524",
    "P-CBSTM03A-525",
    "P-CBSTM03A-526",
    "P-CBSTM03A-527",
    "P-CBSTM03A-528",
    "P-CBSTM03A-529",
    "P-CBSTM03A-530",
    "P-CBSTM03A-531",
    "P-CBSTM03A-532",
    "P-CBSTM03A-533",
    "P-CBSTM03A-534",
    "P-CBSTM03A-535",
    "P-CBSTM03A-536",
    "P-CBSTM03A-537",
    "P-CBSTM03A-538",
    "P-CBSTM03A-539",
    "P-CBSTM03A-540",
    "P-CBSTM03A-541",
    "P-CBSTM03A-542",
    "P-CBSTM03A-543",
    "P-CBSTM03A-544",
    "P-CBSTM03A-545",
    "P-CBSTM03A-546",
    "P-CBSTM03A-547",
    "P-CBSTM03A-548",
    "P-CBSTM03A-549",
    "P-CBSTM03A-550",
    "P-CBSTM03A-551",
    "P-CBSTM03A-552",
    "P-CBSTM03A-553",
    "P-CBSTM03A-554",
    "P-CBSTM03A-555",
    "P-CBSTM03A-556",
    "P-CBSTM03A-557",
    "P-CBSTM03A-558",
    "P-CBSTM03A-559",
    "P-CBSTM03A-560",
    "P-CBSTM03A-561",
    "P-CBSTM03A-562",
    "P-CBSTM03A-563",
    "P-CBSTM03A-564",
    "P-CBSTM03A-565",
    "P-CBSTM03A-566",
    "P-CBSTM03A-567",
    "P-CBSTM03A-568",
    "P-CBSTM03A-569",
    "P-CBSTM03A-570",
    "P-CBSTM03A-571",
    "P-CBSTM03A-572",
    "P-CBSTM03A-573",
    "P-CBSTM03A-574",
    "P-CBSTM03A-575",
    "P-CBSTM03A-576",
    "P-CBSTM03A-577",
    "P-CBSTM03A-578",
    "P-CBSTM03A-579",
    "P-CBSTM03A-580",
    "P-CBSTM03A-581",
    "P-CBSTM03A-582",
    "P-CBSTM03A-583",
    "P-CBSTM03A-584",
    "P-CBSTM03A-585",
    "P-CBSTM03A-586",
    "P-CBSTM03A-587",
    "P-CBSTM03A-588",
    "P-CBSTM03A-589",
    "P-CBSTM03A-590",
    "P-CBSTM03A-591",
    "P-CBSTM03A-592",
    "P-CBSTM03A-593",
    "P-CBSTM03A-594",
    "P-CBSTM03A-595",
    "P-CBSTM03A-596",
    "P-CBSTM03A-597",
    "P-CBSTM03A-598",
    "P-CBSTM03A-599",
    "P-CBSTM03A-600",
    "P-CBSTM03A-601",
    "P-CBSTM03A-602",
    "P-CBSTM03A-603",
    "P-CBSTM03A-604",
    "P-CBSTM03A-605",
    "P-CBSTM03A-606",
    "P-CBSTM03A-607",
    "P-CBSTM03A-608",
    "P-CBSTM03A-609",
    "P-CBSTM03A-610",
    "P-CBSTM03A-611",
    "P-CBSTM03A-612",
    "P-CBSTM03A-613",
    "P-CBSTM03A-614",
    "P-CBSTM03A-615",
    "P-CBSTM03A-616",
    "P-CBSTM03A-617",
    "P-CBSTM03A-618",
    "P-CBSTM03A-619",
    "P-CBSTM03A-620",
    "P-CBSTM03A-621",
    "P-CBSTM03A-622",
    "P-CBSTM03A-623",
    "P-CBSTM03A-624",
    "P-CBSTM03A-625",
    "P-CBSTM03A-626",
    "P-CBSTM03A-627",
    "P-CBSTM03A-628",
    "P-CBSTM03A-629",
    "P-CBSTM03A-630",
    "P-CBSTM03A-631",
    "P-CBSTM03A-632",
    "P-CBSTM03A-633",
    "P-CBSTM03A-634",
    "P-CBSTM03A-635",
    "P-CBSTM03A-636",
    "P-CBSTM03A-637",
    "P-CBSTM03A-638",
    "P-CBSTM03A-639",
    "P-CBSTM03A-640",
    "P-CBSTM03A-641",
    "P-CBSTM03A-642",
    "P-CBSTM03A-643",
    "P-CBSTM03A-644",
    "P-CBSTM03A-645",
    "P-CBSTM03A-646",
    "P-CBSTM03A-647",
    "P-CBSTM03A-648",
    "P-CBSTM03A-649",
    "P-CBSTM03A-650",
    "P-CBSTM03A-651",
    "P-CBSTM03A-652",
    "P-CBSTM03A-653",
    "P-CBSTM03A-654",
    "P-CBSTM03A-655",
    "P-CBSTM03A-656",
    "P-CBSTM03A-657",
    "P-CBSTM03A-658",
    "P-CBSTM03A-659",
    "P-CBSTM03A-660",
    "P-CBSTM03A-661",
    "P-CBSTM03A-662",
    "P-CBSTM03A-663",
    "P-CBSTM03A-664",
    "P-CBSTM03A-665",
    "P-CBSTM03A-666",
    "P-CBSTM03A-667",
    "P-CBSTM03A-668",
    "P-CBSTM03A-669",
    "P-CBSTM03A-670",
    "P-CBSTM03A-671",
    "P-CBSTM03A-672",
    "P-CBSTM03A-673",
    "P-CBSTM03A-674",
    "P-CBSTM03A-675",
    "P-CBSTM03A-676",
    "P-CBSTM03A-677",
    "P-CBSTM03A-678",
    "P-CBSTM03A-679",
    "P-CBSTM03A-680",
    "P-CBSTM03A-681",
    "P-CBSTM03A-682",
    "P-CBSTM03A-683",
    "P-CBSTM03A-684",
    "P-CBSTM03A-685",
    "P-CBSTM03A-686",
    "P-CBSTM03A-687",
    "P-CBSTM03A-688",
    "P-CBSTM03A-689",
    "P-CBSTM03A-690",
    "P-CBSTM03A-691",
    "P-CBSTM03A-692",
    "P-CBSTM03A-693",
    "P-CBSTM03A-694",
    "P-CBSTM03A-695",
    "P-CBSTM03A-696",
    "P-CBSTM03A-697",
    "P-CBSTM03A-698",
    "P-CBSTM03A-699",
    "P-CBSTM03A-700",
    "P-CBSTM03A-701",
    "P-CBSTM03A-702",
    "P-CBSTM03A-703",
    "P-CBSTM03A-704",
    "P-CBSTM03A-705",
    "P-CBSTM03A-706",
    "P-CBSTM03A-707",
    "P-CBSTM03A-708",
    "P-CBSTM03A-709",
    "P-CBSTM03A-710",
    "P-CBSTM03A-711",
    "P-CBSTM03A-712",
    "P-CBSTM03A-713",
    "P-CBSTM03A-714",
    "P-CBSTM03A-715",
    "P-CBSTM03A-716",
    "P-CBSTM03A-717",
    "P-CBSTM03A-718",
    "P-CBSTM03A-719",
    "P-CBSTM03A-720",
    "P-CBSTM03A-721",
    "P-CBSTM03A-722",
    "P-CBSTM03A-723",
    "P-CBSTM03A-724",
    "P-CBSTM03A-725",
    "P-CBSTM03A-726",
    "P-CBSTM03A-727",
    "P-CBSTM03A-728",
    "P-CBSTM03A-729",
    "P-CBSTM03A-730",
    "P-CBSTM03A-731",
    "P-CBSTM03A-732",
    "P-CBSTM03A-733",
    "P-CBSTM03A-734",
    "P-CBSTM03A-735",
    "P-CBSTM03A-736",
    "P-CBSTM03A-737",
    "P-CBSTM03A-738",
    "P-CBSTM03A-739",
    "P-CBSTM03A-740",
    "P-CBSTM03A-741",
    "P-CBSTM03A-742",
    "P-CBSTM03A-743",
    "P-CBSTM03A-744",
    "P-CBSTM03A-745",
    "P-CBSTM03A-746",
    "P-CBSTM03A-747",
    "P-CBSTM03A-748",
    "P-CBSTM03A-749",
    "P-CBSTM03A-750",
    "P-CBSTM03A-751",
    "P-CBSTM03A-752",
    "P-CBSTM03A-753",
    "P-CBSTM03A-754",
    "P-CBSTM03A-755",
    "P-CBSTM03A-756",
    "P-CBSTM03A-757",
    "P-CBSTM03A-758",
    "P-CBSTM03A-759",
    "P-CBSTM03A-760",
    "P-CBSTM03A-761",
    "P-CBSTM03A-762",
    "P-CBSTM03A-763",
    "P-CBSTM03A-764",
    "P-CBSTM03A-765",
    "P-CBSTM03A-766",
    "P-CBSTM03A-767",
    "P-CBSTM03A-768",
    "P-CBSTM03A-769",
    "P-CBSTM03A-770",
    "P-CBSTM03A-771",
    "P-CBSTM03A-772",
    "P-CBSTM03A-773",
    "P-CBSTM03A-774",
    "P-CBSTM03A-775",
    "P-CBSTM03A-776",
    "P-CBSTM03A-777",
    "P-CBSTM03A-778",
    "P-CBSTM03A-779",
    "P-CBSTM03A-780",
    "P-CBSTM03A-781",
    "P-CBSTM03A-782",
    "P-CBSTM03A-783",
    "P-CBSTM03A-784",
    "P-CBSTM03A-785",
    "P-CBSTM03A-786",
    "P-CBSTM03A-787",
    "P-CBSTM03A-788",
    "P-CBSTM03A-789",
    "P-CBSTM03A-790",
    "P-CBSTM03A-791",
    "P-CBSTM03A-792",
    "P-CBSTM03A-793",
    "P-CBSTM03A-794",
    "P-CBSTM03A-795",
    "P-CBSTM03A-796",
    "P-CBSTM03A-797",
    "P-CBSTM03A-798",
    "P-CBSTM03A-799",
    "P-CBSTM03A-800",
    "P-CBSTM03A-801",
    "P-CBSTM03A-802",
    "P-CBSTM03A-803",
    "P-CBSTM03A-804",
    "P-CBSTM03A-805",
    "P-CBSTM03A-806",
    "P-CBSTM03A-807",
    "P-CBSTM03A-808",
    "P-CBSTM03A-809",
    "P-CBSTM03A-810",
    "P-CBSTM03A-811",
    "P-CBSTM03A-812",
    "P-CBSTM03A-813",
    "P-CBSTM03A-814",
    "P-CBSTM03A-815",
    "P-CBSTM03A-816",
    "P-CBSTM03A-817",
    "P-CBSTM03A-818",
    "P-CBSTM03A-819",
    "P-CBSTM03A-820",
    "P-CBSTM03A-821",
    "P-CBSTM03A-822",
    "P-CBSTM03A-823",
    "P-CBSTM03A-824",
    "P-CBSTM03A-825",
    "P-CBSTM03A-826",
    "P-CBSTM03A-827",
    "P-CBSTM03A-828",
    "P-CBSTM03A-829",
    "P-CBSTM03A-830",
    "P-CBSTM03A-831",
    "P-CBSTM03A-832",
    "P-CBSTM03A-833",
    "P-CBSTM03A-834",
    "P-CBSTM03A-835",
    "P-CBSTM03A-836",
    "P-CBSTM03A-837",
    "P-CBSTM03A-838",
    "P-CBSTM03A-839",
    "P-CBSTM03A-840",
    "P-CBSTM03A-841",
    "P-CBSTM03A-842",
    "P-CBSTM03A-843",
    "P-CBSTM03A-844",
    "P-CBSTM03A-845",
    "P-CBSTM03A-846",
    "P-CBSTM03A-847",
    "P-CBSTM03A-848",
    "P-CBSTM03A-849",
    "P-CBSTM03A-850",
    "P-CBSTM03A-851",
    "P-CBSTM03A-852",
    "P-CBSTM03A-853",
    "P-CBSTM03A-854",
    "P-CBSTM03A-855",
    "P-CBSTM03A-856",
    "P-CBSTM03A-857",
    "P-CBSTM03A-858",
    "P-CBSTM03A-859",
    "P-CBSTM03A-860",
    "P-CBSTM03A-861",
    "P-CBSTM03A-862",
    "P-CBSTM03A-863",
    "P-CBSTM03A-864",
    "P-CBSTM03A-865",
    "P-CBSTM03A-866",
    "P-CBSTM03A-867",
    "P-CBSTM03A-868",
    "P-CBSTM03A-869",
    "P-CBSTM03A-870",
    "P-CBSTM03A-871",
    "P-CBSTM03A-872",
    "P-CBSTM03A-873",
    "P-CBSTM03A-874",
    "P-CBSTM03A-875",
    "P-CBSTM03A-876",
    "P-CBSTM03A-877",
    "P-CBSTM03A-878",
    "P-CBSTM03A-879",
    "P-CBSTM03A-880",
    "P-CBSTM03A-881",
    "P-CBSTM03A-882",
    "P-CBSTM03A-883",
    "P-CBSTM03A-884",
    "P-CBSTM03A-885",
    "P-CBSTM03A-886",
    "P-CBSTM03A-887",
    "P-CBSTM03A-888",
    "P-CBSTM03A-889",
    "P-CBSTM03A-890",
    "P-CBSTM03A-891",
    "P-CBSTM03A-892",
    "P-CBSTM03A-893",
    "P-CBSTM03A-894",
    "P-CBSTM03A-895",
    "P-CBSTM03A-896",
    "P-CBSTM03A-897",
    "P-CBSTM03A-898",
    "P-CBSTM03A-899",
    "P-CBSTM03A-900",
    "P-CBSTM03A-901",
    "P-CBSTM03A-902",
    "P-CBSTM03A-903",
    "P-CBSTM03A-904",
    "P-CBSTM03A-905",
    "P-CBSTM03A-906",
    "P-CBSTM03A-907",
    "P-CBSTM03A-908",
    "P-CBSTM03A-909",
    "P-CBSTM03A-910",
    "P-CBSTM03A-911",
    "P-CBSTM03A-912",
    "P-CBSTM03A-913",
    "P-CBSTM03A-914",
    "P-CBSTM03A-915",
    "P-CBSTM03A-916",
    "P-CBSTM03A-917",
    "P-CBSTM03A-918",
    "P-CBSTM03A-919",
    "P-CBSTM03A-920",
    "P-CBSTM03A-921",
    "P-CBSTM03A-922",
    "P-CBSTM03A-923",
    "P-CBSTM03A-924",
    "P-CBSTM03A-925",
    "P-CBSTM03A-926",
    "P-CBSTM03A-927",
    "P-CBSTM03A-928",
    "P-CBSTM03A-929",
    "P-CBSTM03A-930",
    "P-CBSTM03A-931",
    "P-CBSTM03A-932",
    "P-CBSTM03A-933",
    "P-CBSTM03A-934",
    "P-CBSTM03A-935",
    "P-CBSTM03A-936",
    "P-CBSTM03A-937",
    "P-CBSTM03A-938",
    "P-CBSTM03A-939",
    "P-CBSTM03A-940",
    "P-CBSTM03A-941",
    "P-CBSTM03A-942",
    "P-CBSTM03A-943",
    "P-CBSTM03A-944",
    "P-CBSTM03A-945",
    "P-CBSTM03A-946",
    "P-CBSTM03A-947",
    "P-CBSTM03A-948",
    "P-CBSTM03A-949",
    "P-CBSTM03A-950",
    "P-CBSTM03A-951",
    "P-CBSTM03A-952",
    "P-CBSTM03A-953",
    "P-CBSTM03A-954",
    "P-CBSTM03A-955",
    "P-CBSTM03A-956",
    "P-CBSTM03A-957",
    "P-CBSTM03A-958",
    "P-CBSTM03A-959",
    "P-CBSTM03A-960",
    "P-CBSTM03A-961",
    "P-CBSTM03A-962",
    "P-CBSTM03A-963",
    "P-CBSTM03A-964",
    "P-CBSTM03A-965",
    "P-CBSTM03A-966",
    "P-CBSTM03A-967",
    "P-CBSTM03A-968",
    "P-CBSTM03A-969",
    "P-CBSTM03A-970",
    "P-CBSTM03A-971",
    "P-CBSTM03A-972",
    "P-CBSTM03A-973",
    "P-CBSTM03A-974",
    "P-CBSTM03A-975",
    "P-CBSTM03A-976",
    "P-CBSTM03A-977",
    "P-CBSTM03A-978",
    "P-CBSTM03A-979",
    "P-CBSTM03A-980",
    "P-CBSTM03A-981",
    "P-CBSTM03A-982",
    "P-CBSTM03A-983",
    "P-CBSTM03A-984",
    "P-CBSTM03A-985",
    "P-CBSTM03A-986",
    "P-CBSTM03A-987",
    "P-CBSTM03A-988",
    "P-CBSTM03A-989",
    "P-CBSTM03A-990",
    "P-CBSTM03A-991",
    "P-CBSTM03A-992",
    "P-CBSTM03A-993",
    "P-CBSTM03A-994",
    "P-CBSTM03A-995",
    "P-CBSTM03A-996",
    "P-CBSTM03A-997",
    "P-CBSTM03A-998",
    "P-CBSTM03A-999",
    "P-CBSTM03A-1000",
    "P-CBSTM03A-1001",
    "P-CBSTM03A-1002",
    "P-CBSTM03A-1003",
    "P-CBSTM03A-1004",
    "P-CBSTM03A-1005",
    "P-CBSTM03A-1006",
    "P-CBSTM03A-1007",
    "P-CBSTM03A-1008",
    "P-CBSTM03A-1009",
    "P-CBSTM03A-1010",
    "P-CBSTM03A-1011",
    "P-CBSTM03A-1012",
    "P-CBSTM03A-1013",
    "P-CBSTM03A-1014",
    "P-CBSTM03A-1015",
    "P-CBSTM03A-1016",
    "P-CBSTM03A-1017",
    "P-CBSTM03A-1018",
    "P-CBSTM03A-1019",
    "P-CBSTM03A-1020",
    "P-CBSTM03A-1021",
    "P-CBSTM03A-1022",
    "P-CBSTM03A-1023",
    "P-CBSTM03A-1024",
    "P-CBSTM03A-1025",
    "P-CBSTM03A-1026",
    "P-CBSTM03A-1027",
    "P-CBSTM03A-1028",
    "P-CBSTM03A-1029",
    "P-CBSTM03A-1030",
    "P-CBSTM03A-1031",
    "P-CBSTM03A-1032",
    "P-CBSTM03A-1033",
    "P-CBSTM03A-1034",
    "P-CBSTM03A-1035",
    "P-CBSTM03A-1036",
    "P-CBSTM03A-1037",
    "P-CBSTM03A-1038",
    "P-CBSTM03A-1039",
    "P-CBSTM03A-1040",
    "P-CBSTM03A-1041",
    "P-CBSTM03A-1042",
    "P-CBSTM03A-1043",
    "P-CBSTM03A-1044",
    "P-CBSTM03A-1045",
    "P-CBSTM03A-1046",
    "P-CBSTM03A-1047",
    "P-CBSTM03A-1048",
    "P-CBSTM03A-1049",
    "P-CBSTM03A-1050",
    "P-CBSTM03A-1051",
    "P-CBSTM03A-1052",
    "P-CBSTM03A-1053",
    "P-CBSTM03A-1054",
    "P-CBSTM03A-1055",
    "P-CBSTM03A-1056",
    "P-CBSTM03A-1057",
    "P-CBSTM03A-1058",
    "P-CBSTM03A-1059",
    "P-CBSTM03A-1060",
    "P-CBSTM03A-1061",
    "P-CBSTM03A-1062",
    "P-CBSTM03A-1063",
    "P-CBSTM03A-1064",
    "P-CBSTM03A-1065",
    "P-CBSTM03A-1066",
    "P-CBSTM03A-1067",
    "P-CBSTM03A-1068",
    "P-CBSTM03A-1069",
    "P-CBSTM03A-1070",
    "P-CBSTM03A-1071",
    "P-CBSTM03A-1072",
    "P-CBSTM03A-1073",
    "P-CBSTM03A-1074",
    "P-CBSTM03A-1075",
    "P-CBSTM03A-1076",
    "P-CBSTM03A-1077",
    "P-CBSTM03A-1078",
    "P-CBSTM03A-1079",
    "P-CBSTM03A-1080",
    "P-CBSTM03A-1081",
    "P-CBSTM03A-1082",
    "P-CBSTM03A-1083",
    "P-CBSTM03A-1084",
    "P-CBSTM03A-1085",
    "P-CBSTM03A-1086",
    "P-CBSTM03A-1087",
    "P-CBSTM03A-1088",
    "P-CBSTM03A-1089",
    "P-CBSTM03A-1090",
    "P-CBSTM03A-1091",
    "P-CBSTM03A-1092",
    "P-CBSTM03A-1093",
    "P-CBSTM03A-1094",
    "P-CBSTM03A-1095",
    "P-CBSTM03A-1096",
    "P-CBSTM03A-1097",
    "P-CBSTM03A-1098",
    "P-CBSTM03A-1099",
    "P-CBSTM03A-1100",
    "P-CBSTM03A-1101",
    "P-CBSTM03A-1102",
    "P-CBSTM03A-1103",
    "P-CBSTM03A-1104",
    "P-CBSTM03A-1105",
    "P-CBSTM03A-1106",
    "P-CBSTM03A-1107",
    "P-CBSTM03A-1108",
    "P-CBSTM03A-1109",
    "P-CBSTM03A-1110",
    "P-CBSTM03A-1111",
    "P-CBSTM03A-1112",
    "P-CBSTM03A-1113",
    "P-CBSTM03A-1114",
    "P-CBSTM03A-1115",
    "P-CBSTM03A-1116",
    "P-CBSTM03A-1117",
    "P-CBSTM03A-1118",
    "P-CBSTM03A-1119",
    "P-CBSTM03A-1120",
    "P-CBSTM03A-1121",
    "P-CBSTM03A-1122",
    "P-CBSTM03A-1123",
    "P-CBSTM03A-1124",
    "P-CBSTM03A-1125",
    "P-CBSTM03A-1126",
    "P-CBSTM03A-1127",
    "P-CBSTM03A-1128",
    "P-CBSTM03A-1129",
    "P-CBSTM03A-1130",
    "P-CBSTM03A-1131",
    "P-CBSTM03A-1132",
    "P-CBSTM03A-1133",
    "P-CBSTM03A-1134",
    "P-CBSTM03A-1135",
    "P-CBSTM03A-1136",
    "P-CBSTM03A-1137",
    "P-CBSTM03A-1138",
    "P-CBSTM03A-1139",
    "P-CBSTM03A-1140",
    "P-CBSTM03A-1141",
    "P-CBSTM03A-1142",
    "P-CBSTM03A-1143",
    "P-CBSTM03A-1144",
    "P-CBSTM03A-1145",
    "P-CBSTM03A-1146",
    "P-CBSTM03A-1147",
    "P-CBSTM03A-1148",
    "P-CBSTM03A-1149",
    "P-CBSTM03A-1150",
    "P-CBSTM03A-1151",
    "P-CBSTM03A-1152",
    "P-CBSTM03A-1153",
    "P-CBSTM03A-1154",
    "P-CBSTM03A-1155",
    "P-CBSTM03A-1156",
    "P-CBSTM03A-1157",
    "P-CBSTM03A-1158",
    "P-CBSTM03A-1159",
    "P-CBSTM03A-1160",
    "P-CBSTM03A-1161",
    "P-CBSTM03A-1162",
    "P-CBSTM03A-1163",
    "P-CBSTM03A-1164",
    "P-CBSTM03A-1165",
    "P-CBSTM03A-1166",
    "P-CBSTM03A-1167",
    "P-CBSTM03A-1168",
    "P-CBSTM03A-1169",
    "P-CBSTM03A-1170",
    "P-CBSTM03A-1171",
    "P-CBSTM03A-1172",
    "P-CBSTM03A-1173",
    "P-CBSTM03A-1174",
    "P-CBSTM03A-1175",
    "P-CBSTM03A-1176",
    "P-CBSTM03A-1177",
    "P-CBSTM03A-1178",
    "P-CBSTM03A-1179",
    "P-CBSTM03A-1180",
    "P-CBSTM03A-1181",
    "P-CBSTM03A-1182",
    "P-CBSTM03A-1183",
    "P-CBSTM03A-1184",
    "P-CBSTM03A-1185",
    "P-CBSTM03A-1186",
    "P-CBSTM03A-1187",
    "P-CBSTM03A-1188",
    "P-CBSTM03A-1189",
    "P-CBSTM03A-1190",
    "P-CBSTM03A-1191",
    "P-CBSTM03A-1192",
    "P-CBSTM03A-1193",
    "P-CBSTM03A-1194",
    "P-CBSTM03A-1195",
    "P-CBSTM03A-1196",
    "P-CBSTM03A-1197",
    "P-CBSTM03A-1198",
    "P-CBSTM03A-1199",
    "P-CBSTM03A-1200",
    "P-CBSTM03A-1201",
    "P-CBSTM03A-1202",
    "P-CBSTM03A-1203",
    "P-CBSTM03A-1204",
    "P-CBSTM03A-1205",
    "P-CBSTM03A-1206",
    "P-CBSTM03A-1207",
    "P-CBSTM03A-1208",
    "P-CBSTM03A-1209",
    "P-CBSTM03A-1210",
    "P-CBSTM03A-1211",
    "P-CBSTM03A-1212",
    "P-CBSTM03A-1213",
    "P-CBSTM03A-1214",
    "P-CBSTM03A-1215",
    "P-CBSTM03A-1216",
    "P-CBSTM03A-1217",
    "P-CBSTM03A-1218",
    "P-CBSTM03A-1219",
    "P-CBSTM03A-1220",
    "P-CBSTM03A-1221",
    "P-CBSTM03A-1222",
    "P-CBSTM03A-1223",
    "P-CBSTM03A-1224",
    "P-CBSTM03A-1225",
    "P-CBSTM03A-1226",
    "P-CBSTM03A-1227",
    "P-CBSTM03A-1228",
    "P-CBSTM03A-1229",
    "P-CBSTM03A-1230",
    "P-CBSTM03A-1231",
    "P-CBSTM03A-1232",
    "P-CBSTM03A-1233",
    "P-CBSTM03A-1234",
    "P-CBSTM03A-1235",
    "P-CBSTM03A-1236",
    "P-CBSTM03A-1237",
    "P-CBSTM03A-1238",
    "P-CBSTM03A-1239",
    "P-CBSTM03A-1240",
    "P-CBSTM03A-1241",
    "P-CBSTM03A-1242",
    "P-CBSTM03A-1243",
    "P-CBSTM03A-1244",
    "P-CBSTM03A-1245",
    "P-CBSTM03A-1246",
    "P-CBSTM03A-1247",
    "P-CBSTM03A-1248",
    "P-CBSTM03A-1249",
    "P-CBSTM03A-1250",
    "P-CBSTM03A-1251",
    "P-CBSTM03A-1252",
    "P-CBSTM03A-1253",
    "P-CBSTM03A-1254",
    "P-CBSTM03A-1255",
    // Section E: the capacity halt.
    "P-CBSTM03A-1256",
];
