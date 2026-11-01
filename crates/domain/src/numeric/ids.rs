//! Identifiers, codes and keys of the copybook records, the row identities of
//! the replacement tables, and the pending-authorization counters.
//!
//! Section A of `docs/parity/parity-oracle-audit.md` maps every field below to
//! its type; the row IDs in each type's documentation are that audit's.
//!
//! - `9(n)` identifiers and codes ([`AccountId`], [`CustomerId`],
//!   [`TranCategoryCode`], [`Cvv`], [`MerchantId`], [`CustSsn`], [`FicoScore`]
//!   and [`NumericCode`]) hold an unsigned integer below `10^n`. They name a
//!   record or classify one and never take part in arithmetic, so none of them
//!   implements `Add`, `Sub`, `Mul` or `Neg`. Their SQL column is `BIGINT` or
//!   `INTEGER` with `CHECK (0 <= v < 10^n)`, bound through `to_i64` and
//!   `TryFrom<i64>`; their text is the `n`-digit, zero-padded `DISPLAY` form.
//! - `X(n)` keys and codes ([`CardNumber`], [`TranId`], [`TranTypeCode`],
//!   [`UserId`] and [`AccountGroupId`]) hold the `n` stored bytes of an
//!   [`Alnum<n>`](Alnum). Equality and ordering compare the padded bytes: the
//!   VSAM key order under ASCII collation, as [`Alnum`] describes. Their SQL
//!   column is `VARCHAR(n)` holding the `trimmed` text, which persistence pads
//!   again with `move_from`.
//! - Row identities ([`TransactionRowId`], [`RuleId`], [`AccrualId`] and
//!   [`AuthId`]) are `BIGINT` identity columns of the replacement tables, which
//!   no copybook defines.
//! - [`AuthCount`] is the `S9(04) COMP` approved or declined count of a
//!   pending-authorization summary, changed only by a checked step of one.
//!
//! [`TranId::next`] is the only arithmetic on a key: the `ADD 1` by which
//! `COTRN02C` and `COBIL00C` allocate the next TRAN-ID.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use super::alnum::Alnum;
use super::{NumericError, Overflow};

/// `10^exponent` as a `u32`: the limit of a `9(exponent)` code.
const fn pow10(exponent: u8) -> u32 {
    let mut value: u32 = 1;
    let mut done = 0;
    while done < exponent {
        value *= 10;
        done += 1;
    }
    value
}

/// `digits` as a `usize`, counted up so that no cast is needed.
const fn width_of(digits: u8) -> usize {
    let mut width = 0;
    let mut left = digits;
    while left > 0 {
        width += 1;
        left -= 1;
    }
    width
}

/// The API shared by every unsigned `9(n)` field type.
///
/// `impl[...]` carries the impl generics (empty for a concrete type), `$ty` is
/// the type, `$inner` its unsigned integer, `width` the `n` of the PIC clause,
/// `limit` the value `10^n` and the last token the widening to `i64`: `from`
/// for `u16` and `u32`, `try_from` for `u64`.
macro_rules! digit_field_api {
    (@to_i64 from) => {
        /// The value as the `i64` its SQL `BIGINT` or `INTEGER` column binds.
        #[must_use]
        pub fn to_i64(self) -> i64 {
            i64::from(self.0)
        }
    };
    (@to_i64 try_from) => {
        /// The value as the `i64` its SQL `BIGINT` column binds.
        ///
        /// # Panics
        ///
        /// Never in practice: every constructor keeps the value below `10^n`,
        /// and `n` is at most 11, far inside the `i64` range.
        #[must_use]
        pub fn to_i64(self) -> i64 {
            i64::try_from(self.0).expect("a 9(n) value of at most 11 digits fits an i64")
        }
    };
    (
        impl[$($generics:tt)*] $ty:ty, $inner:ty,
        width = $width:expr, limit = $limit:expr, $to_i64:ident
    ) => {
        impl<$($generics)*> $ty {
            /// The number of digits of the PIC clause: the `n` of `9(n)`.
            pub const WIDTH: usize = $width;

            /// `10^n`, the smallest value the field cannot hold.
            const MAX_EXCLUSIVE: $inner = $limit;

            /// The field holding `value`.
            ///
            /// # Errors
            ///
            /// [`NumericError::OutOfRange`] when `value` has more digits than
            /// the PIC clause allows, that is when it is `10^n` or more.
            pub const fn new(value: $inner) -> Result<Self, NumericError> {
                if value < Self::MAX_EXCLUSIVE {
                    Ok(Self(value))
                } else {
                    Err(NumericError::OutOfRange)
                }
            }

            /// The value.
            #[must_use]
            pub const fn get(self) -> $inner {
                self.0
            }

            digit_field_api!(@to_i64 $to_i64);
        }

        /// The value read from its SQL `BIGINT` or `INTEGER` column.
        ///
        /// A negative value is rejected with [`NumericError::Negative`], and a
        /// value of `10^n` or more with [`NumericError::OutOfRange`].
        impl<$($generics)*> TryFrom<i64> for $ty {
            type Error = NumericError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                if value < 0 {
                    return Err(NumericError::Negative);
                }
                <$inner>::try_from(value)
                    .map_err(|_| NumericError::OutOfRange)
                    .and_then(Self::new)
            }
        }

        /// The value zero-padded to the `n` digits of the PIC clause, as the
        /// `DISPLAY` field stores it.
        impl<$($generics)*> fmt::Display for $ty {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                let value = self.0;
                let width = Self::WIDTH;
                write!(formatter, "{value:0width$}")
            }
        }

        /// The value of 1 to `n` ASCII digits; leading zeros are accepted.
        ///
        /// Text that is empty or holds any byte other than an ASCII digit (a
        /// sign, a space, a decimal point) is rejected with
        /// [`NumericError::NotNumeric`], and more than `n` digits with
        /// [`NumericError::OutOfRange`]. The COBOL screen edits of such a field
        /// (blank, non-numeric or zero input, each with its message) belong to
        /// the rules, which apply them before this conversion.
        impl<$($generics)*> FromStr for $ty {
            type Err = NumericError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(NumericError::NotNumeric);
                }
                if text.len() > Self::WIDTH {
                    return Err(NumericError::OutOfRange);
                }
                text.parse::<$inner>()
                    .map_err(|_| NumericError::OutOfRange)
                    .and_then(Self::new)
            }
        }
    };
}

