//! CREASTMT end to end: `carddemo-batch creastmt` run as a subprocess against a testcontainers
//! `PostgreSQL`, with its three statement files, standard output, directory and exit status
//! compared with `modern/audit/batch/CREASTMT.md` and the FIXSTMT rows of
//! `modern/audit/fixtures.md` (the Statements criterion and part of g3b, AAP 0.9.2).
//!
//! - Expected bytes come only from those rows, read at run time, and from the fixture bytes the
//!   rows cite: a `{T:n}` in a quote stands for bytes 1-16 of `dailytran.txt` line n, the
//!   `DALYTRAN-ID` that `posttran` stores as `TRAN-ID`. Nothing expected is computed by
//!   `domain::batch::cbstm03a` or `batch::pdf`, the code under test.
//! - Scenario `POSTED`: the fixture-loaded database, then `posttran` over `dailytran.txt`. No
//!   statement byte depends on a timestamp, so the scenario's `FixedClock` instant, which only
//!   TRANREPT's selection needs, does not apply to this job.
//! - Each test has its own container and its own scratch directory, which receives every file the
//!   jobs write (Local-Only Runtime).
//! - A case a halt candidate blocks is asserted as the replacement's behaviour and reported
//!   `BLOCKED`, never as parity (AAP 0.8.5).
//! - Cardholder Data Redaction: the statement files hold customer names, addresses and balances,
//!   and the tables they come from hold card numbers. A failure names the file, the 1-based record
//!   and the first differing byte offset only, and no record or row is printed.

use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use copybook::{PicU, PicX};
use domain::CardNumber;
use domain::records::{CardXrefRecord, TransactionRecord};
use persistence::{CardXrefRepository, PgStore, TransactionRepository};
use printpdf::lopdf::Document;
use test_support::audit_index::audit_dir;
use test_support::fixtures::{
    fixture_card_numbers, fixture_dir, fixture_path, start_loaded_postgres,
};
use test_support::postgres::{PostgresGuard, start_postgres};

type Outcome<T> = Result<T, Box<dyn Error>>;
type TestResult = Outcome<()>;

/// The text statement, `STMTFILE` (CBSTM03A-002).
const TEXT_FILE: &str = "STATEMNT.PS";

/// The HTML statement, `HTMLFILE` (CBSTM03A-002).
const HTML_FILE: &str = "STATEMNT.HTML";

/// The statement PDF (CBSTM03A-051).
const PDF_FILE: &str = "STATEMNT.PDF";

/// `FD-STMTFILE-REC PIC X(80)` (CBSTM03A-031).
const TEXT_RECORD_LEN: usize = 80;

/// `FD-HTMLFILE-REC PIC X(100)`: the program's FD, not the `LRECL=80` of `STEP030`
/// (CBSTM03A-047, E20).
const HTML_RECORD_LEN: usize = 100;

/// `DALYTRAN-ID PIC X(16)`, bytes 1-16 of a `dailytran.txt` line: what `{T:n}` stands for.
const DALYTRAN_ID_LEN: usize = 16;

/// The `posttran` exit status of `POSTED`, whose reject set is not empty (FIXPOST-903).
const POSTTRAN_STATUS: i32 = 4;

/// The `transactions` rows `POSTED` leaves (FIXPOST-901).
const POSTED_TRANSACTIONS: usize = 262;

/// Statements of `POSTED`, one per `cardxref.txt` line (FIXSTMT-901).
const STATEMENTS: usize = 50;

/// `STATEMNT.PS` records of `POSTED` (FIXSTMT-901).
const TEXT_RECORDS: usize = 1_212;

/// `STATEMNT.HTML` records of `POSTED` (FIXSTMT-901).
const HTML_RECORDS: usize = 6_082;

/// The replacement's exit status after `ABENDING PROGRAM`. `CALL 'CEE3ABD'` passes no abend
/// code, so only a non-zero status is parity (CBSTM03A-049, X-CREASTMT-2).
const ABEND_STATUS: i32 = 16;

/// The `DISPLAY` of 9999-ABEND-PROGRAM (CBSTM03A-049).
const ABENDING_PROGRAM: &str = "ABENDING PROGRAM";

/// What a planted previous run's file holds, so that a file left behind or appended to shows.
const PREVIOUS_RUN: &[u8] = b"previous run\n";

/// Two card numbers in no fixture file that sort after every fixture card, so that every
/// fixture statement is written before the statement of either one is reached.
const LAST_CARD: [u8; 16] = *b"9999999999999999";
const NEXT_TO_LAST_CARD: [u8; 16] = *b"9999999999999998";

/// A customer ID and an account ID that no fixture record holds.
const MISSING_CUSTOMER: u64 = 999_999_999;
const MISSING_ACCOUNT: u64 = 99_999_999_999;

/// A customer and an account that the fixtures hold (`custdata.txt`, `acctdata.txt`).
const FIXTURE_CUSTOMER: u64 = 1;
const FIXTURE_ACCOUNT: u64 = 1;

// The literals of the HTML records, CBSTM03A-033 to CBSTM03A-038 (FIXSTMT-903).
const L01: &str = "<!DOCTYPE html>";
const L02: &str = r#"<html lang="en">"#;
const L03: &str = "<head>";
const L04: &str = r#"<meta charset="utf-8">"#;
const L05: &str = "<title>HTML Table Layout</title>";
const L06: &str = "</head>";
const L07: &str = r#"<body style="margin:0px;">"#;
const L08: &str =
    r#"<table  align="center" frame="box" style="width:70%; font:12px Segoe UI,sans-serif;">"#;
const LTRS: &str = "<tr>";
const LTRE: &str = "</tr>";
const LTDE: &str = "</td>";
const L10: &str = r#"<td colspan="3" style="padding:0px 5px;background-color:#1d1d96b3;">"#;
const L15: &str = r#"<td colspan="3" style="padding:0px 5px;background-color:#FFAF33;">"#;
const L16: &str = r#"<p style="font-size:16px">Bank of XYZ</p>"#;
const L17: &str = "<p>410 Terry Ave N</p>";
const L18: &str = "<p>Seattle WA 99999</p>";
const L22_35: &str = r#"<td colspan="3" style="padding:0px 5px;background-color:#f2f2f2;">"#;
const L30_42: &str =
    r#"<td colspan="3" style="padding:0px 5px;background-color:#33FFD1; text-align:center;">"#;
