//! CBSTM03A, with the file access of its subroutine CBSTM03B: the account statements of job
//! `CREASTMT`, as the pure rules of `carddemo-batch creastmt` (AAP 0.3.4.3, 0.3.4.5).
//!
//! This module ports the statement rows of `modern/audit/batch/CREASTMT.md`: the order of the
//! transactions and the transaction table (CBSTM03A-001, CBSTM03A-006 to CBSTM03A-008), the
//! matching of a cross-reference record's transactions and the statement total (CBSTM03A-013,
//! CBSTM03A-028), the 80-byte records of `STATEMNT.PS` (CBSTM03A-014 to CBSTM03A-031) and the
//! 100-byte records of `STATEMNT.HTML` (CBSTM03A-032 to CBSTM03A-047). The fixture scenario
//! `POSTED` applies them in FIXSTMT-001 to FIXSTMT-903 of `modern/audit/fixtures.md`.
//!
//! # Flow
//!
//! `batch::jobs::creastmt` reads every `transactions` row in card order and counts it in a
//! [`TableCensus`], keeping none; the census fails with [`StatementError::NoTransactions`] when
//! there is none (CBSTM03A-007) and tells whether the table fits the COBOL's bounds
//! (CBSTM03A-008). The job then takes the `card_xref` records in [`xref_sort_key`] order, reads
//! each one's customer and account by key (CBSTM03A-009 to CBSTM03A-011, CBSTM03B-003 to
//! CBSTM03B-005), then its card's transactions, which [`TransactionTable::of_card`] puts in
//! table order, and calls [`statement`] once per cross-reference record, writing
//! [`Statement::text_lines`] to `STATEMNT.PS` and [`Statement::html_records`] to
//! `STATEMNT.HTML`. File names, directories, the PDF, the SYSOUT lines, the error paths and the
//! exit status are the job's (CBSTM03A-002, CBSTM03A-005, CBSTM03A-012, CBSTM03A-048 to
//! CBSTM03A-052): this module reads no clock, does no I/O and has no state between statements,
//! as `INITIALIZE STATEMENT-LINES` leaves none (CBSTM03A-014).
//!
//! The PSA, TCB and TIOT walk that opens the program displays control-block names and affects
//! no record (CBSTM03A-003), and its `ALTER` and `GO TO` dispatch fixes only the order of work,
//! every transaction read before the first cross-reference record (CBSTM03A-004); neither has a
//! counterpart here (escalation candidate X-CREASTMT-1).
//!
//! # Collating order
//!
//! The STEP010 sort, the keys of the TRXFL and cross-reference KSDSs and the scan of
//! 4000-TRNXFILE-GET order card numbers and transaction IDs in the EBCDIC collating sequence
//! (CBSTM03A-001, CBSTM03A-009, CBSTM03A-013): the space below lower-case letters, lower case
//! below upper case, letters below digits. [`sort_key`] and [`xref_sort_key`] weigh each byte by
//! its code page 037 code, the proposed resolution of escalation candidate X-CREASTMT-3; for
//! keys made only of digits, every key the fixtures hold, that is byte order. The job's SQL
//! `ORDER BY` fixes only a deterministic retrieval order: [`TransactionTable::build`] and
//! [`TransactionTable::of_card`] order their input by [`sort_key`] themselves, as the KSDS
//! returns its records in key order (CBSTM03B-002), and the job sorts the cross-reference
//! records by [`xref_sort_key`] (CBSTM03B-003).
//!
//! # Table bounds
//!
//! `WS-TRNX-TABLE` holds 51 cards of 10 transactions each (CBSTM03A-008). A 52nd card or an
//! 11th transaction is stored past its `OCCURS` with no range check, and what each statement
//! then prints depends on the storage layout: halt candidate X-CREASTMT-H2. As CBSTM03A-008
//! records for the replacement, the table here keeps every card and every transaction, neither
//! dropping nor overlaying any, and [`TransactionTable::within_cobol_bounds`], or
//! [`TableCensus::within_cobol_bounds`] for the same records counted one at a time, tells the
//! job when a run leaves the COBOL's bounds, so that such a run is reported as blocked parity
//! rather than claimed.
//!
//! # Money
//!
//! Amounts appear only in the balance, the transaction amounts and the total (CBSTM03A-022,
//! CBSTM03A-027, CBSTM03A-028), stored through `crate::money` and edited by `copybook::edited`,
//! never formatted here. `ST-CURR-BAL PIC 9(9).99-` has nine integer positions for the ten of
//! `ACCT-CURR-BAL`, so the edit drops a ten-digit balance's high-order digit (escalation
//! candidate X-CREASTMT-4). A negative balance or total whose kept digits are all zero edits as
//! zero without a sign, which is not claimed as parity (halt candidate X-CREASTMT-H4).
//!
//! # Cardholder data
//!
//! No statement record carries a card number, an SSN, a CVV or a password (escalation candidate
//! X-CREASTMT-5): card numbers are read through [`CardNumber::expose`] only to order and match
//! transactions. The records leave as bytes; [`Statement`] has no `Debug`, and the derived
//! `Debug` of [`TransactionTable`] and [`TableCensus`] shows card numbers only in their
//! redacted form.

use core::fmt;

use copybook::edited::EditedPicture;
use copybook::{PicU, PicX};

use crate::money::Money;
use crate::records::{AccountRecord, CardXrefRecord, CustomerRecord, TransactionRecord};
use crate::sensitive::CardNumber;

/// Bytes in one `STATEMNT.PS` record, `FD-STMTFILE-REC PIC X(80)` (CBSTM03A-031).
pub const STMT_RECORD_LEN: usize = 80;

/// Bytes in one `STATEMNT.HTML` record, `FD-HTMLFILE-REC PIC X(100)`: the program's FD, which
/// STEP040 matches and which outranks the `LRECL=80` of the STEP030 delete (CBSTM03A-047, E20).
pub const HTML_RECORD_LEN: usize = 100;

/// Card entries of `WS-CARD-TBL OCCURS 51 TIMES` (CBSTM03A-008).
const COBOL_CARD_ENTRIES: usize = 51;

/// Transactions per card entry of `WS-TRAN-TBL OCCURS 10 TIMES` (CBSTM03A-008).
const COBOL_TRANSACTIONS_PER_CARD: usize = 10;

/// Bytes of a card number and of a transaction ID, `PIC X(16)` (CBSTM03A-001).
const KEY_LEN: usize = 16;

/// Characters of the two edited amount pictures, `PIC 9(9).99-` and `PIC Z(9).99-`
/// (CBSTM03A-022, CBSTM03A-027).
const EDITED_LEN: usize = 13;

/// The space, the byte `SPACES`, `INITIALIZE` and every padded `MOVE` supply.
const SPACE: u8 = b' ';

/// One `STATEMNT.PS` record.
type TextRecord = [u8; STMT_RECORD_LEN];

/// One `STATEMNT.HTML` record.
type HtmlRecord = [u8; HTML_RECORD_LEN];

/// The code page 037 code of a byte, its weight in the EBCDIC collating sequence the COBOL sorts
/// and compares in (CBSTM03A-001, CBSTM03A-013; escalation candidate X-CREASTMT-3).
///
/// A `PIC X(n)` field holds printable ASCII only (PIC-002), and each of those 95 characters has
/// its own code. Any other byte weighs `0xFF`, above every printable character, so the order
/// stays total; byte equality, never weight, decides which table entry a card number matches.
const fn ebcdic_weight(byte: u8) -> u8 {
    match byte {
        b' ' => 0x40,
        b'.' => 0x4B,
        b'<' => 0x4C,
        b'(' => 0x4D,
        b'+' => 0x4E,
        b'|' => 0x4F,
        b'&' => 0x50,
        b'!' => 0x5A,
        b'$' => 0x5B,
        b'*' => 0x5C,
        b')' => 0x5D,
        b';' => 0x5E,
        b'-' => 0x60,
        b'/' => 0x61,
        b',' => 0x6B,
        b'%' => 0x6C,
        b'_' => 0x6D,
        b'>' => 0x6E,
        b'?' => 0x6F,
        b'`' => 0x79,
        b':' => 0x7A,
        b'#' => 0x7B,
        b'@' => 0x7C,
        b'\'' => 0x7D,
        b'=' => 0x7E,
        b'"' => 0x7F,
        b'a'..=b'i' => 0x81 + (byte - b'a'),
        b'j'..=b'r' => 0x91 + (byte - b'j'),
        b'~' => 0xA1,
        b's'..=b'z' => 0xA2 + (byte - b's'),
        b'^' => 0xB0,
        b'[' => 0xBA,
        b']' => 0xBB,
        b'{' => 0xC0,
        b'A'..=b'I' => 0xC1 + (byte - b'A'),
        b'}' => 0xD0,
        b'J'..=b'R' => 0xD1 + (byte - b'J'),
        b'\\' => 0xE0,
        b'S'..=b'Z' => 0xE2 + (byte - b'S'),
        b'0'..=b'9' => 0xF0 + (byte - b'0'),
        _ => 0xFF,
    }
}

/// The EBCDIC weights of a 16-byte key, compared as a whole in the order of [`ebcdic_weight`].
fn ebcdic_key(bytes: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    bytes.map(ebcdic_weight)
}

/// The collating key of a card number. It encodes the full number: compare it, never show it.
fn card_key(card: &CardNumber) -> [u8; KEY_LEN] {
    ebcdic_key(card.expose().as_bytes())
}

