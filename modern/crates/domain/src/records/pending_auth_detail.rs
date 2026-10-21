//! `CIPAUDTY`, the pending-authorization detail: IMS child segment `PAUTDTL1`, 200 bytes
//! (`CIPAUDTY.cpy:18-54`, `DBPAUTP0.dbd:36-37`; CIPAUDTY-001 to CIPAUDTY-028).
//!
//! CP00 inserts one detail for every request whose card has a cross-reference row, approved or
//! declined (COPAUA0C-098 to COPAUA0C-103); the CPVS and CPVD screens read it and mark or unmark
//! fraud on it, and `cbpaup0j` purges it. It is stored in table `pending_auth_details`
//! (migration 0004), one column per field.
//!
//! - **Key.** `PA-AUTHORIZATION-KEY`, the inverted date and time (`auth_date_9c`,
//!   `auth_time_9c`), is the segment's first 8 bytes, the unique sequence field `PAUT9CTS`
//!   (`SEQ,U`). The account belongs to the parent segment's `ACCNTID`, outside these 200 bytes
//!   (CIPAUDTY-029), so it is no field of this record. Both parts are held as stored: the
//!   callers that build or read a key convert with `PicS::complement_from` and
//!   `PicS::uncomplement` (`99999 − YYDDD`, `999999999 − HHMMSSmmm`; COPAUA0C-098).
//! - **Text.** Every `PIC X(n)` field keeps its bytes. `PA-AUTH-ORIG-DATE`, `PA-AUTH-ORIG-TIME`,
//!   `PA-CARD-EXPIRY-DATE` and `PA-FRAUD-RPT-DATE` look like dates but are `X(06)`, `X(04)` and
//!   `X(08)`, which Typed Dates and Timestamps does not cover, so they stay `PicX` as received
//!   (CIPAUDTY-003, CIPAUDTY-004, CIPAUDTY-007, CIPAUDTY-027).
//! - **Card number.** `PA-CARD-NUM` is a [`CardNumber`], so the derived `Debug` shows only its
//!   redacted form; [`FixedRecord::encode`] alone reaches the raw bytes, through
//!   [`CardNumber::expose`] (CIPAUDTY-005, PIC-024, E32).
//! - **`FILLER`.** Its 17 bytes are kept whatever they hold (PIC-012). What CP00 leaves there is
//!   halt H2, written as 17 spaces by the caller in the interim (COPAUA0C-102).
//!
//! No fixture holds this segment, so [`FixedRecord::TRAILING_FILLER`] stays 0 and the layout
//! round-trips only through generated records (PIC-010).

use copybook::{Error, FixedRecord, PicS, PicU, PicX, RecordReader, RecordWriter};

use super::{packed_money, put_packed_money};
use crate::money::Money;
use crate::sensitive::CardNumber;