const L31: &str = r#"<p style="font-size:16px">Basic Details</p>"#;
const L43: &str = r#"<p style="font-size:16px">Transaction Summary</p>"#;
const L47: &str =
    r#"<td style="width:25%; padding:0px 5px; background-color:#33FF5E; text-align:left;">"#;
const L48: &str = r#"<p style="font-size:16px">Tran ID</p>"#;
const L50: &str =
    r#"<td style="width:55%; padding:0px 5px; background-color:#33FF5E; text-align:left;">"#;
const L51: &str = r#"<p style="font-size:16px">Tran Details</p>"#;
const L53: &str =
    r#"<td style="width:20%; padding:0px 5px; background-color:#33FF5E; text-align:right;">"#;
const L54: &str = r#"<p style="font-size:16px">Amount</p>"#;
const L58: &str =
    r#"<td style="width:25%; padding:0px 5px; background-color:#f2f2f2; text-align:left;">"#;
const L61: &str =
    r#"<td style="width:55%; padding:0px 5px; background-color:#f2f2f2; text-align:left;">"#;
const L64: &str =
    r#"<td style="width:20%; padding:0px 5px; background-color:#f2f2f2; text-align:right;">"#;
const L75: &str = "<h3>End of Statement</h3>";
const L78: &str = "</table>";
const L79: &str = "</body>";
const L80: &str = "</html>";

/// 5100-WRITE-HTML-HEADER before `HTML-L11` (CBSTM03A-043).
const HEADER_OPEN: [&str; 10] = [L01, L02, L03, L04, L05, L06, L07, L08, LTRS, L10];

/// 5100-WRITE-HTML-HEADER after `HTML-L11` (CBSTM03A-043).
const HEADER_CLOSE: [&str; 11] = [
    LTDE, LTRE, LTRS, L15, L16, L17, L18, LTDE, LTRE, LTRS, L22_35,
];

/// 5200-WRITE-HTML-NMADBS between the address records and the basic-detail records
/// (CBSTM03A-044).
const BASIC_OPEN: [&str; 9] = [LTDE, LTRE, LTRS, L30_42, L31, LTDE, LTRE, LTRS, L22_35];

/// 5200-WRITE-HTML-NMADBS after the basic-detail records (CBSTM03A-044).
const SUMMARY_OPEN: [&str; 18] = [
    LTDE, LTRE, LTRS, L30_42, L43, LTDE, LTRE, LTRS, L47, L48, LTDE, L50, L51, LTDE, L53, L54,
    LTDE, LTRE,
];

/// The last 8 HTML records of every statement (CBSTM03A-046).
const TRAILER: [&str; 8] = [LTRS, L10, L75, LTDE, LTRE, L78, L79, L80];

/// `ST-ACCT-ID X(20)` in the padded `ST-LINE7`, after its `FILLER X(20)` (CBSTM03A-021).
const ST_ACCT_ID: Range<usize> = 20..40;

/// `ST-CURR-BAL PIC 9(9).99-` in the padded `ST-LINE8`, after its `FILLER X(20)` (CBSTM03A-022).
const ST_CURR_BAL: Range<usize> = 20..33;

/// `ST-FICO-SCORE X(20)` in the padded `ST-LINE9`, after its `FILLER X(20)` (CBSTM03A-023).
const ST_FICO_SCORE: Range<usize> = 20..40;

/// `ST-TRANID X(16)` in `ST-LINE14` (CBSTM03A-026).
const ST_TRANID: Range<usize> = 0..16;

/// `ST-TRANDT X(49)` in `ST-LINE14`, after `ST-TRANID` and a space (CBSTM03A-026).
const ST_TRANDT: Range<usize> = 17..66;

/// `ST-TRANAMT PIC Z(9).99-` in `ST-LINE14`, after the `$` (CBSTM03A-026).
const ST_TRANAMT: Range<usize> = 67..80;

/// Tells apart the scratch directories of the tests of one process.
static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A directory of one test's own under the temporary directory, which receives every file the
/// jobs write, removed with its content when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Outcome<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "carddemo-creastmt-test-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    /// `CARDDEMO_VAR_DIR`: the scratch directory itself.
    fn path(&self) -> &Path {
        &self.0
    }

    /// `CARDDEMO_STATEMENT_DIR`, which the job creates when it is missing.
    fn statements(&self) -> PathBuf {
        self.0.join("statements")
    }

    /// `CARDDEMO_INTRDR_DIR`, which no batch job writes.
    fn jobs(&self) -> PathBuf {
        self.0.join("jobs")
    }

    /// The three statement paths, in the order a normal end prints them (CBSTM03A-050).
    fn statement_paths(&self) -> [PathBuf; 3] {
        let dir = self.statements();
        [TEXT_FILE, HTML_FILE, PDF_FILE].map(|name| dir.join(name))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a directory left behind holds only this test's own output.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `carddemo-batch` with `args` from `modern/`, as every binary runs, against
/// `database_url`, with every directory it writes inside `scratch`, span export off and only
/// error events on standard output. The rest of the environment is inherited, coverage
/// instrumentation included.
async fn run_batch(scratch: &Scratch, database_url: &str, args: &[&str]) -> Outcome<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", database_url)
        .env("CARDDEMO_VAR_DIR", scratch.path())
        .env("CARDDEMO_STATEMENT_DIR", scratch.statements())
        .env("CARDDEMO_INTRDR_DIR", scratch.jobs())
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", "error")
        .env_remove("CARDDEMO_OTEL_EXPORT");
    Ok(tokio::task::spawn_blocking(move || command.output()).await??)
}

/// The plain lines of standard output, the paths the job wrote; every JSON tracing event is a
/// line that begins with `{`.
fn written_paths(run: &Output) -> Outcome<Vec<PathBuf>> {
    Ok(std::str::from_utf8(&run.stdout)?
        .lines()
        .filter(|line| !line.starts_with('{'))
        .map(PathBuf::from)
        .collect())
}

/// The `message` of every JSON tracing event on standard output, in order. The `DISPLAY` lines
/// of `CBSTM03A` are such events, each message the line's text exactly.
fn event_messages(run: &Output) -> Outcome<Vec<String>> {
    Ok(std::str::from_utf8(&run.stdout)?
        .lines()
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| {
            let (_, value) = line.split_once(r#""message":""#)?;
            value.split_once('"').map(|(message, _)| message.to_owned())
        })
        .collect())
}