/// A named unsigned `9(n)` identifier or code over the integer `$inner`.
///
/// A leading `sensitive` marks a field whose value diagnostics must not show:
/// the type then derives no `Debug`, and implements it by hand.
macro_rules! digit_id {
    (
        @define [$($debug:ident)?]
        $(#[$attr:meta])*
        $name:ident($inner:ty), width = $width:literal, limit = $limit:literal, $to_i64:ident
    ) => {
        $(#[$attr])*
        #[derive($($debug,)? Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($inner);

        digit_field_api!(
            impl[] $name, $inner, width = $width, limit = $limit, $to_i64
        );
    };
    (
        $(#[$attr:meta])*
        sensitive $name:ident($inner:ty), width = $width:literal, limit = $limit:literal,
        $to_i64:ident
    ) => {
        digit_id! {
            @define []
            $(#[$attr])*
            $name($inner), width = $width, limit = $limit, $to_i64
        }
    };
    (
        $(#[$attr:meta])*
        $name:ident($inner:ty), width = $width:literal, limit = $limit:literal, $to_i64:ident
    ) => {
        digit_id! {
            @define [Debug]
            $(#[$attr])*
            $name($inner), width = $width, limit = $limit, $to_i64
        }
    };
}

/// The `Debug` form of a field whose value diagnostics must not show: the type
/// name around `<redacted>`, whatever the value.
fn debug_redacted(formatter: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    formatter
        .debug_tuple(name)
        .field(&format_args!("<redacted>"))
        .finish()
}

digit_id! {
    /// An account number, `PIC 9(11)`.
    ///
    /// `ACCT-ID` P-CVACT01Y-001 [app/cpy/CVACT01Y.cpy:L5], `CARD-ACCT-ID`
    /// P-CVACT02Y-002 [app/cpy/CVACT02Y.cpy:L6], `XREF-ACCT-ID` P-CVACT03Y-003
    /// [app/cpy/CVACT03Y.cpy:L7], `TRANCAT-ACCT-ID` P-CVTRA01Y-001
    /// [app/cpy/CVTRA01Y.cpy:L6], the packed `PA-ACCT-ID S9(11) COMP-3`
    /// P-CIPAUSMY-001 [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L19]
    /// and the Db2 `ACCT_ID DECIMAL(11)` P-AUTHFRDS-025
    /// [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L26]. SQL
    /// `BIGINT CHECK (0 <= v < 10^11)`.
    AccountId(u64), width = 11, limit = 100_000_000_000, try_from
}

digit_id! {
    /// A customer number, `PIC 9(09)`.
    ///
    /// `CUST-ID` P-CVCUS01Y-001 [app/cpy/CVCUS01Y.cpy:L5], `XREF-CUST-ID`
    /// P-CVACT03Y-002 [app/cpy/CVACT03Y.cpy:L6], `PA-CUST-ID` P-CIPAUSMY-002
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L20] and the Db2
    /// `CUST_ID DECIMAL(9)` P-AUTHFRDS-026
    /// [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L27]. SQL `INTEGER`
    /// or `BIGINT` with `CHECK (0 <= v < 10^9)`.
    CustomerId(u32), width = 9, limit = 1_000_000_000, from
}

digit_id! {
    /// A transaction category code, `PIC 9(04)`.
    ///
    /// `TRAN-CAT-CD` P-CVTRA05Y-003 [app/cpy/CVTRA05Y.cpy:L7],
    /// `DALYTRAN-CAT-CD` P-CVTRA06Y-003 [app/cpy/CVTRA06Y.cpy:L7], `TRANCAT-CD`
    /// P-CVTRA01Y-003 [app/cpy/CVTRA01Y.cpy:L8], `DIS-TRAN-CAT-CD`
    /// P-CVTRA02Y-003 [app/cpy/CVTRA02Y.cpy:L8], the processing copy's
    /// `TRAN-CAT-CD` P-CVTRA04Y-002 [app/cpy/CVTRA04Y.cpy:L7] and the Db2
    /// master's `TRC_TYPE_CATEGORY CHAR(4)` P-TRNTYCAT-002
    /// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L3], which keeps its
    /// `CHAR(4)` text and is read through [`FromStr`]. SQL
    /// `INTEGER CHECK (0 <= v < 10^4)` elsewhere.
    TranCategoryCode(u16), width = 4, limit = 10_000, from
}

digit_id! {
    /// A card verification code, `PIC 9(03)`.
    ///
    /// `CARD-CVV-CD` P-CVACT02Y-003 [app/cpy/CVACT02Y.cpy:L7]. SQL
    /// `INTEGER CHECK (0 <= v < 10^3)`.
    ///
    /// `Debug` shows `Cvv(<redacted>)` and no digit of the code, because a card
    /// verification code is sensitive card data that diagnostics must never
    /// show; `Display` and the SQL conversions keep the full value.
    sensitive Cvv(u16), width = 3, limit = 1_000, from
}

/// The code withheld: `Cvv(<redacted>)`.
impl fmt::Debug for Cvv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_redacted(formatter, "Cvv")
    }
}

digit_id! {
    /// A merchant number, `PIC 9(09)`.
    ///
    /// `TRAN-MERCHANT-ID` P-CVTRA05Y-007 [app/cpy/CVTRA05Y.cpy:L11] and
    /// `DALYTRAN-MERCHANT-ID` P-CVTRA06Y-007 [app/cpy/CVTRA06Y.cpy:L11]. SQL
    /// `INTEGER CHECK (0 <= v < 10^9)`. The pending-authorization
    /// `PA-MERCHANT-ID` is `X(15)` and stays an [`Alnum<15>`](Alnum)
    /// (P-CIPAUDTY-019).
    MerchantId(u32), width = 9, limit = 1_000_000_000, from
}

digit_id! {
    /// A customer's social security number, `PIC 9(09)`.
    ///
    /// `CUST-SSN` P-CVCUS01Y-013 [app/cpy/CVCUS01Y.cpy:L17], which the audit
    /// row labels `Ssn`. SQL `INTEGER CHECK (0 <= v < 10^9)`.
    ///
    /// `Debug` shows `CustSsn(<redacted>)` and no digit of the number, because a
    /// social security number is sensitive customer data; `Display` and the SQL
    /// conversions keep the full value.
    sensitive CustSsn(u32), width = 9, limit = 1_000_000_000, from
}

/// The number withheld: `CustSsn(<redacted>)`.
impl fmt::Debug for CustSsn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_redacted(formatter, "CustSsn")
    }
}

digit_id! {
    /// A customer's FICO credit score, `PIC 9(03)`.
    ///
    /// `CUST-FICO-CREDIT-SCORE` P-CVCUS01Y-018 [app/cpy/CVCUS01Y.cpy:L22]. SQL
    /// `INTEGER CHECK (0 <= v < 10^3)`.
    FicoScore(u16), width = 3, limit = 1_000, from
}

/// An unsigned `PIC 9(N)` code without a type of its own, `N` in `1..=9`.
///
/// The section A rows name these fields after their meaning; their width alone
/// decides the type:
///
/// - `NumericCode<6>`: `PA-PROCESSING-CODE` P-CIPAUDTY-013
///   [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L33] and the request's
///   `PA-RQ-PROCESSING-CODE` P-CCPAURQY-008
///   [app/app-authorization-ims-db2-mq/cpy/CCPAURQY.cpy:L26].
/// - `NumericCode<2>`: `PA-POS-ENTRY-MODE` P-CIPAUDTY-018
///   [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L38], the request's
///   `PA-RQ-POS-ENTRY-MODE` P-CCPAURQY-012
///   [app/app-authorization-ims-db2-mq/cpy/CCPAURQY.cpy:L30], the Db2
///   `POS_ENTRY_MODE SMALLINT` P-AUTHFRDS-015 and the MQ error record's
///   `MQ-APPL-CONDITION-CODE` (P-COACCT01-005, P-CODATE01-005).
/// - `NumericCode<4>`: the reject reason `WS-VALIDATION-FAIL-REASON`
///   P-CBTRN02C-002 [app/cbl/CBTRN02C.cbl:L181].
/// - `NumericCode<5>`: the MQ error record's `MQ-APPL-REASON-CODE`
///   (P-COACCT01-007, P-CODATE01-007).
///
/// A width outside `1..=9` fails to compile wherever a code of that width is
/// built, read or displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumericCode<const N: u8>(u32);

impl<const N: u8> NumericCode<N> {
    /// `N`, checked to lie in `1..=9` so that `10^N` fits a `u32`.
    const DIGITS: u8 = {
        assert!(N >= 1, "NumericCode<N> needs at least one digit");
        assert!(N <= 9, "NumericCode<N> holds at most 9 digits");
        N
    };
}

digit_field_api!(
    impl[const N: u8] NumericCode<N>, u32,
    width = width_of(Self::DIGITS), limit = pow10(Self::DIGITS), from
);

/// A named `X(n)` key or code over the item `Alnum<$width>`.
///
/// A leading `sensitive` marks a key whose text diagnostics must not show in
/// full: the type then derives no `Debug`, and implements it by hand.
macro_rules! text_key {
    ($(#[$attr:meta])* sensitive $name:ident(Alnum<$width:literal>)) => {
        text_key!(@define [] $(#[$attr])* $name(Alnum<$width>));
    };
    ($(#[$attr:meta])* $name:ident(Alnum<$width:literal>)) => {
        text_key!(@define [Debug] $(#[$attr])* $name(Alnum<$width>));
    };
    (@define [$($debug:ident)?] $(#[$attr:meta])* $name:ident(Alnum<$width:literal>)) => {
        $(#[$attr])*
        #[derive($($debug,)? Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Alnum<$width>);

        impl $name {
            /// The COBOL alphanumeric `MOVE` of `text` into the key.
            ///
            /// The text is copied left-justified, padded with spaces and cut on
            /// the right at the key's width, as [`Alnum::move_from`] does. This
            /// is also how a right-trimmed `VARCHAR` value read back from the
            /// database becomes the key again. No case is changed.
            #[must_use]
            pub fn move_from(text: &str) -> Self {
                Self(Alnum::move_from(text))
            }

            /// The key holding exactly the bytes of `value`, as a record codec
            /// decodes the field.
            #[must_use]
            pub const fn from_alnum(value: Alnum<$width>) -> Self {
                Self(value)
            }

            /// The key as the `X(n)` item it is stored in.
            #[must_use]
            pub const fn as_alnum(&self) -> &Alnum<$width> {
                &self.0
            }

            /// The stored bytes, trailing spaces included.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $width] {
                self.0.as_bytes()
            }

            /// The key without its trailing spaces: the `VARCHAR` value
            /// persistence binds.
            #[must_use]
            pub fn trimmed(&self) -> Cow<'_, str> {
                self.0.trimmed()
            }
        }

        /// The full text of the key, trailing spaces included: the COBOL width.
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }
    };
}

text_key! {
    /// A card number, `PIC X(16)`.
    ///
    /// `CARD-NUM` P-CVACT02Y-001 [app/cpy/CVACT02Y.cpy:L5], `XREF-CARD-NUM`
    /// P-CVACT03Y-001 [app/cpy/CVACT03Y.cpy:L5], `TRAN-CARD-NUM`
    /// P-CVTRA05Y-011 [app/cpy/CVTRA05Y.cpy:L15], `DALYTRAN-CARD-NUM`
    /// P-CVTRA06Y-011 [app/cpy/CVTRA06Y.cpy:L15], `PA-CARD-NUM` P-CIPAUDTY-005
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:L24], `PA-RQ-CARD-NUM`
    /// P-CCPAURQY-003, `PA-RL-CARD-NUM` P-CCPAURLY-001 and the Db2
    /// `CARD_NUM CHAR(16)` P-AUTHFRDS-001. SQL `VARCHAR(16)`.
    ///
    /// `Debug` shows the number masked to its last four characters, for example
    /// `CardNumber("************1111")`, because a full card number is sensitive
    /// card data; `Display`, [`CardNumber::trimmed`] and the stored bytes keep
    /// the full text.
    sensitive CardNumber(Alnum<16>)
}

/// The trimmed number with every character but the last four replaced by `*`,
/// and every character when it has four or fewer.
impl fmt::Debug for CardNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The trailing characters a masked number keeps.
        const SHOWN: usize = 4;
        let text = self.trimmed();
        let count = text.chars().count();
        let hidden = if count > SHOWN { count - SHOWN } else { count };
        let masked: String = std::iter::repeat_n('*', hidden)
            .chain(text.chars().skip(hidden))
            .collect();
        formatter.debug_tuple("CardNumber").field(&masked).finish()
    }
}

text_key! {
    /// A transaction ID, `PIC X(16)`: the key of the transaction master.
    ///
    /// `TRAN-ID` P-CVTRA05Y-001 [app/cpy/CVTRA05Y.cpy:L5] and `DALYTRAN-ID`
    /// P-CVTRA06Y-001 [app/cpy/CVTRA06Y.cpy:L5]. SQL `VARCHAR(16)` column
    /// `tran_id`; the row identity of a transaction is the separate
    /// [`TransactionRowId`] (E-09).
    ///
    /// The key is text, as in the COBOL: posting copies `DALYTRAN-ID` whatever
    /// it holds, and [`TranId::next`] is the one place it is read as a number.
    TranId(Alnum<16>)
}

text_key! {
    /// A transaction type code, `PIC X(02)`.
    ///
    /// `TRAN-TYPE-CD` P-CVTRA05Y-002 [app/cpy/CVTRA05Y.cpy:L6],
    /// `DALYTRAN-TYPE-CD` P-CVTRA06Y-002 [app/cpy/CVTRA06Y.cpy:L6],
    /// `TRANCAT-TYPE-CD` P-CVTRA01Y-002 [app/cpy/CVTRA01Y.cpy:L7],
    /// `DIS-TRAN-TYPE-CD` P-CVTRA02Y-002, the processing copies' `TRAN-TYPE`
    /// P-CVTRA03Y-001 and `TRAN-TYPE-CD` P-CVTRA04Y-001, the `MNTTRDB2` input's
    /// `INPUT-REC-NUMBER` P-COBTUPDT-002, and the Db2 masters' `TR_TYPE`
    /// P-TRNTYPE-001 and `TRC_TYPE_CODE` P-TRNTYCAT-001, both `CHAR(2)`. SQL
    /// `VARCHAR(2)`.
    TranTypeCode(Alnum<2>)
}

text_key! {
    /// A user ID, `PIC X(08)`.
    ///
    /// `SEC-USR-ID` P-CSUSR01Y-001 [app/cpy/CSUSR01Y.cpy:L18]. SQL `VARCHAR(8)`
    /// column `user_id`. The key holds the ID as stored: `COSGN00C` upper-cases
    /// the ID typed at sign-on before it reads the user, and that conversion
    /// belongs to the sign-on rule and `auth::password`, never to this type.
    UserId(Alnum<8>)
}

text_key! {
    /// An account's disclosure group, `PIC X(10)`.
    ///
    /// `ACCT-GROUP-ID` P-CVACT01Y-012 [app/cpy/CVACT01Y.cpy:L16], the key by
    /// which `CBACT04C` looks up an interest rate. SQL `VARCHAR(10)` column
    /// `acct_group_id`. All 50 fixture accounts leave it blank (E-06).
    ///
    /// The audit row gives this field the type `Alnum<10>`. The width is the
    /// same: the field has a type of its own because it is a key, like the
    /// other `X(n)` keys, and [`AccountGroupId::as_alnum`] yields the
    /// `Alnum<10>` the row names, which is also the type of the disclosure
    /// group record's `DIS-ACCT-GROUP-ID` (P-CVTRA02Y-001) it is matched with.
    AccountGroupId(Alnum<10>)
}

impl TranId {
    /// `10^16`, one above the largest value of the `PIC 9(16)` working field.
    const MODULUS: u64 = 10_000_000_000_000_000;

    /// Sixteen zeros: `MOVE ZEROS TO TRAN-ID`.
    ///
    /// `COTRN02C` `READPREV-TRANSACT-FILE` stores it when the browse ends
    /// (P-COTRN02C-022) [app/cbl/COTRN02C.cbl:L689], as `COBIL00C` does
    /// (P-COBIL00C-014) [app/cbl/COBIL00C.cbl:L484-496]; [`TranId::next`] then
    /// gives `0000000000000001`.
    #[must_use]
    pub const fn zeros() -> Self {
        Self(Alnum::from_array([b'0'; 16]))
    }

    /// The TRAN-ID one above this one, as `COTRN02C` and `COBIL00C` allocate it.
    ///
    /// COTRN02C ADD-TRANSACTION — P-COTRN02C-020 [app/cbl/COTRN02C.cbl:L442-451];
    /// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-012 [app/cbl/COBIL00C.cbl:L210-219].
    /// Both move the highest key into a `PIC 9(16)` working field
    /// (`WS-TRAN-ID-N` [app/cbl/COTRN02C.cbl:L57], `WS-TRAN-ID-NUM`
    /// [app/cbl/COBIL00C.cbl:L57]), `ADD 1` to it and move it back, so the new
    /// key is 16 digits with leading zeros. The `ADD` has no `ON SIZE ERROR`,
    /// so a sum that does not fit 16 digits loses its high-order digit:
    /// `9999999999999999` is followed by `0000000000000000`. Section C of the
    /// audit records no other outcome for this receiver; a row that came to
    /// record one would decide instead.
    ///
    /// # Errors
    ///
    /// [`NumericError::NotNumeric`] when any of the 16 bytes is not an ASCII
    /// digit: spaces, `HIGH-VALUES`, `LOW-VALUES` or letters. The COBOL would
    /// then `ADD` to non-numeric data, whose result the repository does not
    /// fix, so the case is reported for escalation rather than guessed.
    pub fn next(self) -> Result<Self, NumericError> {
        if !self.0.is_numeric() {
            return Err(NumericError::NotNumeric);
        }
        // Sixteen digits stay below 10^16, so neither the fold nor the `+ 1`
        // can overflow a `u64`.
        let value = self
            .0
            .as_bytes()
            .iter()
            .fold(0_u64, |total, &digit| total * 10 + u64::from(digit - b'0'));
        let next = (value + 1) % Self::MODULUS;
        Ok(Self::move_from(&format!("{next:016}")))
    }
}

/// A `BIGINT` identity column of a replacement table.
macro_rules! row_identity {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(i64);

        impl $name {
            /// The identity value, as its `BIGINT` column holds it.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        /// The identity read from its `BIGINT` column. The database assigns
        /// it, so every value is accepted.
        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value)
            }
        }

        /// The plain decimal integer.
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }
    };
}

