//! `CBSTM03A` with its file-access subroutine `CBSTM03B`: the account
//! statements of the `CREASTMT` job, in plain text (`STMTFILE`, 80-byte
//! records) and HTML (`HTMLFILE`, 100-byte records).
//!
//! The COBOL loads the whole transaction file, sorted by card number then
//! TRAN-ID (`CREASTMT` `STEP010`, P-CREASTMT-002), into a two-dimensional
//! working-storage table (`8500-READTRNX-READ`, P-CBSTM03A-004), then reads the
//! card cross-reference file sequentially and writes one statement per card
//! (`1000-MAINLINE`, P-CBSTM03A-005): the customer and the account read by key,
//! the header (`5000-CREATE-STATEMENT`, `5100-WRITE-HTML-HEADER`,
//! `5200-WRITE-HTML-NMADBS`), one line per transaction of the card
//! (`6000-WRITE-TRANS`) and the total and closing lines (`4000-TRNXFILE-GET`).
//!
//! [`load_table`] is the table load and [`create_statements`] the mainline.
//! `CBSTM03B` [app/cbl/CBSTM03B.CBL] is replaced by the ports and by
//! `batch creastmt`, which reads the transactions in statement order and the
//! cross-references in key order and writes the two files from the returned
//! [`StatementOutput`]. A `CBSTM03B` return code other than `00` becomes a
//! [`StatementError::ReadFailed`] whose text is the `DISPLAY` lines of the
//! abending path (P-CBSTM03A-1257 to P-CBSTM03A-1259, P-CBSTM03A-011) and
//! whose source is the port failure behind it, for the job's log.
//!
//! The z/OS TIOT walk (P-CBSTM03A-001), the `ALTER` and `GO TO` dispatch of
//! `0000-START` (P-CBSTM03A-002) and the opens and closes (P-CBSTM03A-1260 to
//! P-CBSTM03A-1264) write nothing to either file and have no counterpart here.
//!
//! Every byte of a line comes from the `STMTFILE` and `HTMLFILE` builders of
//! [`crate::format`]; this module decides which lines are written, in which
//! order, and with which field contents.

use crate::entities::{Account, CardXref, Customer, Transaction};
use crate::format::{self, HtmlFixedLine};
use crate::messages::{
    ABENDING_PROGRAM, ERROR_READING_ACCTFILE, ERROR_READING_CUSTFILE, ERROR_READING_TRNXFILE,
    ERROR_READING_XREFFILE, RETURN_CODE,
};
use crate::numeric::{Alnum, CardNumber, Money, TranId};
use crate::ports::{AccountReader, CustomerReader, PortError};

/// The card slots of `WS-CARD-TBL OCCURS 51 TIMES` and of its counters
/// `WS-TRN-TBL-CTR OCCURS 51 TIMES` [app/cbl/CBSTM03A.CBL:L225-233]
/// (P-CBSTM03A-004).
pub const CARD_CAPACITY: usize = 51;

/// The transaction slots of each card, `WS-TRAN-TBL OCCURS 10 TIMES`
/// [app/cbl/CBSTM03A.CBL:L228-230] (P-CBSTM03A-004).
pub const TRANSACTIONS_PER_CARD: usize = 10;

/// The records of the sorted transaction file that decide [`load_table`]: one
/// more than the 510 slots of the `CBSTM03A` tables, [`CARD_CAPACITY`] cards of
/// [`TRANSACTIONS_PER_CARD`] transactions (P-CBSTM03A-004).
///
/// The load halts at the first record that has no slot, and every record
/// before it fills one, so that record is at most the 511th; a file that fits
/// the tables has at most 510 records. The first 511 records therefore give
/// the same table, the same [`StatementError::CapacityExceeded`] counts or the
/// same empty-file [`StatementError::ReadFailed`] as the whole file, and a file
/// of more records always ends in the capacity stop (P-CBSTM03A-1256,
/// escalation [`CAPACITY_ESCALATION`]): reading no further drops no record a
/// statement could hold. `batch creastmt` reads at most this many.
///
/// The value is written out because a constant cannot convert the `usize`
/// product to `u16` without a lossy cast; a unit test holds it to the product.
pub const TRNXFILE_READ_LIMIT: u16 = 511;

/// The `halt` escalation that records a transaction file beyond the tables:
/// more than [`CARD_CAPACITY`] cards or [`TRANSACTIONS_PER_CARD`] transactions
/// for one card, where the COBOL subscripts run past their tables
/// (P-CBSTM03A-1256, escalation E-33).
pub const CAPACITY_ESCALATION: &str = "E-33";

/// `CBSTM03B` return code `10`, the end of a sequential file. On the first
/// `TRNXFILE` read it takes the error branch of `8100-TRNXFILE-OPEN`
/// (P-CBSTM03A-003).
const RC_END_OF_FILE: &str = "10";

/// `CBSTM03B` return code `23`: no record has the key (`NOTFND`), the status
/// of [`PortError::NotFound`].
const RC_NOT_FOUND: &str = "23";

/// `CBSTM03B` return code `30`, a permanent I/O error with no further
/// information: the status reported for a store failure, which has no COBOL
/// counterpart. Any status other than `00` takes the abending branch, so the
/// statements and the exit code do not depend on which one it is.
const RC_PERMANENT_ERROR: &str = "30";

/// CBSTM03B 1900-EXIT, 2900-EXIT, 3900-EXIT and 4900-EXIT — P-CBSTM03B-001 to
/// P-CBSTM03B-005: the `CBSTM03B` return code of a failed read, the VSAM file
/// status the subroutine moves into `WS-M03B-RC` [app/cbl/CBSTM03B.CBL:L151-152]
/// [app/cbl/CBSTM03B.CBL:L175-176] [app/cbl/CBSTM03B.CBL:L200-201]
/// [app/cbl/CBSTM03B.CBL:L225-226].
fn return_code(error: &PortError) -> &'static str {
    match error {
        PortError::NotFound => RC_NOT_FOUND,
        // A read reports only `NotFound` or `Unavailable` (crate::ports); every
        // other outcome is the same store failure.
        PortError::Duplicate
        | PortError::Restricted
        | PortError::Contention
        | PortError::Unavailable(_) => RC_PERMANENT_ERROR,
    }
}

/// Why no statement is written.
///
/// The `Display` text of each variant is the lines `batch creastmt` reports on
/// stderr, one per line, before it exits with code 16.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StatementError {
    /// The transaction file needs more than [`CARD_CAPACITY`] card slots or
    /// more than [`TRANSACTIONS_PER_CARD`] transaction slots for one card. The
    /// COBOL stores past its tables without a bound check, so its result is
    /// undefined and nothing is produced (P-CBSTM03A-004, P-CBSTM03A-1256,
    /// [`CAPACITY_ESCALATION`]).
    ///
    /// The load halts at the first record that has no slot, so the counts
    /// describe the records up to and including that one: lower bounds on
    /// what the whole file needs.
    #[error(
        "TRNXFILE needs at least {cards} card slots and at least {max_per_card} transaction slots \
         for one card; the CBSTM03A tables hold {card_capacity} cards and {per_card} transactions \
         per card",
        card_capacity = CARD_CAPACITY,
        per_card = TRANSACTIONS_PER_CARD
    )]
    CapacityExceeded {
        /// The card slots the records up to the halt fill: one per run of
        /// records with the same card number, the halting record's run
        /// included.
        cards: usize,
        /// The most transactions of one card slot among the records up to the
        /// halt, the halting record included.
        max_per_card: usize,
    },
    /// A `CBSTM03B` call returned a code other than `00`: `CBSTM03A` displays
    /// the failure text and `RETURN CODE: ` with the code, then
    /// `9999-ABEND-PROGRAM` displays `ABENDING PROGRAM` and calls `CEE3ABD`
    /// (P-CBSTM03A-003, P-CBSTM03A-1257 to P-CBSTM03A-1259, P-CBSTM03A-011).
    /// The port failure behind the return code is the error's source only.
    #[error("{file}\n{label}{rc}\n{abend}", label = RETURN_CODE, abend = ABENDING_PROGRAM)]
    ReadFailed {
        /// The failure text that names the file, one of the `crate::messages`
        /// constants `ERROR_READING_TRNXFILE`, `ERROR_READING_XREFFILE`,
        /// `ERROR_READING_CUSTFILE` and `ERROR_READING_ACCTFILE`.
        file: &'static str,
        /// The two-character `CBSTM03B` return code, `WS-M03B-RC X(02)`.
        rc: String,
        /// The port failure behind the abend, for the job's log event; `None`
        /// for the end of the transaction file at open, which is no port
        /// failure. Its text is adapter detail and is never part of the
        /// displayed lines.
        #[source]
        cause: Option<PortError>,
    },
}

