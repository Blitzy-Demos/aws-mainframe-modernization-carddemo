//! Codec B for table `cards`: the card master record, `CVACT02Y` `CARD-RECORD`
//! (`app/cpy/CVACT02Y.cpy:4-11`), rows CVACT02Y-001 to CVACT02Y-008 of
//! `modern/audit/copybooks.md`.
//!
//! The `CARDDATA` KSDS, `KEYS(16 0)`, is primary key `card_num`, and its non-unique alternate
//! index on `CARD-ACCT-ID`, `KEYS(11 16)` (`app/jcl/CARDFILE.jcl:54,83-86`), is index
//! `cards_acct_id_idx` (migration 0001). Every column converts through the shared functions of
//! the parent module:
//!
//! | Field (row) | Picture | Column | Row field |
//! |---|---|---|---|
//! | `CARD-NUM` (CVACT02Y-001) | `X(16)` | `card_num CHAR(16)` | `String` (PIC-001, PIC-024) |
//! | `CARD-ACCT-ID` (CVACT02Y-002) | `9(11)` | `acct_id BIGINT` | `i64` (PIC-003, PIC-023) |
//! | `CARD-CVV-CD` (CVACT02Y-003) | `9(03)` | `card_cvv_cd BIGINT` | `i64` (PIC-003, PIC-024) |
//! | `CARD-EMBOSSED-NAME` (CVACT02Y-004) | `X(50)` | `card_embossed_name CHAR(50)` | `String` (PIC-001) |
//! | `CARD-EXPIRAION-DATE` (CVACT02Y-005) | `X(10)` date | `card_expiraion_date DATE` | `Option<NaiveDate>` (PIC-015, PIC-016) |
//! | `CARD-ACTIVE-STATUS` (CVACT02Y-006) | `X(01)` | `card_active_status CHAR(1)` | `String` (PIC-001) |
//! | `FILLER` (CVACT02Y-007) | `X(59)` | `filler CHAR(59)` | `String` (PIC-012) |
//! | none (CVACT02Y-008) | none | `card_ref BIGINT GENERATED ALWAYS AS IDENTITY` | `i64` |
//!
//! `card_ref` is not a copybook field: it is the opaque handle that card list rows, page keys
//! and endpoints 8 to 10 carry in place of the card number (Cardholder Data Redaction, `E32`).
//! Codec A has no field for it, so the handle travels beside the record as a [`CardRef`], never
//! inside it. The handle flows one way, hence two structs: [`CardRow`] is what a read returns,
//! all eight columns in the order the paginated queries `queries/cards_page_forward.sql` and
//! `queries/cards_page_backward.sql` select them, and [`CardBinds`] is what an insert or update
//! binds, the seven copybook columns only. The column is `GENERATED ALWAYS`, so the database
//! assigns it and refuses a bound value.
//!
//! # Redaction
//!
//! `card_num` and `card_cvv_cd` hold the full card number and CVV, which the database keeps in
//! full (PIC-024, `E32`). The record yields them only through `expose()`, and neither struct
//! derives `Debug`, `Clone` or `Serialize`, so the raw values cannot be formatted, duplicated or
//! serialised from here. Errors name the column and the class of the failure, never the value.
//! The redacted forms are the business of the code that sends data out of the process.

use chrono::NaiveDate;
use domain::online::cocrdlic::CardRef;
use domain::records::card::CardRecord;
use domain::sensitive::{CardNumber, Cvv};

use super::{
    column_to_date, date_to_column, i64_to_pic_u, pic_u_to_i64, pic_x_to_string, string_to_pic_x,
};
use crate::error::PersistenceError;

