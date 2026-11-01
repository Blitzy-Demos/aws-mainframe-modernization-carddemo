//! A pending authorization: `CIPAUDTY` `PENDING-AUTH-DETAILS`
//! [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L19-54], the 200-byte
//! child segment `PAUTDTL1` of the IMS database `DBPAUTP0`. Each detail is a
//! twin under its `PAUTSUM0` summary, ordered by the 8-byte sequence field
//! `PAUT9CTS`, the two packed parts of `PA-AUTHORIZATION-KEY`
//! [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L36-37].
//!
//! The segment becomes a row of the SQL table `pending_auth_details`
//! (`migrations/0004_authorizations.sql`). The row adds the identity `auth_id`,
//! the REST key of `GET` and `PUT /authorizations/{id}`, and keeps the IMS key
//! columns: the parent key `pa_acct_id` and the two key parts. A
//! [`NewPendingAuthDetail`] is a detail before its insert assigns the
//! identity; a [`PendingAuthDetail`] is a stored one.
//!
//! Each field is its copybook field lower-snake-cased, prefix and spelling
//! kept (`pa_merchant_catagory_code`), and typed by its reviewed section A row
//! of `docs/parity/parity-oracle-audit.md`, cited at the field. The packed
//! (`COMP-3`) fields are typed by their PIC digits, because the codec encodes
//! no IMS segment. `FILLER PIC X(17)` (P-CIPAUDTY-028)
//! [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L54] is not modelled.
//!
//! `COPAUA0C` `8500-INSERT-AUTH` inserts details (P-COPAUA0C-014), `COPAUS1C`
//! replaces one after fraud marking (P-COPAUS1C-006), and `CBPAUP0C` deletes
//! the expired ones (P-CBPAUP0C-004).

use crate::numeric::{AccountId, Alnum, AuthId, CardNumber, Money, NumericCode};

/// `PA-AUTH-RESP-CODE` of an approved authorization: 88 `PA-AUTH-APPROVED
/// VALUE '00'` (P-CIPAUDTY-011)
/// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L31].
const AUTH_APPROVED: &[u8; 2] = b"00";

/// `PA-MATCH-STATUS PIC X(01)`, P-CIPAUDTY-025
/// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L45-49]: whether the
/// authorization has been matched with a posted transaction.
///
/// One variant per 88-level value, and [`MatchStatus::Other`] for every other
/// byte, the space included, so every stored byte converts back unchanged. The
/// section A row types the field as `Alnum<1>`, the `VARCHAR(1)` column's byte;
/// [`MatchStatus::from_alnum`] and [`MatchStatus::to_alnum`] convert between
/// the two without loss. [`MatchStatus::from_byte`] never yields `Other`
/// holding a byte a named variant covers.
///
/// `COPAUA0C` `8500-INSERT-AUTH` stores only `P`, for an approved
/// authorization, or `D`, for a declined one
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L902-906]
/// (P-COPAUA0C-014). No program in the repository sets `E` or `M`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchStatus {
    /// 88 `PA-MATCH-PENDING VALUE 'P'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L46], P-CIPAUDTY-025.
    Pending,
    /// 88 `PA-MATCH-AUTH-DECLINED VALUE 'D'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L47], P-CIPAUDTY-025.
    AuthDeclined,
    /// 88 `PA-MATCH-PENDING-EXPIRED VALUE 'E'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L48], P-CIPAUDTY-025.
    PendingExpired,
    /// 88 `PA-MATCHED-WITH-TRAN VALUE 'M'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L49], P-CIPAUDTY-025.
    MatchedWithTran,
    /// Any other byte, as stored: no 88-level of `PA-MATCH-STATUS` holds it.
    Other(u8),
}

impl MatchStatus {
    /// The status the stored byte holds.
    #[must_use]
    pub const fn from_byte(b: u8) -> Self {
        match b {
            b'P' => Self::Pending,
            b'D' => Self::AuthDeclined,
            b'E' => Self::PendingExpired,
            b'M' => Self::MatchedWithTran,
            other => Self::Other(other),
        }
    }

    /// The byte the field stores.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Pending => b'P',
            Self::AuthDeclined => b'D',
            Self::PendingExpired => b'E',
            Self::MatchedWithTran => b'M',
            Self::Other(byte) => byte,
        }
    }

    /// The status the `X(01)` item holds.
    #[must_use]
    pub const fn from_alnum(value: Alnum<1>) -> Self {
        Self::from_byte(value.as_bytes()[0])
    }

    /// The `X(01)` item holding the status's byte.
    #[must_use]
    pub const fn to_alnum(self) -> Alnum<1> {
        Alnum::from_array([self.as_byte()])
    }
}

