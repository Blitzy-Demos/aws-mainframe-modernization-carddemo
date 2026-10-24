//! `CIPAUSMY`, the pending-authorization summary: the IMS root segment `PAUTSUM0` of `DBPAUTP0`,
//! 100 bytes (`CIPAUSMY.cpy:19-31`, `DBPAUTP0.dbd:28-30`; CIPAUSMY-001 to CIPAUSMY-013).
//!
//! One summary exists per account. `PA-ACCT-ID` is the root key `ACCNTID` (`SEQ,U`, `START=1`,
//! `BYTES=6`, `TYPE=P`), so it is the primary key of `pending_auth_summaries` (migration 0004),
//! and the account's pending-authorization details (`CIPAUDTY`, segment `PAUTDTL1`) are its
//! children. No fixture holds this layout: it round-trips only through generated records and
//! its row (PIC-010).
//!
//! The layout is binary. Its key, limits, balances and amounts are packed decimal (`COMP-3`,
//! PIC-010) and its two counts halfword binary (`COMP`, PIC-011), so it has no line framing and
//! keeps the default `TRAILING_FILLER` of 0 (PIC-013 names only the fixture layouts).
//!
//! `PA-ACCT-ID PIC S9(11) COMP-3` is signed, while the account key `ACCT-ID PIC 9(11)` is not.
//! Callers join the two through `PicS::<11>::from(PicU<11>)` and the checked
//! `PicU::<11>::try_from(PicS<11>)`, which fails on a negative key, never through a cast
//! (CIPAUSMY-001, COPAUA0C-027; Checked Integer Narrowing).
//!
//! This module holds the layout and its byte form only. CP00's summary rules (the limits it
//! copies, the counts and amounts it adds, the i10 guard over `approved_auth_amt`) are
//! `crate::messaging::copaua0c`; the row mapping is `persistence::rows`.

use copybook::{Count, Error, FixedRecord, PicS, PicU, PicX, RecordReader, RecordWriter};

use super::{packed_money, put_packed_money};
use crate::money::Money;

/// Occurrences of `PA-ACCOUNT-STATUS PIC X(02) OCCURS 5 TIMES` (`CIPAUSMY.cpy:22`;
/// CIPAUSMY-004).
const ACCOUNT_STATUS_OCCURS: usize = 5;

/// One blank `PA-ACCOUNT-STATUS` occurrence, the value [`PendingAuthSummaryRecord::initialized`]
/// gives each of them.
const BLANK_ACCOUNT_STATUS: PicX<2> = PicX::spaces();

