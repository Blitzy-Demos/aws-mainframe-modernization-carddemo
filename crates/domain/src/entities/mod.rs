//! Typed records: one per copybook record or Db2 table a carried program reads
//! or writes, plus the reward entities the request adds (AAP 0.3.5).
//!
//! The records are data with only the invariants that are their own. The codec
//! layouts encode and decode them, the persistence repositories store them, and
//! the ported rules read and change them.
//!
//! # Naming
//!
//! A field takes its copybook name lower-snake-cased, prefix and spelling kept
//! (`acct_curr_bal`, `acct_expiraion_date`, `pa_merchant_catagory_code`), and
//! its SQL column carries the same name except where a key column takes the
//! request's name: `accounts.account_id` stores [`Account::acct_id`] (AAP
//! 0.5.3.4). A Db2 table's record takes its DDL column names instead
//! ([`TranTypeMaster::tr_description`], [`AuthFraud::merchant_catagory_code`]),
//! and a reward entity the columns of `migrations/0005_rewards.sql`. A row
//! identity that is no copybook field, such as
//! [`Transaction::transaction_id`] or [`PendingAuthDetail::auth_id`], sits
//! beside the COBOL key, never in its place. `FILLER` is not modelled: no
//! column stores it, and the codec layout alone keeps its bytes, so a record
//! still re-encodes to the bytes it was decoded from.
//!
//! # Typing
//!
//! Every copybook and Db2 field takes the type its reviewed row of section A
//! (record layouts) of `docs/parity/parity-oracle-audit.md` assigns, and cites
//! that row. The types come from [`crate::numeric`], fixed by the sign, width
//! and scale of the PIC clause: `S9(n)V99` money is
//! [`Money<n>`](crate::numeric::Money), a `9(n)` or `X(n)` key or code its
//! identifier newtype, and other `X(n)` text
//! [`Alnum<n>`](crate::numeric::Alnum). Values with no PIC clause take the type
//! of their column: Db2 `TIMESTAMP` and `DATE` columns and the reward instants
//! are `chrono` values, and the argon2id hash that replaces `SEC-USR-PWD` is a
//! `String` (E-20). The reward entities have no COBOL oracle and no audit rows;
//! their types follow the newtype algebra of AAP 0.5.3.1.
//!
//! # Two versions of the reference data
//!
//! The COBOL keeps transaction types and categories twice, and so do these
//! records (AAP 0.3.2):
//!
//! - [`TranTypeRecord`] and [`TranCategoryRecord`] are the VSAM processing
//!   copies `TRANTYPE` (`CVTRA03Y`) and `TRANCATG` (`CVTRA04Y`), the tables
//!   `trantype_file` and `trancatg_file`. Only `CBTRN03C` reads them, for the
//!   CR00 report and `batch tranrept`.
//! - [`TranTypeMaster`] and [`TranCategoryMaster`] are the Db2 masters
//!   `CARDDEMO.TRANSACTION_TYPE` and `CARDDEMO.TRANSACTION_TYPE_CATEGORY`, the
//!   tables `transaction_types` and `transaction_categories`. CTTU, CTLI and
//!   `batch mnttrdb2` change the type master.
//!
//! The two start different and agree only after `TRANEXTR` (`batch seed --only
//! tranextr`) copies the masters over the processing copies, so neither record
//! converts into the other.

// The declarations follow the copybook order, not the alphabet. Each comment
// names the record's source and its table, and also keeps rustfmt from
// sorting the declarations.

// `CVACT01Y` `ACCOUNT-RECORD`: VSAM `ACCTDAT`, table `accounts`.
pub mod account;
// `CVACT02Y` `CARD-RECORD`: VSAM `CARDDAT`, table `cards`.
pub mod card;
// `CVACT03Y` `CARD-XREF-RECORD`: VSAM `CCXREF`, table `card_xref`.
pub mod card_xref;
// `CVCUS01Y` `CUSTOMER-RECORD`: VSAM `CUSTDAT`, table `customers`.
pub mod customer;
// `CVTRA01Y` `TRAN-CAT-BAL-RECORD`: VSAM `TCATBALF`, table
// `transaction_category_balances`.
pub mod tran_cat_balance;
// `CVTRA02Y` `DIS-GROUP-RECORD`: VSAM `DISCGRP`, table `disclosure_groups`.
pub mod disclosure_group;
// `CVTRA03Y` `TRAN-TYPE-RECORD` (`trantype_file`) and Db2 `TRANSACTION_TYPE`
// (`transaction_types`).
pub mod tran_type;
// `CVTRA04Y` `TRAN-CAT-RECORD` (`trancatg_file`) and Db2
// `TRANSACTION_TYPE_CATEGORY` (`transaction_categories`).
pub mod tran_category;
// `CVTRA05Y` `TRAN-RECORD`: VSAM `TRANSACT`, table `transactions`.
pub mod transaction;
// `CVTRA06Y` `DALYTRAN-RECORD`: the `POSTTRAN` input file; no table.
pub mod daily_tran;
// `CSUSR01Y` `SEC-USER-DATA`: VSAM `USRSEC`, table `users`.
pub mod user_security;
// `CIPAUSMY`: IMS root segment `PAUTSUM0`, table `pending_auth_summaries`.
pub mod pending_auth_summary;
// `CIPAUDTY`: IMS child segment `PAUTDTL1`, table `pending_auth_details`.
pub mod pending_auth_detail;
// Db2 `AUTHFRDS`: table `auth_fraud`.
pub mod auth_fraud;
// Rewards, new: tables `reward_rules`, `reward_balances`, `reward_accruals`.
pub mod reward;

pub use account::Account;
pub use auth_fraud::AuthFraud;
pub use card::Card;
pub use card_xref::CardXref;
pub use customer::Customer;
pub use daily_tran::DailyTran;
pub use disclosure_group::{DEFAULT_GROUP_ID, DisclosureGroup, DisclosureGroupKey};
pub use pending_auth_detail::{FraudFlag, MatchStatus, NewPendingAuthDetail, PendingAuthDetail};
pub use pending_auth_summary::{PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary};
pub use reward::{NewAccrual, NewRewardRule, RewardAccrual, RewardBalance, RewardRule};
pub use tran_cat_balance::{TranCatBalance, TranCatBalanceKey};
pub use tran_category::{TRC_CAT_DATA_MAX_LEN, TranCategoryMaster, TranCategoryRecord};
pub use tran_type::{TR_DESCRIPTION_MAX_LEN, TranTypeMaster, TranTypeRecord};
pub use transaction::{NewTransaction, Transaction};
pub use user_security::UserSecurity;