/// Column of `CARD-NUM` (CVACT02Y-001).
const CARD_NUM: &str = "card_num";
/// Column of `CARD-ACCT-ID`, under the shared account-key name (CVACT02Y-002).
const ACCT_ID: &str = "acct_id";
/// Column of `CARD-CVV-CD` (CVACT02Y-003).
const CARD_CVV_CD: &str = "card_cvv_cd";
/// Column of `CARD-EMBOSSED-NAME` (CVACT02Y-004).
const CARD_EMBOSSED_NAME: &str = "card_embossed_name";
/// Column of `CARD-EXPIRAION-DATE`, the COBOL spelling kept (CVACT02Y-005).
const CARD_EXPIRAION_DATE: &str = "card_expiraion_date";
/// Column of `CARD-ACTIVE-STATUS` (CVACT02Y-006).
const CARD_ACTIVE_STATUS: &str = "card_active_status";
/// Column of the trailing `FILLER` (CVACT02Y-007).
const FILLER: &str = "filler";

/// One `cards` row as a read returns it: every column, `card_ref` included, each field named
/// after its column, so that `sqlx::query_file_as!` fills it from the select list of the
/// paginated card queries and from every other card read (Bounded Pagination).
///
/// `Debug` is not derived: `card_num` and `card_cvv_cd` hold the full card number and CVV
/// (PIC-024, `E32`).
pub struct CardRow {
    /// `card_num CHAR(16)`, `CARD-NUM PIC X(16)`: the full card number (CVACT02Y-001).
    pub(crate) card_num: String,
    /// `acct_id BIGINT`, `CARD-ACCT-ID PIC 9(11)` (CVACT02Y-002).
    pub(crate) acct_id: i64,
    /// `card_cvv_cd BIGINT`, `CARD-CVV-CD PIC 9(03)`: the CVV (CVACT02Y-003).
    pub(crate) card_cvv_cd: i64,
    /// `card_embossed_name CHAR(50)`, `CARD-EMBOSSED-NAME PIC X(50)` (CVACT02Y-004).
    pub(crate) card_embossed_name: String,
    /// `card_expiraion_date DATE`, `NULL` for a blank `CARD-EXPIRAION-DATE` (CVACT02Y-005).
    pub(crate) card_expiraion_date: Option<NaiveDate>,
    /// `card_active_status CHAR(1)`, `CARD-ACTIVE-STATUS PIC X(01)` (CVACT02Y-006).
    pub(crate) card_active_status: String,
    /// `filler CHAR(59)`, the trailing `FILLER PIC X(59)` (CVACT02Y-007).
    pub(crate) filler: String,
    /// `card_ref BIGINT GENERATED ALWAYS AS IDENTITY`, the opaque handle (CVACT02Y-008).
    pub(crate) card_ref: i64,
}

impl CardRow {
    /// The card's handle and the record its seven copybook columns hold, every field rebuilt
    /// with the constructors codec A's decode uses, so a row written from a record reads back
    /// equal to it and re-encodes to the original 150 bytes (binding d9).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the first column, in select-list order, whose value
    /// does not fit its picture:
    ///
    /// - `card_num`, `card_embossed_name`, `card_active_status` or `filler` holding a NUL or
    ///   non-ASCII character (PIC-002), or text other than exactly 16, 50, 1 or 59 bytes;
    /// - `acct_id` or `card_cvv_cd` negative or above `10^11 − 1` or `999` (PIC-003);
    /// - `card_expiraion_date` with a year outside `0..=9999` (PIC-015).
    ///
    /// The error never holds the value it rejected (Cardholder Data Redaction).
    pub fn into_record(self) -> Result<(CardRef, CardRecord), PersistenceError> {
        let record = CardRecord {
            num: CardNumber::new(string_to_pic_x(CARD_NUM, &self.card_num)?),
            acct_id: i64_to_pic_u(ACCT_ID, self.acct_id)?,
            cvv_cd: Cvv::new(i64_to_pic_u(CARD_CVV_CD, self.card_cvv_cd)?),
            embossed_name: string_to_pic_x(CARD_EMBOSSED_NAME, &self.card_embossed_name)?,
            expiraion_date: column_to_date(CARD_EXPIRAION_DATE, self.card_expiraion_date)?,
            active_status: string_to_pic_x(CARD_ACTIVE_STATUS, &self.card_active_status)?,
            filler: string_to_pic_x(FILLER, &self.filler)?,
        };
        Ok((CardRef::new(self.card_ref), record))
    }
}