/// `PENDING-AUTH-SUMMARY`, the `CIPAUSMY` layout of IMS segment `PAUTSUM0`, 100 bytes; each
/// field documents its offset and width in the segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAuthSummaryRecord {
    /// `PA-ACCT-ID PIC S9(11) COMP-3`, offset 0, 6 bytes: the account, root key `ACCNTID` and
    /// primary key `acct_id`; CP00 fills it from `XREF-ACCT-ID` (CIPAUSMY-001, PIC-010).
    pub acct_id: PicS<11>,
    /// `PA-CUST-ID PIC 9(09)`, offset 6, 9 bytes: the customer; CP00 fills it from
    /// `XREF-CUST-ID` (CIPAUSMY-002).
    pub cust_id: PicU<9>,
    /// `PA-AUTH-STATUS PIC X(01)`, offset 15, 1 byte; no in-scope program reads or writes it
    /// (CIPAUSMY-003).
    pub auth_status: PicX<1>,
    /// `PA-ACCOUNT-STATUS PIC X(02) OCCURS 5 TIMES`, offset 16, 10 bytes: occurrence k, 1-based,
    /// sits at offset 16 + 2(k − 1) and is element k − 1, column `account_status_k`
    /// (CIPAUSMY-004).
    pub account_status: [PicX<2>; ACCOUNT_STATUS_OCCURS],
    /// `PA-CREDIT-LIMIT PIC S9(09)V99 COMP-3`, offset 26, 6 bytes; CP00 stores
    /// `ACCT-CREDIT-LIMIT PIC S9(10)V99` here by truncation (CIPAUSMY-005, COPAUA0C-090).
    pub credit_limit: Money<9>,
    /// `PA-CASH-LIMIT PIC S9(09)V99 COMP-3`, offset 32, 6 bytes; CP00 stores
    /// `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99` here by truncation (CIPAUSMY-006, COPAUA0C-090).
    pub cash_limit: Money<9>,
    /// `PA-CREDIT-BALANCE PIC S9(09)V99 COMP-3`, offset 38, 6 bytes: it accumulates approvals
    /// only and the purge never reduces it, which is why CP00's own rule cannot establish i10
    /// (CIPAUSMY-007, E30).
    pub credit_balance: Money<9>,
    /// `PA-CASH-BALANCE PIC S9(09)V99 COMP-3`, offset 44, 6 bytes; CP00 moves 0 here on every
    /// approval (CIPAUSMY-008, COPAUA0C-094).
    pub cash_balance: Money<9>,
    /// `PA-APPROVED-AUTH-CNT PIC S9(04) COMP`, offset 50, 2 bytes big-endian binary
    /// (CIPAUSMY-009, PIC-011).
    pub approved_auth_cnt: Count,
    /// `PA-DECLINED-AUTH-CNT PIC S9(04) COMP`, offset 52, 2 bytes big-endian binary
    /// (CIPAUSMY-010, PIC-011).
    pub declined_auth_cnt: Count,
    /// `PA-APPROVED-AUTH-AMT PIC S9(09)V99 COMP-3`, offset 54, 6 bytes: the sum of the
    /// account's approvals, and the authoritative pending sum of i10. The guard reads it from
    /// the summary row locked in the deciding transaction, never a sum of detail rows, which
    /// would miss an approval whose detail insert failed (AAP 0.3.4.2; CIPAUSMY-011,
    /// COPAUA0C-046, COPAUA0C-049).
    pub approved_auth_amt: Money<9>,
    /// `PA-DECLINED-AUTH-AMT PIC S9(09)V99 COMP-3`, offset 60, 6 bytes. The amount CP00's
    /// declined path adds is halt H2; the record fixes only the type (CIPAUSMY-012).
    pub declined_auth_amt: Money<9>,
    /// `FILLER PIC X(34)`, offset 66, 34 bytes, kept byte for byte (CIPAUSMY-013, PIC-012).
    pub filler: PicX<34>,
}

impl PendingAuthSummaryRecord {
    /// The summary CP00 creates for an account that has none: `INITIALIZE PENDING-AUTH-SUMMARY
    /// REPLACING NUMERIC DATA BY ZERO`, then `MOVE XREF-ACCT-ID TO PA-ACCT-ID` and
    /// `MOVE XREF-CUST-ID TO PA-CUST-ID` (`COPAUA0C.cbl:801-806`). Every limit, balance and
    /// amount is 0.00 and both counts are 0, the values the `lock_or_create` insert writes
    /// (COPAUA0C-025, COPAUA0C-092, ARITH-041).
    ///
    /// `PA-AUTH-STATUS`, every `PA-ACCOUNT-STATUS` and `FILLER` are spaces. That is interim
    /// handling, not a parity result: `INITIALIZE … REPLACING NUMERIC DATA` leaves alphanumeric
    /// fields holding whatever the storage held, so their COBOL bytes have no single value,
    /// `HALTED(X-ARITH-8)` (COPAUA0C-093).
    #[must_use]
    pub fn initialized(acct_id: PicS<11>, cust_id: PicU<9>) -> Self {
        Self {
            acct_id,
            cust_id,
            // HALTED(X-ARITH-8), COPAUA0C-093: interim spaces, not a parity result.
            auth_status: PicX::spaces(),
            account_status: [BLANK_ACCOUNT_STATUS; ACCOUNT_STATUS_OCCURS],
            credit_limit: Money::ZERO,
            cash_limit: Money::ZERO,
            credit_balance: Money::ZERO,
            cash_balance: Money::ZERO,
            approved_auth_cnt: Count::ZERO,
            declined_auth_cnt: Count::ZERO,
            approved_auth_amt: Money::ZERO,
            declined_auth_amt: Money::ZERO,
            filler: PicX::spaces(),
        }
    }
}

