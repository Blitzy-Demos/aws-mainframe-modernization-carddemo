//! The record layouts in scope: the eleven copybook records `CVACT01Y`,
//! `CVACT02Y`, `CVACT03Y`, `CVCUS01Y`, `CVTRA06Y`, `CVTRA05Y`, `CVTRA01Y`,
//! `CVTRA02Y`, `CVTRA03Y`, `CVTRA04Y` and `CSUSR01Y`, and the `CBTRN02C`
//! `REJECT-RECORD` of `DALYREJS`, twelve in all.
//!
//! Each module holds one [`Layout`]: the field specifications in
//! copybook order with their section A audit rows, the typed record, and the
//! decoder and encoder between the two. `FILLER` is kept as its raw bytes in
//! every record that has one, so a decoded record re-encodes to the bytes it
//! came from. No field of any of the twelve is packed (`COMP-3`) or binary
//! (`COMP`), and none of them uses `REDEFINES` or `OCCURS`.
//!
//! There is deliberately no thirteenth layout. The `mnttrdb2` input, the
//! `COBTUPDT` `WS-INPUT-REC` of `X(1)`, `X(2)` and `X(50)` (P-COBTUPDT-001 to
//! P-COBTUPDT-003) [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L71-76], is
//! read in `crates/batch` with [`crate::framing::split_ascii`] and the
//! [`crate::field`] helpers.

// The declarations follow the copybook order the crate documents, not the
// alphabet. Each comment names the record, and also keeps rustfmt from sorting
// the declarations.

// `CVACT01Y` `ACCOUNT-RECORD`, `ACCTDAT`.
pub mod account;
// `CVACT02Y` `CARD-RECORD`, `CARDDAT`.
pub mod card;
// `CVACT03Y` `CARD-XREF-RECORD`, `CCXREF`.
pub mod card_xref;
// `CVCUS01Y` `CUSTOMER-RECORD`, `CUSTDAT`.
pub mod customer;
// `CVTRA06Y` `DALYTRAN-RECORD`, the `POSTTRAN` input.
pub mod daily_tran;
// `CVTRA05Y` `TRAN-RECORD`, `TRANSACT`.
pub mod transaction;
// `CVTRA01Y` `TRAN-CAT-BAL-RECORD`, `TCATBALF`.
pub mod tran_cat_balance;
// `CVTRA02Y` `DIS-GROUP-RECORD`, `DISCGRP`.
pub mod disclosure_group;
// `CVTRA03Y` `TRAN-TYPE-RECORD`, the `TRANTYPE` processing copy.
pub mod tran_type;
// `CVTRA04Y` `TRAN-CAT-RECORD`, the `TRANCATG` processing copy.
pub mod tran_category;
// `CSUSR01Y` `SEC-USER-DATA`, `USRSEC`.
pub mod user_security;
// `CBTRN02C` `REJECT-RECORD`, `DALYREJS`.
pub mod reject_record;

pub use account::{AccountLayout, AccountRecord};
pub use card::{CardLayout, CardRecord};
pub use card_xref::{CardXrefLayout, CardXrefRecord};
pub use customer::{CustomerLayout, CustomerRecord};
pub use daily_tran::{DailyTranLayout, DailyTranRecord};
pub use disclosure_group::{DisclosureGroupLayout, DisclosureGroupRecord};
pub use reject_record::{RejectRecord, RejectRecordLayout};
pub use tran_cat_balance::{TranCatBalanceLayout, TranCatBalanceRecord};
pub use tran_category::{TranCategoryFileRecord, TranCategoryLayout};
pub use tran_type::{TranTypeFileRecord, TranTypeLayout};
pub use transaction::{TransactionLayout, TransactionRecord};
pub use user_security::{UserSecurityLayout, UserSecurityRecord};

/// The layout trait, character set, error and its kind, and file framing the
/// records of this module are encoded with, re-exported from the crate root
/// and [`crate::framing`], so the persistence seed and the batch jobs read and
/// write a record file through `layouts` alone (AAP 0.7.2).
pub use crate::framing::{Framed, decode_ascii_file, decode_ascii_lf, encode_ascii_lf, pad};
pub use crate::{Charset, CodecError, CodecErrorKind, Layout};