/// The order STEP010 sorts the transactions in, `SORT FIELDS=(263,16,CH,A,1,16,CH,A)`: card
/// number, then transaction ID, both ascending in EBCDIC order, which is also the key order of
/// the TRXFL KSDS CBSTM03B reads (CBSTM03A-001, CBSTM03B-002).
///
/// The pair holds the code page 037 codes of the card number's and the transaction ID's bytes;
/// ordering transactions by it orders them as the COBOL does, and for all-digit keys it is the
/// `ORDER BY card_num, tran_id` of the job's query. [`TransactionTable::build`] applies it. The
/// first element encodes the full card number: use the key only to compare, never to display,
/// log or store.
#[must_use]
pub fn sort_key(transaction: &TransactionRecord) -> ([u8; KEY_LEN], [u8; KEY_LEN]) {
    (
        card_key(&transaction.card_num),
        ebcdic_key(transaction.id.as_bytes()),
    )
}

/// The order in which CBSTM03A reads the cross-reference records, one statement each: the key
/// order of the cross-reference KSDS, `XREF-CARD-NUM` ascending in EBCDIC order (CBSTM03A-009,
/// CBSTM03B-003).
///
/// The job sorts the `card_xref` rows by this key before it builds their statements. Like
/// [`sort_key`], it encodes the full card number and serves only to compare.
#[must_use]
pub fn xref_sort_key(xref: &CardXrefRecord) -> [u8; KEY_LEN] {
    card_key(&xref.card_num)
}

/// Why no statement can be produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementError {
    /// The transaction set holds no record: the first read of `TRNXFILE` returns status `10`,
    /// and the program shows `ERROR READING TRNXFILE` and `RETURN CODE: 10` and abends before
    /// writing any statement (CBSTM03A-007, CBSTM03A-049). Whether the COBOL job reaches that
    /// read for an empty TRANSACT file is halt candidate X-CREASTMT-H1.
    NoTransactions,
}

impl fmt::Display for StatementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTransactions => f.write_str(
                "the transaction set holds no record, so the first TRNXFILE read ends with \
                 status 10",
            ),
        }
    }
}

impl std::error::Error for StatementError {}

/// One transaction as the table keeps it: `WS-TRAN-NUM` and the two fields of `WS-TRAN-REST`
/// that a statement shows (CBSTM03A-008, CBSTM03A-026).
#[derive(Clone, Debug, PartialEq, Eq)]
struct TableTransaction {
    /// `TRNX-ID PIC X(16)`, the transaction ID.
    id: PicX<KEY_LEN>,
    /// `TRNX-DESC PIC X(100)`, the description.
    desc: PicX<100>,
    /// `TRNX-AMT PIC S9(09)V99`, the amount.
    amt: Money<9>,
}

/// One card entry of `WS-CARD-TBL`: `WS-CARD-NUM` and, in transaction-ID order, the
/// transactions of `WS-TRAN-TBL` that `WS-TRCT` counts (CBSTM03A-008).
#[derive(Clone, Debug, PartialEq, Eq)]
struct CardEntry {
    /// `WS-CARD-NUM PIC X(16)`; its `Debug` is redacted.
    card_num: CardNumber,
    /// The card's transactions, never empty.
    transactions: Vec<TableTransaction>,
}

/// `WS-TRNX-TABLE`, every transaction grouped by card in collating order, as 8100-TRNXFILE-OPEN
/// and 8500-READTRNX-READ load it before the first statement (CBSTM03A-006, CBSTM03A-008), or
/// the one entry of a single card ([`of_card`](Self::of_card)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionTable {
    /// The card entries, in [`sort_key`] order of their card numbers.
    cards: Vec<CardEntry>,
}

impl TransactionTable {
    /// Loads the table from every transaction record, in any order.
    ///
    /// The records are first put in [`sort_key`] order, the key order in which CBSTM03B reads
    /// TRXFL (CBSTM03A-001, CBSTM03B-002); then, as 8500-READTRNX-READ does, a record whose card
    /// number equals the previous record's joins that card's entry, and any other record opens
    /// the next entry (CBSTM03A-008). Each card therefore has one entry, holding its
    /// transactions in transaction-ID order. Every card and every transaction is kept, beyond
    /// the 51 × 10 bounds of the COBOL table too (halt candidate X-CREASTMT-H2; see
    /// [`within_cobol_bounds`](Self::within_cobol_bounds)).
    ///
    /// # Errors
    ///
    /// [`StatementError::NoTransactions`] when `transactions` yields no record, the abend of the
    /// first read at end of file (CBSTM03A-007).
    pub fn build<'a>(
        transactions: impl IntoIterator<Item = &'a TransactionRecord>,
    ) -> Result<Self, StatementError> {
        let table = Self::grouped(transactions);
        if table.cards.is_empty() {
            return Err(StatementError::NoTransactions);
        }
        Ok(table)
    }

    /// The table of one card's transaction records, in any order, built as
    /// [`build`](Self::build) builds it: the card's entry holds its transactions in
    /// [`sort_key`] order, the order 4000-TRNXFILE-GET prints them in (CBSTM03A-008,
    /// CBSTM03A-013). With no record the table has no entry, the table of a card without
    /// transactions, and is no error.
    ///
    /// Given the records of the card a cross-reference record names, [`statement`] prints from
    /// this table exactly what it prints from the table of every transaction, so a job can build
    /// each statement from its card's records alone and hold no other card's.
    #[must_use]
    pub fn of_card<'a>(transactions: impl IntoIterator<Item = &'a TransactionRecord>) -> Self {
        Self::grouped(transactions)
    }

    /// The table of `transactions`, empty when they yield no record: the records put in
    /// [`sort_key`] order, then each run of one card number made one entry, as
    /// [`build`](Self::build) describes (CBSTM03A-001, CBSTM03A-008, CBSTM03B-002).
    fn grouped<'a>(transactions: impl IntoIterator<Item = &'a TransactionRecord>) -> Self {
        let mut ordered: Vec<&TransactionRecord> = transactions.into_iter().collect();
        // Stable, so two records with equal keys keep their retrieval order.
        ordered.sort_by_key(|transaction| sort_key(transaction));
        let mut cards: Vec<CardEntry> = Vec::new();
        for record in ordered {
            let transaction = TableTransaction {
                id: record.id.clone(),
                desc: record.desc.clone(),
                amt: record.amt,
            };
            match cards.last_mut() {
                Some(entry) if entry.card_num == record.card_num => {
                    entry.transactions.push(transaction);
                }
                _ => cards.push(CardEntry {
                    card_num: record.card_num.clone(),
                    transactions: vec![transaction],
                }),
            }
        }
        Self { cards }
    }

    /// Card entries in the table: the distinct card numbers of the transaction set, with or
    /// without a cross-reference record (CBSTM03A-008).
    #[must_use]
    pub fn card_count(&self) -> usize {
        self.cards.len()
    }

    /// Whether the table fits `WS-TRNX-TABLE`: at most 51 cards, each with at most 10
    /// transactions (CBSTM03A-008).
    ///
    /// Outside those bounds the COBOL result has no single value (halt candidate
    /// X-CREASTMT-H2), so the statements this module builds for such a set are the
    /// replacement's and their parity is blocked, never claimed.
    #[must_use]
    pub fn within_cobol_bounds(&self) -> bool {
        let widest = self
            .cards
            .iter()
            .map(|entry| entry.transactions.len())
            .max()
            .unwrap_or(0);
        fits_cobol_table(self.cards.len(), widest)
    }

    /// The transactions 4000-TRNXFILE-GET prints for `card`, in table order (CBSTM03A-013).
    ///
    /// The scan starts at the first entry and stops at the first whose card number is greater
    /// than `card` in EBCDIC order, or after the last; every entry on the way whose card number
    /// equals `card` byte for byte supplies its transactions.
    fn matching<'t>(&'t self, card: &'t CardNumber) -> impl Iterator<Item = &'t TableTransaction> {
        let key = card_key(card);
        self.cards
            .iter()
            .take_while(move |entry| card_key(&entry.card_num) <= key)
            .filter(move |entry| entry.card_num == *card)
            .flat_map(|entry| entry.transactions.iter())
    }
}

/// Whether a table of `cards` card entries whose widest entry holds `widest` transactions fits
/// `WS-TRNX-TABLE`: `WS-CARD-TBL OCCURS 51 TIMES`, `WS-TRAN-TBL OCCURS 10 TIMES`
/// (CBSTM03A-008, halt candidate X-CREASTMT-H2).
const fn fits_cobol_table(cards: usize, widest: usize) -> bool {
    cards <= COBOL_CARD_ENTRIES && widest <= COBOL_TRANSACTIONS_PER_CARD
}

/// The shape of `WS-TRNX-TABLE`, counted one transaction record at a time as 8100-TRNXFILE-OPEN
/// and 8500-READTRNX-READ read them, without holding any (CBSTM03A-006 to CBSTM03A-008).
///
/// Fed the records in card order, the order STEP010 sorts them into (CBSTM03A-001), it counts
/// what [`TransactionTable::build`] would hold: each run of consecutive records with one card
/// number is one card entry, and the transactions of the widest entry bound the table, so
/// [`card_count`](Self::card_count) and [`within_cobol_bounds`](Self::within_cobol_bounds)
/// answer as the table's do. It keeps the last record's card number, `WS-SAVE-CARD`, and three
/// counters, whatever the number of records. Its derived `Debug` shows that card number only in
/// its redacted form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCensus {
    /// `WS-SAVE-CARD`, the card number of the last record counted; `None` before the first.
    save_card: Option<CardNumber>,
    /// Card entries opened so far.
    cards: usize,
    /// Transactions of the current card entry.
    current: usize,
    /// Transactions of the widest card entry so far.
    widest: usize,
}