impl FixedRecord for PendingAuthSummaryRecord {
    /// `PAUTSUM0` `BYTES=100` (`DBPAUTP0.dbd:28`; CIPAUSMY-013).
    const WIDTH: usize = 100;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        // Struct and array expressions evaluate their operands in the order written, so the
        // fields, the five occurrences included, are read in copybook order.
        let record = Self {
            acct_id: r.pic_s_packed("PA-ACCT-ID")?,
            cust_id: r.pic_u("PA-CUST-ID")?,
            auth_status: r.pic_x("PA-AUTH-STATUS")?,
            account_status: [
                r.pic_x("PA-ACCOUNT-STATUS(1)")?,
                r.pic_x("PA-ACCOUNT-STATUS(2)")?,
                r.pic_x("PA-ACCOUNT-STATUS(3)")?,
                r.pic_x("PA-ACCOUNT-STATUS(4)")?,
                r.pic_x("PA-ACCOUNT-STATUS(5)")?,
            ],
            credit_limit: packed_money(&mut r, "PA-CREDIT-LIMIT")?,
            cash_limit: packed_money(&mut r, "PA-CASH-LIMIT")?,
            credit_balance: packed_money(&mut r, "PA-CREDIT-BALANCE")?,
            cash_balance: packed_money(&mut r, "PA-CASH-BALANCE")?,
            approved_auth_cnt: r.count("PA-APPROVED-AUTH-CNT")?,
            declined_auth_cnt: r.count("PA-DECLINED-AUTH-CNT")?,
            approved_auth_amt: packed_money(&mut r, "PA-APPROVED-AUTH-AMT")?,
            declined_auth_amt: packed_money(&mut r, "PA-DECLINED-AUTH-AMT")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_s_packed(self.acct_id)
            .pic_u(self.cust_id)
            .pic_x(&self.auth_status);
        for status in &self.account_status {
            w.pic_x(status);
        }
        put_packed_money(&mut w, &self.credit_limit);
        put_packed_money(&mut w, &self.cash_limit);
        put_packed_money(&mut w, &self.credit_balance);
        put_packed_money(&mut w, &self.cash_balance);
        w.count(self.approved_auth_cnt)
            .count(self.declined_auth_cnt);
        put_packed_money(&mut w, &self.approved_auth_amt);
        put_packed_money(&mut w, &self.declined_auth_amt);
        w.pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use copybook::zoned::{binary, packed};
    use copybook::{Count, Error, ErrorKind, FixedRecord, PicS, PicU, PicX};

    use super::PendingAuthSummaryRecord;
    use crate::money::{Decimal, Money, MoneyError};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every field the decoder reads, in copybook order, with its offset and width; the five
    /// `PA-ACCOUNT-STATUS` occurrences are read, and named, one by one (CIPAUSMY-001 to
    /// CIPAUSMY-013).
    const LAYOUT: [(&str, usize, usize); 17] = [
        ("PA-ACCT-ID", 0, 6),
        ("PA-CUST-ID", 6, 9),
        ("PA-AUTH-STATUS", 15, 1),
        ("PA-ACCOUNT-STATUS(1)", 16, 2),
        ("PA-ACCOUNT-STATUS(2)", 18, 2),
        ("PA-ACCOUNT-STATUS(3)", 20, 2),
        ("PA-ACCOUNT-STATUS(4)", 22, 2),
        ("PA-ACCOUNT-STATUS(5)", 24, 2),
        ("PA-CREDIT-LIMIT", 26, 6),
        ("PA-CASH-LIMIT", 32, 6),
        ("PA-CREDIT-BALANCE", 38, 6),
        ("PA-CASH-BALANCE", 44, 6),
        ("PA-APPROVED-AUTH-CNT", 50, 2),
        ("PA-DECLINED-AUTH-CNT", 52, 2),
        ("PA-APPROVED-AUTH-AMT", 54, 6),
        ("PA-DECLINED-AUTH-AMT", 60, 6),
        ("FILLER", 66, 34),
    ];

    /// Packed `S9(09)V99` zero: ten zero digits, a zero digit and sign `C`.
    const PACKED_ZERO: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C];