/// The names of the entries of `dir`, sorted.
fn names(dir: &Path) -> Outcome<Vec<String>> {
    let mut names = fs::read_dir(dir)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    names.sort();
    Ok(names)
}

/// The statement directory holding the three files of a previous run.
fn plant_previous_run(scratch: &Scratch) -> TestResult {
    let dir = scratch.statements();
    fs::create_dir_all(&dir)?;
    for name in [TEXT_FILE, HTML_FILE, PDF_FILE] {
        fs::write(dir.join(name), PREVIOUS_RUN)?;
    }
    Ok(())
}

/// Reports a case a halt candidate blocks; its test asserts the replacement's behaviour only
/// (AAP 0.8.5).
fn blocked(rule: &str, halt: &str) {
    println!("BLOCKED {rule} ({halt})");
}

/// The records of a file of `width`-byte records, each followed by LF.
///
/// # Errors
///
/// When the file is not whole records or a record is not followed by LF, naming the file and
/// the 1-based record only.
fn fixed_records<'a>(file: &str, bytes: &'a [u8], width: usize) -> Outcome<Vec<&'a [u8]>> {
    let line = width + 1;
    if bytes.len() % line != 0 {
        return Err(format!(
            "{file}: {} bytes are not whole {width}-byte records each followed by LF",
            bytes.len()
        )
        .into());
    }
    bytes
        .chunks_exact(line)
        .zip(1_usize..)
        .map(|(chunk, number)| match chunk.split_last() {
            Some((b'\n', record)) => Ok(record),
            _ => Err(format!("{file}: record {number} is not followed by LF").into()),
        })
        .collect()
}

/// The offset of the first byte in which `actual` and `expected` differ, or the shorter length
/// when one is the other's prefix.
fn first_difference(actual: &[u8], expected: &[u8]) -> usize {
    actual
        .iter()
        .zip(expected)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| actual.len().min(expected.len()))
}

/// Asserts that `actual` holds the `expected` records in order, reporting a difference by file,
/// 1-based record and byte offset only.
fn assert_records(file: &str, actual: &[&[u8]], expected: &[Vec<u8>]) {
    assert_eq!(actual.len(), expected.len(), "{file}: record count");
    for (number, (record, wanted)) in (1_usize..).zip(actual.iter().zip(expected)) {
        assert!(
            *record == wanted.as_slice(),
            "{file}: record {number} differs from its audited record at byte offset {}",
            first_difference(record, wanted)
        );
    }
}

/// The lines of the text `lopdf` extracts from every page of the PDF `bytes`, in page order:
/// each page's text appended to the previous page's, split at every LF, and only the one empty
/// piece after the last LF dropped. No whitespace is trimmed, collapsed or otherwise
/// normalised (CBSTM03A-052).
fn pdf_lines(bytes: &[u8]) -> Outcome<Vec<Vec<u8>>> {
    let doc = Document::load_mem(bytes)?;
    let mut text = String::new();
    for page in doc.get_pages().into_keys() {
        text.push_str(&doc.extract_text(&[page])?);
    }
    let mut lines: Vec<Vec<u8>> = text
        .split('\n')
        .map(|line| line.as_bytes().to_vec())
        .collect();
    if lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    Ok(lines)
}

/// `bytes` without its ASCII whitespace, to tell a whitespace-only difference apart.
fn without_whitespace(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect()
}

/// Asserts that the lines extracted from `STATEMNT.PDF` are the `STATEMNT.PS` records byte for
/// byte, trailing spaces included, in the same number and order. A whitespace-only difference
/// fails with the instruction to escalate it, never to normalise it (CBSTM03A-052, R7).
fn assert_pdf_lines(lines: &[Vec<u8>], records: &[&[u8]]) {
    assert_eq!(
        lines.len(),
        records.len(),
        "{PDF_FILE}: extracted lines against {TEXT_FILE} records"
    );
    for (number, (line, record)) in (1_usize..).zip(lines.iter().zip(records)) {
        if line.as_slice() == *record {
            continue;
        }
        assert!(
            without_whitespace(line) != without_whitespace(record),
            "{PDF_FILE}: line {number} differs from {TEXT_FILE} record {number} in whitespace \
             only; escalate the difference in modern/audit/ESCALATIONS.md, never normalise it \
             (CBSTM03A-052, AAP 0.3.4.5)"
        );
        assert!(
            line.as_slice() == *record,
            "{PDF_FILE}: line {number} differs from {TEXT_FILE} record {number} at byte offset {}",
            first_difference(line, record)
        );
    }
}

/// The `DALYTRAN-ID` of every `dailytran.txt` line: the 16 bytes a `{T:n}` of `fixtures.md`
/// stands for, the `TRAN-ID` the line is posted under.
struct TranIds(Vec<Vec<u8>>);

impl TranIds {
    fn read() -> Outcome<Self> {
        let bytes = fs::read(fixture_path("dailytran.txt"))?;
        let body = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        let ids = body
            .split(|&byte| byte == b'\n')
            .map(|line| {
                line.get(..DALYTRAN_ID_LEN)
                    .map(<[u8]>::to_vec)
                    .ok_or("a dailytran.txt line shorter than its DALYTRAN-ID")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(ids))
    }

    /// `quote` with each `{T:n}` replaced by the `DALYTRAN-ID` of `dailytran.txt` line n.
    fn expand(&self, quote: &str) -> Outcome<Vec<u8>> {
        let mut out = Vec::with_capacity(quote.len());
        let mut rest = quote;
        while let Some((head, tail)) = rest.split_once("{T:") {
            out.extend_from_slice(head.as_bytes());
            let (line, after) = tail.split_once('}').ok_or("a closed {T:n}")?;
            let line: usize = line.parse()?;
            let id = line
                .checked_sub(1)
                .and_then(|index| self.0.get(index))
                .ok_or_else(|| format!("{{T:{line}}} names no dailytran.txt line"))?;
            out.extend_from_slice(id);
            rest = after;
        }
        out.extend_from_slice(rest.as_bytes());
        Ok(out)
    }
}

/// `records A-B (C)` of a FIXSTMT row: the 1-based first and last record of one statement in a
/// file, and their count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    first: usize,
    last: usize,
    count: usize,
}