/// One pending-authorization detail segment, `PENDING-AUTH-DETAILS` (`CIPAUDTY`), its fields in
/// copybook order. Offsets are 0-based within the 200-byte segment; the unit tests check that the
/// fields follow one another and fill exactly `PAUTDTL1` `BYTES=200` (`DBPAUTP0.dbd:36`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAuthDetailRecord {
    /// `PA-AUTH-DATE-9C PIC S9(05) COMP-3`, offset 0, 3 bytes: `99999 − YYDDD`, the first part of
    /// `PA-AUTHORIZATION-KEY` (CIPAUDTY-001).
    pub auth_date_9c: PicS<5>,
    /// `PA-AUTH-TIME-9C PIC S9(09) COMP-3`, offset 3, 5 bytes: `999999999 − HHMMSSmmm`, the
    /// second part of `PA-AUTHORIZATION-KEY` (CIPAUDTY-002).
    pub auth_time_9c: PicS<9>,
    /// `PA-AUTH-ORIG-DATE PIC X(06)`, offset 8: request field 1, column `auth_orig_date`
    /// (CIPAUDTY-003).
    pub orig_date: PicX<6>,
    /// `PA-AUTH-ORIG-TIME PIC X(06)`, offset 14: request field 2, column `auth_orig_time`
    /// (CIPAUDTY-004).
    pub orig_time: PicX<6>,
    /// `PA-CARD-NUM PIC X(16)`, offset 20: redacted in `Debug` (CIPAUDTY-005).
    pub card_num: CardNumber,
    /// `PA-AUTH-TYPE PIC X(04)`, offset 36 (CIPAUDTY-006).
    pub auth_type: PicX<4>,
    /// `PA-CARD-EXPIRY-DATE PIC X(04)`, offset 40 (CIPAUDTY-007).
    pub card_expiry_date: PicX<4>,
    /// `PA-MESSAGE-TYPE PIC X(06)`, offset 44 (CIPAUDTY-008).
    pub message_type: PicX<6>,
    /// `PA-MESSAGE-SOURCE PIC X(06)`, offset 50 (CIPAUDTY-009).
    pub message_source: PicX<6>,
    /// `PA-AUTH-ID-CODE PIC X(06)`, offset 56 (CIPAUDTY-010).
    pub auth_id_code: PicX<6>,
    /// `PA-AUTH-RESP-CODE PIC X(02)`, offset 62; 88-level `PA-AUTH-APPROVED`
    /// ([`Self::AUTH_APPROVED`]) (CIPAUDTY-011).
    pub auth_resp_code: PicX<2>,
    /// `PA-AUTH-RESP-REASON PIC X(04)`, offset 64 (CIPAUDTY-012).
    pub auth_resp_reason: PicX<4>,
    /// `PA-PROCESSING-CODE PIC 9(06)`, offset 68, 6 digits (CIPAUDTY-013).
    pub processing_code: PicU<6>,
    /// `PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3`, offset 74, 7 bytes (CIPAUDTY-014).
    pub transaction_amt: Money<10>,
    /// `PA-APPROVED-AMT PIC S9(10)V99 COMP-3`, offset 81, 7 bytes (CIPAUDTY-015).
    pub approved_amt: Money<10>,
    /// `PA-MERCHANT-CATAGORY-CODE PIC X(04)`, offset 88, the COBOL spelling kept
    /// (CIPAUDTY-016).
    pub merchant_catagory_code: PicX<4>,
    /// `PA-ACQR-COUNTRY-CODE PIC X(03)`, offset 92 (CIPAUDTY-017).
    pub acqr_country_code: PicX<3>,
    /// `PA-POS-ENTRY-MODE PIC 9(02)`, offset 95, 2 digits (CIPAUDTY-018).
    pub pos_entry_mode: PicU<2>,
    /// `PA-MERCHANT-ID PIC X(15)`, offset 97 (CIPAUDTY-019).
    pub merchant_id: PicX<15>,
    /// `PA-MERCHANT-NAME PIC X(22)`, offset 112 (CIPAUDTY-020).
    pub merchant_name: PicX<22>,
    /// `PA-MERCHANT-CITY PIC X(13)`, offset 134 (CIPAUDTY-021).
    pub merchant_city: PicX<13>,
    /// `PA-MERCHANT-STATE PIC X(02)`, offset 147 (CIPAUDTY-022).
    pub merchant_state: PicX<2>,
    /// `PA-MERCHANT-ZIP PIC X(09)`, offset 149 (CIPAUDTY-023).
    pub merchant_zip: PicX<9>,
    /// `PA-TRANSACTION-ID PIC X(15)`, offset 158 (CIPAUDTY-024).
    pub transaction_id: PicX<15>,
    /// `PA-MATCH-STATUS PIC X(01)`, offset 173; 88-levels [`Self::MATCH_PENDING`],
    /// [`Self::MATCH_AUTH_DECLINED`], [`Self::MATCH_PENDING_EXPIRED`] and
    /// [`Self::MATCHED_WITH_TRAN`] (CIPAUDTY-025).
    pub match_status: PicX<1>,
    /// `PA-AUTH-FRAUD PIC X(01)`, offset 174; 88-levels [`Self::FRAUD_CONFIRMED`] and
    /// [`Self::FRAUD_REMOVED`] (CIPAUDTY-026).
    pub auth_fraud: PicX<1>,
    /// `PA-FRAUD-RPT-DATE PIC X(08)`, offset 175 (CIPAUDTY-027).
    pub fraud_rpt_date: PicX<8>,
    /// `FILLER PIC X(17)`, offset 183, kept byte for byte (CIPAUDTY-028, PIC-012).
    pub filler: PicX<17>,
}

impl PendingAuthDetailRecord {
    /// `88 PA-AUTH-APPROVED VALUE '00'` of `PA-AUTH-RESP-CODE` (`CIPAUDTY.cpy:31`; CIPAUDTY-011).
    pub const AUTH_APPROVED: &[u8; 2] = b"00";