row_identity! {
    /// The row identity of a transaction: `transactions.transaction_id`.
    ///
    /// The reward accruals' foreign key targets it. It is distinct from the
    /// COBOL key [`TranId`], which a row keeps after `batch tranbkp` takes it
    /// out of the live master and which a later add may issue again (E-09).
    TransactionRowId
}

row_identity! {
    /// The identity of a reward rule: `reward_rules.rule_id`.
    ///
    /// Rule selection gives a tie between equal rates to the lowest ID, so the
    /// type is ordered.
    RuleId
}

row_identity! {
    /// The identity of a reward accrual: `reward_accruals.accrual_id`, the key
    /// by which the reward history pages.
    AccrualId
}

row_identity! {
    /// The identity of a pending-authorization detail:
    /// `pending_auth_details.auth_id`.
    ///
    /// It is the REST key of a detail; the IMS key columns of the segment stay
    /// alongside it unchanged.
    AuthId
}

/// An approved or declined authorization count, `PIC S9(04) COMP`.
///
/// `PA-APPROVED-AUTH-CNT` P-CIPAUSMY-009 and `PA-DECLINED-AUTH-CNT`
/// P-CIPAUSMY-010 [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L27-28],
/// which the audit rows label `i16 (count)`. SQL `SMALLINT`. Invariant: the
/// magnitude is at most 9999, the four digits of the PIC clause.
///
/// The COBOL changes a count only by one. `COPAUA0C` `8400-UPDATE-SUMMARY`
/// adds 1 per authorization (P-COPAUA0C-013)
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L814]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L820], and `CBPAUP0C`
/// `4000-CHECK-IF-EXPIRED` subtracts 1 per purged detail (P-CBPAUP0C-004)
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L288]
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L291]: those are
/// [`AuthCount::checked_increment`] and [`AuthCount::checked_decrement`]. The
/// purge test `IF PA-APPROVED-AUTH-CNT <= 0` (P-CBPAUP0C-006)
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L156] is a comparison
/// with [`AuthCount::ZERO`]. No other arithmetic exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthCount(i16);