impl Span {
    /// The span of `count` records that follow `before` records.
    fn after(before: usize, count: usize) -> Self {
        Self {
            first: before + 1,
            last: before + count,
            count,
        }
    }

    /// Moves `rest` past `label` and the `A-B (C): ` after it, and returns that span.
    fn parse(rest: &mut &str, label: &str) -> Outcome<Self> {
        let (range, count) = between(rest, label, "): ")?
            .split_once(" (")
            .ok_or("records A-B (C)")?;
        let (first, last) = range.split_once('-').ok_or("records A-B")?;
        Ok(Self {
            first: first.parse()?,
            last: last.parse()?,
            count: count.parse()?,
        })
    }
}

/// Moves `rest` past the first `open` and the text up to the next `close`, and returns that
/// text. A label of a quote ends in its opening backtick, so `ST-LINE1 ` followed by a
/// backtick never matches `ST-LINE14`.
fn between<'a>(rest: &mut &'a str, open: &str, close: &str) -> Outcome<&'a str> {
    let (_, tail) = rest
        .split_once(open)
        .ok_or_else(|| format!("`{open}` in the row"))?;
    let (text, after) = tail
        .split_once(close)
        .ok_or_else(|| format!("`{close}` after `{open}`"))?;
    *rest = after;
    Ok(text)
}

/// Moves `rest` past `label`, which ends in the opening backtick, and its quote, and returns the
/// quote.
fn quote(rest: &mut &str, label: &str) -> Outcome<String> {
    Ok(between(rest, label, "`")?.to_owned())
}

/// What every statement row's Expected result holds after its last quote.
const ROW_END: &str = "; basic-detail and transaction records from the values above by FIXSTMT-903";

/// One of the rows FIXSTMT-001 to FIXSTMT-050: the quotes of its Expected result, with
/// trailing spaces omitted and `{T:n}` unexpanded, and the positions it gives the statement.
struct StatementRow {
    /// The nnn of the rule ID: the statement's position and its `cardxref.txt` line.
    number: usize,
    /// `STATEMNT.PS records A-B (C)`.
    text: Span,
    /// `ST-LINE1` to `ST-LINE4`: the name and the three address lines.
    heading: [String; 4],
    /// `ST-LINE7` to `ST-LINE9`: account ID, current balance and FICO score.
    details: [String; 3],
    /// The k of `ST-LINE14 × k`.
    listed: usize,
    /// One `ST-LINE14` per transaction, in `tran_id` order.
    tran_lines: Vec<String>,
    /// `ST-LINE14A`, the card's total.
    total_line: String,
    /// `STATEMNT.HTML records D-E (F)`.
    html: Span,
    /// `HTML-L11`, the account-number heading.
    html_title: String,
    /// The name record and the three address records.
    html_heading: [String; 4],
}

impl StatementRow {
    /// Parses row `number` from its Input condition and Expected result cells.
    fn parse(number: usize, input: &str, expected: &str) -> Outcome<Self> {
        let mut rest = input;
        let position: usize = between(&mut rest, "POSTED; statement ", " of 50: ")?.parse()?;
        let xref_line: usize = between(&mut rest, "`cardxref.txt:", "`")?.parse()?;
        if (position, xref_line) != (number, number) {
            return Err(format!(
                "FIXSTMT-{number:03}: statement {position} of cardxref.txt line {xref_line}"
            )
            .into());
        }
        let mut rest = expected;
        let text = Span::parse(&mut rest, "STATEMNT.PS records ")?;
        let heading = [
            quote(&mut rest, "ST-LINE1 `")?,
            quote(&mut rest, "; ST-LINE2 `")?,
            quote(&mut rest, "; ST-LINE3 `")?,
            quote(&mut rest, "; ST-LINE4 `")?,
        ];
        let details = [
            quote(&mut rest, "; ST-LINE7 `")?,
            quote(&mut rest, "; ST-LINE8 `")?,
            quote(&mut rest, "; ST-LINE9 `")?,
        ];
        let listed = between(&mut rest, "; ST-LINE14 \u{d7} ", ": ")?.parse()?;
        let (list, tail) = rest
            .split_once("; ST-LINE14A `")
            .ok_or("an ST-LINE14A quote")?;
        let tran_lines = list
            .strip_prefix('`')
            .and_then(|list| list.strip_suffix('`'))
            .ok_or("backtick-quoted ST-LINE14 records")?
            .split("`; `")
            .map(str::to_owned)
            .collect();
        let (total_line, tail) = tail.split_once('`').ok_or("a closed ST-LINE14A quote")?;
        rest = tail;
        let html = Span::parse(&mut rest, ". STATEMNT.HTML records ")?;
        let html_title = quote(&mut rest, "HTML-L11 `")?;
        let html_heading = [
            quote(&mut rest, "; name `")?,
            quote(&mut rest, "; addresses `")?,
            quote(&mut rest, ", `")?,
            quote(&mut rest, ", `")?,
        ];
        if rest != ROW_END {
            return Err(format!("FIXSTMT-{number:03}: unparsed text after the quotes").into());
        }
        Ok(Self {
            number,
            text,
            heading,
            details,
            listed,
            tran_lines,
            total_line: total_line.to_owned(),
            html,
            html_title,
            html_heading,
        })
    }

    /// The statement's `STATEMNT.PS` records, padded to 80 bytes, in the order of CBSTM03A-031
    /// and FIXSTMT-902: `ST-LINE0`, 1, 2, 3, 4, 5, 6, 5, 7, 8, 9, 10, 11, 12, 13, 12, one
    /// `ST-LINE14` per transaction, 12, 14A, 15.
    fn text_records(&self, ids: &TranIds, fixed: &FixedLines) -> Outcome<Vec<Vec<u8>>> {
        let mut lines = vec![fixed.start.clone()];
        lines.extend(self.heading.iter().map(|line| line.as_bytes().to_vec()));
        lines.extend([
            fixed.dashes.clone(),
            fixed.basic.clone(),
            fixed.dashes.clone(),
        ]);
        lines.extend(self.details.iter().map(|line| line.as_bytes().to_vec()));
        lines.extend([
            fixed.dashes.clone(),
            fixed.summary.clone(),
            fixed.dashes.clone(),
            fixed.columns.clone(),
            fixed.dashes.clone(),
        ]);
        for line in &self.tran_lines {
            lines.push(ids.expand(line)?);
        }
        lines.extend([
            fixed.dashes.clone(),
            self.total_line.as_bytes().to_vec(),
            fixed.end.clone(),
        ]);
        lines
            .iter()
            .map(|line| fitted(line, TEXT_RECORD_LEN))
            .collect()
    }

