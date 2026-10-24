//! The nine load subcommands of `carddemo-batch` as pure rules: the eight IDCAMS jobs that
//! delete, define and REPRO a VSAM cluster from an ASCII fixture (ACCTFILE, CARDFILE, XREFFILE,
//! CUSTFILE, DISCGRP, TCATBALF, TRANCATG, TRANTYPE) and DUSRSECJ, which copies its in-stream
//! records with IEBGENER and then loads them the same way (`modern/audit/batch/LOADS.md`,
//! LOADS-001 to LOADS-009; AAP 0.2.3, 0.3.3.4, 0.3.4.3).
//!
//! This module turns the bytes of a load's input into typed, framed records and states how a
//! load replaces its table. It opens no file, takes no path and reaches no database: the
//! `batch::jobs` modules read the input in place (LOADS-010, LOADS-011, LOADS-019) and write the
//! records, and the test harness reuses the same decode.
//!
//! # Jobs
//!
//! [`LoadJob::ALL`] lists the jobs in the `scripts/dev-up.sh` order, the refresh order of
//! `scripts/remote_refresh.sh:16-44` less TRANFILE, which is out of scope (LOADS-026).
//!
//! | Subcommand | Input | Layout | `KEYS` / `RECORDSIZE` | Table | Rule |
//! |---|---|---|---|---|---|
//! | `acctfile` | `acctdata.txt` | `CVACT01Y` | `(11 0)` / 300 | `accounts` | LOADS-001 |
//! | `cardfile` | `carddata.txt` | `CVACT02Y` | `(16 0)` / 150 | `cards` | LOADS-002 |
//! | `xreffile` | `cardxref.txt` | `CVACT03Y` | `(16 0)` / 50 | `card_xref` | LOADS-003 |
//! | `custfile` | `custdata.txt` | `CVCUS01Y` | `(9 0)` / 500 | `customers` | LOADS-004 |
//! | `discgrp` | `discgrp.txt` | `CVTRA02Y` | `(16 0)` / 50 | `disclosure_groups` | LOADS-005 |
//! | `tcatbalf` | `tcatbal.txt` | `CVTRA01Y` | `(17 0)` / 50 | `tran_cat_balances` | LOADS-006 |
//! | `trancatg` | `trancatg.txt` | `CVTRA04Y` | `(6 0)` / 60 | `transaction_categories` | LOADS-007 |
//! | `trantype` | `trantype.txt` | `CVTRA03Y` | `(2 0)` / 60 | `transaction_types` | LOADS-008 |
//! | `dusrsecj` | in-stream records of `DUSRSECJ.jcl` | `CSUSR01Y` | `(8,0)` / 80 | `users` | LOADS-009 |
//!
//! The fixtures hold 50, 50, 50, 50, 51, 50, 18 and 7 records and the deck 10 (FIXLOAD-001 to
//! FIXLOAD-009).
//!
//! # Decoding
//!
//! - [`decode_fixture`] is codec A over a whole fixture: one framed record per line, its
//!   terminator and body length kept as framing (LOADS-030, LOADS-031, PIC-013, PIC-014, E04,
//!   E06), every `FILLER` byte kept (LOADS-032, E05), the dates typed by the layouts. The first
//!   line codec A rejects fails the decode, naming that line and the failing field; nothing is
//!   skipped, defaulted or normalised (LOADS-016, R7).
//! - [`check_key_sequence`] refuses a record key that is not above the previous one, before any
//!   write (LOADS-017).
//! - [`usrsec_instream_lines`] finds the DUSRSECJ in-stream records, and [`decode_usrsec`]
//!   decodes them as 80-byte card images and checks their key sequence (LOADS-012, LOADS-013,
//!   LOADS-017, LOADS-034).
//! - [`hash_user`] replaces `SEC-USR-PWD` with its argon2id hash for storage (LOADS-033, E25).
//!
//! An input that holds no record decodes to an empty set without error. Whether a load may then
//! replace its table with no rows is escalation candidate X-LOADS-5 of `LOADS.md`, which awaits
//! the reviewer; no rule row asserts the case, and nothing here decides it.
//!
//! # Replacement
//!
//! - Each job replaces its [`LoadJob::target_table`] inside one `PostgreSQL` transaction: it
//!   deletes every row, then inserts every decoded record through codec B, the counterpart of
//!   IDCAMS DELETE CLUSTER, DEFINE CLUSTER and REPRO into the new, empty cluster. Once it
//!   commits, the table holds exactly the input set (LOADS-014); an empty table is no error
//!   (LOADS-015).
//! - Every failure leaves the previous contents: a decode or key error before the transaction
//!   opens, a store error by rolling it back (LOADS-016 to LOADS-019).
//! - A load issues no DDL and never migrates; the tables, keys and indexes come from the
//!   migrations `carddemo-migrate` applies (LOADS-020 to LOADS-024).
//! - No foreign key joins the nine tables, so the loads succeed in any order, on an empty or a
//!   loaded database, and a rerun leaves the same copybook-derived contents (LOADS-025,
//!   LOADS-026). A load writes only its own table (LOADS-028).
//! - `trancatg` and `trantype` load only the VSAM data version, `transaction_categories` and
//!   `transaction_types`; schema `db2`, seeded by its migration, is never read or written, and
//!   nothing is copied between the versions (LOADS-029, E23).
//! - `LineFraming` describes the file line and is never stored: codec B writes only
//!   `Framed::record` (LOADS-030, LOADS-031).
//!
//! # Redaction and totality
//!
//! Every decoder here takes bytes from outside the process and is total: for any input of any
//! length it returns a value or a typed [`LoadError`] and never panics (Total Parsers Over
//! Untrusted Input). No function indexes, slices or panics. A [`LoadError`] carries line numbers,
//! the copybook field name, offset and width, never a byte of a record, and a [`HashedUser`]
//! prints neither the password nor its hash, so card numbers, CVVs, SSNs and passwords stay
//! inside the records (LOADS-016, LOADS-039, Cardholder Data Redaction).