/// `PA-AUTH-FRAUD PIC X(01)`, P-CIPAUDTY-026
/// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L50-52]: whether the
/// authorization has been reported as fraud.
///
/// As for [`MatchStatus`], every byte converts back unchanged: one variant per
/// 88-level value, [`FraudFlag::Blank`] for the space a new detail holds, and
/// [`FraudFlag::Other`] for every other byte. The section A row types the field
/// as `Alnum<1>`; [`FraudFlag::from_alnum`] and [`FraudFlag::to_alnum`]
/// convert without loss. The `VARCHAR(1)` column stores the space
/// right-trimmed as `''`, which `Alnum::from_trimmed` reads back as the space.
///
/// `COPAUA0C` stores a space on insert
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L908] (P-COPAUA0C-014).
/// `COPAUS1C` `MARK-AUTH-FRAUD` toggles the flag: a detail holding `F` becomes
/// `R`, and any other detail becomes `F`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L236-242]
/// (P-COPAUS1C-006). That toggle is a rule of `rules::auth_detail`, not of
/// this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FraudFlag {
    /// 88 `PA-FRAUD-CONFIRMED VALUE 'F'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L51], P-CIPAUDTY-026.
    Confirmed,
    /// 88 `PA-FRAUD-REMOVED VALUE 'R'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L52], P-CIPAUDTY-026.
    Removed,
    /// A space, which no 88-level names: `COPAUA0C` `MOVE SPACE TO
    /// PA-AUTH-FRAUD` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L908].
    Blank,
    /// Any other byte, as stored.
    Other(u8),
}

impl FraudFlag {
    /// The flag the stored byte holds.
    #[must_use]
    pub const fn from_byte(b: u8) -> Self {
        match b {
            b'F' => Self::Confirmed,
            b'R' => Self::Removed,
            b' ' => Self::Blank,
            other => Self::Other(other),
        }
    }

    /// The byte the field stores.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Confirmed => b'F',
            Self::Removed => b'R',
            Self::Blank => b' ',
            Self::Other(byte) => byte,
        }
    }

    /// The flag the `X(01)` item holds.
    #[must_use]
    pub const fn from_alnum(value: Alnum<1>) -> Self {
        Self::from_byte(value.as_bytes()[0])
    }

    /// The `X(01)` item holding the flag's byte.
    #[must_use]
    pub const fn to_alnum(self) -> Alnum<1> {
        Alnum::from_array([self.as_byte()])
    }
}