impl StatementError {
    /// CBSTM03A 2000-CUSTFILE-GET and 3000-ACCTFILE-GET — P-CBSTM03A-1258,
    /// P-CBSTM03A-1259: the read failure displayed as `file` with the return
    /// code of `error`, which it keeps as its cause
    /// [app/cbl/CBSTM03A.CBL:L379-386] [app/cbl/CBSTM03A.CBL:L403-410]; also the
    /// abend of the transaction and cross-reference reads, through
    /// [`Self::trnxfile_read_failed`] and [`Self::xreffile_read_failed`].
    fn read_failed(file: &'static str, error: PortError) -> Self {
        Self::ReadFailed {
            file,
            rc: return_code(&error).to_owned(),
            cause: Some(error),
        }
    }

    /// CBSTM03A 8100-TRNXFILE-OPEN and 8500-READTRNX-READ — P-CBSTM03A-002,
    /// P-CBSTM03A-003: a failed read of the transaction file, whose records
    /// `batch creastmt` reads in statement order before [`load_table`]: the
    /// `ERROR READING TRNXFILE` branch of `8100-TRNXFILE-OPEN` and
    /// `8500-READTRNX-READ` [app/cbl/CBSTM03A.CBL:L748-754]
    /// [app/cbl/CBSTM03A.CBL:L843-846].
    #[must_use]
    pub fn trnxfile_read_failed(error: &PortError) -> Self {
        Self::read_failed(ERROR_READING_TRNXFILE, error.clone())
    }

    /// CBSTM03A 1000-XREFFILE-GET-NEXT — P-CBSTM03A-1257: a failed read of the
    /// card cross-reference file, which `batch creastmt` reads in key order
    /// before [`create_statements`]: the `WHEN OTHER` branch of
    /// `1000-XREFFILE-GET-NEXT` [app/cbl/CBSTM03A.CBL:L353-362].
    #[must_use]
    pub fn xreffile_read_failed(error: &PortError) -> Self {
        Self::read_failed(ERROR_READING_XREFFILE, error.clone())
    }
}

/// One transaction slot of the table: the fields of the `COSTM01`
/// `TRNX-RECORD` [app/cpy/COSTM01.CPY:L20-37] that a statement shows.
///
/// The `STEP010` sort moves the card number in front of the transaction record
/// (P-CREASTMT-002), so `TRNX-ID`, `TRNX-DESC` and `TRNX-AMT` hold `TRAN-ID`,
/// `TRAN-DESC` and `TRAN-AMT` of the `CVTRA05Y` record unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementTransaction {
    /// `TRNX-CARD-NUM PIC X(16)`, the card slot's `WS-CARD-NUM`.
    pub card_num: CardNumber,
    /// `TRNX-ID PIC X(16)`, stored in `WS-TRAN-NUM` and shown as `ST-TRANID`.
    pub tran_id: TranId,
    /// `TRNX-DESC PIC X(100)`, shown cut to `ST-TRANDT X(49)`.
    pub description: Alnum<100>,
    /// `TRNX-AMT PIC S9(09)V99`, shown edited `Z(9).99-` and added to the card
    /// total.
    pub amount: Money<9>,
}

impl StatementTransaction {
    /// CBSTM03A 8500-READTRNX-READ — P-CBSTM03A-004, P-CREASTMT-002: the slot
    /// content of the transaction `tran`, as `MOVE TRNX-CARD-NUM`, `MOVE
    /// TRNX-ID` and `MOVE TRNX-REST` fill `WS-CARD-NUM (CR-CNT)` and the slot
    /// `(CR-CNT, TR-CNT)` [app/cbl/CBSTM03A.CBL:L827-829].
    fn from_transaction(tran: &Transaction) -> Self {
        Self {
            card_num: tran.tran_card_num,
            tran_id: tran.tran_id,
            description: tran.tran_desc,
            amount: tran.tran_amt,
        }
    }
}

/// `WS-TRNX-TABLE` with its counters `WS-TRN-TBL-CNTR`
/// [app/cbl/CBSTM03A.CBL:L225-233]: the card slots in load order, each with
/// its card number and its transactions in file order.
///
/// Only [`load_table`] builds one, so a table never exceeds
/// [`CARD_CAPACITY`] slots of [`TRANSACTIONS_PER_CARD`] transactions and never
/// is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionTable {
    /// `WS-CARD-NUM (CR-CNT)` and the `WS-TRCT (CR-CNT)` transactions of each
    /// filled slot.
    cards: Vec<(CardNumber, Vec<StatementTransaction>)>,
}

impl TransactionTable {
    /// CBSTM03A 4000-TRNXFILE-GET — P-CBSTM03A-008: the transactions the
    /// paragraph writes for the card `card` [app/cbl/CBSTM03A.CBL:L417-432]:
    /// the slots are scanned from the first `UNTIL CR-JMP > CR-CNT OR
    /// WS-CARD-NUM (CR-JMP) > XREF-CARD-NUM`, and every slot whose card number
    /// equals the card gives all of its transactions, in slot order.
    fn transactions_of(&self, card: CardNumber) -> impl Iterator<Item = &StatementTransaction> {
        self.cards
            .iter()
            .take_while(move |(slot_card, _)| *slot_card <= card)
            .filter(move |(slot_card, _)| *slot_card == card)
            .flat_map(|(_, transactions)| transactions)
    }
}

/// The records `CBSTM03A` writes, in write order: `STMTFILE` lines of
/// [`format::STMT_RECORD_LEN`] bytes and `HTMLFILE` lines of
/// [`format::HTML_RECORD_LEN`] bytes, without line terminators
/// (P-CBSTM03A-015).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatementOutput {
    /// The `FD-STMTFILE-REC PIC X(80)` records [app/cbl/CBSTM03A.CBL:L44-45].
    pub stmt_lines: Vec<String>,
    /// The `FD-HTMLFILE-REC PIC X(100)` records [app/cbl/CBSTM03A.CBL:L46-47].
    pub html_lines: Vec<String>,
}

/// `5100-WRITE-HTML-HEADER` up to `HTML-L11` [app/cbl/CBSTM03A.CBL:L508-527]:
/// HTML lines 1 to 10 of every statement (P-CBSTM03A-027 to P-CBSTM03A-036).
const HTML_HEADER_OPEN: [HtmlFixedLine; 10] = [
    HtmlFixedLine::L01,
    HtmlFixedLine::L02,
    HtmlFixedLine::L03,
    HtmlFixedLine::L04,
    HtmlFixedLine::L05,
    HtmlFixedLine::L06,
    HtmlFixedLine::L07,
    HtmlFixedLine::L08,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L10,
];