impl TableCensus {
    /// Counts `transaction`, the next record of the read (CBSTM03A-006, CBSTM03A-008): a record
    /// whose card number equals the previous record's adds 1 to the current entry's count; any
    /// other record, the first included, opens the next entry with a count of 1.
    pub fn count(&mut self, transaction: &TransactionRecord) {
        if self.save_card.as_ref() == Some(&transaction.card_num) {
            self.current += 1;
        } else {
            self.cards += 1;
            self.current = 1;
            self.save_card = Some(transaction.card_num.clone());
        }
        self.widest = self.widest.max(self.current);
    }

    /// Card entries counted: the distinct card numbers of a read in card order, with or without
    /// a cross-reference record (CBSTM03A-008).
    #[must_use]
    pub const fn card_count(&self) -> usize {
        self.cards
    }

    /// Whether the counted table fits `WS-TRNX-TABLE`, as
    /// [`TransactionTable::within_cobol_bounds`] answers for the table of the same records:
    /// at most 51 cards, each with at most 10 transactions (CBSTM03A-008). Outside them the
    /// statements are the replacement's, with their parity blocked (halt candidate
    /// X-CREASTMT-H2).
    #[must_use]
    pub const fn within_cobol_bounds(&self) -> bool {
        fits_cobol_table(self.cards, self.widest)
    }

    /// Ends the count at the end of the read, which 8500-READTRNX-READ reaches with status
    /// `10`, and gives the census of every record counted.
    ///
    /// # Errors
    ///
    /// [`StatementError::NoTransactions`] when no record was counted, the abend of the first
    /// read at end of file (CBSTM03A-007).
    pub fn finish(self) -> Result<Self, StatementError> {
        if self.cards == 0 {
            Err(StatementError::NoTransactions)
        } else {
            Ok(self)
        }
    }
}

/// The `DELIMITED BY` phrase of one sending item of a COBOL `STRING`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delimiter {
    /// `DELIMITED BY SIZE`: the whole item.
    Size,
    /// `DELIMITED BY ' '`: the item's first word, the bytes before its first space.
    Space,
    /// `DELIMITED BY '  '`: the bytes before the item's first two consecutive spaces.
    DoubleSpace,
    /// `DELIMITED BY '*'`: the bytes before the item's first `*`.
    Asterisk,
}

impl Delimiter {
    /// The bytes of `item` this phrase sends: those before the first occurrence of the
    /// delimiter, or the whole item when it holds none (CBSTM03A-016, CBSTM03A-040).
    fn sent(self, item: &[u8]) -> &[u8] {
        let delimiter: &[u8] = match self {
            Self::Size => return item,
            Self::Space => b" ",
            Self::DoubleSpace => b"  ",
            Self::Asterisk => b"*",
        };
        item.windows(delimiter.len())
            .position(|window| window == delimiter)
            .and_then(|at| item.get(..at))
            .unwrap_or(item)
    }
}

/// COBOL `STRING sending … INTO receiver END-STRING`, with no `POINTER` and no `ON OVERFLOW`.
///
/// Each item sends the bytes its [`Delimiter`] selects, left to right from the receiver's first
/// byte. When the receiver is full the statement ends, whatever is left to send, and the bytes
/// it did not reach keep what the program left there: spaces after the `INITIALIZE` or
/// `MOVE SPACES` that precedes every `STRING` of CBSTM03A (CBSTM03A-014, CBSTM03A-016,
/// CBSTM03A-018, CBSTM03A-040 to CBSTM03A-042, CBSTM03A-045).
fn cobol_string(receiver: &mut [u8], sending: &[(&[u8], Delimiter)]) {
    let mut free = receiver.iter_mut();
    for &(item, delimiter) in sending {
        for &byte in delimiter.sent(item) {
            let Some(slot) = free.next() else {
                // Overflow: the receiver is full, so nothing more is sent.
                return;
            };
            *slot = byte;
        }
    }
}

/// An `HTML-FILE` record built by `MOVE SPACES` and a `STRING` into it (CBSTM03A-040 to
/// CBSTM03A-042, CBSTM03A-045).
fn strung(sending: &[(&[u8], Delimiter)]) -> HtmlRecord {
    let mut record = [SPACE; HTML_RECORD_LEN];
    cobol_string(&mut record, sending);
    record
}

/// `src` moved into an alphanumeric field of `N` bytes: left-justified, padded with spaces and
/// cut on the right, the COBOL `MOVE` rule. It is also what `SET` of an 88-level value of
/// `HTML-FIXED-LN` stores (CBSTM03A-032).
fn moved<const N: usize>(src: &[u8]) -> [u8; N] {
    *PicX::<N>::moved_from(src).as_bytes()
}

/// A group item of `N` bytes: `fields` in declaration order, then spaces. A group shorter than
/// the record it is written from is padded with spaces, as `WRITE … FROM` does (CBSTM03A-039).
fn group<const N: usize>(fields: &[&[u8]]) -> [u8; N] {
    let mut record = [SPACE; N];
    for (slot, &byte) in record.iter_mut().zip(fields.iter().copied().flatten()) {
        *slot = byte;
    }
    record
}

/// The `N` digits of an unsigned integer field, leading zeros kept: the sending bytes of a
/// `MOVE` from `PIC 9(n)` to an alphanumeric field (CBSTM03A-021, CBSTM03A-023).
fn digits<const N: u32>(value: PicU<N>) -> Vec<u8> {
    let mut out = Vec::new();
    value.encode_into(&mut out);
    out
}

/// `amount` moved to a 13-character numeric-edited field of `picture` (CBSTM03A-022,
/// CBSTM03A-027). `copybook::edited` stores the value at the picture's positions and edits it.
fn edited<const INT: u32>(picture: EditedPicture, amount: Money<INT>) -> [u8; EDITED_LEN] {
    moved(picture.format(amount.as_decimal()).as_bytes())
}

/// An `HTML-FIXED-LN` record: the 88-level literal padded with spaces to 100 bytes
/// (CBSTM03A-032).
fn html(literal: &[u8]) -> HtmlRecord {
    moved(literal)
}

// The 88-level values of `HTML-FIXED-LN` (CBSTM03A-033 to CBSTM03A-038). A continued literal
// is the text of its first line through column 72 followed by the continuation's text, with no
// space added (CBSTM03A-032). `HTML-LTDS` is declared and never written, so it has no constant.

/// `HTML-L01` (CBSTM03A-033).
const HTML_L01: &[u8] = b"<!DOCTYPE html>";
/// `HTML-L02` (CBSTM03A-033).
const HTML_L02: &[u8] = b"<html lang=\"en\">";
/// `HTML-L03` (CBSTM03A-033).
const HTML_L03: &[u8] = b"<head>";
/// `HTML-L04` (CBSTM03A-033).
const HTML_L04: &[u8] = b"<meta charset=\"utf-8\">";
/// `HTML-L05` (CBSTM03A-033).
const HTML_L05: &[u8] = b"<title>HTML Table Layout</title>";
/// `HTML-L06` (CBSTM03A-033).
const HTML_L06: &[u8] = b"</head>";
/// `HTML-L07` (CBSTM03A-033).
const HTML_L07: &[u8] = b"<body style=\"margin:0px;\">";
/// `HTML-L08`, continued, two spaces after `<table` (CBSTM03A-033).
const HTML_L08: &[u8] =
    b"<table  align=\"center\" frame=\"box\" style=\"width:70%; font:12px Segoe UI,sans-serif;\">";
/// `HTML-LTRS` (CBSTM03A-034).
const HTML_LTRS: &[u8] = b"<tr>";
/// `HTML-LTRE` (CBSTM03A-034).
const HTML_LTRE: &[u8] = b"</tr>";
/// `HTML-LTDE` (CBSTM03A-034).
const HTML_LTDE: &[u8] = b"</td>";
/// `HTML-L10`, continued (CBSTM03A-034).
const HTML_L10: &[u8] = b"<td colspan=\"3\" style=\"padding:0px 5px;background-color:#1d1d96b3;\">";
/// `HTML-L15`, continued (CBSTM03A-034).
const HTML_L15: &[u8] = b"<td colspan=\"3\" style=\"padding:0px 5px;background-color:#FFAF33;\">";
/// `HTML-L16` (CBSTM03A-035).
const HTML_L16: &[u8] = b"<p style=\"font-size:16px\">Bank of XYZ</p>";
/// `HTML-L17` (CBSTM03A-035).
const HTML_L17: &[u8] = b"<p>410 Terry Ave N</p>";
/// `HTML-L18` (CBSTM03A-035).
const HTML_L18: &[u8] = b"<p>Seattle WA 99999</p>";
/// `HTML-L22-35`, continued (CBSTM03A-034).
const HTML_L22_35: &[u8] =
    b"<td colspan=\"3\" style=\"padding:0px 5px;background-color:#f2f2f2;\">";
/// `HTML-L30-42`, continued (CBSTM03A-034).
const HTML_L30_42: &[u8] = b"<td colspan=\"3\" style=\"padding:0px 5px;background-color:#33FFD1; \
                             text-align:center;\">";