use core::fmt;

use copybook::{Error as CodecError, FixedRecord, Framed, Line, lines};

use crate::password;
use crate::records::{
    AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
    TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord, UserSecurityRecord,
};

/// The DD statement that opens the in-stream records, `DUSRSECJ.jcl:34` (LOADS-012).
const SYSUT1_DD_STAR: &[u8] = b"//SYSUT1   DD *";

/// The delimiter that closes in-stream data: a line beginning with these bytes,
/// `DUSRSECJ.jcl:45` (LOADS-012).
const INSTREAM_DELIMITER: &[u8] = b"/*";

/// The byte that pads a card to its 80 columns, and the only byte that may follow
/// [`SYSUT1_DD_STAR`] on its card.
const SPACE: u8 = b' ';

/// One of the nine load subcommands (LOADS-001 to LOADS-009).
///
/// The variants are declared in the `scripts/dev-up.sh` order, which [`LoadJob::ALL`] repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadJob {
    /// ACCTFILE: `acctdata.txt` into `accounts`, `KEYS(11 0)`, `RECORDSIZE(300 300)`
    /// (`ACCTFILE.jcl:40-41,56-61`; LOADS-001, FIXLOAD-001).
    Acctfile,
    /// CARDFILE: `carddata.txt` into `cards`, `KEYS(16 0)`, `RECORDSIZE(150 150)`, its AIX
    /// `KEYS(11 16)` the index on `cards.acct_id` (`CARDFILE.jcl:54-55,70-75,85`; LOADS-002,
    /// LOADS-022, FIXLOAD-002).
    Cardfile,
    /// XREFFILE: `cardxref.txt` into `card_xref`, `KEYS(16 0)`, `RECORDSIZE(50 50)`, its AIX
    /// `KEYS(11,25)` the index on `card_xref.acct_id`; every body is 36 bytes, the missing tail
    /// its `FILLER` (`XREFFILE.jcl:43-44,59-64,74`; LOADS-003, LOADS-023, LOADS-030,
    /// FIXLOAD-003).
    Xreffile,
    /// CUSTFILE: `custdata.txt` into `customers`, `KEYS(9 0)`, `RECORDSIZE(500 500)`
    /// (`CUSTFILE.jcl:50-51,66-71`; LOADS-004, FIXLOAD-004).
    Custfile,
    /// DISCGRP: `discgrp.txt` into `disclosure_groups`, `KEYS(16 0)`, `RECORDSIZE(50 50)`
    /// (`DISCGRP.jcl:40-41,56-61`; LOADS-005, FIXLOAD-005).
    Discgrp,
    /// TCATBALF: `tcatbal.txt` into `tran_cat_balances`, `KEYS(17 0)`, `RECORDSIZE(50 50)`
    /// (`TCATBALF.jcl:40-41,56-61`; LOADS-006, FIXLOAD-006).
    Tcatbalf,
    /// TRANCATG: `trancatg.txt` into `transaction_categories`, the VSAM data version,
    /// `KEYS(6 0)`, `RECORDSIZE(60 60)` (`TRANCATG.jcl:40-41,56-61`; LOADS-007, LOADS-029,
    /// FIXLOAD-007).
    Trancatg,
    /// TRANTYPE: `trantype.txt` into `transaction_types`, the VSAM data version, `KEYS(2 0)`,
    /// `RECORDSIZE(60 60)` (`TRANTYPE.jcl:40-41,56-61`; LOADS-008, LOADS-029, FIXLOAD-008).
    Trantype,
    /// DUSRSECJ: the in-stream records of `DUSRSECJ.jcl:35-44` into `users`, `KEYS(8,0)`,
    /// `RECORDSIZE(80,80)`, the password stored only in hashed form
    /// (`DUSRSECJ.jcl:34-48,64-66,87`; LOADS-009, LOADS-012, LOADS-033, FIXLOAD-009).
    Dusrsecj,
}

impl LoadJob {
    /// The nine jobs in the `scripts/dev-up.sh` order: `acctfile`, `cardfile`, `xreffile`,
    /// `custfile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype`, `dusrsecj`
    /// (`remote_refresh.sh:16-44` less TRANFILE; LOADS-026, FIXLOAD-010).
    pub const ALL: [LoadJob; 9] = [
        Self::Acctfile,
        Self::Cardfile,
        Self::Xreffile,
        Self::Custfile,
        Self::Discgrp,
        Self::Tcatbalf,
        Self::Trancatg,
        Self::Trantype,
        Self::Dusrsecj,
    ];