    /// The statement's `STATEMNT.HTML` records, padded to 100 bytes, by FIXSTMT-903 and
    /// CBSTM03A-043 to CBSTM03A-046: the header, the name and address records, the basic-detail
    /// records built from `ST-LINE7` to `ST-LINE9`, the summary heading, 11 records per
    /// transaction built from its `ST-LINE14`, and the trailer.
    fn html_records(&self, ids: &TranIds, fixed: &FixedLines) -> Outcome<Vec<Vec<u8>>> {
        let padded = |line: &str| fitted(line.as_bytes(), TEXT_RECORD_LEN);
        let [account, balance, score] = &self.details;
        let mut records = literals(&HEADER_OPEN);
        records.push(self.html_title.as_bytes().to_vec());
        records.extend(literals(&HEADER_CLOSE));
        records.extend(
            self.html_heading
                .iter()
                .map(|line| line.as_bytes().to_vec()),
        );
        records.extend(literals(&BASIC_OPEN));
        records.push(paragraph(
            &fixed.account_label,
            &padded(account)?,
            ST_ACCT_ID,
        )?);
        records.push(paragraph(
            &fixed.balance_label,
            &padded(balance)?,
            ST_CURR_BAL,
        )?);
        records.push(paragraph(
            &fixed.score_label,
            &padded(score)?,
            ST_FICO_SCORE,
        )?);
        records.extend(literals(&SUMMARY_OPEN));
        for line in &self.tran_lines {
            let line = fitted(&ids.expand(line)?, TEXT_RECORD_LEN)?;
            records.extend(literals(&[LTRS, L58]));
            records.push(paragraph(b"<p>", &line, ST_TRANID)?);
            records.extend(literals(&[LTDE, L61]));
            records.push(paragraph(b"<p>", &line, ST_TRANDT)?);
            records.extend(literals(&[LTDE, L64]));
            records.push(paragraph(b"<p>", &line, ST_TRANAMT)?);
            records.extend(literals(&[LTDE, LTRE]));
        }
        records.extend(literals(&TRAILER));
        records
            .iter()
            .map(|record| fitted(record, HTML_RECORD_LEN))
            .collect()
    }
}

/// The bytes of each literal of `records`.
fn literals(records: &[&str]) -> Vec<Vec<u8>> {
    records
        .iter()
        .map(|record| record.as_bytes().to_vec())
        .collect()
}

/// `bytes` padded with spaces to a `len`-byte record; bytes beyond the record are an error,
/// never cut off.
fn fitted(bytes: &[u8], len: usize) -> Outcome<Vec<u8>> {
    if bytes.len() > len {
        return Err(format!("{} bytes for a {len}-byte record", bytes.len()).into());
    }
    let mut record = bytes.to_vec();
    record.resize(len, b' ');
    Ok(record)
}

/// `open`, the bytes `range` of the padded 80-byte `line`, then `</p>`: one HTML field record
/// (CBSTM03A-042, CBSTM03A-045).
fn paragraph(open: &[u8], line: &[u8], range: Range<usize>) -> Outcome<Vec<u8>> {
    let field = line.get(range).ok_or("a field inside its line")?;
    Ok([open, field, b"</p>"].concat())
}