/// Defines [`NewPendingAuthDetail`] and [`PendingAuthDetail`] from one field
/// list, so the two records keep the same field names, types and docs, and
/// the conversions between them name every field.
macro_rules! pending_auth_detail_records {
    ($($(#[$attr:meta])* $field:ident: $ty:ty,)+) => {
        /// A pending authorization before its insert assigns an [`AuthId`]: the
        /// detail `COPAUA0C` `8500-INSERT-AUTH` builds and inserts as `PAUTDTL1`
        /// under its summary
        /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L854-933]
        /// (P-COPAUA0C-014).
        ///
        /// The fields follow `CIPAUDTY` order, after the parent key.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct NewPendingAuthDetail {
            $($(#[$attr])* pub $field: $ty,)+
        }

        /// A stored pending authorization, a row of `pending_auth_details`: its
        /// identity, then the fields of a [`NewPendingAuthDetail`].
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct PendingAuthDetail {
            /// The row identity `auth_id`, the REST key of a detail. It is no
            /// copybook field; the IMS key columns stay alongside it.
            pub auth_id: AuthId,
            $($(#[$attr])* pub $field: $ty,)+
        }

        impl PendingAuthDetail {
            /// The stored detail `new` became when its insert returned
            /// `auth_id`.
            #[must_use]
            pub fn from_new(auth_id: AuthId, new: NewPendingAuthDetail) -> Self {
                let NewPendingAuthDetail { $($field),+ } = new;
                Self { auth_id, $($field),+ }
            }

            /// The detail without its identity: the segment as the COBOL holds
            /// it.
            #[must_use]
            pub fn to_new(&self) -> NewPendingAuthDetail {
                NewPendingAuthDetail { $($field: self.$field),+ }
            }
        }
    };
}

pending_auth_detail_records! {
    /// `PA-ACCT-ID PIC S9(11) COMP-3` of the parent `PAUTSUM0`, P-CIPAUSMY-001
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L19]: the `ACCNTID`
    /// the detail is inserted under. It is no `CIPAUDTY` field. `COPAUA0C`
    /// fills it from the unsigned `XREF-ACCT-ID`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L911].
    pa_acct_id: AccountId,
    /// `PA-AUTH-DATE-9C PIC S9(05) COMP-3`, P-CIPAUDTY-001
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L20]: the first part
    /// of `PA-AUTHORIZATION-KEY`. SQL `INTEGER`.
    ///
    /// The key is complemented. `COPAUA0C` stores `99999 - YYDDD` here and
    /// `999999999 - (HHMMSS * 1000 + milliseconds)` in `pa_auth_time_9c`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875]
    /// (P-COPAUA0C-014), the pair [`crate::clock::auth_key_complement`]
    /// computes. The key shrinks as time advances, so ascending key order lists
    /// the newest detail first. `CBPAUP0C` complements the date back
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L280]
    /// (P-CBPAUP0C-004), and `COPAUS2C` the time
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L107]
    /// (P-COPAUS2C-001).
    pa_auth_date_9c: i32,
    /// `PA-AUTH-TIME-9C PIC S9(09) COMP-3`, P-CIPAUDTY-002
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L21]: the second part
    /// of `PA-AUTHORIZATION-KEY`, complemented as `pa_auth_date_9c` describes.
    /// SQL `INTEGER`.
    ///
    /// An `i32`, as its reviewed section A row and the `INTEGER` column type
    /// it, rather than the `i64` in which `auth_key_complement` returns this
    /// part: nine digits always fit, and a caller converts with `i32::try_from`.
    pa_auth_time_9c: i32,
    /// `PA-AUTH-ORIG-DATE PIC X(06)`, P-CIPAUDTY-003
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L22]: the request's
    /// date as `YYMMDD`, which `COPAUS2C` splits into year, month and day
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L103-105].
    pa_auth_orig_date: Alnum<6>,
    /// `PA-AUTH-ORIG-TIME PIC X(06)`, P-CIPAUDTY-004
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L23].
    pa_auth_orig_time: Alnum<6>,
    /// `PA-CARD-NUM PIC X(16)`, P-CIPAUDTY-005
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L24].
    pa_card_num: CardNumber,
    /// `PA-AUTH-TYPE PIC X(04)`, P-CIPAUDTY-006
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L25].
    pa_auth_type: Alnum<4>,
    /// `PA-CARD-EXPIRY-DATE PIC X(04)`, P-CIPAUDTY-007
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L26].
    pa_card_expiry_date: Alnum<4>,
    /// `PA-MESSAGE-TYPE PIC X(06)`, P-CIPAUDTY-008
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L27].
    pa_message_type: Alnum<6>,
    /// `PA-MESSAGE-SOURCE PIC X(06)`, P-CIPAUDTY-009
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L28].
    pa_message_source: Alnum<6>,
    /// `PA-AUTH-ID-CODE PIC X(06)`, P-CIPAUDTY-010
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L29].
    pa_auth_id_code: Alnum<6>,
    /// `PA-AUTH-RESP-CODE PIC X(02)`, P-CIPAUDTY-011
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L30]; its 88
    /// `PA-AUTH-APPROVED VALUE '00'`
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L31] is
    /// `is_approved`.
    pa_auth_resp_code: Alnum<2>,
    /// `PA-AUTH-RESP-REASON PIC X(04)`, P-CIPAUDTY-012
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L32].
    pa_auth_resp_reason: Alnum<4>,
    /// `PA-PROCESSING-CODE PIC 9(06)`, P-CIPAUDTY-013
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L33], the row's
    /// `ProcessingCode`. SQL `INTEGER`.
    pa_processing_code: NumericCode<6>,
    /// `PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3`, P-CIPAUDTY-014
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L34]. SQL
    /// `NUMERIC(12,2)`.
    pa_transaction_amt: Money<10>,
    /// `PA-APPROVED-AMT PIC S9(10)V99 COMP-3`, P-CIPAUDTY-015
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L35]. SQL
    /// `NUMERIC(12,2)`.
    pa_approved_amt: Money<10>,
    /// `PA-MERCHANT-CATAGORY-CODE PIC X(04)`, P-CIPAUDTY-016
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L36], spelled as the
    /// copybook spells it.
    pa_merchant_catagory_code: Alnum<4>,
    /// `PA-ACQR-COUNTRY-CODE PIC X(03)`, P-CIPAUDTY-017
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L37].
    pa_acqr_country_code: Alnum<3>,
    /// `PA-POS-ENTRY-MODE PIC 9(02)`, P-CIPAUDTY-018
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L38], the row's
    /// `PosEntryMode`. SQL `INTEGER`.
    pa_pos_entry_mode: NumericCode<2>,
    /// `PA-MERCHANT-ID PIC X(15)`, P-CIPAUDTY-019
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L39].
    pa_merchant_id: Alnum<15>,
    /// `PA-MERCHANT-NAME PIC X(22)`, P-CIPAUDTY-020
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L40].
    pa_merchant_name: Alnum<22>,
    /// `PA-MERCHANT-CITY PIC X(13)`, P-CIPAUDTY-021
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L41].
    pa_merchant_city: Alnum<13>,
    /// `PA-MERCHANT-STATE PIC X(02)`, P-CIPAUDTY-022
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L42].
    pa_merchant_state: Alnum<2>,
    /// `PA-MERCHANT-ZIP PIC X(09)`, P-CIPAUDTY-023
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L43].
    pa_merchant_zip: Alnum<9>,
    /// `PA-TRANSACTION-ID PIC X(15)`, P-CIPAUDTY-024
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L44]: the
    /// authorization's own reference, unrelated to the transaction master.
    pa_transaction_id: Alnum<15>,
    /// `PA-MATCH-STATUS PIC X(01)`, P-CIPAUDTY-025
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L45-49], with its
    /// four 88-levels.
    pa_match_status: MatchStatus,
    /// `PA-AUTH-FRAUD PIC X(01)`, P-CIPAUDTY-026
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L50-52], with its
    /// two 88-levels.
    pa_auth_fraud: FraudFlag,
    /// `PA-FRAUD-RPT-DATE PIC X(08)`, P-CIPAUDTY-027
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L53]: spaces on
    /// insert [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L909]. When
    /// fraud is marked, `COPAUS2C` fills it from `FORMATTIME MMDDYY DATESEP`,
    /// giving `mm/dd/yy`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L95-101]
    /// (P-COPAUS2C-001).
    pa_fraud_rpt_date: Alnum<8>,
}