    /// The `carddemo-batch` subcommand that runs the job, the JCL member name in lower case
    /// (LOADS-001 to LOADS-009; AAP 0.2.3).
    #[must_use]
    pub const fn subcommand(self) -> &'static str {
        match self {
            Self::Acctfile => "acctfile",
            Self::Cardfile => "cardfile",
            Self::Xreffile => "xreffile",
            Self::Custfile => "custfile",
            Self::Discgrp => "discgrp",
            Self::Tcatbalf => "tcatbalf",
            Self::Trancatg => "trancatg",
            Self::Trantype => "trantype",
            Self::Dusrsecj => "dusrsecj",
        }
    }

    /// The fixture file the job loads, inside `CARDDEMO_FIXTURE_DIR`, or `None` for
    /// [`LoadJob::Dusrsecj`], whose input is the in-stream data of `DUSRSECJ.jcl` (LOADS-010,
    /// LOADS-012).
    #[must_use]
    pub const fn fixture_file(self) -> Option<&'static str> {
        match self {
            Self::Acctfile => Some("acctdata.txt"),
            Self::Cardfile => Some("carddata.txt"),
            Self::Xreffile => Some("cardxref.txt"),
            Self::Custfile => Some("custdata.txt"),
            Self::Discgrp => Some("discgrp.txt"),
            Self::Tcatbalf => Some("tcatbal.txt"),
            Self::Trancatg => Some("trancatg.txt"),
            Self::Trantype => Some("trantype.txt"),
            Self::Dusrsecj => None,
        }
    }

    /// The `RECORDSIZE` the job's DEFINE CLUSTER declares, in bytes: the `FixedRecord::WIDTH` of
    /// its layout, which a compile-time assertion below checks (LOADS-001 to LOADS-009).
    #[must_use]
    pub const fn record_width(self) -> usize {
        match self {
            Self::Acctfile => 300,
            Self::Cardfile => 150,
            Self::Custfile => 500,
            Self::Xreffile | Self::Discgrp | Self::Tcatbalf => 50,
            Self::Trancatg | Self::Trantype => 60,
            Self::Dusrsecj => 80,
        }
    }

    /// The length n of the record key `KEYS(n 0)`: the first n bytes of every record, compared
    /// by [`check_key_sequence`] (LOADS-017).
    #[must_use]
    pub const fn key_len(self) -> usize {
        match self {
            Self::Acctfile => 11,
            Self::Cardfile | Self::Xreffile | Self::Discgrp => 16,
            Self::Custfile => 9,
            Self::Tcatbalf => 17,
            Self::Trancatg => 6,
            Self::Trantype => 2,
            Self::Dusrsecj => 8,
        }
    }

    /// The table the job replaces, and the only one it writes (LOADS-001 to LOADS-009,
    /// LOADS-014, LOADS-028). `trancatg` and `trantype` name the VSAM data version, never the
    /// `db2` schema (LOADS-029, E23).
    #[must_use]
    pub const fn target_table(self) -> &'static str {
        match self {
            Self::Acctfile => "accounts",
            Self::Cardfile => "cards",
            Self::Xreffile => "card_xref",
            Self::Custfile => "customers",
            Self::Discgrp => "disclosure_groups",
            Self::Tcatbalf => "tran_cat_balances",
            Self::Trancatg => "transaction_categories",
            Self::Trantype => "transaction_types",
            Self::Dusrsecj => "users",
        }
    }
}

// The `RECORDSIZE` of each job's JCL equals the width of the copybook layout it loads.
const _: () = {
    assert!(AccountRecord::WIDTH == LoadJob::Acctfile.record_width());
    assert!(CardRecord::WIDTH == LoadJob::Cardfile.record_width());
    assert!(CardXrefRecord::WIDTH == LoadJob::Xreffile.record_width());
    assert!(CustomerRecord::WIDTH == LoadJob::Custfile.record_width());
    assert!(DisclosureGroupRecord::WIDTH == LoadJob::Discgrp.record_width());
    assert!(TranCatBalanceRecord::WIDTH == LoadJob::Tcatbalf.record_width());
    assert!(TransactionCategoryRecord::WIDTH == LoadJob::Trancatg.record_width());
    assert!(TransactionTypeRecord::WIDTH == LoadJob::Trantype.record_width());
    assert!(UserSecurityRecord::WIDTH == LoadJob::Dusrsecj.record_width());
};

/// Why a load's input was refused: where it failed and what failed.
///
/// `Display` and `Debug` show line numbers and the [`LoadErrorKind`] only, never a byte of the
/// input, so no card number, CVV, SSN or password can reach a log or an error text through it
/// (LOADS-016, LOADS-039, Cardholder Data Redaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadError {
    /// 1-based line of the input the failure belongs to, `None` when it belongs to no line.
    line: Option<usize>,
    /// What failed.
    kind: LoadErrorKind,
}