/// `HTML-L31` (CBSTM03A-035).
const HTML_L31: &[u8] = b"<p style=\"font-size:16px\">Basic Details</p>";
/// `HTML-L43` (CBSTM03A-035).
const HTML_L43: &[u8] = b"<p style=\"font-size:16px\">Transaction Summary</p>";
/// `HTML-L47`, continued (CBSTM03A-036).
const HTML_L47: &[u8] = b"<td style=\"width:25%; padding:0px 5px; background-color:#33FF5E; \
                          text-align:left;\">";
/// `HTML-L48` (CBSTM03A-036).
const HTML_L48: &[u8] = b"<p style=\"font-size:16px\">Tran ID</p>";
/// `HTML-L50`, continued (CBSTM03A-036).
const HTML_L50: &[u8] = b"<td style=\"width:55%; padding:0px 5px; background-color:#33FF5E; \
                          text-align:left;\">";
/// `HTML-L51` (CBSTM03A-036).
const HTML_L51: &[u8] = b"<p style=\"font-size:16px\">Tran Details</p>";
/// `HTML-L53`, continued (CBSTM03A-036).
const HTML_L53: &[u8] = b"<td style=\"width:20%; padding:0px 5px; background-color:#33FF5E; \
                          text-align:right;\">";
/// `HTML-L54` (CBSTM03A-036).
const HTML_L54: &[u8] = b"<p style=\"font-size:16px\">Amount</p>";
/// `HTML-L58`, continued (CBSTM03A-037).
const HTML_L58: &[u8] = b"<td style=\"width:25%; padding:0px 5px; background-color:#f2f2f2; \
                          text-align:left;\">";
/// `HTML-L61`, continued (CBSTM03A-037).
const HTML_L61: &[u8] = b"<td style=\"width:55%; padding:0px 5px; background-color:#f2f2f2; \
                          text-align:left;\">";
/// `HTML-L64`, continued (CBSTM03A-037).
const HTML_L64: &[u8] = b"<td style=\"width:20%; padding:0px 5px; background-color:#f2f2f2; \
                          text-align:right;\">";
/// `HTML-L75` (CBSTM03A-035).
const HTML_L75: &[u8] = b"<h3>End of Statement</h3>";
/// `HTML-L78` (CBSTM03A-038).
const HTML_L78: &[u8] = b"</table>";
/// `HTML-L79` (CBSTM03A-038).
const HTML_L79: &[u8] = b"</body>";
/// `HTML-L80` (CBSTM03A-038).
const HTML_L80: &[u8] = b"</html>";

/// The first `FILLER` of `HTML-L11`, `PIC X(34)`, ending in a space (CBSTM03A-039).
const HTML_L11_PREFIX: &[u8] = b"<h3>Statement for Account Number: ";
/// The last `FILLER` of `HTML-L11`, `PIC X(05)` (CBSTM03A-039).
const HTML_L11_SUFFIX: &[u8] = b"</h3>";
/// The literal that opens the name record of 5200-WRITE-HTML-NMADBS (CBSTM03A-040).
const NAME_OPEN: &[u8] = b"<p style=\"font-size:16px\">";
/// The `<p>` that opens the address and transaction records (CBSTM03A-041, CBSTM03A-045).
const P_OPEN: &[u8] = b"<p>";
/// The `</p>` that closes every strung record (CBSTM03A-040 to CBSTM03A-042, CBSTM03A-045).
const P_CLOSE: &[u8] = b"</p>";
/// The `'  ' DELIMITED BY SIZE` of the name and address records (CBSTM03A-040, CBSTM03A-041).
const TWO_SPACES: &[u8] = b"  ";
/// The literal that opens the account-ID record (CBSTM03A-042).
const BASIC_ACCT_ID: &[u8] = b"<p>Account ID         : ";
/// The literal that opens the balance record (CBSTM03A-042).
const BASIC_CURR_BAL: &[u8] = b"<p>Current Balance    : ";
/// The literal that opens the FICO-score record (CBSTM03A-042).
const BASIC_FICO_SCORE: &[u8] = b"<p>FICO Score         : ";

/// The records of one statement, in the order CBSTM03A writes them (CBSTM03A-031,
/// CBSTM03A-047).
///
/// It deliberately has no `Debug`: the records hold the customer's name and address, and leave
/// this module only as the bytes the job writes.
#[derive(Clone, PartialEq, Eq)]
pub struct Statement {
    /// The `STATEMNT.PS` records, 19 + n for n transactions: `ST-LINE0` to `ST-LINE5`,
    /// `ST-LINE6`, `ST-LINE5`, `ST-LINE7` to `ST-LINE13`, `ST-LINE12`, one `ST-LINE14` per
    /// transaction, `ST-LINE12`, `ST-LINE14A` and `ST-LINE15` (CBSTM03A-031).
    pub text_lines: Vec<[u8; STMT_RECORD_LEN]>,
    /// The `STATEMNT.HTML` records, one complete document of 64 + 11n records: 22 from
    /// 5100-WRITE-HTML-HEADER, 34 from 5200-WRITE-HTML-NMADBS, 11 per transaction from
    /// 6000-WRITE-TRANS and the 8 of the trailer (CBSTM03A-043 to CBSTM03A-047).
    pub html_records: Vec<[u8; HTML_RECORD_LEN]>,
}

/// The statement fields 5000-CREATE-STATEMENT sets after `INITIALIZE STATEMENT-LINES`, before
/// the records that show them (CBSTM03A-014, CBSTM03A-016 to CBSTM03A-023).
struct Header {
    /// `ST-NAME PIC X(75)` (CBSTM03A-016).
    name: [u8; 75],
    /// `ST-ADD1 PIC X(50)` (CBSTM03A-017).
    add1: [u8; 50],
    /// `ST-ADD2 PIC X(50)` (CBSTM03A-017).
    add2: [u8; 50],
    /// `ST-ADD3 PIC X(80)` (CBSTM03A-018).
    add3: [u8; 80],
    /// `ST-ACCT-ID PIC X(20)`, whose value `L11-ACCT PIC X(20)` also receives (CBSTM03A-021,
    /// CBSTM03A-039).
    acct_id: [u8; 20],
    /// `ST-CURR-BAL PIC 9(9).99-` (CBSTM03A-022).
    curr_bal: [u8; EDITED_LEN],
    /// `ST-FICO-SCORE PIC X(20)` (CBSTM03A-023).
    fico_score: [u8; 20],
}