    /// `PA-ACCT-ID` 50 packed: `00000000050` and sign `C`.
    const PACKED_ACCT_50: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x05, 0x0C];

    /// The amount of `cents` hundredths.
    fn money(cents: i64) -> Result<Money<9>, MoneyError> {
        Money::new(Decimal::new(cents, 2))
    }

    /// A hand-built segment with positive and negative amounts and counts, field by field.
    fn sample_bytes() -> Vec<u8> {
        let parts: [&[u8]; 13] = [
            &PACKED_ACCT_50,
            b"000000050",
            b"A",
            b"0102030405",
            // 5000.00, 1500.00, -1234.56 and 0.00.
            &[0x00, 0x00, 0x05, 0x00, 0x00, 0x0C],
            &[0x00, 0x00, 0x01, 0x50, 0x00, 0x0C],
            &[0x00, 0x00, 0x01, 0x23, 0x45, 0x6D],
            &PACKED_ZERO,
            // Counts 3 and -2, big-endian two's complement.
            &[0x00, 0x03],
            &[0xFF, 0xFE],
            // 999999999.99, the largest `S9(09)V99`, and -0.01.
            &[0x99, 0x99, 0x99, 0x99, 0x99, 0x9C],
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x1D],
            b"FILLER KEPT AS STORED, NOT BLANKED",
        ];
        parts.concat()
    }

    /// The record [`sample_bytes`] holds.
    fn sample_record() -> Result<PendingAuthSummaryRecord, Box<dyn std::error::Error>> {
        Ok(PendingAuthSummaryRecord {
            acct_id: PicS::new(50)?,
            cust_id: PicU::new(50)?,
            auth_status: PicX::from_array(*b"A"),
            account_status: [
                PicX::from_array(*b"01"),
                PicX::from_array(*b"02"),
                PicX::from_array(*b"03"),
                PicX::from_array(*b"04"),
                PicX::from_array(*b"05"),
            ],
            credit_limit: money(500_000)?,
            cash_limit: money(150_000)?,
            credit_balance: money(-123_456)?,
            cash_balance: Money::ZERO,
            approved_auth_cnt: Count::new(3),
            declined_auth_cnt: Count::new(-2),
            approved_auth_amt: money(99_999_999_999)?,
            declined_auth_amt: money(-1)?,
            filler: PicX::from_array(*b"FILLER KEPT AS STORED, NOT BLANKED"),
        })
    }

    // audit: CIPAUSMY-001
    // audit: CIPAUSMY-004
    // audit: CIPAUSMY-013
    #[test]
    fn layout_is_contiguous_and_fills_the_segment() -> TestResult {
        let mut next = 0_usize;
        for (field, offset, width) in LAYOUT {
            assert_eq!(offset, next, "{field} starts where the previous field ends");
            next = next.checked_add(width).ok_or("layout width overflows")?;
        }
        assert_eq!(next, PendingAuthSummaryRecord::WIDTH);
        // The codec widths behind the table: 11 packed digits, `S9(11)` and `S9(09)V99` alike,
        // take 6 bytes, and `S9(04) COMP` takes 2 (PIC-010, PIC-011).
        assert_eq!(packed::byte_width(11)?, 6);
        assert_eq!(binary::WIDTH, 2);
        Ok(())
    }

    // audit: PIC-010
    // audit: PIC-011
    // audit: PIC-012
    #[test]
    fn hand_built_segment_decodes_field_by_field_and_round_trips() -> TestResult {
        let bytes = sample_bytes();
        let record = PendingAuthSummaryRecord::decode(&bytes)?;
        assert_eq!(record, sample_record()?);
        let encoded = record.encode();
        assert_eq!(encoded.len(), PendingAuthSummaryRecord::WIDTH);
        assert_eq!(encoded, bytes);
        // The signed key joins the `9(11)` account key through the checked conversion.
        assert_eq!(PicU::<11>::try_from(record.acct_id)?.get(), 50);
        Ok(())
    }

    // audit: PIC-010
    // audit: PIC-011
    #[test]
    fn extreme_values_round_trip_and_a_negative_key_has_no_account() -> TestResult {
        // -99999999999 and -999999999.99: eleven nines and sign `D`.
        let nines = [0x99, 0x99, 0x99, 0x99, 0x99, 0x9D];
        let parts: [&[u8]; 12] = [
            &nines,
            b"999999999",
            b"~",
            b"~~~~~~~~~~",
            &nines,
            &nines,
            &nines,
            &nines,
            // Counts -32768 and 32767: every halfword is a valid count.
            &[0x80, 0x00, 0x7F, 0xFF],
            &nines,
            &nines,
            &[b'~'; 34],
        ];
        let bytes = parts.concat();
        let record = PendingAuthSummaryRecord::decode(&bytes)?;
        assert_eq!(record.acct_id.get(), -99_999_999_999);
        assert_eq!(record.cust_id.get(), 999_999_999);
        assert_eq!(record.approved_auth_cnt, Count::new(i16::MIN));
        assert_eq!(record.declined_auth_cnt, Count::new(i16::MAX));
        for amount in [
            record.credit_limit,
            record.cash_limit,
            record.credit_balance,
            record.cash_balance,
            record.approved_auth_amt,
            record.declined_auth_amt,
        ] {
            assert_eq!(amount, money(-99_999_999_999)?);
        }
        assert_eq!(record.encode(), bytes);
        assert_eq!(
            PicU::<11>::try_from(record.acct_id),
            Err(ErrorKind::OutOfRange)
        );
        Ok(())
    }

    // audit: COPAUA0C-092
    // audit: COPAUA0C-093
    #[test]
    fn initialized_summary_holds_the_keys_zeros_and_interim_spaces() -> TestResult {
        let record = PendingAuthSummaryRecord::initialized(PicS::new(50)?, PicU::new(50)?);
        let parts: [&[u8]; 11] = [
            &PACKED_ACCT_50,
            b"000000050",
            // `PA-AUTH-STATUS` and the five `PA-ACCOUNT-STATUS` occurrences.
            &[b' '; 11],
            &PACKED_ZERO,
            &PACKED_ZERO,
            &PACKED_ZERO,
            &PACKED_ZERO,
            &[0x00; 4],
            &PACKED_ZERO,
            &PACKED_ZERO,
            &[b' '; 34],
        ];
        let expected = parts.concat();
        assert_eq!(record.encode(), expected);
        assert_eq!(PendingAuthSummaryRecord::decode(&expected)?, record);
        assert_eq!(record.approved_auth_cnt, Count::ZERO);
        assert!(record.approved_auth_amt.is_zero() && record.credit_balance.is_zero());
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-010
    #[test]
    fn malformed_fields_are_typed_errors_at_their_offset() -> TestResult {
        // The byte of `sample_bytes` overwritten, its new value, the field that then fails and
        // how. Counts have no case: every halfword is a valid count (PIC-011).
        let cases: [(usize, u8, &str, ErrorKind); 11] = [
            (5, 0x0F, "PA-ACCT-ID", ErrorKind::InvalidPackedSign),
            (6, b' ', "PA-CUST-ID", ErrorKind::NonDigit),
            (15, 0x09, "PA-AUTH-STATUS", ErrorKind::NonPrintable),
            (25, 0x7F, "PA-ACCOUNT-STATUS(5)", ErrorKind::NonPrintable),
            (31, 0x0F, "PA-CREDIT-LIMIT", ErrorKind::InvalidPackedSign),
            (37, 0x0A, "PA-CASH-LIMIT", ErrorKind::InvalidPackedSign),
            (40, 0xA1, "PA-CREDIT-BALANCE", ErrorKind::InvalidPackedDigit),
            (49, 0x0D, "PA-CASH-BALANCE", ErrorKind::NegativeZero),
            (
                59,
                0x9B,
                "PA-APPROVED-AUTH-AMT",
                ErrorKind::InvalidPackedSign,
            ),
            (
                65,
                0x1E,
                "PA-DECLINED-AUTH-AMT",
                ErrorKind::InvalidPackedSign,
            ),
            (99, 0x00, "FILLER", ErrorKind::NonPrintable),
        ];
        for (at, value, field, kind) in cases {
            let (name, offset, width) = LAYOUT
                .into_iter()
                .find(|&(name, _, _)| name == field)
                .ok_or("field missing from the layout")?;
            let mut bytes = sample_bytes();
            *bytes.get_mut(at).ok_or("byte outside the segment")? = value;
            assert_eq!(
                PendingAuthSummaryRecord::decode(&bytes),
                Err(Error::new(name, offset, width, kind)),
                "{field}",
            );
        }
        Ok(())
    }

    #[test]
    fn slices_of_any_other_length_are_width_errors_never_panics() {
        let bytes = sample_bytes();
        for len in [0, 1, 6, 65, 99, 101, 200] {
            let slice: Vec<u8> = bytes.iter().copied().cycle().take(len).collect();
            assert_eq!(
                PendingAuthSummaryRecord::decode(&slice),
                Err(Error::new(
                    "record",
                    0,
                    PendingAuthSummaryRecord::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: PendingAuthSummaryRecord::WIDTH,
                        actual: len,
                    },
                )),
            );
        }
    }
}