/// The values an insert or update of `cards` binds: the seven copybook columns, in the order of
/// [`CardRow`], without `card_ref`, which the database assigns (CVACT02Y-008).
///
/// `Debug` is not derived: `card_num` and `card_cvv_cd` hold the full card number and CVV
/// (PIC-024, `E32`).
pub struct CardBinds {
    /// `card_num CHAR(16)`: the full card number (CVACT02Y-001).
    pub(crate) card_num: String,
    /// `acct_id BIGINT` (CVACT02Y-002).
    pub(crate) acct_id: i64,
    /// `card_cvv_cd BIGINT`: the CVV (CVACT02Y-003).
    pub(crate) card_cvv_cd: i64,
    /// `card_embossed_name CHAR(50)` (CVACT02Y-004).
    pub(crate) card_embossed_name: String,
    /// `card_expiraion_date DATE`, `None` for a blank date (CVACT02Y-005).
    pub(crate) card_expiraion_date: Option<NaiveDate>,
    /// `card_active_status CHAR(1)` (CVACT02Y-006).
    pub(crate) card_active_status: String,
    /// `filler CHAR(59)`, its bytes kept whatever they hold (CVACT02Y-007).
    pub(crate) filler: String,
}

impl CardBinds {
    /// The column values of `record`: every `PIC X(n)` field's bytes verbatim, padding and
    /// `FILLER` included (PIC-001, PIC-012); the account key and the CVV as `BIGINT` values
    /// (PIC-003, PIC-023); the expiry date as a `DATE`, `None` when blank (PIC-015). The card
    /// number and the CVV are read through `expose()` only (PIC-024).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] with [`DecodeKind::InvalidByte`] naming the first column,
    /// in [`CardRow`] order, whose field holds a NUL or non-ASCII byte, which no `CHAR(n)` column
    /// stores byte for byte (PIC-002). Codec A's decode admits only printable ASCII, so this
    /// arises only for a record built in code. The account key and CVV conversions are checked
    /// too, but a `9(11)` or `9(03)` value never exceeds `i64::MAX`. The error never holds the
    /// value.
    ///
    /// [`DecodeKind::InvalidByte`]: crate::error::DecodeKind::InvalidByte
    pub fn from_record(record: &CardRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            card_num: pic_x_to_string(CARD_NUM, record.num.expose())?,
            acct_id: pic_u_to_i64(ACCT_ID, record.acct_id)?,
            card_cvv_cd: pic_u_to_i64(CARD_CVV_CD, *record.cvv_cd.expose())?,
            card_embossed_name: pic_x_to_string(CARD_EMBOSSED_NAME, &record.embossed_name)?,
            card_expiraion_date: date_to_column(record.expiraion_date),
            card_active_status: pic_x_to_string(CARD_ACTIVE_STATUS, &record.active_status)?,
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::records::card::CardRecord;
    use domain::sensitive::CardNumber;

    use super::{CardBinds, CardRow};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Not a fixture card number: no test here quotes `carddata.txt`.
    const CARD_DIGITS: &str = "9876543210987654";
    /// [`CARD_DIGITS`] one byte short, the 15-byte `card_num` an error must not repeat.
    const CARD_FIFTEEN: &str = "987654321098765";
    /// The last four digits, which even the redacted form shows, so no error may hold them.
    const CARD_LAST4: &str = "7654";
    /// `CARD-CVV-CD` of [`line`].
    const CVV_DIGITS: &str = "913";
    /// `CARD-CVV-CD` of [`line`] as its column holds it.
    const CVV_VALUE: i64 = 913;
    /// A CVV beyond `PIC 9(03)`, which an error must not repeat either.
    const CVV_TOO_BIG: i64 = 1000;

    /// A `carddata.txt`-shaped line without its LF, shaped as `carddata.txt:1` with another card
    /// number and CVV: [`CARD_DIGITS`], account `00000000050`, [`CVV_DIGITS`], then the embossed
    /// name, expiry date, status and `FILLER`, each padded to its width (CVACT02Y-001 to
    /// CVACT02Y-007).
    fn line(name: &str, expiry: &str, status: &str, filler: &str) -> String {
        format!(
            "{CARD_DIGITS}{account:011}{CVV_DIGITS}{name:<50}{expiry:<10}{status:<1}{filler:<59}",
            account = 50
        )
    }

    /// The record of the [`line`] of an active card that expires on 2023-03-09.
    fn active_card() -> Result<CardRecord, Box<dyn StdError>> {
        Ok(CardRecord::decode(
            line("Aniya Von", "2023-03-09", "Y", "").as_bytes(),
        )?)
    }

    /// The row a read returns once `binds` is stored and the database assigned `card_ref`.
    fn stored(binds: CardBinds, card_ref: i64) -> CardRow {
        CardRow {
            card_num: binds.card_num,
            acct_id: binds.acct_id,
            card_cvv_cd: binds.card_cvv_cd,
            card_embossed_name: binds.card_embossed_name,
            card_expiraion_date: binds.card_expiraion_date,
            card_active_status: binds.card_active_status,
            filler: binds.filler,
            card_ref,
        }
    }

    /// `text` left-justified and space-padded to N bytes, without indexing.
    fn padded<const N: usize>(text: &[u8]) -> [u8; N] {
        let mut field = [b' '; N];
        for (slot, &byte) in field.iter_mut().zip(text) {
            *slot = byte;
        }
        field
    }

    /// Cardholder Data Redaction: `error` names `column`, and neither its `Display` nor its
    /// `Debug` repeats the card number, any part of it, or a CVV.
    fn assert_names_only_the_column(error: &PersistenceError, column: &str) {
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(text.contains(column), "{text}");
            for value in [CARD_DIGITS, CARD_FIFTEEN, CARD_LAST4, CVV_DIGITS, "1000"] {
                assert!(!text.contains(value), "{text}");
            }
        }
    }