impl AuthCount {
    /// No authorizations: the count a new summary is initialised with.
    pub const ZERO: Self = Self(0);

    /// The largest magnitude four digits hold.
    const LIMIT: u16 = 9_999;

    /// The count holding `value`.
    ///
    /// # Errors
    ///
    /// [`NumericError::OutOfRange`] when the magnitude of `value` exceeds
    /// 9999.
    pub const fn new(value: i16) -> Result<Self, NumericError> {
        if value.unsigned_abs() <= Self::LIMIT {
            Ok(Self(value))
        } else {
            Err(NumericError::OutOfRange)
        }
    }

    /// The count, as its `SMALLINT` column holds it.
    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }

    /// The count plus one: `ADD 1 TO PA-APPROVED-AUTH-CNT` or
    /// `PA-DECLINED-AUTH-CNT`.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when the count would exceed 9999. The COBOL `ADD` has no
    /// `ON SIZE ERROR`, and whether a binary field then keeps 10000 or is cut
    /// to 0000 depends on the `TRUNC` compiler option, which neither the
    /// repository nor the audit fixes; the size error is therefore reported
    /// rather than guessed.
    pub fn checked_increment(self) -> Result<Self, Overflow> {
        // The invariant keeps the sum inside `i16`; `new` rejects 10000.
        Self::new(self.0 + 1).map_err(|_| Overflow)
    }

    /// The count minus one: `SUBTRACT 1 FROM PA-APPROVED-AUTH-CNT` or
    /// `PA-DECLINED-AUTH-CNT`. The field is signed, so a count below zero is
    /// held as such.
    ///
    /// # Errors
    ///
    /// [`Overflow`] when the count would fall below -9999, for the reason
    /// [`AuthCount::checked_increment`] gives.
    pub fn checked_decrement(self) -> Result<Self, Overflow> {
        // The invariant keeps the difference inside `i16`; `new` rejects -10000.
        Self::new(self.0 - 1).map_err(|_| Overflow)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cmp::Ordering;
    use std::fmt;

    use super::{
        AccountGroupId, AccountId, AccrualId, Alnum, AuthCount, AuthId, CardNumber, CustSsn,
        CustomerId, Cvv, FicoScore, MerchantId, NumericCode, NumericError, Overflow, RuleId,
        TranCategoryCode, TranId, TranTypeCode, TransactionRowId, UserId,
    };

    /// `10^width`, computed independently of the types under test.
    fn ten_to_the(width: usize) -> u64 {
        let exponent = u32::try_from(width).expect("a PIC width fits a u32");
        10_u64.pow(exponent)
    }

    #[test]
    fn widths_and_limits_follow_the_pic_clauses() {
        let cases: [(&str, usize, u64, usize); 11] = [
            ("AccountId", AccountId::WIDTH, AccountId::MAX_EXCLUSIVE, 11),
            (
                "CustomerId",
                CustomerId::WIDTH,
                u64::from(CustomerId::MAX_EXCLUSIVE),
                9,
            ),
            (
                "TranCategoryCode",
                TranCategoryCode::WIDTH,
                u64::from(TranCategoryCode::MAX_EXCLUSIVE),
                4,
            ),
            ("Cvv", Cvv::WIDTH, u64::from(Cvv::MAX_EXCLUSIVE), 3),
            (
                "MerchantId",
                MerchantId::WIDTH,
                u64::from(MerchantId::MAX_EXCLUSIVE),
                9,
            ),
            (
                "CustSsn",
                CustSsn::WIDTH,
                u64::from(CustSsn::MAX_EXCLUSIVE),
                9,
            ),
            (
                "FicoScore",
                FicoScore::WIDTH,
                u64::from(FicoScore::MAX_EXCLUSIVE),
                3,
            ),
            (
                "NumericCode<1>",
                NumericCode::<1>::WIDTH,
                u64::from(NumericCode::<1>::MAX_EXCLUSIVE),
                1,
            ),
            (
                "NumericCode<2>",
                NumericCode::<2>::WIDTH,
                u64::from(NumericCode::<2>::MAX_EXCLUSIVE),
                2,
            ),
            (
                "NumericCode<6>",
                NumericCode::<6>::WIDTH,
                u64::from(NumericCode::<6>::MAX_EXCLUSIVE),
                6,
            ),
            (
                "NumericCode<9>",
                NumericCode::<9>::WIDTH,
                u64::from(NumericCode::<9>::MAX_EXCLUSIVE),
                9,
            ),
        ];
        for (name, width, limit, pic_digits) in cases {
            assert_eq!(width, pic_digits, "{name}");
            assert_eq!(limit, ten_to_the(width), "{name}");
        }
    }

    #[test]
    fn new_accepts_values_below_ten_to_the_width_only() -> Result<(), NumericError> {
        assert_eq!(AccountId::new(99_999_999_999)?.get(), 99_999_999_999);
        assert_eq!(
            AccountId::new(100_000_000_000),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(CustomerId::new(999_999_999)?.get(), 999_999_999);
        assert_eq!(
            CustomerId::new(1_000_000_000),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(TranCategoryCode::new(9_999)?.get(), 9_999);
        assert_eq!(TranCategoryCode::new(10_000), Err(NumericError::OutOfRange));
        assert_eq!(Cvv::new(999)?.get(), 999);
        assert_eq!(Cvv::new(1_000), Err(NumericError::OutOfRange));
        assert_eq!(FicoScore::new(1_000), Err(NumericError::OutOfRange));
        assert_eq!(
            MerchantId::new(1_000_000_000),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(CustSsn::new(1_000_000_000), Err(NumericError::OutOfRange));
        assert_eq!(NumericCode::<6>::new(999_999)?.get(), 999_999);
        assert_eq!(
            NumericCode::<6>::new(1_000_000),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(NumericCode::<2>::new(100), Err(NumericError::OutOfRange));
        assert_eq!(NumericCode::<1>::new(9)?.get(), 9);
        Ok(())
    }

    #[test]
    fn digit_text_is_one_to_width_ascii_digits() {
        let cases: [(&str, Result<u64, NumericError>); 13] = [
            ("99999999999", Ok(99_999_999_999)),
            ("00000000001", Ok(1)),
            ("1", Ok(1)),
            ("0", Ok(0)),
            ("1234567890A", Err(NumericError::NotNumeric)),
            ("", Err(NumericError::NotNumeric)),
            ("           ", Err(NumericError::NotNumeric)),
            (" 1", Err(NumericError::NotNumeric)),
            ("+1", Err(NumericError::NotNumeric)),
            ("-1", Err(NumericError::NotNumeric)),
            ("1\u{663}", Err(NumericError::NotNumeric)),
            ("123456789012", Err(NumericError::OutOfRange)),
            ("000000000001", Err(NumericError::OutOfRange)),
        ];
        for (text, expected) in cases {
            assert_eq!(
                text.parse::<AccountId>().map(AccountId::get),
                expected,
                "{text:?}"
            );
        }
        assert_eq!(
            "0001"
                .parse::<TranCategoryCode>()
                .map(TranCategoryCode::get),
            Ok(1)
        );
        assert_eq!(
            "00001".parse::<TranCategoryCode>(),
            Err(NumericError::OutOfRange)
        );
        assert_eq!("05".parse::<NumericCode<2>>().map(NumericCode::get), Ok(5));
        assert_eq!(
            "5 ".parse::<NumericCode<2>>(),
            Err(NumericError::NotNumeric)
        );
    }

    #[test]
    fn sql_values_convert_within_the_pic_range() {
        let cases: [(i64, Result<u64, NumericError>); 6] = [
            (0, Ok(0)),
            (99_999_999_999, Ok(99_999_999_999)),
            (100_000_000_000, Err(NumericError::OutOfRange)),
            (i64::MAX, Err(NumericError::OutOfRange)),
            (-1, Err(NumericError::Negative)),
            (i64::MIN, Err(NumericError::Negative)),
        ];
        for (value, expected) in cases {
            assert_eq!(
                AccountId::try_from(value).map(AccountId::get),
                expected,
                "{value}"
            );
        }
        // Values beyond the inner integer and beyond the PIC width alike.
        assert_eq!(Cvv::try_from(70_000_i64), Err(NumericError::OutOfRange));
        assert_eq!(Cvv::try_from(1_000_i64), Err(NumericError::OutOfRange));
        assert_eq!(Cvv::try_from(999_i64).map(Cvv::get), Ok(999));
        assert_eq!(
            CustomerId::try_from(5_000_000_000_i64),
            Err(NumericError::OutOfRange)
        );
        assert_eq!(
            NumericCode::<6>::try_from(-5_i64),
            Err(NumericError::Negative)
        );
    }

    #[test]
    fn sql_round_trip_keeps_the_value() -> Result<(), NumericError> {
        for value in [0, 1, 99_999_999_999] {
            assert_eq!(AccountId::try_from(value)?.to_i64(), value);
        }
        for value in [0, 1, 999_999_999] {
            assert_eq!(CustomerId::try_from(value)?.to_i64(), value);
            assert_eq!(MerchantId::try_from(value)?.to_i64(), value);
            assert_eq!(CustSsn::try_from(value)?.to_i64(), value);
        }
        for value in [0, 1, 999] {
            assert_eq!(Cvv::try_from(value)?.to_i64(), value);
            assert_eq!(FicoScore::try_from(value)?.to_i64(), value);
        }
        for value in [0, 1, 9_999] {
            assert_eq!(TranCategoryCode::try_from(value)?.to_i64(), value);
        }
        for value in [0, 1, 999_999] {
            assert_eq!(NumericCode::<6>::try_from(value)?.to_i64(), value);
        }
        Ok(())
    }

    #[test]
    fn digit_fields_display_zero_padded_to_their_width() -> Result<(), NumericError> {
        let cases = [
            (AccountId::new(1)?.to_string(), "00000000001"),
            (AccountId::new(99_999_999_999)?.to_string(), "99999999999"),
            (CustomerId::new(1)?.to_string(), "000000001"),
            (TranCategoryCode::new(1)?.to_string(), "0001"),
            (Cvv::new(7)?.to_string(), "007"),
            (MerchantId::new(800_000_000)?.to_string(), "800000000"),
            (CustSsn::new(20_973_888)?.to_string(), "020973888"),
            (FicoScore::new(0)?.to_string(), "000"),
            (NumericCode::<6>::new(3)?.to_string(), "000003"),
            (NumericCode::<2>::new(5)?.to_string(), "05"),
        ];
        for (shown, expected) in cases {
            assert_eq!(shown, expected);
        }
        Ok(())
    }

    #[test]
    fn displayed_digits_parse_back_to_the_same_value() -> Result<(), NumericError> {
        let account = AccountId::new(42)?;
        assert_eq!(account.to_string().parse::<AccountId>(), Ok(account));
        let category = TranCategoryCode::new(5)?;
        assert_eq!(
            category.to_string().parse::<TranCategoryCode>(),
            Ok(category)
        );
        let code = NumericCode::<6>::new(123)?;
        assert_eq!(code.to_string().parse::<NumericCode<6>>(), Ok(code));
        Ok(())
    }

    #[test]
    fn digit_fields_order_by_value() -> Result<(), NumericError> {
        assert!(AccountId::new(1)? < AccountId::new(2)?);
        assert_eq!(
            AccountId::new(10)?.cmp(&AccountId::new(9)?),
            Ordering::Greater
        );
        Ok(())
    }

    #[test]
    fn sensitive_fields_withhold_their_value_from_debug_only() -> Result<(), NumericError> {
        // (Debug, Display, expected Debug, expected Display, text Debug must not hold)
        let cases: [(String, String, &str, &str, &str); 7] = [
            debug_case(&Cvv::new(123)?, "Cvv(<redacted>)", "123", "123"),
            debug_case(&Cvv::new(7)?, "Cvv(<redacted>)", "007", "7"),
            debug_case(
                &CustSsn::new(123_456_789)?,
                "CustSsn(<redacted>)",
                "123456789",
                "123456789",
            ),
            debug_case(
                &CardNumber::move_from("4111111111111111"),
                r#"CardNumber("************1111")"#,
                "4111111111111111",
                "411111111111",
            ),
            debug_case(
                &CardNumber::move_from("12345"),
                r#"CardNumber("*2345")"#,
                "12345           ",
                "1",
            ),
            debug_case(
                &CardNumber::move_from("123"),
                r#"CardNumber("***")"#,
                "123             ",
                "123",
            ),
            debug_case(
                &CardNumber::move_from(""),
                r#"CardNumber("")"#,
                "                ",
                " ",
            ),
        ];
        for (debug, display, expected_debug, expected_display, hidden) in cases {
            assert_eq!(debug, expected_debug, "{expected_display}");
            assert!(!debug.contains(hidden), "{expected_debug} shows {hidden}");
            assert_eq!(display, expected_display, "{expected_debug}");
        }
        Ok(())
    }

    /// The `Debug` and `Display` text of `value`, with the expected texts.
    fn debug_case<'a, T: fmt::Debug + fmt::Display>(
        value: &T,
        expected_debug: &'a str,
        expected_display: &'a str,
        hidden: &'a str,
    ) -> (String, String, &'a str, &'a str, &'a str) {
        (
            format!("{value:?}"),
            value.to_string(),
            expected_debug,
            expected_display,
            hidden,
        )
    }

    #[test]
    fn other_fields_keep_the_derived_debug() -> Result<(), NumericError> {
        let cases = [
            (format!("{:?}", AccountId::new(42)?), "AccountId(42)"),
            (format!("{:?}", FicoScore::new(750)?), "FicoScore(750)"),
            (
                format!("{:?}", TranId::move_from("0000000000000001")),
                r#"TranId(Alnum<16>("0000000000000001"))"#,
            ),
        ];
        for (debug, expected) in cases {
            assert_eq!(debug, expected);
        }
        Ok(())
    }

    #[test]
    fn text_keys_move_with_padding_and_truncation() {
        let admin = UserId::move_from("ADMIN001");
        assert_eq!(admin.to_string(), "ADMIN001");
        assert_eq!(admin.trimmed(), "ADMIN001");

        let short = UserId::move_from("ABC");
        assert_eq!(short.as_bytes(), b"ABC     ");
        assert_eq!(short.to_string(), "ABC     ");
        assert_eq!(short.trimmed(), "ABC");
        assert!(matches!(short.trimmed(), Cow::Borrowed("ABC")));

        assert_eq!(UserId::move_from("ADMIN0012"), admin);
        // No case conversion: sign-on upper-cases before it builds the key.
        assert_eq!(UserId::move_from("user0001").as_bytes(), b"user0001");
        assert_ne!(UserId::move_from("user0001"), UserId::move_from("USER0001"));

        assert_eq!(TranTypeCode::move_from("1").as_bytes(), b"1 ");
        assert_eq!(TranTypeCode::move_from("01").to_string(), "01");
        // E-06: a blank group ID is a key of ten spaces, trimmed to nothing.
        let blank = AccountGroupId::move_from("");
        assert_eq!(blank.as_bytes(), b"          ");
        assert_eq!(blank.trimmed(), "");
        assert!(blank.as_alnum().is_blank());
    }

    #[test]
    fn text_keys_convert_to_and_from_their_items() -> Result<(), NumericError> {
        let item = Alnum::<16>::from_bytes(b"0683586198171516")?;
        let card = CardNumber::from_alnum(item);
        assert_eq!(card.as_alnum(), &item);
        assert_eq!(card.as_bytes(), item.as_bytes());
        assert_eq!(card, CardNumber::move_from("0683586198171516"));

        let group = AccountGroupId::from_alnum(Alnum::move_from("DEFAULT"));
        assert_eq!(group.as_alnum().cobol_cmp(b"DEFAULT"), Ordering::Equal);
        assert_eq!(group.to_string(), "DEFAULT   ");
        Ok(())
    }

    #[test]
    fn text_key_order_is_the_byte_order_of_the_padded_key() {
        let ascending = [
            "0500024453765740",
            "0683586198171516",
            "1",
            "123",
            "1234",
            "A",
        ]
        .map(CardNumber::move_from);
        for pair in ascending.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?}");
        }
    }

    #[test]
    fn next_tran_id_adds_one_as_sixteen_digits() -> Result<(), NumericError> {
        assert_eq!(TranId::zeros().as_bytes(), b"0000000000000000");
        assert_eq!(
            TranId::zeros().next()?,
            TranId::move_from("0000000000000001")
        );
        let cases = [
            ("0000000000000001", "0000000000000002"),
            ("0000000000000099", "0000000000000100"),
            ("0000000996722787", "0000000996722788"),
            ("2022071800000050", "2022071800000051"),
            ("9999999999999998", "9999999999999999"),
            // 9(16) without ON SIZE ERROR: the high-order digit is lost.
            ("9999999999999999", "0000000000000000"),
        ];
        for (current, expected) in cases {
            assert_eq!(
                TranId::move_from(current).next()?,
                TranId::move_from(expected),
                "{current}"
            );
        }
        Ok(())
    }

    #[test]
    fn next_tran_id_rejects_a_key_that_is_not_sixteen_digits() {
        let keys = [
            TranId::move_from("000000000000001 "),
            TranId::move_from("1"),
            TranId::move_from(""),
            TranId::move_from("ABCDEFGHIJKLMNOP"),
            TranId::move_from("-000000000000001"),
            TranId::from_alnum(Alnum::from_array([0xFF; 16])),
            TranId::from_alnum(Alnum::from_array([0x00; 16])),
        ];
        for key in keys {
            assert_eq!(key.next(), Err(NumericError::NotNumeric), "{key:?}");
        }
    }

    #[test]
    fn auth_count_holds_four_digits_of_either_sign() {
        let cases: [(i16, Result<i16, NumericError>); 7] = [
            (0, Ok(0)),
            (9_999, Ok(9_999)),
            (-9_999, Ok(-9_999)),
            (10_000, Err(NumericError::OutOfRange)),
            (-10_000, Err(NumericError::OutOfRange)),
            (i16::MAX, Err(NumericError::OutOfRange)),
            (i16::MIN, Err(NumericError::OutOfRange)),
        ];
        for (value, expected) in cases {
            assert_eq!(
                AuthCount::new(value).map(AuthCount::get),
                expected,
                "{value}"
            );
        }
    }

    #[test]
    fn auth_count_steps_by_one_and_reports_the_size_error() -> Result<(), NumericError> {
        type Step = Result<i16, Overflow>;
        let cases: [(i16, Step, Step); 4] = [
            (0, Ok(1), Ok(-1)),
            (9_998, Ok(9_999), Ok(9_997)),
            (9_999, Err(Overflow), Ok(9_998)),
            (-9_999, Ok(-9_998), Err(Overflow)),
        ];
        for (start, incremented, decremented) in cases {
            let count = AuthCount::new(start)?;
            assert_eq!(
                count.checked_increment().map(AuthCount::get),
                incremented,
                "{start} + 1"
            );
            assert_eq!(
                count.checked_decrement().map(AuthCount::get),
                decremented,
                "{start} - 1"
            );
        }
        Ok(())
    }

    #[test]
    fn auth_count_compares_with_zero_for_the_purge_test() -> Result<(), NumericError> {
        assert_eq!(AuthCount::ZERO.get(), 0);
        assert_eq!(AuthCount::new(0)?, AuthCount::ZERO);
        assert!(AuthCount::new(0)? <= AuthCount::ZERO);
        assert!(AuthCount::new(-1)? <= AuthCount::ZERO);
        assert!(AuthCount::new(1)? > AuthCount::ZERO);
        Ok(())
    }

    #[test]
    fn row_identities_wrap_their_bigint() {
        assert_eq!(RuleId::from(7).get(), 7);
        assert_eq!(RuleId::from(7).to_string(), "7");
        assert!(RuleId::from(1) < RuleId::from(2));
        assert_eq!(TransactionRowId::from(42).get(), 42);
        assert_eq!(TransactionRowId::from(42).to_string(), "42");
        assert_eq!(AccrualId::from(i64::MAX).get(), i64::MAX);
        assert_eq!(AuthId::from(i64::MAX).to_string(), "9223372036854775807");
    }
}