/// `parts` one after another.
fn joined(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// The records every statement holds whatever its account: the fixed text lines of FIXSTMT-902
/// and the labels of the basic-detail HTML records of CBSTM03A-042, each built from the counts
/// and literals the rows state, before padding.
struct FixedLines {
    /// `ST-LINE0`: 31 `*`, `START OF STATEMENT`, 31 `*` (CBSTM03A-015).
    start: Vec<u8>,
    /// `ST-LINE5`, `ST-LINE10` and `ST-LINE12`: 80 `-` (CBSTM03A-019).
    dashes: Vec<u8>,
    /// `ST-LINE6`: 33 spaces, `Basic Details` (CBSTM03A-020).
    basic: Vec<u8>,
    /// `ST-LINE11`: 30 spaces, `TRANSACTION SUMMARY` (CBSTM03A-024).
    summary: Vec<u8>,
    /// `ST-LINE13`: `Tran ID`, 9 spaces, `Tran Details`, 41 spaces, `Tran Amount`
    /// (CBSTM03A-025).
    columns: Vec<u8>,
    /// `ST-LINE15`: 32 `*`, `END OF STATEMENT`, 32 `*` (CBSTM03A-030).
    end: Vec<u8>,
    /// `<p>`, `Account ID`, 9 spaces, `:` and a space (CBSTM03A-042).
    account_label: Vec<u8>,
    /// `<p>`, `Current Balance`, 4 spaces, `:` and a space (CBSTM03A-042).
    balance_label: Vec<u8>,
    /// `<p>`, `FICO Score`, 9 spaces, `:` and a space (CBSTM03A-042).
    score_label: Vec<u8>,
}

impl FixedLines {
    fn of_every_statement() -> Self {
        Self {
            start: joined(&[&[b'*'; 31], b"START OF STATEMENT", &[b'*'; 31]]),
            dashes: vec![b'-'; TEXT_RECORD_LEN],
            basic: joined(&[&[b' '; 33], b"Basic Details"]),
            summary: joined(&[&[b' '; 30], b"TRANSACTION SUMMARY"]),
            columns: joined(&[
                b"Tran ID",
                &[b' '; 9],
                b"Tran Details",
                &[b' '; 41],
                b"Tran Amount",
            ]),
            end: joined(&[&[b'*'; 32], b"END OF STATEMENT", &[b'*'; 32]]),
            account_label: joined(&[b"<p>", b"Account ID", &[b' '; 9], b": "]),
            balance_label: joined(&[b"<p>", b"Current Balance", &[b' '; 4], b": "]),
            score_label: joined(&[b"<p>", b"FICO Score", &[b' '; 9], b": "]),
        }
    }
}

/// The rows FIXSTMT-001 to FIXSTMT-050 of `fixtures.md`, in file order; the file rows from
/// FIXSTMT-901 on are skipped.
fn statement_rows() -> Outcome<Vec<StatementRow>> {
    let table = fs::read_to_string(audit_dir().join("fixtures.md"))?;
    let mut rows = Vec::new();
    for line in table.lines() {
        let Some(rest) = line.strip_prefix("| FIXSTMT-") else {
            continue;
        };
        let number: usize = rest
            .get(..3)
            .ok_or("a three-digit FIXSTMT number")?
            .parse()?;
        if number >= 901 {
            continue;
        }
        // Rule ID | Program | Paragraph | Copybook field or PIC | Input condition |
        // Expected result | Source lines | Fixture record.
        let mut cells = line.split(" | ");
        let input = cells.nth(4).ok_or("an Input condition cell")?;
        let expected = cells.next().ok_or("an Expected result cell")?;
        rows.push(StatementRow::parse(number, input, expected)?);
    }
    Ok(rows)
}

/// The `POSTED` statement files the audit states: every record of `STATEMNT.PS` and of
/// `STATEMNT.HTML`, padded and without its LF, statement after statement in `cardxref.txt` line
/// order.
struct Expected {
    text: Vec<Vec<u8>>,
    html: Vec<Vec<u8>>,
}

impl Expected {
    /// Builds both files from FIXSTMT-001 to FIXSTMT-050, checking each row's positions against
    /// the records the rows before it hold.
    fn from_audit() -> Outcome<Self> {
        let rows = statement_rows()?;
        // audit: FIXSTMT-901
        assert!(
            rows.iter().map(|row| row.number).eq(1..=STATEMENTS),
            "FIXSTMT-001 to FIXSTMT-050: one row per statement, in cardxref.txt line order"
        );
        let ids = TranIds::read()?;
        let fixed = FixedLines::of_every_statement();
        let mut expected = Self {
            text: Vec::new(),
            html: Vec::new(),
        };
        for row in &rows {
            let (n, k) = (row.number, row.tran_lines.len());
            assert_eq!(row.listed, k, "FIXSTMT-{n:03}: ST-LINE14 records quoted");
            // audit: CBSTM03A-031
            assert_eq!(
                row.text,
                Span::after(expected.text.len(), 19 + k),
                "FIXSTMT-{n:03}: STATEMNT.PS records A-B (C), 19 + n per statement"
            );
            // audit: CBSTM03A-047
            assert_eq!(
                row.html,
                Span::after(expected.html.len(), 64 + 11 * k),
                "FIXSTMT-{n:03}: STATEMNT.HTML records D-E (F), 64 + 11n per statement"
            );
            expected.text.extend(row.text_records(&ids, &fixed)?);
            expected.html.extend(row.html_records(&ids, &fixed)?);
        }
        // audit: FIXSTMT-901
        assert_eq!(
            (rows.len(), expected.text.len(), expected.html.len()),
            (STATEMENTS, TEXT_RECORDS, HTML_RECORDS),
            "FIXSTMT-901: statements, STATEMNT.PS records and STATEMNT.HTML records"
        );
        Ok(expected)
    }
}

/// Scenario `POSTED` on `db`, the fixture-loaded database: `posttran` over `dailytran.txt`.
/// Returns the `transactions` rows it leaves, in card-number then transaction-ID order.
async fn post_fixture_transactions(
    scratch: &Scratch,
    db: &PostgresGuard,
) -> Outcome<Vec<TransactionRecord>> {
    let dalytran = fixture_path("dailytran.txt");
    let dalytran = dalytran.to_str().ok_or("a UTF-8 fixture path")?;
    let posted = run_batch(
        scratch,
        db.database_url(),
        &["posttran", "--dalytran", dalytran],
    )
    .await?;
    assert_eq!(
        posted.status.code(),
        Some(POSTTRAN_STATUS),
        "posttran over dailytran.txt (FIXPOST-903)"
    );
    let rows = transactions(db).await?;
    assert_eq!(
        rows.len(),
        POSTED_TRANSACTIONS,
        "transactions after posttran (FIXPOST-901)"
    );
    Ok(rows)
}

/// Every `transactions` row of `db`, in card-number then transaction-ID order.
async fn transactions(db: &PostgresGuard) -> Outcome<Vec<TransactionRecord>> {
    let mut store = PgStore::new(db.pool().clone());
    Ok(TransactionRepository::find_all_in_card_order(&mut store).await?)
}

/// Asserts the normal end of a `creastmt` run (CBSTM03A-048, CBSTM03A-050): exit status 0, the
/// three statement paths and nothing else as plain standard output, no error line, and the
/// statement directory holding exactly the three files, under their fixed names (CBSTM03A-002).
fn assert_normal_end(scratch: &Scratch, run: &Output) -> TestResult {
    // audit: CBSTM03A-050
    assert_eq!(run.status.code(), Some(0), "creastmt exit status");
    // audit: CBSTM03A-003
    assert_eq!(
        written_paths(run)?,
        scratch.statement_paths(),
        "standard output: the three statement paths, in order, and nothing else"
    );
    let errors: Vec<String> = event_messages(run)?
        .into_iter()
        .filter(|message| message.starts_with("ERROR"))
        .collect();
    // audit: CBSTM03A-005
    // audit: CBSTM03A-048
    assert!(
        errors.is_empty(),
        "a normal end shows no error line: {errors:?}"
    );
    let mut files = vec![TEXT_FILE, HTML_FILE, PDF_FILE];
    files.sort_unstable();
    // audit: CBSTM03A-002
    assert_eq!(
        names(&scratch.statements())?,
        files,
        "the statement directory holds the three fixed-name files and nothing else"
    );
    Ok(())
}

/// Asserts an abended `creastmt` run (CBSTM03A-049): a non-zero exit status, the replacement's
/// 16; no path printed; `display`, the two lines of the failed read and `ABENDING PROGRAM`, one
/// after the other; and a statement directory that holds no file.
fn assert_abend(scratch: &Scratch, run: &Output, display: [&str; 3]) -> TestResult {
    let status = run.status.code();
    // audit: CBSTM03A-049
    assert!(
        status.is_some_and(|code| code != 0),
        "an abend ends with a non-zero status, not {status:?}"
    );
    assert_eq!(status, Some(ABEND_STATUS), "the replacement's abend status");
    assert!(
        written_paths(run)?.is_empty(),
        "an abended run prints no path"
    );
    let messages = event_messages(run)?;
    assert!(
        messages
            .windows(display.len())
            .any(|window| window.iter().map(String::as_str).eq(display)),
        "the DISPLAY lines {display:?}, in program order, among {messages:?}"
    );
    assert_eq!(
        names(&scratch.statements())?,
        Vec::<String>::new(),
        "no statement file remains after an abend"
    );
    Ok(())
}

/// Asserts the three statement files of a `POSTED` run against `expected`: `STATEMNT.PS` and
/// `STATEMNT.HTML` record for record, and the text extracted from `STATEMNT.PDF` line for line
/// against `STATEMNT.PS`.
fn assert_statement_files(scratch: &Scratch, expected: &Expected) -> TestResult {
    let [text_path, html_path, pdf_path] = scratch.statement_paths();
    let text = fs::read(text_path)?;
    // audit: CBSTM03A-031
    let text_records = fixed_records(TEXT_FILE, &text, TEXT_RECORD_LEN)?;
    // audit: FIXSTMT-001
    // audit: FIXSTMT-002
    // audit: FIXSTMT-003
    // audit: FIXSTMT-004
    // audit: FIXSTMT-005
    // audit: FIXSTMT-006
    // audit: FIXSTMT-007
    // audit: FIXSTMT-008
    // audit: FIXSTMT-009
    // audit: FIXSTMT-010
    // audit: FIXSTMT-011
    // audit: FIXSTMT-012
    // audit: FIXSTMT-013
    // audit: FIXSTMT-014
    // audit: FIXSTMT-015
    // audit: FIXSTMT-016
    // audit: FIXSTMT-017
    // audit: FIXSTMT-018
    // audit: FIXSTMT-019
    // audit: FIXSTMT-020
    // audit: FIXSTMT-021
    // audit: FIXSTMT-022
    // audit: FIXSTMT-023
    // audit: FIXSTMT-024
    // audit: FIXSTMT-025
    // audit: FIXSTMT-026
    // audit: FIXSTMT-027
    // audit: FIXSTMT-028
    // audit: FIXSTMT-029
    // audit: FIXSTMT-030
    // audit: FIXSTMT-031
    // audit: FIXSTMT-032
    // audit: FIXSTMT-033
    // audit: FIXSTMT-034
    // audit: FIXSTMT-035
    // audit: FIXSTMT-036
    // audit: FIXSTMT-037
    // audit: FIXSTMT-038
    // audit: FIXSTMT-039
    // audit: FIXSTMT-040
    // audit: FIXSTMT-041
    // audit: FIXSTMT-042
    // audit: FIXSTMT-043
    // audit: FIXSTMT-044
    // audit: FIXSTMT-045
    // audit: FIXSTMT-046
    // audit: FIXSTMT-047
    // audit: FIXSTMT-048
    // audit: FIXSTMT-049
    // audit: FIXSTMT-050
    // audit: FIXSTMT-902
    // audit: CBSTM03A-001
    // audit: CBSTM03A-004
    // audit: CBSTM03A-006
    // audit: CBSTM03A-008
    // audit: CBSTM03A-009
    // audit: CBSTM03A-010
    // audit: CBSTM03A-011
    // audit: CBSTM03A-013
    // audit: CBSTM03A-014
    // audit: CBSTM03A-015
    // audit: CBSTM03A-016
    // audit: CBSTM03A-017
    // audit: CBSTM03A-018
    // audit: CBSTM03A-019
    // audit: CBSTM03A-020
    // audit: CBSTM03A-021
    // audit: CBSTM03A-022
    // audit: CBSTM03A-023
    // audit: CBSTM03A-024
    // audit: CBSTM03A-025
    // audit: CBSTM03A-026
    // audit: CBSTM03A-027
    // audit: CBSTM03A-028
    // audit: CBSTM03A-029
    // audit: CBSTM03A-030
    // audit: CBSTM03B-001
    // audit: CBSTM03B-002
    // audit: CBSTM03B-003
    // audit: CBSTM03B-004
    // audit: CBSTM03B-005
    // audit: CBSTM03B-006
    assert_records(TEXT_FILE, &text_records, &expected.text);
    let html = fs::read(html_path)?;
    // audit: CBSTM03A-047
    let html_records = fixed_records(HTML_FILE, &html, HTML_RECORD_LEN)?;
    // audit: FIXSTMT-903
    // audit: CBSTM03A-032
    // audit: CBSTM03A-033
    // audit: CBSTM03A-034
    // audit: CBSTM03A-035
    // audit: CBSTM03A-036
    // audit: CBSTM03A-037
    // audit: CBSTM03A-038
    // audit: CBSTM03A-039
    // audit: CBSTM03A-040
    // audit: CBSTM03A-041
    // audit: CBSTM03A-042
    // audit: CBSTM03A-043
    // audit: CBSTM03A-044
    // audit: CBSTM03A-045
    // audit: CBSTM03A-046
    assert_records(HTML_FILE, &html_records, &expected.html);
    let lines = pdf_lines(&fs::read(pdf_path)?)?;
    // audit: FIXSTMT-904
    // audit: CBSTM03A-051
    // audit: CBSTM03A-052
    assert_pdf_lines(&lines, &text_records);
    Ok(())
}

/// A cross-reference of `card`, a card number in no fixture file, to customer `cust_id` and
/// account `acct_id`.
fn added_xref(card: [u8; 16], cust_id: u64, acct_id: u64) -> Outcome<CardXrefRecord> {
    Ok(CardXrefRecord {
        card_num: CardNumber::new(PicX::from_array(card)),
        cust_id: PicU::new(cust_id)?,
        acct_id: PicU::new(acct_id)?,
        filler: PicX::spaces(),
    })
}

/// The URL of the `postgres` database of the server `database_url` names. `initdb` creates it
/// in every `postgres:16` container and no migration touches it, so it holds none of the
/// tables a job reads.
fn schema_less_database(database_url: &str) -> Outcome<String> {
    let (server, _database) = database_url
        .rsplit_once('/')
        .ok_or("a database URL ending in /<database>")?;
    Ok(format!("{server}/postgres"))
}

/// `POSTED`: `creastmt` writes the 50 statements of FIXSTMT-001 to FIXSTMT-050 as 80-byte text
/// records, 100-byte HTML records and a PDF whose text is the text records, prints the three
/// paths, ends with status 0 and leaves `transactions` as it found it.
#[tokio::test(flavor = "multi_thread")]
async fn statements_match_audit() -> TestResult {
    let expected = Expected::from_audit()?;
    let db = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    let posted = post_fixture_transactions(&scratch, &db).await?;
    let run = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    assert_normal_end(&scratch, &run)?;
    assert_statement_files(&scratch, &expected)?;
    // The job requests no write or rewrite of any file it reads.
    // audit: CBSTM03B-007
    assert!(
        transactions(&db).await? == posted,
        "creastmt writes no table: the transactions are as posttran left them"
    );
    Ok(())
}

/// `LOADED`, whose `transactions` is empty: the first read is at end of file, so the run shows
/// `ERROR READING TRNXFILE`, `RETURN CODE: 10` and `ABENDING PROGRAM`, removes the previous
/// run's files and its own, prints no path and exits non-zero. Halt candidate X-CREASTMT-H1
/// blocks this case's parity, so it is the replacement's behaviour that is asserted.
#[tokio::test(flavor = "multi_thread")]
async fn empty_transactions_abend() -> TestResult {
    let db = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    assert!(
        transactions(&db).await?.is_empty(),
        "LOADED holds no transaction"
    );
    plant_previous_run(&scratch)?;
    let run = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    // audit: CBSTM03A-002
    // audit: CBSTM03A-007
    // audit: CBSTM03A-049
    // audit: CBSTM03B-006
    assert_abend(
        &scratch,
        &run,
        [
            "ERROR READING TRNXFILE",
            "RETURN CODE: 10",
            ABENDING_PROGRAM,
        ],
    )?;
    blocked("CBSTM03A-007", "X-CREASTMT-H1");
    blocked("CBSTM03A-049", "X-CREASTMT-H1");
    Ok(())
}

/// Two `POSTED` runs into one statement directory: the second replaces the three fixed-name
/// files, so the directory holds only them, `STATEMNT.PS` and `STATEMNT.HTML` hold the first
/// run's bytes again although the first run's files were made longer in between, and the PDF
/// text is still the text records. No generation is kept.
#[tokio::test(flavor = "multi_thread")]
async fn rerun_replaces_fixed_name_files() -> TestResult {
    let db = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    post_fixture_transactions(&scratch, &db).await?;
    let first = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    assert_normal_end(&scratch, &first)?;
    let [text_path, html_path, pdf_path] = scratch.statement_paths();
    let text = fs::read(&text_path)?;
    let html = fs::read(&html_path)?;
    // A second run that appended to the first run's files, or left one in place, would leave
    // these bytes behind.
    for path in [&text_path, &html_path, &pdf_path] {
        OpenOptions::new()
            .append(true)
            .open(path)?
            .write_all(PREVIOUS_RUN)?;
    }
    let second = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    // audit: CBSTM03A-002
    assert_normal_end(&scratch, &second)?;
    assert!(
        fs::read(&text_path)? == text,
        "{TEXT_FILE}: the second run's bytes differ from the first run's"
    );
    assert!(
        fs::read(&html_path)? == html,
        "{HTML_FILE}: the second run's bytes differ from the first run's"
    );
    let records = fixed_records(TEXT_FILE, &text, TEXT_RECORD_LEN)?;
    // audit: CBSTM03A-051
    // audit: CBSTM03A-052
    assert_pdf_lines(&pdf_lines(&fs::read(&pdf_path)?)?, &records);
    Ok(())
}

/// `POSTED` with one added cross-reference whose customer no `customers` row holds, then with
/// one more, sorting before it, whose account no `accounts` row holds: each run writes every
/// fixture statement, reaches the added record, shows the failed read with status `23` and
/// `ABENDING PROGRAM`, and leaves no statement file, those already written included.
#[tokio::test(flavor = "multi_thread")]
async fn missing_customer_or_account_abends() -> TestResult {
    assert!(
        fixture_card_numbers()?
            .iter()
            .all(|card| card.as_bytes() < NEXT_TO_LAST_CARD.as_slice()),
        "the added cards sort after every fixture card"
    );
    let db = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    post_fixture_transactions(&scratch, &db).await?;
    let mut store = PgStore::new(db.pool().clone());
    let no_customer = added_xref(LAST_CARD, MISSING_CUSTOMER, FIXTURE_ACCOUNT)?;
    CardXrefRepository::insert(&mut store, &no_customer).await?;
    plant_previous_run(&scratch)?;
    let run = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    // audit: CBSTM03A-010
    // audit: CBSTM03A-049
    // audit: CBSTM03B-004
    // audit: CBSTM03B-006
    assert_abend(
        &scratch,
        &run,
        [
            "ERROR READING CUSTFILE",
            "RETURN CODE: 23",
            ABENDING_PROGRAM,
        ],
    )?;
    let no_account = added_xref(NEXT_TO_LAST_CARD, FIXTURE_CUSTOMER, MISSING_ACCOUNT)?;
    CardXrefRepository::insert(&mut store, &no_account).await?;
    plant_previous_run(&scratch)?;
    let run = run_batch(&scratch, db.database_url(), &["creastmt"]).await?;
    // audit: CBSTM03A-011
    // audit: CBSTM03A-049
    // audit: CBSTM03B-005
    // audit: CBSTM03B-006
    assert_abend(
        &scratch,
        &run,
        [
            "ERROR READING ACCTFILE",
            "RETURN CODE: 23",
            ABENDING_PROGRAM,
        ],
    )?;
    Ok(())
}

/// A database without the schema: the read of `transactions` fails, so the run shows
/// `ERROR READING TRNXFILE`, a return-code line and `ABENDING PROGRAM`, removes the previous
/// run's files and exits non-zero. The DD name, the order of the lines, the removal and the
/// non-zero status are parity; the status characters `30` are the replacement's, which halt
/// candidate X-CREASTMT-H3 blocks.
#[tokio::test(flavor = "multi_thread")]
async fn unreadable_transactions_abend() -> TestResult {
    let db = start_postgres().await?;
    let scratch = Scratch::new()?;
    let schema_less = schema_less_database(db.database_url())?;
    plant_previous_run(&scratch)?;
    let run = run_batch(&scratch, &schema_less, &["creastmt"]).await?;
    // audit: CBSTM03A-012
    // audit: CBSTM03A-049
    assert_abend(
        &scratch,
        &run,
        [
            "ERROR READING TRNXFILE",
            "RETURN CODE: 30",
            ABENDING_PROGRAM,
        ],
    )?;
    blocked("CBSTM03A-012", "X-CREASTMT-H3");
    Ok(())
}