/// `5100-WRITE-HTML-HEADER` after `HTML-L11` [app/cbl/CBSTM03A.CBL:L531-552]:
/// HTML lines 12 to 22, the bank address block (P-CBSTM03A-037 to
/// P-CBSTM03A-047).
const HTML_HEADER_BANK: [HtmlFixedLine; 11] = [
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L15,
    HtmlFixedLine::L16,
    HtmlFixedLine::L17,
    HtmlFixedLine::L18,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L22And35,
];

/// `5200-WRITE-HTML-NMADBS` between the address and the basic details
/// [app/cbl/CBSTM03A.CBL:L594-611]: HTML lines 27 to 35 (P-CBSTM03A-048 to
/// P-CBSTM03A-056).
const HTML_BASIC_DETAILS_OPEN: [HtmlFixedLine; 9] = [
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L30And42,
    HtmlFixedLine::L31,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L22And35,
];

/// The end of `5200-WRITE-HTML-NMADBS` [app/cbl/CBSTM03A.CBL:L634-669]: HTML
/// lines 39 to 56, the transaction summary heading and its column headings
/// (P-CBSTM03A-057 to P-CBSTM03A-074).
const HTML_SUMMARY_HEADINGS: [HtmlFixedLine; 18] = [
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L30And42,
    HtmlFixedLine::L43,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L47,
    HtmlFixedLine::L48,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::L50,
    HtmlFixedLine::L51,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::L53,
    HtmlFixedLine::L54,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
];

/// The closing lines of `4000-TRNXFILE-GET` [app/cbl/CBSTM03A.CBL:L439-454]:
/// the last 8 HTML lines of every statement (P-CBSTM03A-083 to
/// P-CBSTM03A-090).
const HTML_STATEMENT_CLOSE: [HtmlFixedLine; 8] = [
    HtmlFixedLine::Ltrs,
    HtmlFixedLine::L10,
    HtmlFixedLine::L75,
    HtmlFixedLine::Ltde,
    HtmlFixedLine::Ltre,
    HtmlFixedLine::L78,
    HtmlFixedLine::L79,
    HtmlFixedLine::L80,
];

/// CBSTM03A 5100-WRITE-HTML-HEADER — P-CBSTM03A-009: appends the 100-byte
/// records of the fixed HTML lines `fixed`, each written after `SET HTML-… TO
/// TRUE` [app/cbl/CBSTM03A.CBL:L506-555] (also for the fixed lines of
/// `5200-WRITE-HTML-NMADBS`, `4000-TRNXFILE-GET` and `6000-WRITE-TRANS`).
fn push_fixed(html_lines: &mut Vec<String>, fixed: &[HtmlFixedLine]) {
    html_lines.extend(fixed.iter().copied().map(HtmlFixedLine::record));
}

/// CBSTM03A 5000-CREATE-STATEMENT — P-CBSTM03A-006: a `STRING` whose every
/// source is `DELIMITED BY ' '` and followed by `' ' DELIMITED BY SIZE`, as the
/// paragraph builds `ST-NAME` and `ST-ADD3` [app/cbl/CBSTM03A.CBL:L462-481]:
/// each source up to its first space, then one space.
///
/// `INITIALIZE STATEMENT-LINES` cleared the receiver to spaces first, and the
/// line builders cut the text at the receiver width, where the `STRING` stops
/// on overflow.
fn string_delimited_by_space(sources: &[&str]) -> String {
    sources
        .iter()
        .flat_map(|source| [format::delimited_by(source, " "), " "])
        .collect()
}

/// The `STATEMENT-LINES` fields `5000-CREATE-STATEMENT` fills from the
/// customer and the account [app/cbl/CBSTM03A.CBL:L459-485] (P-CBSTM03A-006).
struct HeaderFields {
    /// `ST-NAME`: first, middle and last name, each up to its first space and
    /// followed by a space.
    name: String,
    /// `ST-ADD1`: `CUST-ADDR-LINE-1`.
    addr1: String,
    /// `ST-ADD2`: `CUST-ADDR-LINE-2`.
    addr2: String,
    /// `ST-ADD3`: address line 3, state, country and ZIP, each up to its first
    /// space and followed by a space.
    addr3: String,
    /// `ACCT-ID 9(11)`, moved into `ST-ACCT-ID` and `L11-ACCT`.
    account_id: u64,
    /// `ACCT-CURR-BAL S9(10)V99`, moved into `ST-CURR-BAL 9(9).99-`.
    curr_bal: Money<10>,
    /// `CUST-FICO-CREDIT-SCORE 9(03)`, moved into `ST-FICO-SCORE`.
    fico_score: u16,
}

impl HeaderFields {
    /// CBSTM03A 5000-CREATE-STATEMENT — P-CBSTM03A-006: the fields of the
    /// statement of `customer` and `account` [app/cbl/CBSTM03A.CBL:L459-485].
    fn new(customer: &Customer, account: &Account) -> Self {
        let name = string_delimited_by_space(&[
            &customer.cust_first_name.as_text(),
            &customer.cust_middle_name.as_text(),
            &customer.cust_last_name.as_text(),
        ]);
        let addr3 = string_delimited_by_space(&[
            &customer.cust_addr_line_3.as_text(),
            &customer.cust_addr_state_cd.as_text(),
            &customer.cust_addr_country_cd.as_text(),
            &customer.cust_addr_zip.as_text(),
        ]);
        Self {
            name,
            addr1: customer.cust_addr_line_1.as_text().into_owned(),
            addr2: customer.cust_addr_line_2.as_text().into_owned(),
            addr3,
            account_id: account.acct_id.get(),
            curr_bal: account.acct_curr_bal,
            fico_score: customer.cust_fico_credit_score.get(),
        }
    }
}

/// CBSTM03A 5100-WRITE-HTML-HEADER — P-CBSTM03A-009, P-CBSTM03A-027 to
/// P-CBSTM03A-047, P-CBSTM03A-110: HTML lines 1 to 22, the document head, the
/// account number heading and the bank address [app/cbl/CBSTM03A.CBL:L506-555].
fn html_header(html_lines: &mut Vec<String>, fields: &HeaderFields) {
    push_fixed(html_lines, &HTML_HEADER_OPEN);
    html_lines.push(format::html_l11(fields.account_id));
    push_fixed(html_lines, &HTML_HEADER_BANK);
}

/// CBSTM03A 5200-WRITE-HTML-NMADBS — P-CBSTM03A-010, P-CBSTM03A-048 to
/// P-CBSTM03A-074, P-CBSTM03A-111 to P-CBSTM03A-117: HTML lines 23 to 56, the
/// name, the address, the basic details and the transaction column headings
/// [app/cbl/CBSTM03A.CBL:L558-672].
fn html_name_address_basics(html_lines: &mut Vec<String>, fields: &HeaderFields) {
    html_lines.extend([
        format::html_l23(&fields.name),
        format::html_add1_ln(&fields.addr1),
        format::html_add2_ln(&fields.addr2),
        format::html_add3_ln(&fields.addr3),
    ]);
    push_fixed(html_lines, &HTML_BASIC_DETAILS_OPEN);
    html_lines.extend([
        format::html_acct_ln(fields.account_id),
        format::html_bal_ln(fields.curr_bal),
        format::html_fico_ln(fields.fico_score),
    ]);
    push_fixed(html_lines, &HTML_SUMMARY_HEADINGS);
}