    // audit: CVACT02Y-001
    // audit: CVACT02Y-002
    // audit: CVACT02Y-003
    // audit: CVACT02Y-004
    // audit: CVACT02Y-005
    // audit: CVACT02Y-006
    // audit: CVACT02Y-007
    // audit: CVACT02Y-008
    // audit: PIC-001
    // audit: PIC-003
    // audit: PIC-012
    // audit: PIC-015
    #[test]
    fn every_column_round_trips_to_the_record_and_its_handle() -> TestResult {
        let expiry = NaiveDate::from_ymd_opt(2023, 3, 9).ok_or("2023-03-09 is a calendar date")?;
        let cases = [
            (line("Aniya Von", "2023-03-09", "Y", ""), Some(expiry), 1),
            (
                line("Ward Jones", "", "N", "FILLER kept {as is} ~ 0123"),
                None,
                i64::MAX,
            ),
        ];
        for (text, date, handle) in cases {
            let bytes = text.as_bytes();
            assert_eq!(bytes.len(), CardRecord::WIDTH);
            let record = CardRecord::decode(bytes)?;
            let binds = CardBinds::from_record(&record)?;

            // Compared by equality, so a failure never prints the card number or the CVV.
            assert!(
                binds.card_num == CARD_DIGITS,
                "card_num holds the full number"
            );
            assert!(binds.card_cvv_cd == CVV_VALUE, "card_cvv_cd holds the CVV");
            assert_eq!(binds.acct_id, 50);
            assert_eq!(binds.card_expiraion_date, date);
            for (column, width) in [
                (&binds.card_embossed_name, 50),
                (&binds.card_active_status, 1),
                (&binds.filler, 59),
            ] {
                assert_eq!(column.len(), width, "CHAR(n) text keeps its padding");
            }

            let (card_ref, back) = stored(binds, handle).into_record()?;
            assert_eq!(card_ref, CardRef::new(handle));
            assert_eq!(back, record);
            assert!(back.encode() == bytes, "the record re-encodes to its line");
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-003
    // audit: PIC-016
    // audit: PIC-024
    #[test]
    fn stored_values_outside_their_pictures_are_typed_errors() -> TestResult {
        let record = active_card()?;
        let far = NaiveDate::from_ymd_opt(10_000, 1, 1).ok_or("year 10000 is a chrono date")?;
        let changed = |change: &dyn Fn(&mut CardRow)| -> Result<CardRow, PersistenceError> {
            let mut row = stored(CardBinds::from_record(&record)?, 7);
            change(&mut row);
            Ok(row)
        };
        let width = |expected, actual| DecodeKind::WidthMismatch { expected, actual };
        let cases = [
            (
                changed(&|row| row.card_cvv_cd = CVV_TOO_BIG)?,
                "card_cvv_cd",
                DecodeKind::OutOfRange,
            ),
            (
                changed(&|row| row.card_cvv_cd = -1)?,
                "card_cvv_cd",
                DecodeKind::OutOfRange,
            ),
            (
                changed(&|row| row.card_num = CARD_FIFTEEN.to_owned())?,
                "card_num",
                width(16, 15),
            ),
            (
                changed(&|row| row.card_num = format!("{CARD_FIFTEEN}\u{e9}"))?,
                "card_num",
                DecodeKind::InvalidByte,
            ),
            (
                changed(&|row| row.acct_id = -1)?,
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                changed(&|row| row.acct_id = 100_000_000_000)?,
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                changed(&|row| row.card_embossed_name = format!("{:<49}", "Ani\u{e9}ya Von"))?,
                "card_embossed_name",
                DecodeKind::InvalidByte,
            ),
            (
                changed(&|row| row.card_embossed_name = "X".repeat(51))?,
                "card_embossed_name",
                width(50, 51),
            ),
            (
                changed(&|row| row.card_expiraion_date = Some(far))?,
                "card_expiraion_date",
                DecodeKind::OutOfRange,
            ),
            (
                changed(&|row| row.card_active_status = String::new())?,
                "card_active_status",
                width(1, 0),
            ),
            (
                changed(&|row| row.filler = format!("{:<58}\0", ""))?,
                "filler",
                DecodeKind::InvalidByte,
            ),
        ];
        for (row, column, kind) in cases {
            let error = row.into_record().err().ok_or("the row must be rejected")?;
            assert_eq!(error, PersistenceError::decode(column, kind));
            assert_names_only_the_column(&error, column);
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-024
    #[test]
    fn record_bytes_no_char_column_stores_are_typed_errors() -> TestResult {
        let record = active_card()?;
        let mut nul_digit = *record.num.expose().as_bytes();
        if let Some(last) = nul_digit.last_mut() {
            *last = 0;
        }
        let cases = [
            (
                CardRecord {
                    num: CardNumber::new(PicX::from_array(nul_digit)),
                    ..record.clone()
                },
                "card_num",
            ),
            (
                CardRecord {
                    embossed_name: PicX::from_array(padded(b"Ani\xC3\xA9ya Von")),
                    ..record.clone()
                },
                "card_embossed_name",
            ),
            (
                CardRecord {
                    active_status: PicX::from_array([0x80]),
                    ..record.clone()
                },
                "card_active_status",
            ),
            (
                CardRecord {
                    filler: PicX::from_array(padded(b"\xFF")),
                    ..record
                },
                "filler",
            ),
        ];
        for (rejected, column) in cases {
            let error = CardBinds::from_record(&rejected)
                .err()
                .ok_or("the record must be rejected")?;
            assert_eq!(
                error,
                PersistenceError::decode(column, DecodeKind::InvalidByte)
            );
            assert_names_only_the_column(&error, column);
        }
        Ok(())
    }
}