impl NewPendingAuthDetail {
    /// 88 `PA-AUTH-APPROVED VALUE '00'` (P-CIPAUDTY-011)
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L31]: the response
    /// code is exactly `00`. `CBPAUP0C` makes the same test when it purges a
    /// detail [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L287].
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.pa_auth_resp_code.as_bytes() == AUTH_APPROVED
    }
}

impl PendingAuthDetail {
    /// 88 `PA-AUTH-APPROVED VALUE '00'`, as [`NewPendingAuthDetail::is_approved`].
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.pa_auth_resp_code.as_bytes() == AUTH_APPROVED
    }
}

#[cfg(test)]
mod tests {
    use super::{FraudFlag, MatchStatus, NewPendingAuthDetail, PendingAuthDetail};
    use crate::numeric::{AccountId, Alnum, AuthId, CardNumber, Money, NumericCode, NumericError};

    /// The 88-level variants of each flag with their bytes.
    const MATCH_VALUES: [(MatchStatus, u8); 4] = [
        (MatchStatus::Pending, b'P'),
        (MatchStatus::AuthDeclined, b'D'),
        (MatchStatus::PendingExpired, b'E'),
        (MatchStatus::MatchedWithTran, b'M'),
    ];
    const FRAUD_VALUES: [(FraudFlag, u8); 3] = [
        (FraudFlag::Confirmed, b'F'),
        (FraudFlag::Removed, b'R'),
        (FraudFlag::Blank, b' '),
    ];