    /// `88 PA-MATCH-PENDING VALUE 'P'` of `PA-MATCH-STATUS`, written by CP00 for an approval
    /// (`CIPAUDTY.cpy:46`; CIPAUDTY-025, COPAUA0C-101).
    pub const MATCH_PENDING: &[u8; 1] = b"P";

    /// `88 PA-MATCH-AUTH-DECLINED VALUE 'D'`, written by CP00 for a decline (`CIPAUDTY.cpy:47`;
    /// CIPAUDTY-025, COPAUA0C-101).
    pub const MATCH_AUTH_DECLINED: &[u8; 1] = b"D";

    /// `88 PA-MATCH-PENDING-EXPIRED VALUE 'E'` (`CIPAUDTY.cpy:48`; CIPAUDTY-025).
    pub const MATCH_PENDING_EXPIRED: &[u8; 1] = b"E";

    /// `88 PA-MATCHED-WITH-TRAN VALUE 'M'` (`CIPAUDTY.cpy:49`; CIPAUDTY-025).
    pub const MATCHED_WITH_TRAN: &[u8; 1] = b"M";

    /// `88 PA-FRAUD-CONFIRMED VALUE 'F'` of `PA-AUTH-FRAUD` (`CIPAUDTY.cpy:51`; CIPAUDTY-026).
    pub const FRAUD_CONFIRMED: &[u8; 1] = b"F";

    /// `88 PA-FRAUD-REMOVED VALUE 'R'` (`CIPAUDTY.cpy:52`; CIPAUDTY-026).
    pub const FRAUD_REMOVED: &[u8; 1] = b"R";

    /// COBOL `IF PA-AUTH-APPROVED`: the response code is `00` (CIPAUDTY-011).
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.auth_resp_code.as_bytes() == Self::AUTH_APPROVED
    }

    /// COBOL `IF PA-FRAUD-CONFIRMED`: the fraud flag is `F` (CIPAUDTY-026).
    #[must_use]
    pub fn is_fraud_confirmed(&self) -> bool {
        self.auth_fraud.as_bytes() == Self::FRAUD_CONFIRMED
    }
}

impl FixedRecord for PendingAuthDetailRecord {
    const WIDTH: usize = 200;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            auth_date_9c: r.pic_s_packed("PA-AUTH-DATE-9C")?,
            auth_time_9c: r.pic_s_packed("PA-AUTH-TIME-9C")?,
            orig_date: r.pic_x("PA-AUTH-ORIG-DATE")?,
            orig_time: r.pic_x("PA-AUTH-ORIG-TIME")?,
            card_num: CardNumber::new(r.pic_x("PA-CARD-NUM")?),
            auth_type: r.pic_x("PA-AUTH-TYPE")?,
            card_expiry_date: r.pic_x("PA-CARD-EXPIRY-DATE")?,
            message_type: r.pic_x("PA-MESSAGE-TYPE")?,
            message_source: r.pic_x("PA-MESSAGE-SOURCE")?,
            auth_id_code: r.pic_x("PA-AUTH-ID-CODE")?,
            auth_resp_code: r.pic_x("PA-AUTH-RESP-CODE")?,
            auth_resp_reason: r.pic_x("PA-AUTH-RESP-REASON")?,
            processing_code: r.pic_u("PA-PROCESSING-CODE")?,
            transaction_amt: packed_money(&mut r, "PA-TRANSACTION-AMT")?,
            approved_amt: packed_money(&mut r, "PA-APPROVED-AMT")?,
            merchant_catagory_code: r.pic_x("PA-MERCHANT-CATAGORY-CODE")?,
            acqr_country_code: r.pic_x("PA-ACQR-COUNTRY-CODE")?,
            pos_entry_mode: r.pic_u("PA-POS-ENTRY-MODE")?,
            merchant_id: r.pic_x("PA-MERCHANT-ID")?,
            merchant_name: r.pic_x("PA-MERCHANT-NAME")?,
            merchant_city: r.pic_x("PA-MERCHANT-CITY")?,
            merchant_state: r.pic_x("PA-MERCHANT-STATE")?,
            merchant_zip: r.pic_x("PA-MERCHANT-ZIP")?,
            transaction_id: r.pic_x("PA-TRANSACTION-ID")?,
            match_status: r.pic_x("PA-MATCH-STATUS")?,
            auth_fraud: r.pic_x("PA-AUTH-FRAUD")?,
            fraud_rpt_date: r.pic_x("PA-FRAUD-RPT-DATE")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_s_packed(self.auth_date_9c)
            .pic_s_packed(self.auth_time_9c)
            .pic_x(&self.orig_date)
            .pic_x(&self.orig_time)
            .pic_x(self.card_num.expose())
            .pic_x(&self.auth_type)
            .pic_x(&self.card_expiry_date)
            .pic_x(&self.message_type)
            .pic_x(&self.message_source)
            .pic_x(&self.auth_id_code)
            .pic_x(&self.auth_resp_code)
            .pic_x(&self.auth_resp_reason)
            .pic_u(self.processing_code);
        put_packed_money(&mut w, &self.transaction_amt);
        put_packed_money(&mut w, &self.approved_amt);
        w.pic_x(&self.merchant_catagory_code)
            .pic_x(&self.acqr_country_code)
            .pic_u(self.pos_entry_mode)
            .pic_x(&self.merchant_id)
            .pic_x(&self.merchant_name)
            .pic_x(&self.merchant_city)
            .pic_x(&self.merchant_state)
            .pic_x(&self.merchant_zip)
            .pic_x(&self.transaction_id)
            .pic_x(&self.match_status)
            .pic_x(&self.auth_fraud)
            .pic_x(&self.fraud_rpt_date)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::ErrorKind::{InvalidPackedDigit, NonDigit, NonPrintable, NonZeroPadNibble};
    use copybook::{Error, ErrorKind, FixedRecord, PicS, PicU, PicX};