/// CBSTM03A 5000-CREATE-STATEMENT — P-CBSTM03A-006, P-CBSTM03A-016 to
/// P-CBSTM03A-024, P-CBSTM03A-095 to P-CBSTM03A-101: the 16 header lines of
/// `STMTFILE` (`ST-LINE0`, then `ST-LINE1` to `ST-LINE13` with `ST-LINE5` and
/// `ST-LINE12` written twice) and, through `5100` and `5200`, the 56 header
/// lines of `HTMLFILE` [app/cbl/CBSTM03A.CBL:L458-504].
fn statement_header(output: &mut StatementOutput, customer: &Customer, account: &Account) {
    let fields = HeaderFields::new(customer, account);
    output.stmt_lines.push(format::st_line0());
    html_header(&mut output.html_lines, &fields);
    html_name_address_basics(&mut output.html_lines, &fields);
    output.stmt_lines.extend([
        format::st_line1(&fields.name),
        format::st_line2(&fields.addr1),
        format::st_line3(&fields.addr2),
        format::st_line4(&fields.addr3),
        format::st_line5(),
        format::st_line6(),
        format::st_line5(),
        format::st_line7(fields.account_id),
        format::st_line8(fields.curr_bal),
        format::st_line9(fields.fico_score),
        format::st_line10(),
        format::st_line11(),
        format::st_line12(),
        format::st_line13(),
        format::st_line12(),
    ]);
}

/// CBSTM03A 6000-WRITE-TRANS — P-CBSTM03A-007, P-CBSTM03A-075 to
/// P-CBSTM03A-082, P-CBSTM03A-091 to P-CBSTM03A-093, P-CBSTM03A-102: one
/// `ST-LINE14` and the 11 HTML lines of the transaction's table row
/// [app/cbl/CBSTM03A.CBL:L675-723].
fn write_trans(output: &mut StatementOutput, tran: &StatementTransaction) {
    let tran_id = tran.tran_id.as_alnum().as_text();
    let details = tran.description.as_text();
    output
        .stmt_lines
        .push(format::st_line14(&tran_id, &details, tran.amount));
    let html_lines = &mut output.html_lines;
    push_fixed(html_lines, &[HtmlFixedLine::Ltrs, HtmlFixedLine::L58]);
    html_lines.push(format::html_tranid_ln(&tran_id));
    push_fixed(html_lines, &[HtmlFixedLine::Ltde, HtmlFixedLine::L61]);
    html_lines.push(format::html_trandt_ln(&details));
    push_fixed(html_lines, &[HtmlFixedLine::Ltde, HtmlFixedLine::L64]);
    html_lines.push(format::html_tranamt_ln(tran.amount));
    push_fixed(html_lines, &[HtmlFixedLine::Ltde, HtmlFixedLine::Ltre]);
}

/// CBSTM03A 4000-TRNXFILE-GET — P-CBSTM03A-008, P-CBSTM03A-012: writes every
/// transaction the table holds for `card` and returns their total.
///
/// `ADD TRNX-AMT TO WS-TOTAL-AMT` stores each sum into `WS-TOTAL-AMT COMP-3
/// S9(9)V99` without `ON SIZE ERROR`, so high-order digits beyond nine are
/// dropped at every addition [app/cbl/CBSTM03A.CBL:L429] [app/cbl/CBSTM03A.CBL:L65].
/// The total starts at zero for every statement, `MOVE ZERO TO WS-TOTAL-AMT`
/// [app/cbl/CBSTM03A.CBL:L325].
fn card_transactions(
    output: &mut StatementOutput,
    table: &TransactionTable,
    card: CardNumber,
) -> Money<9> {
    let mut total = Money::<9>::ZERO;
    for tran in table.transactions_of(card) {
        write_trans(output, tran);
        total = (total + tran.amount).store::<9>();
    }
    total
}

/// CBSTM03A 4000-TRNXFILE-GET — P-CBSTM03A-008, P-CBSTM03A-025, P-CBSTM03A-026,
/// P-CBSTM03A-083 to P-CBSTM03A-090, P-CBSTM03A-109: the closing lines after
/// the card's transactions, `ST-LINE12`, `ST-LINE14A` with `total` moved
/// through `WS-TRN-AMT S9(9)V99` into `ST-TOTAL-TRAMT`, `ST-LINE15`, and the 8
/// closing HTML lines [app/cbl/CBSTM03A.CBL:L433-454].
fn totals(output: &mut StatementOutput, total: Money<9>) {
    output.stmt_lines.extend([
        format::st_line12(),
        format::st_line14a(total),
        format::st_line15(),
    ]);
    push_fixed(&mut output.html_lines, &HTML_STATEMENT_CLOSE);
}

/// CBSTM03A 8100-TRNXFILE-OPEN and 8500-READTRNX-READ — P-CBSTM03A-003,
/// P-CBSTM03A-004, P-CBSTM03A-014, P-CBSTM03A-1256: loads the transaction file
/// into the card and transaction tables [app/cbl/CBSTM03A.CBL:L744-853].
///
/// `transactions` are the records of the `STEP010` sort, card number then
/// TRAN-ID (P-CREASTMT-002). A record whose card number equals the previous
/// record's takes the next transaction slot of the current card; any other
/// record opens the next card slot. The grouping is by run, as the COBOL
/// compares each record with `WS-SAVE-CARD` only.
///
/// At most [`TRNXFILE_READ_LIMIT`] records are taken from `transactions`: the
/// result over the first that many records is the result over the whole file,
/// so the caller need not read past them.
///
/// # Errors
///
/// - [`StatementError::ReadFailed`] with `ERROR READING TRNXFILE` and return
///   code `10` when there is no record: the first read at open meets the end
///   of the file, which `8100-TRNXFILE-OPEN` treats as an error, so no
///   statement is produced (P-CBSTM03A-003).
/// - [`StatementError::CapacityExceeded`] at the first record that needs a
///   card slot beyond [`CARD_CAPACITY`] or a transaction slot beyond
///   [`TRANSACTIONS_PER_CARD`] in its card slot. The COBOL result is undefined
///   there, so the table is not built and no further record is taken from
///   `transactions`: the counts describe the records up to that one
///   (escalation [`CAPACITY_ESCALATION`]).
pub fn load_table(
    transactions: impl IntoIterator<Item = Transaction>,
) -> Result<TransactionTable, StatementError> {
    let mut cards: Vec<(CardNumber, Vec<StatementTransaction>)> = Vec::new();
    for tran in transactions {
        let slot = StatementTransaction::from_transaction(&tran);
        let filled = cards.len();
        match cards.last_mut() {
            Some((card, slots)) if *card == slot.card_num => {
                if slots.len() == TRANSACTIONS_PER_CARD {
                    return Err(StatementError::CapacityExceeded {
                        cards: filled,
                        max_per_card: TRANSACTIONS_PER_CARD + 1,
                    });
                }
                slots.push(slot);
            }
            _ => {
                if filled == CARD_CAPACITY {
                    // The new slot holds this one record.
                    let max_per_card = cards
                        .iter()
                        .map(|(_, slots)| slots.len())
                        .fold(1, usize::max);
                    return Err(StatementError::CapacityExceeded {
                        cards: CARD_CAPACITY + 1,
                        max_per_card,
                    });
                }
                cards.push((slot.card_num, vec![slot]));
            }
        }
    }
    if cards.is_empty() {
        return Err(StatementError::ReadFailed {
            file: ERROR_READING_TRNXFILE,
            rc: RC_END_OF_FILE.to_owned(),
            cause: None,
        });
    }
    Ok(TransactionTable { cards })
}