    /// An approved detail as `COPAUA0C` inserts it under fixture account 50,
    /// keyed by the clock pinned at 2022-07-01T12:00:00.000Z (P-COPAUA0C-030).
    fn sample() -> Result<NewPendingAuthDetail, NumericError> {
        Ok(NewPendingAuthDetail {
            pa_acct_id: AccountId::new(50)?,
            pa_auth_date_9c: 77_817,
            pa_auth_time_9c: 879_999_999,
            pa_auth_orig_date: Alnum::move_from("220701"),
            pa_auth_orig_time: Alnum::move_from("120000"),
            pa_card_num: CardNumber::move_from("0500024453765740"),
            pa_auth_type: Alnum::move_from("SALE"),
            pa_card_expiry_date: Alnum::move_from("0225"),
            pa_message_type: Alnum::move_from("0100"),
            pa_message_source: Alnum::move_from("POS"),
            pa_auth_id_code: Alnum::move_from("123456"),
            pa_auth_resp_code: Alnum::move_from("00"),
            pa_auth_resp_reason: Alnum::move_from("0000"),
            pa_processing_code: NumericCode::new(0)?,
            pa_transaction_amt: Money::from_cents(12_345)?,
            pa_approved_amt: Money::from_cents(12_345)?,
            pa_merchant_catagory_code: Alnum::move_from("5411"),
            pa_acqr_country_code: Alnum::move_from("840"),
            pa_pos_entry_mode: NumericCode::new(5)?,
            pa_merchant_id: Alnum::move_from("000000000000001"),
            pa_merchant_name: Alnum::move_from("CORNER GROCERY"),
            pa_merchant_city: Alnum::move_from("SEATTLE"),
            pa_merchant_state: Alnum::move_from("WA"),
            pa_merchant_zip: Alnum::move_from("98101"),
            pa_transaction_id: Alnum::move_from("TX0000000000001"),
            pa_match_status: MatchStatus::Pending,
            pa_auth_fraud: FraudFlag::Blank,
            pa_fraud_rpt_date: Alnum::spaces(),
        })
    }

    #[test]
    fn named_values_convert_to_and_from_their_bytes() {
        for (status, byte) in MATCH_VALUES {
            assert_eq!(status.as_byte(), byte, "{status:?}");
            assert_eq!(MatchStatus::from_byte(byte), status);
            assert_eq!(MatchStatus::from_byte(status.as_byte()), status);
            assert_eq!(MatchStatus::from_alnum(status.to_alnum()), status);
        }
        for (flag, byte) in FRAUD_VALUES {
            assert_eq!(flag.as_byte(), byte, "{flag:?}");
            assert_eq!(FraudFlag::from_byte(byte), flag);
            assert_eq!(FraudFlag::from_byte(flag.as_byte()), flag);
            assert_eq!(FraudFlag::from_alnum(flag.to_alnum()), flag);
        }
    }

    #[test]
    fn every_byte_is_kept_and_unnamed_bytes_are_other() {
        for byte in u8::MIN..=u8::MAX {
            let item = Alnum::from_array([byte]);

            let status = MatchStatus::from_byte(byte);
            assert_eq!(status.as_byte(), byte);
            assert_eq!(status.to_alnum(), item);
            assert_eq!(MatchStatus::from_alnum(item), status);
            let named = MATCH_VALUES.iter().any(|&(_, value)| value == byte);
            assert_eq!(
                matches!(status, MatchStatus::Other(_)),
                !named,
                "{byte:#04x}"
            );

            let flag = FraudFlag::from_byte(byte);
            assert_eq!(flag.as_byte(), byte);
            assert_eq!(flag.to_alnum(), item);
            assert_eq!(FraudFlag::from_alnum(item), flag);
            let named = FRAUD_VALUES.iter().any(|&(_, value)| value == byte);
            assert_eq!(matches!(flag, FraudFlag::Other(_)), !named, "{byte:#04x}");
        }
        // A new detail's match status is never a space; a space there is kept.
        assert_eq!(MatchStatus::from_byte(b' '), MatchStatus::Other(b' '));
    }

    #[test]
    fn only_response_code_00_is_approved() -> Result<(), NumericError> {
        for (code, expected) in [("00", true), ("05", false), ("  ", false), ("0", false)] {
            let new = NewPendingAuthDetail {
                pa_auth_resp_code: Alnum::move_from(code),
                ..sample()?
            };
            assert_eq!(new.is_approved(), expected, "{code:?}");
            let stored = PendingAuthDetail::from_new(AuthId::from(1), new);
            assert_eq!(stored.is_approved(), expected, "{code:?}");
        }
        Ok(())
    }

    #[test]
    fn from_new_then_to_new_returns_the_detail() -> Result<(), NumericError> {
        let new = sample()?;
        let stored = PendingAuthDetail::from_new(AuthId::from(42), new.clone());
        assert_eq!(stored.auth_id, AuthId::from(42));
        assert_eq!(stored.pa_card_num, new.pa_card_num);
        assert_eq!(stored.pa_auth_time_9c, new.pa_auth_time_9c);
        assert_eq!(stored.to_new(), new);
        Ok(())
    }
}