    use super::PendingAuthDetailRecord;
    use crate::money::{Decimal, Money};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number outside the fixture corpus.
    const CARD: &str = "1234567890123456";

    /// Every field as `copybooks.md` lays it out: name, offset, width, a first byte its codec
    /// rejects and the error that byte raises (CIPAUDTY-001 to CIPAUDTY-028). A first nibble `A`
    /// is a bad digit for the odd-digit keys and a bad pad nibble for the even-digit amounts.
    const LAYOUT: [(&str, usize, usize, u8, ErrorKind); 28] = [
        ("PA-AUTH-DATE-9C", 0, 3, 0xAA, InvalidPackedDigit),
        ("PA-AUTH-TIME-9C", 3, 5, 0xAA, InvalidPackedDigit),
        ("PA-AUTH-ORIG-DATE", 8, 6, 0x00, NonPrintable),
        ("PA-AUTH-ORIG-TIME", 14, 6, 0x00, NonPrintable),
        ("PA-CARD-NUM", 20, 16, 0x00, NonPrintable),
        ("PA-AUTH-TYPE", 36, 4, 0x00, NonPrintable),
        ("PA-CARD-EXPIRY-DATE", 40, 4, 0x00, NonPrintable),
        ("PA-MESSAGE-TYPE", 44, 6, 0x00, NonPrintable),
        ("PA-MESSAGE-SOURCE", 50, 6, 0x00, NonPrintable),
        ("PA-AUTH-ID-CODE", 56, 6, 0x00, NonPrintable),
        ("PA-AUTH-RESP-CODE", 62, 2, 0x00, NonPrintable),
        ("PA-AUTH-RESP-REASON", 64, 4, 0x00, NonPrintable),
        ("PA-PROCESSING-CODE", 68, 6, b'x', NonDigit),
        ("PA-TRANSACTION-AMT", 74, 7, 0xAA, NonZeroPadNibble),
        ("PA-APPROVED-AMT", 81, 7, 0xAA, NonZeroPadNibble),
        ("PA-MERCHANT-CATAGORY-CODE", 88, 4, 0x00, NonPrintable),
        ("PA-ACQR-COUNTRY-CODE", 92, 3, 0x00, NonPrintable),
        ("PA-POS-ENTRY-MODE", 95, 2, b'x', NonDigit),
        ("PA-MERCHANT-ID", 97, 15, 0x00, NonPrintable),
        ("PA-MERCHANT-NAME", 112, 22, 0x00, NonPrintable),
        ("PA-MERCHANT-CITY", 134, 13, 0x00, NonPrintable),
        ("PA-MERCHANT-STATE", 147, 2, 0x00, NonPrintable),
        ("PA-MERCHANT-ZIP", 149, 9, 0x00, NonPrintable),
        ("PA-TRANSACTION-ID", 158, 15, 0x00, NonPrintable),
        ("PA-MATCH-STATUS", 173, 1, 0x00, NonPrintable),
        ("PA-AUTH-FRAUD", 174, 1, 0x00, NonPrintable),
        ("PA-FRAUD-RPT-DATE", 175, 8, 0x00, NonPrintable),
        ("FILLER", 183, 17, 0x7F, NonPrintable),
    ];