impl Header {
    /// The fields of the statement of `customer` and `account`.
    fn new(customer: &CustomerRecord, account: &AccountRecord) -> Self {
        // `INITIALIZE STATEMENT-LINES` leaves `ST-NAME` and `ST-ADD3` as spaces, so each STRING
        // result is padded with spaces (CBSTM03A-014).
        let mut name = [SPACE; 75];
        cobol_string(
            &mut name,
            &[
                (customer.first_name.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
                (customer.middle_name.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
                (customer.last_name.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
            ],
        );
        let mut add3 = [SPACE; 80];
        cobol_string(
            &mut add3,
            &[
                (customer.addr_line_3.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
                (customer.addr_state_cd.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
                (customer.addr_country_cd.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
                (customer.addr_zip.as_bytes(), Delimiter::Space),
                (b" ", Delimiter::Size),
            ],
        );
        Self {
            name,
            add1: *customer.addr_line_1.as_bytes(),
            add2: *customer.addr_line_2.as_bytes(),
            add3,
            acct_id: moved(&digits(account.id)),
            // TRUNCATION: `ST-CURR-BAL PIC 9(9).99-` keeps nine of the ten integer digits of
            // `ACCT-CURR-BAL PIC S9(10)V99`, so the COBOL MOVE drops the high-order digit
            // (CBSTM03A-022, escalation candidate X-CREASTMT-4); `copybook::edited` stores it so.
            curr_bal: edited(EditedPicture::Nines9Dot99Minus, account.curr_bal),
            fico_score: moved(&digits(customer.fico_credit_score)),
        }
    }
}

/// The fields of `ST-LINE14` that 6000-WRITE-TRANS sets for one transaction (CBSTM03A-026).
struct TransactionFields {
    /// `ST-TRANID PIC X(16)`, the 16 bytes of `TRNX-ID`.
    id: [u8; KEY_LEN],
    /// `ST-TRANDT PIC X(49)`, the first 49 bytes of `TRNX-DESC`: the `MOVE` cuts the other 51.
    details: [u8; 49],
    /// `ST-TRANAMT PIC Z(9).99-`, the amount edited (CBSTM03A-027).
    amount: [u8; EDITED_LEN],
}

impl TransactionFields {
    /// The fields `transaction` sets.
    fn new(transaction: &TableTransaction) -> Self {
        Self {
            id: *transaction.id.as_bytes(),
            details: moved(transaction.desc.as_bytes()),
            amount: edited(EditedPicture::Z9Dot99Minus, transaction.amt),
        }
    }
}

/// `ST-LINE5`, `ST-LINE10` and `ST-LINE12`: 80 `-` (CBSTM03A-019).
const fn dashes() -> TextRecord {
    [b'-'; STMT_RECORD_LEN]
}

/// `ST-LINE0`: 31 `*`, `START OF STATEMENT`, 31 `*` (CBSTM03A-015).
fn st_line0() -> TextRecord {
    group(&[&[b'*'; 31], b"START OF STATEMENT", &[b'*'; 31]])
}

/// `ST-LINE6`: 33 spaces, `Basic Details` in its 14-byte `FILLER`, 33 spaces (CBSTM03A-020).
fn st_line6() -> TextRecord {
    group(&[&[SPACE; 33], &moved::<14>(b"Basic Details")])
}

/// `ST-LINE11`: 30 spaces, `TRANSACTION SUMMARY ` (20), 30 spaces (CBSTM03A-024).
fn st_line11() -> TextRecord {
    group(&[&[SPACE; 30], b"TRANSACTION SUMMARY "])
}

/// `ST-LINE13`: `Tran ID         ` (16), `Tran Details    ` padded to 51, `  Tran Amount` (13)
/// (CBSTM03A-025).
fn st_line13() -> TextRecord {
    group(&[
        b"Tran ID         ",
        &moved::<51>(b"Tran Details    "),
        b"  Tran Amount",
    ])
}

/// `ST-LINE15`: 32 `*`, `END OF STATEMENT`, 32 `*` (CBSTM03A-030).
fn st_line15() -> TextRecord {
    group(&[&[b'*'; 32], b"END OF STATEMENT", &[b'*'; 32]])
}

/// The text records 5000-CREATE-STATEMENT writes after 5200-WRITE-HTML-NMADBS: `ST-LINE1` to
/// `ST-LINE5`, `ST-LINE6`, `ST-LINE5`, `ST-LINE7` to `ST-LINE13` and `ST-LINE12`
/// (CBSTM03A-016 to CBSTM03A-025, CBSTM03A-031).
fn header_lines(header: &Header) -> [TextRecord; 15] {
    [
        group(&[&header.name]),
        group(&[&header.add1]),
        group(&[&header.add2]),
        group(&[&header.add3]),
        dashes(),
        st_line6(),
        dashes(),
        group(&[b"Account ID         :", &header.acct_id]),
        group(&[b"Current Balance    :", &header.curr_bal]),
        group(&[b"FICO Score         :", &header.fico_score]),
        dashes(),
        st_line11(),
        dashes(),
        st_line13(),
        dashes(),
    ]
}

/// `ST-LINE14`: `ST-TRANID`, a space, `ST-TRANDT`, `$`, `ST-TRANAMT` (CBSTM03A-026).
fn st_line14(fields: &TransactionFields) -> TextRecord {
    group(&[&fields.id, b" ", &fields.details, b"$", &fields.amount])
}

/// `ST-LINE14A`: `Total EXP:`, 56 spaces, `$`, `ST-TOTAL-TRAMT` (CBSTM03A-029).
fn st_line14a(total: Money<9>) -> TextRecord {
    group(&[
        b"Total EXP:",
        &[SPACE; 56],
        b"$",
        &edited(EditedPicture::Z9Dot99Minus, total),
    ])
}

/// The 22 records of 5100-WRITE-HTML-HEADER (CBSTM03A-033 to CBSTM03A-035, CBSTM03A-039,
/// CBSTM03A-043).
fn html_header(out: &mut Vec<HtmlRecord>, header: &Header) {
    out.extend(
        [
            HTML_L01, HTML_L02, HTML_L03, HTML_L04, HTML_L05, HTML_L06, HTML_L07, HTML_L08,
            HTML_LTRS, HTML_L10,
        ]
        .map(html),
    );
    out.push(group(&[HTML_L11_PREFIX, &header.acct_id, HTML_L11_SUFFIX]));
    out.extend(
        [
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L15,
            HTML_L16,
            HTML_L17,
            HTML_L18,
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L22_35,
        ]
        .map(html),
    );
}

/// The 34 records of 5200-WRITE-HTML-NMADBS: the name, the three address lines, the basic
/// details and the transaction-summary heading (CBSTM03A-034 to CBSTM03A-036, CBSTM03A-040 to
/// CBSTM03A-042, CBSTM03A-044).
fn html_name_address_details(out: &mut Vec<HtmlRecord>, header: &Header) {
    // `MOVE ST-NAME TO L23-NAME` keeps the first 50 of its 75 bytes (CBSTM03A-040).
    let l23_name: [u8; 50] = moved(&header.name);
    out.push(strung(&[
        (NAME_OPEN, Delimiter::Asterisk),
        (&l23_name, Delimiter::DoubleSpace),
        (TWO_SPACES, Delimiter::Size),
        (P_CLOSE, Delimiter::Asterisk),
    ]));
    for line in [
        header.add1.as_slice(),
        header.add2.as_slice(),
        header.add3.as_slice(),
    ] {
        out.push(strung(&[
            (P_OPEN, Delimiter::Asterisk),
            (line, Delimiter::DoubleSpace),
            (TWO_SPACES, Delimiter::Size),
            (P_CLOSE, Delimiter::Asterisk),
        ]));
    }
    out.extend(
        [
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L30_42,
            HTML_L31,
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L22_35,
        ]
        .map(html),
    );
    for (label, field) in [
        (BASIC_ACCT_ID, header.acct_id.as_slice()),
        (BASIC_CURR_BAL, header.curr_bal.as_slice()),
        (BASIC_FICO_SCORE, header.fico_score.as_slice()),
    ] {
        out.push(strung(&[
            (label, Delimiter::Asterisk),
            (field, Delimiter::Asterisk),
            (P_CLOSE, Delimiter::Asterisk),
        ]));
    }
    out.extend(
        [
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L30_42,
            HTML_L43,
            HTML_LTDE,
            HTML_LTRE,
            HTML_LTRS,
            HTML_L47,
            HTML_L48,
            HTML_LTDE,
            HTML_L50,
            HTML_L51,
            HTML_LTDE,
            HTML_L53,
            HTML_L54,
            HTML_LTDE,
            HTML_LTRE,
        ]
        .map(html),
    );
}

/// The 11 records 6000-WRITE-TRANS writes for one transaction after its `ST-LINE14`
/// (CBSTM03A-034, CBSTM03A-037, CBSTM03A-045).
fn html_transaction(out: &mut Vec<HtmlRecord>, fields: &TransactionFields) {
    let cell = |field: &[u8]| {
        strung(&[
            (P_OPEN, Delimiter::Asterisk),
            (field, Delimiter::Asterisk),
            (P_CLOSE, Delimiter::Asterisk),
        ])
    };
    out.extend([HTML_LTRS, HTML_L58].map(html));
    out.push(cell(&fields.id));
    out.extend([HTML_LTDE, HTML_L61].map(html));
    out.push(cell(&fields.details));
    out.extend([HTML_LTDE, HTML_L64].map(html));
    out.push(cell(&fields.amount));
    out.extend([HTML_LTDE, HTML_LTRE].map(html));
}

/// The 8 records that end each statement's document (CBSTM03A-034, CBSTM03A-035, CBSTM03A-038,
/// CBSTM03A-046).
const HTML_TRAILER: [&[u8]; 8] = [
    HTML_LTRS, HTML_L10, HTML_L75, HTML_LTDE, HTML_LTRE, HTML_L78, HTML_L79, HTML_L80,
];

/// The statement of one cross-reference record: 5000-CREATE-STATEMENT, then 4000-TRNXFILE-GET
/// (CBSTM03A-009, CBSTM03A-013 to CBSTM03A-047).
///
/// `customer` and `account` are the records the job read for `xref`'s `XREF-CUST-ID` and
/// `XREF-ACCT-ID` (CBSTM03A-010, CBSTM03A-011); the job handles a missing one before calling
/// this. The statement shows the transactions `table` holds for `xref`'s card number, in table
/// order, and their total: `WS-TOTAL-AMT` starts at zero, and each amount is added after its
/// `ST-LINE14` is written (CBSTM03A-028). A card with no transactions gets no `ST-LINE14`, no
/// transaction records and a total of 0.00 (CBSTM03A-013).
#[must_use]
pub fn statement(
    xref: &CardXrefRecord,
    customer: &CustomerRecord,
    account: &AccountRecord,
    table: &TransactionTable,
) -> Statement {
    let header = Header::new(customer, account);
    let mut text_lines = vec![st_line0()];
    let mut html_records = Vec::new();
    html_header(&mut html_records, &header);
    html_name_address_details(&mut html_records, &header);
    text_lines.extend(header_lines(&header));
    let mut total = Money::<9>::ZERO;
    for transaction in table.matching(&xref.card_num) {
        let fields = TransactionFields::new(transaction);
        text_lines.push(st_line14(&fields));
        html_transaction(&mut html_records, &fields);
        // TRUNCATION: `ADD TRNX-AMT TO WS-TOTAL-AMT` into `PIC S9(9)V99 COMP-3` keeps the sum
        // modulo 10^9 with its sign, the COBOL store rule with no `ON SIZE ERROR`
        // (CBSTM03A-028).
        total = (total + transaction.amt).store::<9>();
    }
    text_lines.extend([dashes(), st_line14a(total), st_line15()]);
    html_records.extend(HTML_TRAILER.map(html));
    Statement {
        text_lines,
        html_records,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use copybook::{PicDate, PicTimestamp, PicU, PicX};

    use super::{
        HTML_RECORD_LEN, STMT_RECORD_LEN, Statement, StatementError, TableCensus, TransactionTable,
        ebcdic_weight, sort_key, statement, xref_sort_key,
    };
    use crate::money::{Decimal, Money};
    use crate::records::{AccountRecord, CardXrefRecord, CustomerRecord, TransactionRecord};
    use crate::sensitive::{CardNumber, Ssn};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The card of the statements under test.
    const CARD: &str = "4000000000000001";
    /// A card whose transactions no statement under test shows.
    const OTHER_CARD: &str = "4000000000000002";
    /// A card the cross-reference names and no transaction carries.
    const BARE_CARD: &str = "4000000000000003";

    /// First, middle and last name of the default customer.
    const NAMES: [&str; 3] = ["Aniya", "Alba", "Von"];
    /// Address lines 1 to 3, state, country and ZIP of the default customer.
    const ADDRESS: [&str; 6] = [
        "1588 Nienow Cape",
        "Suite 187",
        "New Hope",
        "OR",
        "USA",
        "04257",
    ];

    /// A customer with the given name and address parts and FICO score; every other field
    /// blank or zero.
    fn customer(
        names: [&str; 3],
        address: [&str; 6],
        fico: u64,
    ) -> Result<CustomerRecord, Box<dyn Error>> {
        let [first, middle, last] = names;
        let [line_1, line_2, line_3, state, country, zip] = address;
        Ok(CustomerRecord {
            id: PicU::new(50)?,
            first_name: PicX::from_text(first)?,
            middle_name: PicX::from_text(middle)?,
            last_name: PicX::from_text(last)?,
            addr_line_1: PicX::from_text(line_1)?,
            addr_line_2: PicX::from_text(line_2)?,
            addr_line_3: PicX::from_text(line_3)?,
            addr_state_cd: PicX::from_text(state)?,
            addr_country_cd: PicX::from_text(country)?,
            addr_zip: PicX::from_text(zip)?,
            phone_num_1: PicX::spaces(),
            phone_num_2: PicX::spaces(),
            ssn: Ssn::new(PicU::ZERO),
            govt_issued_id: PicX::spaces(),
            dob_yyyy_mm_dd: PicDate::BLANK,
            eft_account_id: PicX::spaces(),
            pri_card_holder_ind: PicX::spaces(),
            fico_credit_score: PicU::new(fico)?,
            filler: PicX::spaces(),
        })
    }

    /// Account `id` with the balance `cents` hundredths; every other amount zero.
    fn account(id: u64, cents: i64) -> Result<AccountRecord, Box<dyn Error>> {
        Ok(AccountRecord {
            id: PicU::new(id)?,
            active_status: PicX::from_text("Y")?,
            curr_bal: Money::new(Decimal::new(cents, 2))?,
            credit_limit: Money::ZERO,
            cash_credit_limit: Money::ZERO,
            open_date: PicDate::BLANK,
            expiraion_date: PicDate::BLANK,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::ZERO,
            addr_zip: PicX::spaces(),
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// Transaction `id` of `card` with description `desc` and amount `cents` hundredths.
    fn transaction(
        card: &str,
        id: &str,
        desc: &str,
        cents: i64,
    ) -> Result<TransactionRecord, Box<dyn Error>> {
        Ok(TransactionRecord {
            id: PicX::from_text(id)?,
            type_cd: PicX::from_text("01")?,
            cat_cd: PicU::new(1)?,
            source: PicX::from_text("POS TERM")?,
            desc: PicX::from_text(desc)?,
            amt: Money::new(Decimal::new(cents, 2))?,
            merchant_id: PicU::ZERO,
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            card_num: CardNumber::new(PicX::from_text(card)?),
            orig_ts: PicTimestamp::BLANK,
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// The cross-reference record of `card`, customer 50 and account 50.
    fn xref(card: &str) -> Result<CardXrefRecord, Box<dyn Error>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_text(card)?),
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::spaces(),
        })
    }

    /// The statement of `card` for the given customer and balance over `transactions`.
    fn statement_of(
        card: &str,
        customer: &CustomerRecord,
        cents: i64,
        transactions: &[TransactionRecord],
    ) -> Result<Statement, Box<dyn Error>> {
        let table = TransactionTable::build(transactions)?;
        Ok(statement(
            &xref(card)?,
            customer,
            &account(50, cents)?,
            &table,
        ))
    }

    /// The statement of [`CARD`] for the default customer with balance 1945.87.
    fn default_statement(transactions: &[TransactionRecord]) -> Result<Statement, Box<dyn Error>> {
        statement_of(CARD, &customer(NAMES, ADDRESS, 623)?, 194_587, transactions)
    }

    /// `text` padded with spaces to `width` bytes.
    fn padded(text: &str, width: usize) -> Vec<u8> {
        format!("{text:<width$}").into_bytes()
    }

    /// Text record `at` of `st`.
    fn text_line(st: &Statement, at: usize) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(st.text_lines.get(at).ok_or("no such text record")?.to_vec())
    }

    /// HTML record `at` of `st`.
    fn html_record(st: &Statement, at: usize) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(st
            .html_records
            .get(at)
            .ok_or("no such HTML record")?
            .to_vec())
    }

    /// The text records of the default statement with one transaction of 183.88, built from
    /// the layouts of `CBSTM03A.CBL:85-146`.
    fn expected_text() -> Vec<Vec<u8>> {
        let dashes = "-".repeat(80);
        [
            format!("{0}START OF STATEMENT{0}", "*".repeat(31)),
            "Aniya Alba Von".to_owned(),
            "1588 Nienow Cape".to_owned(),
            "Suite 187".to_owned(),
            "New OR USA 04257".to_owned(),
            dashes.clone(),
            format!("{}Basic Details", " ".repeat(33)),
            dashes.clone(),
            "Account ID         :00000000050".to_owned(),
            "Current Balance    :000001945.87".to_owned(),
            "FICO Score         :623".to_owned(),
            dashes.clone(),
            format!("{}TRANSACTION SUMMARY", " ".repeat(30)),
            dashes.clone(),
            format!(
                "Tran ID{}Tran Details{}Tran Amount",
                " ".repeat(9),
                " ".repeat(41)
            ),
            dashes.clone(),
            format!(
                "0000000000000001 {:<49}${:>13}",
                "Purchase at Blick-Rippin", "183.88 "
            ),
            dashes,
            format!("Total EXP:{}${:>13}", " ".repeat(56), "183.88 "),
            format!("{0}END OF STATEMENT{0}", "*".repeat(32)),
        ]
        .iter()
        .map(|line| padded(line, STMT_RECORD_LEN))
        .collect()
    }

    /// The HTML records of the default statement with one transaction of 183.88, written out
    /// from the 88-level values and `STRING` statements of `CBSTM03A.CBL:148-223,506-723`.
    fn expected_html() -> Vec<Vec<u8>> {
        let navy = "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#1d1d96b3;\">";
        let grey = "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#f2f2f2;\">";
        let teal = "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#33FFD1; \
                    text-align:center;\">";
        let cell = |width: &str, colour: &str, align: &str| {
            format!(
                "<td style=\"width:{width}%; padding:0px 5px; background-color:#{colour}; \
                 text-align:{align};\">"
            )
        };
        let lines: Vec<String> = vec![
            "<!DOCTYPE html>".into(),
            "<html lang=\"en\">".into(),
            "<head>".into(),
            "<meta charset=\"utf-8\">".into(),
            "<title>HTML Table Layout</title>".into(),
            "</head>".into(),
            "<body style=\"margin:0px;\">".into(),
            "<table  align=\"center\" frame=\"box\" style=\"width:70%; font:12px Segoe UI,\
             sans-serif;\">"
                .into(),
            "<tr>".into(),
            navy.into(),
            "<h3>Statement for Account Number: 00000000050         </h3>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#FFAF33;\">".into(),
            "<p style=\"font-size:16px\">Bank of XYZ</p>".into(),
            "<p>410 Terry Ave N</p>".into(),
            "<p>Seattle WA 99999</p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            grey.into(),
            "<p style=\"font-size:16px\">Aniya Alba Von  </p>".into(),
            "<p>1588 Nienow Cape  </p>".into(),
            "<p>Suite 187  </p>".into(),
            "<p>New OR USA 04257  </p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            teal.into(),
            "<p style=\"font-size:16px\">Basic Details</p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            grey.into(),
            "<p>Account ID         : 00000000050         </p>".into(),
            "<p>Current Balance    : 000001945.87 </p>".into(),
            "<p>FICO Score         : 623                 </p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            teal.into(),
            "<p style=\"font-size:16px\">Transaction Summary</p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            cell("25", "33FF5E", "left"),
            "<p style=\"font-size:16px\">Tran ID</p>".into(),
            "</td>".into(),
            cell("55", "33FF5E", "left"),
            "<p style=\"font-size:16px\">Tran Details</p>".into(),
            "</td>".into(),
            cell("20", "33FF5E", "right"),
            "<p style=\"font-size:16px\">Amount</p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            cell("25", "f2f2f2", "left"),
            "<p>0000000000000001</p>".into(),
            "</td>".into(),
            cell("55", "f2f2f2", "left"),
            format!("<p>{:<49}</p>", "Purchase at Blick-Rippin"),
            "</td>".into(),
            cell("20", "f2f2f2", "right"),
            "<p>      183.88 </p>".into(),
            "</td>".into(),
            "</tr>".into(),
            "<tr>".into(),
            navy.into(),
            "<h3>End of Statement</h3>".into(),
            "</td>".into(),
            "</tr>".into(),
            "</table>".into(),
            "</body>".into(),
            "</html>".into(),
        ];
        lines
            .iter()
            .map(|line| padded(line, HTML_RECORD_LEN))
            .collect()
    }

    /// The default statement with one transaction of 183.88 on [`CARD`].
    fn one_transaction_statement() -> Result<Statement, Box<dyn Error>> {
        default_statement(&[transaction(
            CARD,
            "0000000000000001",
            "Purchase at Blick-Rippin",
            18_388,
        )?])
    }

    // audit: CBSTM03A-014, CBSTM03A-015, CBSTM03A-016, CBSTM03A-017, CBSTM03A-018
    // audit: CBSTM03A-019, CBSTM03A-020, CBSTM03A-021, CBSTM03A-022, CBSTM03A-023
    // audit: CBSTM03A-024, CBSTM03A-025, CBSTM03A-026, CBSTM03A-029, CBSTM03A-030
    // audit: CBSTM03A-031
    #[test]
    fn text_records_follow_the_layouts_in_program_order() -> TestResult {
        let st = one_transaction_statement()?;
        let got: Vec<Vec<u8>> = st.text_lines.iter().map(|line| line.to_vec()).collect();
        assert_eq!(got, expected_text());
        Ok(())
    }

    // audit: CBSTM03A-032, CBSTM03A-033, CBSTM03A-034, CBSTM03A-035, CBSTM03A-036
    // audit: CBSTM03A-037, CBSTM03A-038, CBSTM03A-039, CBSTM03A-040, CBSTM03A-041
    // audit: CBSTM03A-042, CBSTM03A-043, CBSTM03A-044, CBSTM03A-045, CBSTM03A-046
    // audit: CBSTM03A-047
    #[test]
    fn html_records_form_one_document_in_program_order() -> TestResult {
        let st = one_transaction_statement()?;
        let got: Vec<Vec<u8>> = st
            .html_records
            .iter()
            .map(|record| record.to_vec())
            .collect();
        assert_eq!(got, expected_html());
        Ok(())
    }

    // audit: CBSTM03A-016, CBSTM03A-040
    #[test]
    fn the_name_keeps_the_first_word_of_each_part() -> TestResult {
        let long = ["A".repeat(25), "B".repeat(25), "C".repeat(25)];
        let overflowed = format!("{} {} {}", long[0], long[1], "C".repeat(23));
        let first_fifty = format!("{} {}", long[0], "B".repeat(24));
        let cases: [([&str; 3], &str, &str); 6] = [
            (NAMES, "Aniya Alba Von", "Aniya Alba Von"),
            (["Ann", "", "Lee"], "Ann  Lee", "Ann"),
            (["", "Bo", "Lee"], " Bo Lee", " Bo Lee"),
            (["", "", "Lee"], "  Lee", ""),
            (["Mary Ann", "B", "Cole Jr"], "Mary B Cole", "Mary B Cole"),
            (
                [long[0].as_str(), long[1].as_str(), long[2].as_str()],
                overflowed.as_str(),
                first_fifty.as_str(),
            ),
        ];
        for (names, line, html) in cases {
            let st = statement_of(
                CARD,
                &customer(names, ADDRESS, 623)?,
                0,
                &[transaction(OTHER_CARD, "0000000000000001", "x", 1)?],
            )?;
            assert_eq!(
                text_line(&st, 1)?,
                padded(line, STMT_RECORD_LEN),
                "{names:?}"
            );
            let record = format!("<p style=\"font-size:16px\">{html}  </p>");
            assert_eq!(
                html_record(&st, 22)?,
                padded(&record, HTML_RECORD_LEN),
                "{names:?}"
            );
        }
        Ok(())
    }

    // audit: CBSTM03A-017, CBSTM03A-018, CBSTM03A-041
    #[test]
    fn address_lines_move_whole_and_the_third_keeps_first_words() -> TestResult {
        let fifty = "x".repeat(50);
        let cases: [([&str; 6], [&str; 3], [&str; 3]); 4] = [
            (
                ADDRESS,
                ["1588 Nienow Cape", "Suite 187", "New OR USA 04257"],
                ["1588 Nienow Cape", "Suite 187", "New OR USA 04257"],
            ),
            (
                ["12  Main St", "  Suite 9", "Town", "", "USA", "12345"],
                ["12  Main St", "  Suite 9", "Town  USA 12345"],
                ["12", "", "Town"],
            ),
            (
                [
                    fifty.as_str(),
                    "Apt. 453",
                    "Port Lane",
                    "GU",
                    "USA",
                    "07923-8822",
                ],
                [fifty.as_str(), "Apt. 453", "Port GU USA 07923-8822"],
                [fifty.as_str(), "Apt. 453", "Port GU USA 07923-8822"],
            ),
            (
                ["", "", "", "OR", "USA", "1"],
                ["", "", " OR USA 1"],
                ["", "", " OR USA 1"],
            ),
        ];
        for (address, lines, html) in cases {
            let st = statement_of(
                CARD,
                &customer(NAMES, address, 623)?,
                0,
                &[transaction(OTHER_CARD, "0000000000000001", "x", 1)?],
            )?;
            for (at, (line, text)) in lines.iter().zip(html).enumerate() {
                assert_eq!(
                    text_line(&st, 2 + at)?,
                    padded(line, STMT_RECORD_LEN),
                    "{address:?}"
                );
                let record = format!("<p>{text}  </p>");
                assert_eq!(html_record(&st, 23 + at)?, padded(&record, HTML_RECORD_LEN));
            }
        }
        Ok(())
    }

    // audit: CBSTM03A-021, CBSTM03A-022, CBSTM03A-023, CBSTM03A-039, CBSTM03A-042
    #[test]
    fn basic_details_show_the_account_balance_and_score() -> TestResult {
        let cases: [(i64, &str); 4] = [
            (51_230, "000000512.30 "),
            (-7_645, "000000076.45-"),
            (0, "000000000.00 "),
            (123_456_789_012, "234567890.12 "),
        ];
        for (cents, balance) in cases {
            let st = statement_of(
                CARD,
                &customer(NAMES, ADDRESS, 78)?,
                cents,
                &[transaction(CARD, "0000000000000001", "x", 1)?],
            )?;
            let line = format!("Current Balance    :{balance}");
            assert_eq!(
                text_line(&st, 9)?,
                padded(&line, STMT_RECORD_LEN),
                "{cents}"
            );
            let record = format!("<p>Current Balance    : {balance}</p>");
            assert_eq!(
                html_record(&st, 36)?,
                padded(&record, HTML_RECORD_LEN),
                "{cents}"
            );
            assert_eq!(
                text_line(&st, 10)?,
                padded("FICO Score         :078", STMT_RECORD_LEN)
            );
            let fico = "<p>FICO Score         : 078                 </p>";
            assert_eq!(html_record(&st, 37)?, padded(fico, HTML_RECORD_LEN));
        }
        Ok(())
    }

    // audit: CBSTM03A-013, CBSTM03A-026, CBSTM03A-027, CBSTM03A-028, CBSTM03A-029
    // audit: CBSTM03A-045
    #[test]
    fn transactions_of_the_card_print_with_their_total() -> TestResult {
        let desc = "Purchase at a merchant whose name runs past the forty-ninth byte";
        let st = default_statement(&[
            transaction(OTHER_CARD, "0000000000000002", "Not this card", 99_900)?,
            transaction(CARD, "0000000000000003", "Refund*partial", -3_510)?,
            transaction(CARD, "0000000000000001", desc, 27_106)?,
        ])?;
        assert_eq!(st.text_lines.len(), 21);
        assert_eq!(st.html_records.len(), 86);
        let first = format!(
            "0000000000000001 {}$      271.06 ",
            desc.get(..49).ok_or("desc")?
        );
        assert_eq!(text_line(&st, 16)?, first.into_bytes());
        let second = format!("0000000000000003 {:<49}$       35.10-", "Refund*partial");
        assert_eq!(text_line(&st, 17)?, second.into_bytes());
        let total = format!("Total EXP:{}$      235.96 ", " ".repeat(56));
        assert_eq!(text_line(&st, 19)?, total.into_bytes());
        // The `*` cuts the HTML text and leaves the text record whole.
        let cells = [
            (58, "<p>0000000000000001</p>".to_owned()),
            (61, format!("<p>{}</p>", desc.get(..49).ok_or("desc")?)),
            (64, "<p>      271.06 </p>".to_owned()),
            (69, "<p>0000000000000003</p>".to_owned()),
            (72, "<p>Refund</p>".to_owned()),
            (75, "<p>       35.10-</p>".to_owned()),
        ];
        for (at, cell) in cells {
            assert_eq!(
                html_record(&st, at)?,
                padded(&cell, HTML_RECORD_LEN),
                "{at}"
            );
        }
        Ok(())
    }

    // audit: CBSTM03A-027, CBSTM03A-028, CBSTM03A-029
    #[test]
    fn the_total_is_stored_modulo_ten_to_the_ninth() -> TestResult {
        let cases: [(&[i64], &str); 4] = [
            (&[27_106, -3_510], "      235.96 "),
            (&[7], "         .07 "),
            (&[99_999_999_999, 1], "         .00 "),
            (&[-99_999_999_999, -100], "         .99-"),
        ];
        for (amounts, edited) in cases {
            let mut transactions = Vec::new();
            for (n, &cents) in (1_u32..).zip(amounts) {
                transactions.push(transaction(CARD, &format!("{n:016}"), "x", cents)?);
            }
            let st = default_statement(&transactions)?;
            let total = format!("Total EXP:{}${edited}", " ".repeat(56));
            let at = st
                .text_lines
                .len()
                .checked_sub(2)
                .ok_or("short statement")?;
            assert_eq!(text_line(&st, at)?, total.into_bytes(), "{amounts:?}");
        }
        Ok(())
    }

    // audit: CBSTM03A-009, CBSTM03A-013, CBSTM03A-031, CBSTM03A-047
    #[test]
    fn a_card_without_transactions_gets_an_empty_summary() -> TestResult {
        let st = statement_of(
            BARE_CARD,
            &customer(NAMES, ADDRESS, 623)?,
            0,
            &[
                transaction(CARD, "0000000000000001", "x", 100)?,
                transaction("4000000000000009", "0000000000000002", "y", 200)?,
            ],
        )?;
        assert_eq!(st.text_lines.len(), 19);
        assert_eq!(st.html_records.len(), 64);
        let total = format!("Total EXP:{}$         .00 ", " ".repeat(56));
        assert_eq!(text_line(&st, 17)?, total.into_bytes());
        Ok(())
    }

    // audit: CBSTM03A-007
    #[test]
    fn an_empty_transaction_set_is_the_first_read_abend() {
        let none: [TransactionRecord; 0] = [];
        assert_eq!(
            TransactionTable::build(&none),
            Err(StatementError::NoTransactions)
        );
        let message = StatementError::NoTransactions.to_string();
        assert!(message.contains("TRNXFILE") && message.contains("status 10"));
    }

    // audit: CBSTM03A-001, CBSTM03A-006, CBSTM03A-008, CBSTM03B-002
    #[test]
    fn the_table_groups_each_card_in_key_order() -> TestResult {
        let table = TransactionTable::build(&[
            transaction(OTHER_CARD, "0000000000000002", "b", 100)?,
            transaction(CARD, "0000000000000003", "a3", 100)?,
            transaction(CARD, "0000000000000001", "a1", 100)?,
        ])?;
        assert_eq!(table.card_count(), 2);
        let st = statement(
            &xref(CARD)?,
            &customer(NAMES, ADDRESS, 623)?,
            &account(50, 0)?,
            &table,
        );
        let ids: Vec<&[u8]> = [16, 17]
            .iter()
            .filter_map(|&at| st.text_lines.get(at)?.get(..16))
            .collect();
        assert_eq!(ids, [b"0000000000000001", b"0000000000000003"]);
        Ok(())
    }

    // audit: CBSTM03A-001, CBSTM03A-009, CBSTM03A-013, CBSTM03B-002, CBSTM03B-003
    #[test]
    fn keys_collate_in_ebcdic_order() -> TestResult {
        let mut printable: Vec<u8> = (0x20_u8..=0x7E).collect();
        printable.sort_by_key(|&byte| ebcdic_weight(byte));
        let expected = " .<(+|&!$*);-/,%_>?`:#@'=\"abcdefghijklmnopqr~stuvwxyz^[]{ABCDEFGHI}\
                        JKLMNOPQR\\STUVWXYZ0123456789";
        assert_eq!(printable, expected.as_bytes());
        // A byte no `PIC X(n)` field holds weighs above every printable one (PIC-002).
        for byte in [0x00_u8, 0x0A, 0x7F, 0xC1] {
            assert!(ebcdic_weight(byte) > ebcdic_weight(b'9'), "{byte:#04x}");
        }
        // Letters sort below digits, so `A…` comes first in the card's statement.
        let st = default_statement(&[
            transaction(CARD, "0000000000000001", "digits", 100)?,
            transaction(CARD, "A000000000000001", "letter", 100)?,
        ])?;
        let first = text_line(&st, 16)?;
        assert_eq!(first.get(..16), Some(b"A000000000000001".as_slice()));
        let letter = transaction("A000000000000000", "0000000000000001", "x", 1)?;
        let digit = transaction("0000000000000000", "0000000000000001", "x", 1)?;
        assert!(sort_key(&letter) < sort_key(&digit));
        assert!(
            xref_sort_key(&xref("a000000000000000")?) < xref_sort_key(&xref("A000000000000000")?)
        );
        assert_eq!(
            sort_key(&digit).0,
            xref_sort_key(&xref("0000000000000000")?)
        );
        Ok(())
    }

    // audit: CBSTM03A-008
    #[test]
    fn the_table_reports_when_it_leaves_the_cobol_bounds() -> TestResult {
        let records =
            |cards: u32, per_card: u32| -> Result<Vec<TransactionRecord>, Box<dyn Error>> {
                let mut out = Vec::new();
                for card in 1..=cards {
                    for n in 1..=per_card {
                        let id = format!("{card:08}{n:08}");
                        out.push(transaction(&format!("{card:016}"), &id, "x", 1)?);
                    }
                }
                Ok(out)
            };
        let cases: [(u32, u32, bool); 3] = [(51, 10, true), (52, 1, false), (1, 11, false)];
        for (cards, per_card, within) in cases {
            let records = records(cards, per_card)?;
            let table = TransactionTable::build(&records)?;
            assert_eq!(table.card_count(), usize::try_from(cards)?);
            assert_eq!(table.within_cobol_bounds(), within, "{cards} x {per_card}");
            // The census of the same records, counted one at a time in card order, agrees.
            let mut census = TableCensus::default();
            for record in &records {
                census.count(record);
            }
            let census = census.finish()?;
            assert_eq!(
                census.card_count(),
                table.card_count(),
                "{cards} x {per_card}"
            );
            assert_eq!(census.within_cobol_bounds(), within, "{cards} x {per_card}");
        }
        Ok(())
    }

    /// The census opens one entry per run of one card number and keeps the widest entry's
    /// count; with no record counted it ends as the first read at end of file; its `Debug`
    /// shows no card number.
    // audit: CBSTM03A-006, CBSTM03A-007, CBSTM03A-008
    #[test]
    fn the_census_counts_card_entries_and_the_widest_one_record_at_a_time() -> TestResult {
        assert_eq!(
            TableCensus::default().finish(),
            Err(StatementError::NoTransactions)
        );
        let records = [
            transaction(CARD, "0000000000000001", "a1", 100)?,
            transaction(CARD, "0000000000000002", "a2", 100)?,
            transaction(CARD, "0000000000000003", "a3", 100)?,
            transaction(OTHER_CARD, "0000000000000004", "b", 100)?,
            transaction("4000000000000009", "0000000000000005", "c1", 100)?,
            transaction("4000000000000009", "0000000000000006", "c2", 100)?,
        ];
        let mut census = TableCensus::default();
        let mut counted = Vec::new();
        for record in &records {
            census.count(record);
            counted.push((census.card_count(), census.widest));
        }
        assert_eq!(counted, [(1, 1), (1, 2), (1, 3), (2, 3), (3, 3), (3, 3)]);
        let census = census.finish()?;
        assert_eq!(
            census.card_count(),
            TransactionTable::build(&records)?.card_count()
        );
        assert!(census.within_cobol_bounds());
        let shown = format!("{census:?}");
        assert!(!shown.contains("4000000000000009"), "{shown}");
        Ok(())
    }

    /// The statement of each card built from that card's records alone, given in byte order as
    /// the keyed read returns them, equals its statement from the table of every transaction:
    /// a letter-led ID still prints before a digit-led one, and a card without records gets the
    /// empty summary.
    // audit: CBSTM03A-001, CBSTM03A-008, CBSTM03A-013
    #[test]
    fn a_statement_from_its_card_alone_equals_one_from_every_transaction() -> TestResult {
        let ninth = "4000000000000009";
        let all = [
            transaction(CARD, "0000000000000001", "digits", 1_000)?,
            transaction(OTHER_CARD, "0000000000000002", "other", 2_000)?,
            transaction(CARD, "A000000000000001", "letter", -300)?,
            transaction(ninth, "0000000000000004", "ninth", 400)?,
            transaction(CARD, "0000000000000003", "later", 500)?,
        ];
        let every = TransactionTable::build(&all)?;
        let customer = customer(NAMES, ADDRESS, 623)?;
        let account = account(50, 194_587)?;
        for card in [CARD, OTHER_CARD, BARE_CARD, ninth] {
            let xref = xref(card)?;
            let mut rows: Vec<&TransactionRecord> = all
                .iter()
                .filter(|record| record.card_num == xref.card_num)
                .collect();
            rows.sort_by_key(|record| *record.id.as_bytes());
            let alone = TransactionTable::of_card(rows.iter().copied());
            assert_eq!(alone.card_count(), usize::from(!rows.is_empty()), "{card}");
            let from_every = statement(&xref, &customer, &account, &every);
            let from_alone = statement(&xref, &customer, &account, &alone);
            assert_eq!(from_alone.text_lines, from_every.text_lines, "{card}");
            assert_eq!(from_alone.html_records, from_every.html_records, "{card}");
        }
        // In any input order, the letter-led ID prints first.
        let card = xref(CARD)?;
        let reversed = all
            .iter()
            .rev()
            .filter(|record| record.card_num == card.card_num);
        let st = statement(
            &card,
            &customer,
            &account,
            &TransactionTable::of_card(reversed),
        );
        let ids: Vec<&[u8]> = [16, 17, 18]
            .iter()
            .filter_map(|&at| st.text_lines.get(at)?.get(..16))
            .collect();
        assert_eq!(
            ids,
            [
                b"A000000000000001",
                b"0000000000000001",
                b"0000000000000003"
            ]
        );
        let none: [TransactionRecord; 0] = [];
        let bare = statement(
            &xref(BARE_CARD)?,
            &customer,
            &account,
            &TransactionTable::of_card(&none),
        );
        assert_eq!(bare.text_lines.len(), 19);
        Ok(())
    }
}