/// The class of a [`LoadError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadErrorKind {
    /// Codec A rejected the line, an in-stream card longer than 80 bytes included: the copybook
    /// error names the field, its offset within the record and its width (LOADS-013,
    /// LOADS-016, R7).
    Record(CodecError),
    /// No `//SYSUT1   DD *` statement opens the in-stream records, so the deck holds none
    /// (LOADS-013). The error belongs to no line.
    InStreamStartMissing,
    /// No line beginning `/*` follows the `//SYSUT1   DD *` statement, whose line the error
    /// names (LOADS-013).
    InStreamEndMissing,
    /// The record's key is not above the key of the record on `previous_line`: a duplicate or
    /// an out-of-sequence key (LOADS-017).
    KeySequence {
        /// 1-based line of the record whose key the failing key does not exceed.
        previous_line: usize,
    },
    /// argon2id could not hash a password. The error belongs to no line and carries nothing of
    /// the password (LOADS-033).
    Hash,
}

impl LoadError {
    /// A failure on the 1-based `line`.
    const fn at(line: usize, kind: LoadErrorKind) -> Self {
        Self {
            line: Some(line),
            kind,
        }
    }

    /// A failure that belongs to no line.
    const fn unplaced(kind: LoadErrorKind) -> Self {
        Self { line: None, kind }
    }

    /// The 1-based line of the input the failure belongs to: the fixture line for
    /// [`decode_fixture`] and [`check_key_sequence`], the line of `DUSRSECJ.jcl` for
    /// [`decode_usrsec`]. `None` for [`LoadErrorKind::InStreamStartMissing`] and
    /// [`LoadErrorKind::Hash`].
    #[must_use]
    pub const fn line(&self) -> Option<usize> {
        self.line
    }

    /// What failed.
    #[must_use]
    pub const fn kind(&self) -> LoadErrorKind {
        self.kind
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.kind),
            None => fmt::Display::fmt(&self.kind, f),
        }
    }
}

impl fmt::Display for LoadErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(err) => write!(f, "record rejected: {err}"),
            Self::InStreamStartMissing => {
                f.write_str("no '//SYSUT1   DD *' statement opens the in-stream records")
            }
            Self::InStreamEndMissing => f.write_str(
                "the in-stream records this statement opens are not closed by a line beginning '/*'",
            ),
            Self::KeySequence { previous_line } => write!(
                f,
                "record key is not above the key on line {previous_line}: \
                 a duplicate or out-of-sequence key"
            ),
            Self::Hash => f.write_str("password hashing failed"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Decodes every line of a fixture file into its framed record with codec A, in file order
/// (LOADS-001 to LOADS-008, LOADS-016, LOADS-030 to LOADS-032).
///
/// Line n of the input is record n of the result. Each record keeps the line's terminator and
/// body length in its framing, so [`copybook::framing::encode_all`] over the result reproduces
/// the input byte for byte, mixed CRLF and LF terminators and the 36-byte `cardxref.txt` bodies
/// included (PIC-013, PIC-014, E04, E06, FIXFRM-010). Nothing is trimmed, padded, skipped or
/// defaulted beyond what `Framed::decode` defines (R7). An input without a final LF ends with
/// a line framed `Terminator::None`, and an input of no bytes holds no line and decodes to no
/// record.
///
/// The decode applies no load rule beyond codec A, so `posttran` and the tests use it for
/// `dailytran.txt` too. A load also calls [`check_key_sequence`] on the result before it writes
/// (LOADS-017).
///
/// # Errors
///
/// [`LoadErrorKind::Record`] on the 1-based line of the first line codec A rejects: a body
/// longer than the record, a body shorter than it by more than its trailing `FILLER`, or a field
/// that fails to decode (LOADS-016; the typed errors of PIC-002 to PIC-016).
pub fn decode_fixture<R: FixedRecord>(bytes: &[u8]) -> Result<Vec<Framed<R>>, LoadError> {
    lines(bytes)
        .enumerate()
        .map(|(index, line)| {
            Framed::decode(line)
                .map_err(|err| LoadError::at(line_number(index), LoadErrorKind::Record(err)))
        })
        .collect()
}

/// Checks that every record's key, its first [`LoadJob::key_len`] bytes, is above the previous
/// record's, byte by byte, as `KEYS(n 0)` requires of the records a REPRO loads (LOADS-017,
/// FIXFRM-020).
///
/// `records` are the result of [`decode_fixture`], so record n came from line n and the error
/// names fixture lines. [`decode_usrsec`] applies the same check itself with the line numbers
/// of `DUSRSECJ.jcl`. A load runs the check before it opens its transaction, so a refused input
/// leaves the table unchanged. IDCAMS would instead report the record and load the others; that
/// result is no parity result, and no fixture reaches the case (X-LOADS-1).
///
/// # Errors
///
/// [`LoadErrorKind::KeySequence`] on the line of the first record whose key equals or precedes
/// the previous record's key, naming that previous line. Neither key is reported, since a key
/// can be a card number.
pub fn check_key_sequence<R: FixedRecord>(
    job: LoadJob,
    records: &[Framed<R>],
) -> Result<(), LoadError> {
    check_keys(
        job.key_len(),
        records
            .iter()
            .enumerate()
            .map(|(index, framed)| (line_number(index), &framed.record)),
    )
}

/// The in-stream records of the DUSRSECJ deck: the lines after the first `//SYSUT1   DD *`
/// statement, up to and not including the next line that begins `/*` (`DUSRSECJ.jcl:34-45`;
/// LOADS-012, FIXLOAD-009).
///
/// The statement line must be exactly `//SYSUT1   DD *`, followed by nothing but spaces, the
/// blank columns of its card. Each returned line keeps its body and framing as the file holds
/// them, so the ten lines of `DUSRSECJ.jcl` come back as their 57-byte bodies, each ended by LF.
/// Nothing after the delimiter is read.
///
/// # Errors
///
/// - [`LoadErrorKind::InStreamStartMissing`], on no line, when no line is the statement.
/// - [`LoadErrorKind::InStreamEndMissing`], on the statement's line, when no line after it
///   begins `/*` (LOADS-013).
pub fn usrsec_instream_lines(jcl: &[u8]) -> Result<Vec<Line<'_>>, LoadError> {
    Ok(instream(jcl)?.into_iter().map(|(_, line)| line).collect())
}