/// CBSTM03A 1000-MAINLINE with 2000-CUSTFILE-GET and 3000-ACCTFILE-GET —
/// P-CBSTM03A-005, P-CBSTM03A-015, P-CBSTM03A-1257 to P-CBSTM03A-1259,
/// P-CBSTM03B-004, P-CBSTM03B-005: one statement per cross-reference record,
/// in the order of `xrefs` [app/cbl/CBSTM03A.CBL:L316-414].
///
/// `xrefs` are the records of the card cross-reference file in card-number
/// order, as `1000-XREFFILE-GET-NEXT` reads them (P-CBSTM03B-003). For each,
/// the customer is read by `XREF-CUST-ID`, then the account by
/// `XREF-ACCT-ID`, and the statement is written: the header, the card's
/// transactions from `table` and the closing lines with the card total. A card
/// with no transaction gets the header and a total of zero. Each statement has
/// `19 + n` `STMTFILE` lines and `64 + 11n` `HTMLFILE` lines for its `n`
/// transactions.
///
/// The returned future is `Send` whenever `S` is.
///
/// # Errors
///
/// [`StatementError::ReadFailed`] when a customer or account read fails,
/// `NOTFND` included, with `ERROR READING CUSTFILE` or `ERROR READING
/// ACCTFILE` and the return code: `23` for a missing record, `30` for any
/// other failure. The COBOL abends there, so no output is returned
/// (P-CBSTM03A-1258, P-CBSTM03A-1259).
pub async fn create_statements<S: CustomerReader + AccountReader>(
    store: &mut S,
    xrefs: Vec<CardXref>,
    table: &TransactionTable,
) -> Result<StatementOutput, StatementError> {
    let mut output = StatementOutput::default();
    for xref in xrefs {
        let customer = store
            .read_customer(xref.xref_cust_id)
            .await
            .map_err(|error| StatementError::read_failed(ERROR_READING_CUSTFILE, error))?;
        let account = store
            .read_account(xref.xref_acct_id)
            .await
            .map_err(|error| StatementError::read_failed(ERROR_READING_ACCTFILE, error))?;
        statement_header(&mut output, &customer, &account);
        let total = card_transactions(&mut output, table, xref.xref_card_num);
        totals(&mut output, total);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::error::Error as _;
    use std::future::Future;

    use super::{
        CAPACITY_ESCALATION, CARD_CAPACITY, StatementError, StatementOutput, TRANSACTIONS_PER_CARD,
        TRNXFILE_READ_LIMIT, TransactionTable, create_statements, load_table,
    };
    use crate::entities::{Account, CardXref, Customer, NewTransaction, Transaction};
    use crate::format::HtmlFixedLine::{
        L01, L02, L03, L04, L05, L06, L07, L08, L10, L15, L16, L17, L18, L22And35, L30And42, L31,
        L43, L47, L48, L50, L51, L53, L54, L58, L61, L64, L75, L78, L79, L80, Ltde, Ltre, Ltrs,
    };
    use crate::format::{HTML_RECORD_LEN, HtmlFixedLine, STMT_RECORD_LEN};
    use crate::messages::{
        ABENDING_PROGRAM, ERROR_READING_ACCTFILE, ERROR_READING_CUSTFILE, ERROR_READING_TRNXFILE,
        ERROR_READING_XREFFILE,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustSsn, CustomerId, FicoScore, MerchantId,
        Money, TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The first cross-reference card of the fixtures, account and customer 50
    /// (P-CBSTM03A-094).
    const CARD_A: &str = "0500024453765740";
    /// The second cross-reference card of the fixtures (P-CBSTM03A-118).
    const CARD_B: &str = "0683586198171516";

    fn pad(text: &str, width: usize) -> String {
        format!("{text:width$}")
    }

    fn stmt(text: &str) -> String {
        pad(text, STMT_RECORD_LEN)
    }

    fn html(text: &str) -> String {
        pad(text, HTML_RECORD_LEN)
    }

    fn fixed(lines: &[HtmlFixedLine]) -> Vec<String> {
        lines.iter().map(|line| line.record()).collect()
    }

    fn cents(value: i64) -> Money<9> {
        Money::from_cents(value).expect("an amount within S9(09)V99")
    }

    fn card(number: &str) -> CardNumber {
        CardNumber::move_from(number)
    }

    /// Customer 50 of `custdata.txt` (P-CBSTM03A-095 to P-CBSTM03A-101).
    fn customer_50() -> Customer {
        Customer {
            cust_id: CustomerId::new(50).expect("a 9-digit customer number"),
            cust_first_name: Alnum::move_from("Aniya"),
            cust_middle_name: Alnum::move_from("Alba"),
            cust_last_name: Alnum::move_from("Von"),
            cust_addr_line_1: Alnum::move_from("1588 Nienow Cape"),
            cust_addr_line_2: Alnum::move_from("Suite 187"),
            cust_addr_line_3: Alnum::move_from("New Aricchester"),
            cust_addr_state_cd: Alnum::move_from("OR"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("04257"),
            cust_phone_num_1: Alnum::move_from("(325)301-0827"),
            cust_phone_num_2: Alnum::move_from("(493)985-9283"),
            cust_ssn: CustSsn::new(931_248_469).expect("a 9-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000303878241"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1960-12-01"),
            cust_eft_account_id: Alnum::move_from("0000000000"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(623).expect("a 3-digit score"),
        }
    }

    /// Account 50 as the README cycle leaves it (P-CBSTM03A-099,
    /// P-CBSTM03A-100).
    fn account_50() -> Account {
        Account {
            acct_id: AccountId::new(50).expect("an 11-digit account number"),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::from_cents(194_587).expect("an amount within S9(10)V99"),
            acct_credit_limit: Money::from_cents(616_900).expect("an amount within S9(10)V99"),
            acct_cash_credit_limit: Money::from_cents(458_700).expect("an amount within S9(10)V99"),
            acct_open_date: Alnum::move_from("2011-04-22"),
            acct_expiraion_date: Alnum::move_from("2023-03-09"),
            acct_reissue_date: Alnum::move_from("2023-03-09"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn xref(card_number: &str, id: u32) -> CardXref {
        CardXref {
            xref_card_num: card(card_number),
            xref_cust_id: CustomerId::new(id).expect("a 9-digit customer number"),
            xref_acct_id: AccountId::new(u64::from(id)).expect("an 11-digit account number"),
        }
    }

    fn tran(card_number: &str, tran_id: &str, description: &str, amount: i64) -> Transaction {
        Transaction::from_new(
            TransactionRowId::from(1),
            NewTransaction {
                tran_id: TranId::move_from(tran_id),
                tran_type_cd: TranTypeCode::move_from("01"),
                tran_cat_cd: TranCategoryCode::new(1).expect("a 4-digit category"),
                tran_source: Alnum::move_from("POS TERM"),
                tran_desc: Alnum::move_from(description),
                tran_amt: cents(amount),
                tran_merchant_id: MerchantId::new(800_000_000).expect("a 9-digit merchant"),
                tran_merchant_name: Alnum::move_from("Blick-Rippin"),
                tran_merchant_city: Alnum::move_from("North Enoch"),
                tran_merchant_zip: Alnum::move_from("56398"),
                tran_card_num: card(card_number),
                tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
                tran_proc_ts: Alnum::move_from("2022-07-18 00:00:00.000000"),
            },
        )
    }

    /// Two transactions of card A from `dailytran.txt` lines 21 and 184
    /// (P-CBSTM03A-102, P-CBSTM03A-105).
    fn card_a_transactions() -> Vec<Transaction> {
        vec![
            tran(
                CARD_A,
                "0000000058866561",
                "Purchase at Blick-Rippin",
                18_388,
            ),
            tran(
                CARD_A,
                "0000000577826814",
                "Return item at DuBuque, Wuckert and Mraz",
                -4_788,
            ),
        ]
    }

    fn store_50() -> FakeStore {
        FakeStore::default()
            .with_customer(customer_50())
            .with_account(account_50())
    }

    fn table(transactions: Vec<Transaction>) -> TransactionTable {
        load_table(transactions).expect("a table within the CBSTM03A capacity")
    }

    fn run(
        store: &mut FakeStore,
        xrefs: Vec<CardXref>,
        table: &TransactionTable,
    ) -> Result<StatementOutput, StatementError> {
        block_on(require_send(create_statements(store, xrefs, table)))
    }

    /// Fails to compile unless `future` is `Send`, as the multi-threaded batch
    /// runtime needs.
    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    /// The 16 `STMTFILE` header lines of customer 50 and account 50
    /// (P-CBSTM03A-016 to P-CBSTM03A-024, P-CBSTM03A-095 to P-CBSTM03A-101).
    fn expected_stmt_header() -> Vec<String> {
        let rule = "-".repeat(STMT_RECORD_LEN);
        vec![
            stmt(
                "*******************************START OF STATEMENT*******************************",
            ),
            stmt("Aniya Alba Von"),
            stmt("1588 Nienow Cape"),
            stmt("Suite 187"),
            stmt("New OR USA 04257"),
            rule.clone(),
            stmt("                                 Basic Details"),
            rule.clone(),
            stmt("Account ID         :00000000050"),
            stmt("Current Balance    :000001945.87"),
            stmt("FICO Score         :623"),
            rule.clone(),
            stmt("                              TRANSACTION SUMMARY"),
            rule.clone(),
            stmt(
                "Tran ID         Tran Details                                         Tran Amount",
            ),
            rule,
        ]
    }

    /// The 3 closing `STMTFILE` lines with the edited total `edited_total`
    /// (P-CBSTM03A-025, P-CBSTM03A-026, P-CBSTM03A-109).
    fn expected_stmt_close(edited_total: &str) -> Vec<String> {
        vec![
            "-".repeat(STMT_RECORD_LEN),
            format!("Total EXP:{:56}${edited_total}", ""),
            stmt(
                "********************************END OF STATEMENT********************************",
            ),
        ]
    }

    /// The 56 `HTMLFILE` header lines of customer 50 and account 50
    /// (P-CBSTM03A-027 to P-CBSTM03A-074, P-CBSTM03A-110 to P-CBSTM03A-117).
    fn expected_html_header() -> Vec<String> {
        let mut lines = fixed(&[L01, L02, L03, L04, L05, L06, L07, L08, Ltrs, L10]);
        lines.push(html(
            "<h3>Statement for Account Number: 00000000050         </h3>",
        ));
        lines.extend(fixed(&[
            Ltde, Ltre, Ltrs, L15, L16, L17, L18, Ltde, Ltre, Ltrs, L22And35,
        ]));
        lines.extend([
            html("<p style=\"font-size:16px\">Aniya Alba Von  </p>"),
            html("<p>1588 Nienow Cape  </p>"),
            html("<p>Suite 187  </p>"),
            html("<p>New OR USA 04257  </p>"),
        ]);
        lines.extend(fixed(&[
            Ltde, Ltre, Ltrs, L30And42, L31, Ltde, Ltre, Ltrs, L22And35,
        ]));
        lines.extend([
            html("<p>Account ID         : 00000000050         </p>"),
            html("<p>Current Balance    : 000001945.87 </p>"),
            html("<p>FICO Score         : 623                 </p>"),
        ]);
        lines.extend(fixed(&[
            Ltde, Ltre, Ltrs, L30And42, L43, Ltde, Ltre, Ltrs, L47, L48, Ltde, L50, L51, Ltde, L53,
            L54, Ltde, Ltre,
        ]));
        lines
    }

    /// The 11 `HTMLFILE` lines of one transaction row (P-CBSTM03A-075 to
    /// P-CBSTM03A-082, P-CBSTM03A-091 to P-CBSTM03A-093).
    fn expected_html_row(tran_id: &str, details: &str, edited_amount: &str) -> Vec<String> {
        let mut lines = fixed(&[Ltrs, L58]);
        lines.push(html(&format!("<p>{tran_id}</p>")));
        lines.extend(fixed(&[Ltde, L61]));
        lines.push(html(&format!("<p>{details:49}</p>")));
        lines.extend(fixed(&[Ltde, L64]));
        lines.push(html(&format!("<p>{edited_amount}</p>")));
        lines.extend(fixed(&[Ltde, Ltre]));
        lines
    }

    /// The 8 closing `HTMLFILE` lines (P-CBSTM03A-083 to P-CBSTM03A-090).
    fn expected_html_close() -> Vec<String> {
        fixed(&[Ltrs, L10, L75, Ltde, Ltre, L78, L79, L80])
    }

    fn assert_record_widths(output: &StatementOutput) {
        for line in &output.stmt_lines {
            assert_eq!(line.len(), STMT_RECORD_LEN, "{line:?}");
        }
        for line in &output.html_lines {
            assert_eq!(line.len(), HTML_RECORD_LEN, "{line:?}");
        }
    }

    #[test]
    fn fixture_card_statement_is_the_audited_text_and_html() {
        let mut store = store_50();
        let table = table(card_a_transactions());
        let output = run(&mut store, vec![xref(CARD_A, 50)], &table).expect("statements");

        let mut expected_stmt = expected_stmt_header();
        expected_stmt.extend([
            stmt("0000000058866561 Purchase at Blick-Rippin                         $      183.88"),
            stmt(
                "0000000577826814 Return item at DuBuque, Wuckert and Mraz         $       47.88-",
            ),
        ]);
        expected_stmt.extend(expected_stmt_close("      136.00 "));
        assert_eq!(output.stmt_lines, expected_stmt);

        let mut expected_html = expected_html_header();
        expected_html.extend(expected_html_row(
            "0000000058866561",
            "Purchase at Blick-Rippin",
            "      183.88 ",
        ));
        expected_html.extend(expected_html_row(
            "0000000577826814",
            "Return item at DuBuque, Wuckert and Mraz",
            "       47.88-",
        ));
        expected_html.extend(expected_html_close());
        assert_eq!(output.html_lines, expected_html);

        assert_record_widths(&output);
        assert_eq!(store.calls, ["read_customer", "read_account"]);
    }

    #[test]
    fn card_without_transactions_gets_the_header_and_a_zero_total() {
        let mut store = store_50();
        let table = table(vec![tran(CARD_B, "0000000130111733", "Purchase", 77_733)]);
        let output = run(&mut store, vec![xref(CARD_A, 50)], &table).expect("statements");

        let mut expected_stmt = expected_stmt_header();
        expected_stmt.extend(expected_stmt_close("         .00 "));
        assert_eq!(output.stmt_lines, expected_stmt);
        let mut expected_html = expected_html_header();
        expected_html.extend(expected_html_close());
        assert_eq!(output.html_lines, expected_html);
        assert_record_widths(&output);
    }

    #[test]
    fn statements_follow_the_cross_references_with_19_plus_n_and_64_plus_11n_lines() {
        let mut store = store_50();
        let table = table(card_a_transactions());
        let xrefs = vec![xref(CARD_A, 50), xref(CARD_B, 50)];
        let output = run(&mut store, xrefs, &table).expect("statements");

        assert_eq!(output.stmt_lines.len(), (19 + 2) + 19);
        assert_eq!(output.html_lines.len(), (64 + 11 * 2) + 64);
        assert_eq!(
            output.stmt_lines[21], output.stmt_lines[0],
            "a second header"
        );
        assert_eq!(
            output.html_lines[86], output.html_lines[0],
            "a second header"
        );
        assert_eq!(
            store.calls,
            [
                "read_customer",
                "read_account",
                "read_customer",
                "read_account"
            ]
        );
        assert_record_widths(&output);
    }

    /// `cards` card numbers of `per_card` transactions each, in sort order,
    /// each record made only when it is pulled, so a corpus may be larger
    /// than memory.
    fn corpus(cards: usize, per_card: usize) -> impl Iterator<Item = Transaction> {
        (0..cards).flat_map(move |card_index| {
            (0..per_card).map(move |tran_index| {
                tran(
                    &format!("{card_index:016}"),
                    &format!("{tran_index:016}"),
                    "Purchase",
                    100,
                )
            })
        })
    }

    #[test]
    fn load_table_holds_51_cards_of_10_and_halts_beyond_either_limit() {
        /// A corpus of `cards` cards of `per_card` records, the counts of its
        /// `CapacityExceeded` and the records the load pulls before it.
        struct Case {
            cards: usize,
            per_card: usize,
            halt_cards: usize,
            halt_per_card: usize,
            pulled: usize,
        }
        let full = load_table(corpus(CARD_CAPACITY, TRANSACTIONS_PER_CARD))
            .expect("the tables hold 51 cards of 10 transactions");
        assert_eq!(full.cards.len(), CARD_CAPACITY);
        assert!(
            full.cards
                .iter()
                .all(|(_, slots)| slots.len() == TRANSACTIONS_PER_CARD)
        );

        let beyond = [
            // The 52nd card opens a slot past WS-CARD-TBL.
            Case {
                cards: CARD_CAPACITY + 1,
                per_card: 1,
                halt_cards: 52,
                halt_per_card: 1,
                pulled: 52,
            },
            // The 11th transaction of the first card has no slot.
            Case {
                cards: 1,
                per_card: TRANSACTIONS_PER_CARD + 1,
                halt_cards: 1,
                halt_per_card: 11,
                pulled: 11,
            },
            // Beyond both limits, the first card's 11th record halts the load
            // before any later card is read.
            Case {
                cards: CARD_CAPACITY + 1,
                per_card: TRANSACTIONS_PER_CARD + 1,
                halt_cards: 1,
                halt_per_card: 11,
                pulled: 11,
            },
            // 51 full cards are held; the first record of the 52nd halts.
            Case {
                cards: CARD_CAPACITY + 1,
                per_card: TRANSACTIONS_PER_CARD,
                halt_cards: 52,
                halt_per_card: 10,
                pulled: 511,
            },
            // Endless inputs end at the first record beyond the tables.
            Case {
                cards: usize::MAX,
                per_card: 1,
                halt_cards: 52,
                halt_per_card: 1,
                pulled: 52,
            },
            Case {
                cards: 1,
                per_card: usize::MAX,
                halt_cards: 1,
                halt_per_card: 11,
                pulled: 11,
            },
        ];
        for case in beyond {
            let pulled = Cell::new(0_usize);
            let records =
                corpus(case.cards, case.per_card).inspect(|_| pulled.set(pulled.get() + 1));
            let error = load_table(records).expect_err("beyond the tables");
            let label = format!("{} cards of {}", case.cards, case.per_card);
            assert_eq!(
                error,
                StatementError::CapacityExceeded {
                    cards: case.halt_cards,
                    max_per_card: case.halt_per_card,
                },
                "{label}"
            );
            assert_eq!(
                error.to_string(),
                format!(
                    "TRNXFILE needs at least {} card slots and at least {} transaction slots for \
                     one card; the CBSTM03A tables hold 51 cards and 10 transactions per card",
                    case.halt_cards, case.halt_per_card
                ),
                "{label}"
            );
            assert_eq!(pulled.get(), case.pulled, "records pulled from {label}");
        }
        assert_eq!(CAPACITY_ESCALATION, "E-33");
    }

    #[test]
    fn the_trnxfile_read_limit_is_one_record_past_the_tables() {
        assert_eq!(
            usize::from(TRNXFILE_READ_LIMIT),
            CARD_CAPACITY * TRANSACTIONS_PER_CARD + 1
        );
    }

    #[test]
    fn the_first_records_up_to_the_read_limit_decide_the_load() {
        /// The records of one card per entry of `per_card`, in sort order,
        /// each card with the entry's count of transactions.
        fn records(per_card: &[usize]) -> Vec<Transaction> {
            per_card
                .iter()
                .enumerate()
                .flat_map(|(card_index, &count)| {
                    (0..count).map(move |tran_index| {
                        tran(
                            &format!("{card_index:016}"),
                            &format!("{tran_index:016}"),
                            "Purchase",
                            100,
                        )
                    })
                })
                .collect()
        }
        let limit = usize::from(TRNXFILE_READ_LIMIT);
        let cases: [(&str, Vec<usize>); 10] = [
            ("an empty file", Vec::new()),
            ("one record", vec![1]),
            ("51 cards of 10", vec![TRANSACTIONS_PER_CARD; CARD_CAPACITY]),
            ("1 card of 10", vec![TRANSACTIONS_PER_CARD]),
            ("52 cards of 1", vec![1; CARD_CAPACITY + 1]),
            ("1 card of 11", vec![TRANSACTIONS_PER_CARD + 1]),
            (
                "52 cards of 10",
                vec![TRANSACTIONS_PER_CARD; CARD_CAPACITY + 1],
            ),
            (
                "51 cards, the last of 11",
                [
                    vec![TRANSACTIONS_PER_CARD; CARD_CAPACITY - 1],
                    vec![TRANSACTIONS_PER_CARD + 1],
                ]
                .concat(),
            ),
            ("1000 cards of 1", vec![1; 1000]),
            ("1 card of 1000", vec![1000]),
        ];
        for (label, per_card) in cases {
            let file = records(&per_card);
            let whole = load_table(file.clone());
            let bounded = load_table(file.iter().take(limit).cloned());
            assert_eq!(bounded, whole, "{label}");
            if file.len() > limit {
                assert!(
                    matches!(whole, Err(StatementError::CapacityExceeded { .. })),
                    "{label}: a file past the limit halts, {whole:?}"
                );
            }
        }
    }

    #[test]
    fn an_empty_transaction_file_abends_at_its_first_read() {
        let error = load_table(Vec::new()).expect_err("an empty TRNXFILE");
        assert_eq!(
            error,
            StatementError::ReadFailed {
                file: ERROR_READING_TRNXFILE,
                rc: "10".to_owned(),
                cause: None,
            }
        );
        assert!(
            error.source().is_none(),
            "the end of file is no port failure"
        );
        let lines: Vec<String> = error.to_string().lines().map(str::to_owned).collect();
        assert_eq!(
            lines,
            [ERROR_READING_TRNXFILE, "RETURN CODE: 10", ABENDING_PROGRAM]
        );
    }

    #[test]
    fn records_group_by_run_and_the_scan_stops_at_the_first_greater_card() {
        // Card A appears again after card B: the run grouping gives it a
        // third slot, which the 4000 scan never reaches for card A.
        let loaded = table(vec![
            tran(CARD_A, "0000000000000001", "First", 100),
            tran(CARD_A, "0000000000000002", "Second", 200),
            tran(CARD_B, "0000000000000003", "Third", 300),
            tran(CARD_A, "0000000000000004", "Fourth", 400),
        ]);
        let slots: Vec<(String, usize)> = loaded
            .cards
            .iter()
            .map(|(number, slots)| (number.to_string(), slots.len()))
            .collect();
        assert_eq!(
            slots,
            [
                (CARD_A.to_owned(), 2),
                (CARD_B.to_owned(), 1),
                (CARD_A.to_owned(), 1)
            ]
        );

        let ids_of = |number: &str| -> Vec<String> {
            loaded
                .transactions_of(card(number))
                .map(|slot| slot.tran_id.to_string())
                .collect()
        };
        assert_eq!(ids_of(CARD_A), ["0000000000000001", "0000000000000002"]);
        assert_eq!(ids_of(CARD_B), ["0000000000000003"]);
        assert!(ids_of("0000000000000000").is_empty());
        assert!(ids_of("9999999999999999").is_empty());
    }

    #[test]
    fn the_card_total_drops_high_order_digits_at_each_addition() {
        let mut store = store_50();
        let table = table(vec![
            tran(CARD_A, "0000000000000001", "Large", 99_999_999_999),
            tran(CARD_A, "0000000000000002", "Small", 2),
        ]);
        let output = run(&mut store, vec![xref(CARD_A, 50)], &table).expect("statements");
        // 999999999.99 + 0.02 = 1000000000.01 stored into S9(9)V99 is 0.01.
        assert_eq!(
            output.stmt_lines[16 + 2 + 1],
            format!("Total EXP:{:56}${:>12} ", "", ".01")
        );
    }

    #[test]
    fn names_and_addresses_keep_each_field_up_to_its_first_space() {
        struct Case {
            first: &'static str,
            middle: &'static str,
            last: &'static str,
            line_3: &'static str,
            zip: &'static str,
            st_line1: String,
            st_line4: String,
            html_name: String,
            html_addr3: String,
        }
        // A name that fills its X(25) field.
        const FULL: &str = "AAAAAAAAAAAAAAAAAAAAAAAAA";
        assert_eq!(FULL.len(), 25);
        let cases = [
            // An empty middle name leaves two spaces, which end the HTML name
            // (`DELIMITED BY '  '`) after the first name.
            Case {
                first: "Mary Ann",
                middle: "",
                last: "Smith Jones",
                line_3: "Port Moresby",
                zip: "07923 8822",
                st_line1: stmt("Mary  Smith"),
                st_line4: stmt("Port OR USA 07923"),
                html_name: html("<p style=\"font-size:16px\">Mary  </p>"),
                html_addr3: html("<p>Port OR USA 07923  </p>"),
            },
            // Three full names give 78 bytes: ST-NAME X(75) keeps 75 and
            // L23-NAME X(50) keeps 50, with no double space to stop at. Blank
            // fields still add their separating space.
            Case {
                first: FULL,
                middle: FULL,
                last: FULL,
                line_3: "",
                zip: "",
                st_line1: stmt(&format!("{FULL} {FULL} {}", &FULL[..23])),
                st_line4: stmt(" OR USA"),
                html_name: html(&format!(
                    "<p style=\"font-size:16px\">{FULL} {}  </p>",
                    &FULL[..24]
                )),
                html_addr3: html("<p> OR USA  </p>"),
            },
        ];
        for case in cases {
            let customer = Customer {
                cust_first_name: Alnum::move_from(case.first),
                cust_middle_name: Alnum::move_from(case.middle),
                cust_last_name: Alnum::move_from(case.last),
                cust_addr_line_3: Alnum::move_from(case.line_3),
                cust_addr_zip: Alnum::move_from(case.zip),
                ..customer_50()
            };
            let mut store = FakeStore::default()
                .with_customer(customer)
                .with_account(account_50());
            let table = table(card_a_transactions());
            let output = run(&mut store, vec![xref(CARD_A, 50)], &table).expect("statements");
            assert_eq!(output.stmt_lines[1], case.st_line1, "{}", case.first);
            assert_eq!(output.stmt_lines[4], case.st_line4, "{}", case.first);
            assert_eq!(output.html_lines[22], case.html_name, "{}", case.first);
            assert_eq!(output.html_lines[25], case.html_addr3, "{}", case.first);
            assert_record_widths(&output);
        }
    }

    #[test]
    fn a_description_is_shown_cut_to_49_bytes() {
        let description = format!("{}{}", "D".repeat(49), "beyond ST-TRANDT");
        let mut store = store_50();
        let table = table(vec![tran(CARD_A, "0000000000000001", &description, 100)]);
        let output = run(&mut store, vec![xref(CARD_A, 50)], &table).expect("statements");
        let shown = "D".repeat(49);
        assert_eq!(
            output.stmt_lines[16],
            format!("0000000000000001 {shown}${:>12} ", "1.00")
        );
        assert_eq!(output.html_lines[56 + 5], html(&format!("<p>{shown}</p>")));
    }

    #[test]
    fn a_failed_customer_or_account_read_abends_without_output() {
        let cases = [
            (
                "read_customer",
                PortError::NotFound,
                ERROR_READING_CUSTFILE,
                "23",
                "record not found",
                vec!["read_customer"],
            ),
            (
                "read_customer",
                PortError::Unavailable("connection reset".to_owned()),
                ERROR_READING_CUSTFILE,
                "30",
                "store unavailable: connection reset",
                vec!["read_customer"],
            ),
            (
                "read_account",
                PortError::NotFound,
                ERROR_READING_ACCTFILE,
                "23",
                "record not found",
                vec!["read_customer", "read_account"],
            ),
            (
                "read_account",
                PortError::Unavailable("connection reset".to_owned()),
                ERROR_READING_ACCTFILE,
                "30",
                "store unavailable: connection reset",
                vec!["read_customer", "read_account"],
            ),
        ];
        for (method, port_error, file, rc, source, calls) in cases {
            let mut store = store_50();
            store.fail_next(method, port_error.clone());
            let table = table(card_a_transactions());
            let error = run(&mut store, vec![xref(CARD_A, 50)], &table).expect_err("an abend");
            assert_eq!(
                error,
                StatementError::ReadFailed {
                    file,
                    rc: rc.to_owned(),
                    cause: Some(port_error),
                },
                "{method}"
            );
            assert_eq!(
                error.source().map(ToString::to_string).as_deref(),
                Some(source),
                "{method}"
            );
            assert_eq!(
                error.to_string(),
                format!("{file}\nRETURN CODE: {rc}\n{ABENDING_PROGRAM}")
            );
            assert!(!error.to_string().contains(source), "{method}");
            assert_eq!(store.calls, calls, "{method}");
        }
    }

    #[test]
    fn a_missing_customer_of_a_later_card_ends_the_run() {
        let mut store = store_50();
        let table = table(card_a_transactions());
        let xrefs = vec![xref(CARD_A, 50), xref(CARD_B, 27)];
        let error = run(&mut store, xrefs, &table).expect_err("customer 27 is missing");
        assert_eq!(
            error,
            StatementError::ReadFailed {
                file: ERROR_READING_CUSTFILE,
                rc: "23".to_owned(),
                cause: Some(PortError::NotFound),
            }
        );
    }

    #[test]
    fn job_side_reads_report_their_file_and_return_code() {
        let unavailable = PortError::Unavailable("timeout".to_owned());
        let cases = [
            (
                StatementError::trnxfile_read_failed(&unavailable),
                ERROR_READING_TRNXFILE,
                "30",
                unavailable.clone(),
                "store unavailable: timeout",
            ),
            (
                StatementError::xreffile_read_failed(&unavailable),
                ERROR_READING_XREFFILE,
                "30",
                unavailable.clone(),
                "store unavailable: timeout",
            ),
            (
                StatementError::xreffile_read_failed(&PortError::NotFound),
                ERROR_READING_XREFFILE,
                "23",
                PortError::NotFound,
                "record not found",
            ),
        ];
        for (error, file, rc, cause, source) in cases {
            assert_eq!(
                error,
                StatementError::ReadFailed {
                    file,
                    rc: rc.to_owned(),
                    cause: Some(cause),
                }
            );
            assert_eq!(
                error.source().map(ToString::to_string).as_deref(),
                Some(source),
                "{file}"
            );
            assert_eq!(
                error.to_string(),
                format!("{file}\nRETURN CODE: {rc}\n{ABENDING_PROGRAM}")
            );
            assert!(!error.to_string().contains(source), "{file}");
        }
    }
}