    /// The bytes of the approved detail CP00 writes on day 32 of 2025 at 13:45:07.123, keys
    /// `99999 − 25032` and `999999999 − 134507123` (COPAUA0C-098), 123.45 approved in full.
    fn approved_image() -> Vec<u8> {
        let parts: [&[u8]; 28] = [
            &[0x74, 0x96, 0x7C],
            &[0x86, 0x54, 0x92, 0x87, 0x6C],
            b"250201",
            b"134507",
            CARD.as_bytes(),
            b"AUTH",
            b"0927",
            b"0100  ",
            b"POS   ",
            b"134507",
            b"00",
            b"0000",
            b"003000",
            &[0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x5C],
            &[0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x5C],
            b"5411",
            b"840",
            b"05",
            b"MERCH0000000001",
            b"CORNER GROCERY        ",
            b"SEATTLE      ",
            b"WA",
            b"98101    ",
            b"TXN000000000001",
            b"P",
            b" ",
            b"        ",
            b"                 ",
        ];
        parts.concat()
    }

    /// The typed record [`approved_image`] holds.
    fn approved_record() -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        let amount = Money::new(Decimal::new(12_345, 2))?;
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(74_967)?,
            auth_time_9c: PicS::new(865_492_876)?,
            orig_date: PicX::from_text("250201")?,
            orig_time: PicX::from_text("134507")?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("AUTH")?,
            card_expiry_date: PicX::from_text("0927")?,
            message_type: PicX::from_text("0100")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("134507")?,
            auth_resp_code: PicX::from_array(*PendingAuthDetailRecord::AUTH_APPROVED),
            auth_resp_reason: PicX::from_text("0000")?,
            processing_code: PicU::new(3_000)?,
            transaction_amt: amount,
            approved_amt: amount,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCH0000000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_array(*PendingAuthDetailRecord::MATCH_PENDING),
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// Replaces the bytes of `image` from `offset` with `bytes`.
    fn overwrite(image: &mut [u8], offset: usize, bytes: &[u8]) -> Result<(), &'static str> {
        image
            .get_mut(offset..offset + bytes.len())
            .ok_or("the bytes run past the segment")?
            .copy_from_slice(bytes);
        Ok(())
    }

    /// A declined detail with a negative amount, later marked as fraud removed, whose `FILLER`
    /// holds non-space bytes: its record and its bytes.
    fn declined() -> Result<(PendingAuthDetailRecord, Vec<u8>), Box<dyn StdError>> {
        let mut record = approved_record()?;
        record.auth_resp_code = PicX::from_text("05")?;
        record.auth_resp_reason = PicX::from_text("4100")?;
        record.transaction_amt = Money::new(Decimal::new(-525, 2))?;
        record.approved_amt = Money::ZERO;
        record.match_status = PicX::from_array(*PendingAuthDetailRecord::MATCH_AUTH_DECLINED);
        record.auth_fraud = PicX::from_array(*PendingAuthDetailRecord::FRAUD_REMOVED);
        record.fraud_rpt_date = PicX::from_text("20250201")?;
        record.filler = PicX::from_text("FILLER BYTES KEPT")?;

        let mut image = approved_image();
        overwrite(&mut image, 62, b"054100")?;
        overwrite(&mut image, 74, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x52, 0x5D])?;
        overwrite(&mut image, 81, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C])?;
        overwrite(&mut image, 173, b"DR20250201FILLER BYTES KEPT")?;
        Ok((record, image))
    }

    // audit: PIC-010
    // audit: PIC-012
    #[test]
    fn hand_built_segments_round_trip_byte_for_byte() -> TestResult {
        for (record, image) in [(approved_record()?, approved_image()), declined()?] {
            assert_eq!(image.len(), PendingAuthDetailRecord::WIDTH);
            assert_eq!(PendingAuthDetailRecord::decode(&image)?, record);
            assert_eq!(record.encode(), image);
        }
        Ok(())
    }

    // audit: CIPAUDTY-005
    // audit: PIC-024
    #[test]
    fn debug_shows_only_the_redacted_card_number() -> TestResult {
        let shown = format!("{:?}", approved_record()?);
        assert!(!shown.contains(CARD));
        assert!(shown.contains("************3456"));
        Ok(())
    }

    // audit: CIPAUDTY-011
    // audit: CIPAUDTY-026
    #[test]
    fn predicates_test_the_88_level_values() -> TestResult {
        let mut record = approved_record()?;
        for (code, approved) in [
            (b"00", true),
            (b"05", false),
            (b"0 ", false),
            (b"  ", false),
        ] {
            record.auth_resp_code = PicX::from_array(*code);
            assert_eq!(record.is_approved(), approved);
        }
        for (flag, confirmed) in [(b"F", true), (b"R", false), (b" ", false), (b"f", false)] {
            record.auth_fraud = PicX::from_array(*flag);
            assert_eq!(record.is_fraud_confirmed(), confirmed);
        }
        Ok(())
    }

    #[test]
    fn any_other_length_is_a_record_width_error() {
        let image = approved_image();
        for len in [0, 1, 8, 199, 201, 400] {
            let bytes: Vec<u8> = image.iter().copied().cycle().take(len).collect();
            assert_eq!(
                PendingAuthDetailRecord::decode(&bytes),
                Err(Error::new(
                    "record",
                    0,
                    PendingAuthDetailRecord::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: PendingAuthDetailRecord::WIDTH,
                        actual: len,
                    },
                )),
            );
        }
    }

    // audit: CIPAUDTY-001
    // audit: CIPAUDTY-002
    // audit: CIPAUDTY-003
    // audit: CIPAUDTY-004
    // audit: CIPAUDTY-005
    // audit: CIPAUDTY-006
    // audit: CIPAUDTY-007
    // audit: CIPAUDTY-008
    // audit: CIPAUDTY-009
    // audit: CIPAUDTY-010
    // audit: CIPAUDTY-011
    // audit: CIPAUDTY-012
    // audit: CIPAUDTY-013
    // audit: CIPAUDTY-014
    // audit: CIPAUDTY-015
    // audit: CIPAUDTY-016
    // audit: CIPAUDTY-017
    // audit: CIPAUDTY-018
    // audit: CIPAUDTY-019
    // audit: CIPAUDTY-020
    // audit: CIPAUDTY-021
    // audit: CIPAUDTY-022
    // audit: CIPAUDTY-023
    // audit: CIPAUDTY-024
    // audit: CIPAUDTY-025
    // audit: CIPAUDTY-026
    // audit: CIPAUDTY-027
    // audit: CIPAUDTY-028
    #[test]
    fn each_field_fails_at_its_copybook_offset_and_width() -> TestResult {
        let mut next = 0;
        for (field, offset, width, bad, kind) in LAYOUT {
            assert_eq!(
                offset, next,
                "{field} does not start where the previous field ends"
            );
            next = offset + width;
            let mut image = approved_image();
            overwrite(&mut image, offset, &[bad])?;
            assert_eq!(
                PendingAuthDetailRecord::decode(&image),
                Err(Error::new(field, offset, width, kind)),
            );
        }
        assert_eq!(next, PendingAuthDetailRecord::WIDTH);
        Ok(())
    }

    // audit: PIC-010
    #[test]
    fn bad_packed_nibbles_are_typed_errors() -> TestResult {
        let cases: [(usize, &[u8], Error); 4] = [
            // Sign nibble `A` on the date key.
            (
                2,
                &[0x7A],
                Error::new("PA-AUTH-DATE-9C", 0, 3, ErrorKind::InvalidPackedSign),
            ),
            // Sign nibble `F`, the unsigned sign, on the time key.
            (
                7,
                &[0x6F],
                Error::new("PA-AUTH-TIME-9C", 3, 5, ErrorKind::InvalidPackedSign),
            ),
            // Digit nibble `A` inside the amount.
            (
                78,
                &[0x1A],
                Error::new("PA-TRANSACTION-AMT", 74, 7, ErrorKind::InvalidPackedDigit),
            ),
            // An all-zero magnitude signed `D`.
            (
                81,
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D],
                Error::new("PA-APPROVED-AMT", 81, 7, ErrorKind::NegativeZero),
            ),
        ];
        for (offset, bytes, expected) in cases {
            let mut image = approved_image();
            overwrite(&mut image, offset, bytes)?;
            assert_eq!(PendingAuthDetailRecord::decode(&image), Err(expected));
        }
        Ok(())
    }
}