/// Decodes the in-stream records of the DUSRSECJ deck, as `users` receives them (LOADS-009,
/// LOADS-012, LOADS-013, LOADS-017, LOADS-034, FIXUSR-001 to FIXUSR-010).
///
/// Each line of [`usrsec_instream_lines`] is padded with spaces to the 80-byte card image that
/// IEBGENER copies (`LRECL=80`), and is otherwise unchanged, as `SYSIN DD DUMMY` edits nothing.
/// The image is decoded as `CSUSR01Y`, so `SEC-USR-FILLER` holds 23 spaces after a 57-byte line
/// and the record encodes to that same 80-byte image (X-COPYBOOKS-1 of `copybooks.md`). Each
/// record keeps its line's framing, so `Framed::encode` gives back the deck line itself:
/// [`copybook::framing::encode_all`] over the ten records of `DUSRSECJ.jcl` reproduces lines 35
/// to 44 byte for byte. A card is never cut. The keys are then checked as
/// [`check_key_sequence`] checks them, with `DUSRSECJ.jcl` line numbers.
///
/// # Errors
///
/// - [`LoadErrorKind::InStreamStartMissing`] or [`LoadErrorKind::InStreamEndMissing`] as
///   [`usrsec_instream_lines`] reports them (LOADS-013).
/// - [`LoadErrorKind::Record`] on the line of the first card codec A rejects: one longer than
///   80 bytes, the width mismatch of LOADS-013, or a field that fails to decode (LOADS-016).
/// - [`LoadErrorKind::KeySequence`] on the line of the first record whose `SEC-USR-ID` does not
///   exceed the previous one (LOADS-017).
pub fn decode_usrsec(jcl: &[u8]) -> Result<Vec<Framed<UserSecurityRecord>>, LoadError> {
    let deck = instream(jcl)?;
    let records = deck
        .iter()
        .map(|&(number, line)| {
            decode_card_image(line).map_err(|err| LoadError::at(number, LoadErrorKind::Record(err)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    check_keys(
        LoadJob::Dusrsecj.key_len(),
        deck.iter()
            .map(|&(number, _)| number)
            .zip(records.iter().map(|framed| &framed.record)),
    )?;
    Ok(records)
}

/// A `USRSEC` record ready for `users`: the decoded record and the argon2id PHC string that
/// `users.password_hash` stores in place of `SEC-USR-PWD` (LOADS-033, LOADS-034, E25).
///
/// `Debug` prints the record, whose password redacts, and hides the hash; no `Display` exists.
#[derive(Clone)]
pub struct HashedUser {
    /// The decoded record. Its `pwd` field is never stored: the store writes every other field
    /// and [`HashedUser::password_hash`] (CSUSR01Y-004).
    pub record: UserSecurityRecord,
    /// The argon2id PHC string over the record's 8 `SEC-USR-PWD` bytes, beginning `$argon2id$`.
    pub password_hash: String,
}

impl fmt::Debug for HashedUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashedUser")
            .field("record", &self.record)
            .field("password_hash", &format_args!("<redacted>"))
            .finish()
    }
}

/// Hashes a record's `SEC-USR-PWD` with argon2id for `users.password_hash` (LOADS-033, E25).
///
/// The hash covers exactly the 8 stored bytes, case and padding as the deck holds them: no case
/// folding happens here, so a lower-case password would never verify at sign-on, which is the
/// COBOL's behaviour (E26). Each call draws a new salt, so a rerun stores a different string that
/// `crate::password::verify` accepts for the same bytes.
///
/// # Errors
///
/// [`LoadErrorKind::Hash`], on no line, when the salt cannot be drawn or argon2id fails. The
/// error carries nothing of the password.
pub fn hash_user(record: UserSecurityRecord) -> Result<HashedUser, LoadError> {
    let password_hash = password::hash(record.pwd.expose())
        .map_err(|_| LoadError::unplaced(LoadErrorKind::Hash))?;
    Ok(HashedUser {
        record,
        password_hash,
    })
}

/// The 1-based number of the line at 0-based `index`.
///
/// A slice holds at most `isize::MAX` bytes and every line consumes at least one of them, so an
/// index never reaches `usize::MAX` and the addition never saturates.
const fn line_number(index: usize) -> usize {
    index.saturating_add(1)
}

/// The in-stream lines of `jcl` with their 1-based line numbers; see [`usrsec_instream_lines`].
fn instream(jcl: &[u8]) -> Result<Vec<(usize, Line<'_>)>, LoadError> {
    let mut numbered = lines(jcl)
        .enumerate()
        .map(|(index, line)| (line_number(index), line));
    let Some((start, _)) = numbered.find(|(_, line)| opens_instream(line.body)) else {
        return Err(LoadError::unplaced(LoadErrorKind::InStreamStartMissing));
    };
    let mut deck = Vec::new();
    for (number, line) in numbered {
        if line.body.starts_with(INSTREAM_DELIMITER) {
            return Ok(deck);
        }
        deck.push((number, line));
    }
    Err(LoadError::at(start, LoadErrorKind::InStreamEndMissing))
}

/// Whether a line body is the `//SYSUT1   DD *` statement, followed by blank columns only.
fn opens_instream(body: &[u8]) -> bool {
    body.strip_prefix(SYSUT1_DD_STAR)
        .is_some_and(|rest| rest.iter().all(|&byte| byte == SPACE))
}

/// Decodes one in-stream line as the 80-byte card image IEBGENER copies (LOADS-012), keeping
/// the line's own framing.
fn decode_card_image(line: Line<'_>) -> Result<Framed<UserSecurityRecord>, CodecError> {
    let mut image = line.body.to_vec();
    // Pads a short card and leaves a longer one whole, so the decode reports it as the width
    // mismatch of LOADS-013 instead of cutting it.
    image.resize(image.len().max(UserSecurityRecord::WIDTH), SPACE);
    Ok(Framed {
        record: UserSecurityRecord::decode(&image)?,
        framing: line.framing,
    })
}

/// Fails on the first record whose key, its first `key_len` encoded bytes, does not exceed the
/// previous record's, given each record with its 1-based line (LOADS-017).
fn check_keys<'r, R: FixedRecord + 'r>(
    key_len: usize,
    records: impl IntoIterator<Item = (usize, &'r R)>,
) -> Result<(), LoadError> {
    let mut previous: Option<(usize, Vec<u8>)> = None;
    for (line, record) in records {
        let mut key = record.encode();
        key.truncate(key_len);
        if let Some((previous_line, _)) = previous
            .as_ref()
            .filter(|(_, previous_key)| key <= *previous_key)
        {
            return Err(LoadError::at(
                line,
                LoadErrorKind::KeySequence {
                    previous_line: *previous_line,
                },
            ));
        }
        previous = Some((line, key));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::framing::encode_all;
    use copybook::{ErrorKind, FixedRecord, Framed, Line, Terminator};

    use super::{
        LoadError, LoadErrorKind, LoadJob, check_key_sequence, decode_fixture, decode_usrsec,
        hash_user, usrsec_instream_lines,
    };
    use crate::password;
    use crate::records::{TransactionTypeRecord, UserSecurityRecord};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Type codes in file order, and the line whose key fails with the line it does not exceed.
    type KeyCase = (&'static [&'static [u8; 2]], Option<(usize, usize)>);

    /// A deck shaped as `DUSRSECJ.jcl`: the statement on line 3, two in-stream records on lines
    /// 4 and 5 (the 57-byte cards of `DUSRSECJ.jcl:35,40`), the delimiter on line 6 and a
    /// statement after it.
    const DECK: &[u8] = b"//DUSRSECJ JOB 'DEF USRSEC FILE'\n\
//STEP01  EXEC PGM=IEBGENER\n\
//SYSUT1   DD *\n\
ADMIN001MARGARET            GOLD                PASSWORDA\n\
USER0001LAWRENCE            THOMAS              PASSWORDU\n\
/*\n\
//SYSUT2   DD DUMMY\n";

    /// The two in-stream lines of [`DECK`], terminators included.
    const DECK_RECORDS: &[u8] = b"ADMIN001MARGARET            GOLD                PASSWORDA\n\
USER0001LAWRENCE            THOMAS              PASSWORDU\n";

    /// A 60-byte `CVTRA03Y` body: the type code, the description padded to 50 bytes and the
    /// 8-byte `FILLER` of `0` characters every `trantype.txt` line holds (E05).
    fn type_body(code: &[u8], desc: &[u8]) -> Vec<u8> {
        let mut body = code.to_vec();
        body.extend_from_slice(desc);
        body.resize(52, b' ');
        body.extend_from_slice(b"00000000");
        body
    }

    /// A `trantype.txt`-shaped file: one body per code, each ended by `terminator`.
    fn type_file(codes: &[&[u8; 2]], terminator: Terminator) -> Vec<u8> {
        codes
            .iter()
            .flat_map(|code| {
                let mut line = type_body(code.as_slice(), b"Purchase");
                line.extend_from_slice(terminator.as_bytes());
                line
            })
            .collect()
    }

    #[test]
    fn jobs_follow_the_dev_up_order_with_their_inputs_widths_keys_and_tables() {
        let expected = [
            ("acctfile", Some("acctdata.txt"), 300, 11, "accounts"),
            ("cardfile", Some("carddata.txt"), 150, 16, "cards"),
            ("xreffile", Some("cardxref.txt"), 50, 16, "card_xref"),
            ("custfile", Some("custdata.txt"), 500, 9, "customers"),
            ("discgrp", Some("discgrp.txt"), 50, 16, "disclosure_groups"),
            ("tcatbalf", Some("tcatbal.txt"), 50, 17, "tran_cat_balances"),
            (
                "trancatg",
                Some("trancatg.txt"),
                60,
                6,
                "transaction_categories",
            ),
            ("trantype", Some("trantype.txt"), 60, 2, "transaction_types"),
            ("dusrsecj", None, 80, 8, "users"),
        ];
        let actual = LoadJob::ALL.map(|job| {
            (
                job.subcommand(),
                job.fixture_file(),
                job.record_width(),
                job.key_len(),
                job.target_table(),
            )
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_fixture_decodes_line_by_line_and_re_encodes_identically() -> TestResult {
        let mut input = type_file(&[b"01", b"02"], Terminator::CrLf);
        input.extend_from_slice(&type_file(&[b"03"], Terminator::Lf));
        input.extend_from_slice(&type_body(b"04", b"no terminator"));

        let records = decode_fixture::<TransactionTypeRecord>(&input)?;

        let ends: Vec<Terminator> = records.iter().map(|f| f.framing.terminator).collect();
        assert_eq!(
            ends,
            [
                Terminator::CrLf,
                Terminator::CrLf,
                Terminator::Lf,
                Terminator::None
            ]
        );
        assert!(records.iter().all(|f| f.framing.body_len == 60));
        assert_eq!(
            records.first().map(|f| f.record.filler.as_bytes()),
            Some(b"00000000")
        );
        assert!(encode_all(&records) == input, "re-encoded fixture differs");
        assert!(decode_fixture::<TransactionTypeRecord>(b"")?.is_empty());
        Ok(())
    }

    #[test]
    fn the_first_rejected_line_fails_the_decode_with_its_number_and_no_bytes() -> TestResult {
        let mut input = type_file(&[b"01"], Terminator::Lf);
        // Line 2 is one byte short of the record, which has no trailing `FILLER` to lack.
        let mut short = type_body(b"02", b"Purchase");
        short.truncate(59);
        input.extend_from_slice(&short);
        input.push(b'\n');
        // Line 3 holds a CR inside its body, which line 2 stops the decode from reaching.
        input.extend_from_slice(&type_body(b"03", b"Pur\rchase"));

        let err = decode_fixture::<TransactionTypeRecord>(&input)
            .err()
            .ok_or("the short line was accepted")?;

        assert_eq!(err.line(), Some(2));
        assert_eq!(
            err.kind(),
            LoadErrorKind::Record(copybook::Error::new(
                "record",
                0,
                60,
                ErrorKind::ShortBodyNotFiller {
                    expected: 60,
                    actual: 59,
                },
            ))
        );
        let text = err.to_string();
        assert!(text.starts_with("line 2: record rejected: record (offset 0, width 60)"));
        assert!(!text.contains("Purchase"));
        Ok(())
    }

    #[test]
    fn keys_must_rise_strictly_through_the_first_key_len_bytes() -> TestResult {
        let cases: [KeyCase; 5] = [
            (&[b"01", b"02", b"07"], None),
            (&[], None),
            (&[b"01", b"01"], Some((2, 1))),
            (&[b"01", b"03", b"02"], Some((3, 2))),
            (&[b"02", b"01", b"03"], Some((2, 1))),
        ];
        for (codes, expected) in cases {
            let records =
                decode_fixture::<TransactionTypeRecord>(&type_file(codes, Terminator::Lf))?;
            let outcome = check_key_sequence(LoadJob::Trantype, &records)
                .map_err(|err| (err.line(), err.kind()));
            let wanted = expected.map_or(Ok(()), |(line, previous_line)| {
                Err((Some(line), LoadErrorKind::KeySequence { previous_line }))
            });
            assert_eq!(outcome, wanted, "{} records", codes.len());
        }
        // Records that differ only beyond the 2-byte key still duplicate it.
        let mut same_key = type_body(b"01", b"Purchase");
        same_key.push(b'\n');
        same_key.extend_from_slice(&type_body(b"01", b"Payment"));
        let records = decode_fixture::<TransactionTypeRecord>(&same_key)?;
        let err = check_key_sequence(LoadJob::Trantype, &records)
            .err()
            .ok_or("a duplicate key was accepted")?;
        assert_eq!(err.line(), Some(2));
        assert_eq!(
            err.to_string(),
            "line 2: record key is not above the key on line 1: a duplicate or out-of-sequence key"
        );
        Ok(())
    }

    #[test]
    fn the_deck_yields_the_lines_between_the_statement_and_the_delimiter() -> TestResult {
        let deck = usrsec_instream_lines(DECK)?;
        assert_eq!(
            deck,
            [
                Line::new(
                    b"ADMIN001MARGARET            GOLD                PASSWORDA",
                    Terminator::Lf
                ),
                Line::new(
                    b"USER0001LAWRENCE            THOMAS              PASSWORDU",
                    Terminator::Lf
                ),
            ]
        );
        // Blank columns after the statement are allowed; an empty data set is no error.
        let blank = usrsec_instream_lines(b"//SYSUT1   DD *   \r\n/*\n")?;
        assert!(blank.is_empty());
        Ok(())
    }

    #[test]
    fn in_stream_records_decode_as_80_byte_card_images_in_their_own_framing() -> TestResult {
        let records = decode_usrsec(DECK)?;

        assert_eq!(records.len(), 2);
        assert!(encode_all(&records) == DECK_RECORDS, "deck lines differ");
        assert!(records.iter().all(|f| f.framing.body_len == 57));
        assert!(
            records
                .iter()
                .all(|f| f.record.filler.as_bytes() == &[b' '; 23])
        );
        let first = records.first().ok_or("no record")?;
        let image = first.record.encode();
        assert_eq!(image.len(), UserSecurityRecord::WIDTH);
        assert!(image.starts_with(b"ADMIN001MARGARET") && image.ends_with(&[b' '; 23]));
        assert!(first.record.is_admin());

        // A card shorter than the record's data fields is padded, never refused.
        let short = decode_usrsec(b"//SYSUT1   DD *\nUSER0009\n/*\n")?;
        assert_eq!(short.len(), 1);
        let framed = short.first();
        assert_eq!(framed.map(|f| f.record.usr_type.as_bytes()), Some(b" "));
        assert_eq!(framed.map(Framed::encode), Some(b"USER0009\n".to_vec()));
        Ok(())
    }

    #[test]
    fn deck_errors_name_their_line_and_class() -> TestResult {
        let mut long_card = b"//SYSUT1   DD *\n".to_vec();
        long_card.extend_from_slice(&[b'A'; 81]);
        long_card.extend_from_slice(b"\n/*\n");
        let swapped = b"//SYSUT1   DD *\n\
USER0001LAWRENCE            THOMAS              PASSWORDU\n\
ADMIN001MARGARET            GOLD                PASSWORDA\n/*\n";
        let too_long = copybook::Error::new(
            "record",
            0,
            80,
            ErrorKind::WidthMismatch {
                expected: 80,
                actual: 81,
            },
        );
        let control_byte = copybook::Error::new("SEC-USR-ID", 0, 8, ErrorKind::NonPrintable);
        let cases: [(&[u8], Option<usize>, LoadErrorKind); 6] = [
            (b"", None, LoadErrorKind::InStreamStartMissing),
            (
                b"//SYSUT1   DD *,DLM=$$\nX\n/*\n",
                None,
                LoadErrorKind::InStreamStartMissing,
            ),
            (
                b"/*\n//SYSUT1   DD *\nUSER0009\n",
                Some(2),
                LoadErrorKind::InStreamEndMissing,
            ),
            (&long_card, Some(2), LoadErrorKind::Record(too_long)),
            (
                b"//SYSUT1   DD *\nUSER\x7f001\n/*\n",
                Some(2),
                LoadErrorKind::Record(control_byte),
            ),
            (
                swapped,
                Some(3),
                LoadErrorKind::KeySequence { previous_line: 2 },
            ),
        ];
        for (deck, line, kind) in cases {
            let err = decode_usrsec(deck).err().ok_or("the deck was accepted")?;
            assert_eq!((err.line(), err.kind()), (line, kind));
            let text = err.to_string();
            assert!(
                !text.contains("PASSWORD") && !text.contains("USER0"),
                "{text}"
            );
        }
        let unclosed = usrsec_instream_lines(b"//SYSUT1   DD *\n")
            .err()
            .ok_or("an unclosed deck was accepted")?;
        assert_eq!(
            unclosed.to_string(),
            "line 1: the in-stream records this statement opens are not closed by a line \
             beginning '/*'"
        );
        assert_eq!(
            LoadError::unplaced(LoadErrorKind::InStreamStartMissing).to_string(),
            "no '//SYSUT1   DD *' statement opens the in-stream records"
        );
        assert_eq!(
            LoadError::unplaced(LoadErrorKind::Hash).to_string(),
            "password hashing failed"
        );
        Ok(())
    }

    #[test]
    fn a_hashed_user_keeps_its_record_and_verifies_its_password_bytes() -> TestResult {
        let record = decode_usrsec(DECK)?
            .into_iter()
            .next()
            .map(|framed| framed.record)
            .ok_or("no record")?;
        let pwd = *record.pwd.expose();

        let hashed = hash_user(record.clone())?;

        assert!(hashed.record == record);
        assert!(hashed.password_hash.starts_with("$argon2id$"));
        assert!(!hashed.password_hash.contains("PASSWORD"));
        assert_eq!(password::verify(&pwd, &hashed.password_hash), Ok(true));
        assert_eq!(
            password::verify(b"password", &hashed.password_hash),
            Ok(false)
        );
        let debug = format!("{hashed:?}");
        assert!(debug.contains("<redacted>") && debug.contains("ADMIN001"));
        assert!(!debug.contains(hashed.password_hash.as_str()) && !debug.contains("PASSWORD"));
        Ok(())
    }
}
