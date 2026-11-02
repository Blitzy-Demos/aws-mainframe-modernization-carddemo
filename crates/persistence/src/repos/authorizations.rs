//! The pending authorizations and their fraud reports: the IMS HIDAM database
//! `DBPAUTP0`, root segment `PAUTSUM0` (`CIPAUSMY`, table
//! `pending_auth_summaries`) and child segment `PAUTDTL1` (`CIPAUDTY`, table
//! `pending_auth_details`), and the Db2 table `CARDDEMO.AUTHFRDS` (table
//! `auth_fraud`), as `migrations/0004_authorizations.sql` defines them.
//!
//! [`AuthorizationStore`] on [`PgStore`], used by CP00 (`COPAUA0C`), CPVS
//! (`COPAUS0C`), CPVD (`COPAUS1C` with `COPAUS2C`) and `batch cbpaup0j`
//! (`CBPAUP0C`), and the inherent [`PgStore::read_fraud`], which reads back
//! the fraud report the port only writes.
//!
//! # Key order
//!
//! - Summaries: the root sequence field `ACCNTID` is the unique packed account
//!   number [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L28-30]
//!   (P-CIPAUSMY-001). The column admits only non-negative values, whose
//!   packed bytes order as the numbers do, so ascending `pa_acct_id` is the
//!   `GN` order of `CBPAUP0C` (P-CBPAUP0C-002).
//! - Details: the twins of one summary are ordered by their unique sequence
//!   field `PAUT9CTS`, eight bytes compared as characters (`TYPE=C`): the
//!   packed `PA-AUTH-DATE-9C S9(05)` followed by the packed
//!   `PA-AUTH-TIME-9C S9(09)`
//!   [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L36-37]
//!   (P-CIPAUDTY-001, P-CIPAUDTY-002). `COPAUA0C` stores both parts
//!   complemented, `99999 - YYDDD` and `999999999 - HHMMSSmmm`
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875]
//!   (P-COPAUA0C-014): non-negative values of one sign nibble, whose packed
//!   bytes order as the numbers do. Ascending (`pa_auth_date_9c`,
//!   `pa_auth_time_9c`) under one `pa_acct_id` is therefore IMS twin order,
//!   which lists the newest authorization first (P-COPAUS0C-010). No browse
//!   orders by `auth_id` or by the original date.
//!
//! # Writes
//!
//! Every `ISRT`, `REPL` and `DLET`, and the `AUTHFRDS` insert and update, is
//! one legacy write under its own savepoint, through `crate::tx::savepoint`
//! and `crate::tx::settle`. CP00 writes the summary, then the detail
//! (P-COPAUA0C-035): a failed detail insert (location I004) leaves the summary
//! write in place, as `COPAUA0C` ends that path with an `EXEC CICS RETURN`
//! that commits it. The CPVD fraud marking is backed out by its caller, which
//! rolls the whole unit of work back when the detail replace fails, as
//! `COPAUS1C` `ROLL-BACK` backs out the `AUTHFRDS` write (P-COPAUS1C-015).
//!
//! The replace methods replace the row they are given and track no IMS hold.
//! Whether `COPAUS1C`'s `REPL` after a non-hold `GNP`, or `COPAUA0C`'s after a
//! plain `GU`, succeeds is decided by the audit and reproduced by the rules
//! `auth_detail` and `auth_decision`, not by this adapter.
//!
//! Every read runs under its own savepoint too, so a failed `GU`, `GN` or
//! `GNP` leaves the deletes `CBPAUP0C` made before it committable
//! (P-CBPAUP0C-005, P-CBPAUP0C-009), and a failed CPVD summary read
//! [`AuthorizationStore::read_summary_of_detail`], which `READ-AUTH-RECORD`
//! follows with the detail read (P-COPAUS1C-014), leaves the transaction
//! usable for that read. No statement writes the current date or time: the
//! `AUTHFRDS` report date is the value the rule takes from the processing
//! clock.
//!
//! # Locks
//!
//! One read locks: the CPVD fraud mark's detail read,
//! [`AuthorizationStore::read_detail_for_update`], a `SELECT … FOR UPDATE`
//! under its own savepoint whose row lock lasts until the caller's
//! transaction commits or rolls back. Two overlapping fraud marks of one
//! authorization therefore serialize: the second reads the flag the first
//! committed, and no toggle is lost. `COPAUS1C` takes no lock of its own;
//! this one is the concurrency safeguard of the replacement. Every other read,
//! the CPVD view and its PF8 step included, takes no lock.

use carddemo_domain::entities::{
    AuthFraud, FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS,
    PendingAuthDetail, PendingAuthSummary,
};
use carddemo_domain::numeric::{AccountId, Alnum, AuthId, CardNumber, CustomerId, Money};
use carddemo_domain::ports::{AuthorizationStore, Browse, Direction, PortError, PortResult};
use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use sqlx::{Postgres, Transaction};

use super::{
    account_id, alnum, auth_count, card_number, customer_id, expect_affected, limit_i64, money,
    numeric_code, opt_alnum, opt_money,
};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table of the `PAUTSUM0` root segments, as a misfit names it.
const SUMMARIES: &str = "pending_auth_summaries";

/// The table of the `PAUTDTL1` child segments, as a misfit names it.
const DETAILS: &str = "pending_auth_details";

/// The table of the Db2 `AUTHFRDS` fraud reports, as a misfit names it.
const FRAUD: &str = "auth_fraud";

/// The column of the `PA-ACCOUNT-STATUS X(02) OCCURS 5 TIMES` table
/// (P-CIPAUSMY-004), as a misfit names it.
const ACCOUNT_STATUS: &str = "pa_account_status";

/// A row of `pending_auth_summaries`, in `CIPAUSMY` order.
///
/// Each field is its column without the `pa_` prefix every column shares,
/// aliased so in the `SELECT`: clippy's `struct_field_names` rejects a struct
/// whose fields all carry one prefix.
struct SummaryRow {
    acct_id: i64,
    cust_id: i32,
    auth_status: String,
    account_status: Vec<String>,
    credit_limit: Decimal,
    cash_limit: Decimal,
    credit_balance: Decimal,
    cash_balance: Decimal,
    approved_auth_cnt: i16,
    declined_auth_cnt: i16,
    approved_auth_amt: Decimal,
    declined_auth_amt: Decimal,
}

/// A row of `pending_auth_details`: the identity, the parent key, then the
/// `CIPAUDTY` fields in copybook order.
struct DetailRow {
    auth_id: i64,
    pa_acct_id: i64,
    pa_auth_date_9c: i32,
    pa_auth_time_9c: i32,
    pa_auth_orig_date: String,
    pa_auth_orig_time: String,
    pa_card_num: String,
    pa_auth_type: String,
    pa_card_expiry_date: String,
    pa_message_type: String,
    pa_message_source: String,
    pa_auth_id_code: String,
    pa_auth_resp_code: String,
    pa_auth_resp_reason: String,
    pa_processing_code: i32,
    pa_transaction_amt: Decimal,
    pa_approved_amt: Decimal,
    pa_merchant_catagory_code: String,
    pa_acqr_country_code: String,
    pa_pos_entry_mode: i32,
    pa_merchant_id: String,
    pa_merchant_name: String,
    pa_merchant_city: String,
    pa_merchant_state: String,
    pa_merchant_zip: String,
    pa_transaction_id: String,
    pa_match_status: String,
    pa_auth_fraud: String,
    pa_fraud_rpt_date: String,
}

/// A row of `auth_fraud`, in `AUTHFRDS.ddl` order, with the DDL
/// nullability: every column but the key (`card_num`, `auth_ts`) may be
/// `NULL`.
struct FraudRow {
    card_num: String,
    auth_ts: NaiveDateTime,
    auth_type: Option<String>,
    card_expiry_date: Option<String>,
    message_type: Option<String>,
    message_source: Option<String>,
    auth_id_code: Option<String>,
    auth_resp_code: Option<String>,
    auth_resp_reason: Option<String>,
    processing_code: Option<String>,
    transaction_amt: Option<Decimal>,
    approved_amt: Option<Decimal>,
    merchant_catagory_code: Option<String>,
    acqr_country_code: Option<String>,
    pos_entry_mode: Option<i16>,
    merchant_id: Option<String>,
    merchant_name: Option<String>,
    merchant_city: Option<String>,
    merchant_state: Option<String>,
    merchant_zip: Option<String>,
    transaction_id: Option<String>,
    match_status: Option<String>,
    auth_fraud: Option<String>,
    fraud_rpt_date: Option<NaiveDate>,
    acct_id: Option<i64>,
    cust_id: Option<i64>,
}

/// The column values a summary binds, in `CIPAUSMY` order: text
/// right-trimmed, money as `NUMERIC`, the counts as `SMALLINT`. Each field is
/// its column without the shared `pa_` prefix, as in [`SummaryRow`].
struct SummaryColumns {
    acct_id: i64,
    cust_id: i32,
    auth_status: String,
    account_status: Vec<String>,
    credit_limit: Decimal,
    cash_limit: Decimal,
    credit_balance: Decimal,
    cash_balance: Decimal,
    approved_auth_cnt: i16,
    declined_auth_cnt: i16,
    approved_auth_amt: Decimal,
    declined_auth_amt: Decimal,
}

impl SummaryColumns {
    /// The values of `summary`, `PA-ACCOUNT-STATUS` occurrence `i` at index
    /// `i - 1` (P-CIPAUSMY-004).
    fn new(summary: &PendingAuthSummary) -> PortResult<Self> {
        Ok(Self {
            acct_id: summary.pa_acct_id.to_i64(),
            cust_id: narrow(summary.pa_cust_id.get(), "pa_cust_id")?,
            auth_status: text(&summary.pa_auth_status),
            account_status: summary.pa_account_status.iter().map(text).collect(),
            credit_limit: summary.pa_credit_limit.to_decimal(),
            cash_limit: summary.pa_cash_limit.to_decimal(),
            credit_balance: summary.pa_credit_balance.to_decimal(),
            cash_balance: summary.pa_cash_balance.to_decimal(),
            approved_auth_cnt: summary.pa_approved_auth_cnt.get(),
            declined_auth_cnt: summary.pa_declined_auth_cnt.get(),
            approved_auth_amt: summary.pa_approved_auth_amt.to_decimal(),
            declined_auth_amt: summary.pa_declined_auth_amt.to_decimal(),
        })
    }
}

/// The column values a detail binds, in `CIPAUDTY` order after the parent
/// key: text right-trimmed, codes and key parts as `INTEGER`, money as
/// `NUMERIC`. Each field is its column without the shared `pa_` prefix, for
/// the reason [`SummaryRow`] gives.
struct DetailColumns {
    acct_id: i64,
    auth_date_9c: i32,
    auth_time_9c: i32,
    auth_orig_date: String,
    auth_orig_time: String,
    card_num: String,
    auth_type: String,
    card_expiry_date: String,
    message_type: String,
    message_source: String,
    auth_id_code: String,
    auth_resp_code: String,
    auth_resp_reason: String,
    processing_code: i32,
    transaction_amt: Decimal,
    approved_amt: Decimal,
    merchant_catagory_code: String,
    acqr_country_code: String,
    pos_entry_mode: i32,
    merchant_id: String,
    merchant_name: String,
    merchant_city: String,
    merchant_state: String,
    merchant_zip: String,
    transaction_id: String,
    match_status: String,
    auth_fraud: String,
    fraud_rpt_date: String,
}

impl DetailColumns {
    /// The values of `detail`; the match status and the fraud flag bind the
    /// byte they hold, as [`MatchStatus::to_alnum`] and
    /// [`FraudFlag::to_alnum`] give it.
    fn new(detail: &NewPendingAuthDetail) -> PortResult<Self> {
        Ok(Self {
            acct_id: detail.pa_acct_id.to_i64(),
            auth_date_9c: detail.pa_auth_date_9c,
            auth_time_9c: detail.pa_auth_time_9c,
            auth_orig_date: text(&detail.pa_auth_orig_date),
            auth_orig_time: text(&detail.pa_auth_orig_time),
            card_num: detail.pa_card_num.trimmed().into_owned(),
            auth_type: text(&detail.pa_auth_type),
            card_expiry_date: text(&detail.pa_card_expiry_date),
            message_type: text(&detail.pa_message_type),
            message_source: text(&detail.pa_message_source),
            auth_id_code: text(&detail.pa_auth_id_code),
            auth_resp_code: text(&detail.pa_auth_resp_code),
            auth_resp_reason: text(&detail.pa_auth_resp_reason),
            processing_code: narrow(detail.pa_processing_code.get(), "pa_processing_code")?,
            transaction_amt: detail.pa_transaction_amt.to_decimal(),
            approved_amt: detail.pa_approved_amt.to_decimal(),
            merchant_catagory_code: text(&detail.pa_merchant_catagory_code),
            acqr_country_code: text(&detail.pa_acqr_country_code),
            pos_entry_mode: narrow(detail.pa_pos_entry_mode.get(), "pa_pos_entry_mode")?,
            merchant_id: text(&detail.pa_merchant_id),
            merchant_name: text(&detail.pa_merchant_name),
            merchant_city: text(&detail.pa_merchant_city),
            merchant_state: text(&detail.pa_merchant_state),
            merchant_zip: text(&detail.pa_merchant_zip),
            transaction_id: text(&detail.pa_transaction_id),
            match_status: text(&detail.pa_match_status.to_alnum()),
            auth_fraud: text(&detail.pa_auth_fraud.to_alnum()),
            fraud_rpt_date: text(&detail.pa_fraud_rpt_date),
        })
    }
}

impl AuthorizationStore for PgStore<'_, '_> {
    /// COPAUA0C 5500-READ-AUTH-SUMMRY — P-COPAUA0C-008
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L616-640]; COPAUS0C
    /// GET-AUTH-SUMMARY — P-COPAUS0C-009
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L966-997]:
    /// `GU PAUTSUM0 WHERE (ACCNTID = PA-ACCT-ID)`. No row is the `GE` status.
    async fn read_summary(&mut self, id: AccountId) -> PortResult<PendingAuthSummary> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            SummaryRow,
            r#"SELECT pa_acct_id AS acct_id, pa_cust_id AS cust_id,
                      pa_auth_status AS auth_status, pa_account_status AS account_status,
                      pa_credit_limit AS credit_limit, pa_cash_limit AS cash_limit,
                      pa_credit_balance AS credit_balance, pa_cash_balance AS cash_balance,
                      pa_approved_auth_cnt AS approved_auth_cnt,
                      pa_declined_auth_cnt AS declined_auth_cnt,
                      pa_approved_auth_amt AS approved_auth_amt,
                      pa_declined_auth_amt AS declined_auth_amt
                 FROM pending_auth_summaries
                WHERE pa_acct_id = $1"#,
            id.to_i64()
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(summary_from_row(row)?)
    }

    /// COPAUS1C READ-AUTH-RECORD — P-COPAUS1C-003, P-COPAUS1C-004
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L436-462]:
    /// `GU PAUTSUM0 WHERE (ACCNTID = PA-ACCT-ID)` for the account of the
    /// detail `id`, which stands for the COMMAREA `CDEMO-ACCT-ID` the request
    /// does not carry (P-COPAUS1C-012).
    ///
    /// One statement joins the detail to its parent summary on the primary
    /// keys of both tables. No detail with the identity, or no summary over
    /// it, is the `GE` status: [`PortError::NotFound`].
    ///
    /// The statement runs under its own savepoint, so a failed summary read
    /// is rolled back to it and leaves the transaction usable: an `OTHER`
    /// status of the `GU` leaves `AUTHS-NOT-EOF` as it stood, and
    /// `READ-AUTH-RECORD` still issues the detail `GNP` after it
    /// (P-COPAUS1C-014), whose outcome decides between the summary and the
    /// detail error text (V-COPAUS1C-003, V-COPAUS1C-006).
    async fn read_summary_of_detail(&mut self, id: AuthId) -> PortResult<PendingAuthSummary> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            SummaryRow,
            r#"SELECT s.pa_acct_id AS acct_id, s.pa_cust_id AS cust_id,
                      s.pa_auth_status AS auth_status, s.pa_account_status AS account_status,
                      s.pa_credit_limit AS credit_limit, s.pa_cash_limit AS cash_limit,
                      s.pa_credit_balance AS credit_balance, s.pa_cash_balance AS cash_balance,
                      s.pa_approved_auth_cnt AS approved_auth_cnt,
                      s.pa_declined_auth_cnt AS declined_auth_cnt,
                      s.pa_approved_auth_amt AS approved_auth_amt,
                      s.pa_declined_auth_amt AS declined_auth_amt
                 FROM pending_auth_details d
                 JOIN pending_auth_summaries s ON s.pa_acct_id = d.pa_acct_id
                WHERE d.auth_id = $1"#,
            id.get()
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(summary_from_row(row)?)
    }

    /// COPAUA0C 8400-UPDATE-SUMMARY — P-COPAUA0C-013
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L829-833]:
    /// `ISRT PAUTSUM0` of a summary the `GU` did not find. A second summary
    /// for the account violates the primary key, SQLSTATE 23505, the IMS `II`
    /// status: [`PortError::Duplicate`]. The failure is logged at I003 by the
    /// flow, not here.
    async fn insert_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
        let columns = SummaryColumns::new(&summary)?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            r#"INSERT INTO pending_auth_summaries
                   (pa_acct_id, pa_cust_id, pa_auth_status, pa_account_status,
                    pa_credit_limit, pa_cash_limit, pa_credit_balance, pa_cash_balance,
                    pa_approved_auth_cnt, pa_declined_auth_cnt,
                    pa_approved_auth_amt, pa_declined_auth_amt)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
            columns.acct_id,
            columns.cust_id,
            columns.auth_status,
            &columns.account_status,
            columns.credit_limit,
            columns.cash_limit,
            columns.credit_balance,
            columns.cash_balance,
            columns.approved_auth_cnt,
            columns.declined_auth_cnt,
            columns.approved_auth_amt,
            columns.declined_auth_amt
        )
        .execute(&mut *sp)
        .await;
        settle(sp, result).await.map(|_| ())
    }

    /// COPAUA0C 8400-UPDATE-SUMMARY — P-COPAUA0C-013
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L823-828]:
    /// `REPL PAUTSUM0` of the summary the `GU` found, every field but the key
    /// `ACCNTID` replaced. No row for `summary.pa_acct_id` is
    /// [`PortError::NotFound`].
    async fn update_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
        let columns = SummaryColumns::new(&summary)?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            r#"UPDATE pending_auth_summaries
                  SET pa_cust_id = $2,
                      pa_auth_status = $3,
                      pa_account_status = $4,
                      pa_credit_limit = $5,
                      pa_cash_limit = $6,
                      pa_credit_balance = $7,
                      pa_cash_balance = $8,
                      pa_approved_auth_cnt = $9,
                      pa_declined_auth_cnt = $10,
                      pa_approved_auth_amt = $11,
                      pa_declined_auth_amt = $12
                WHERE pa_acct_id = $1"#,
            columns.acct_id,
            columns.cust_id,
            columns.auth_status,
            &columns.account_status,
            columns.credit_limit,
            columns.cash_limit,
            columns.credit_balance,
            columns.cash_balance,
            columns.approved_auth_cnt,
            columns.declined_auth_cnt,
            columns.approved_auth_amt,
            columns.declined_auth_amt
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// CBPAUP0C 6000-DELETE-AUTH-SUMMARY — P-CBPAUP0C-007
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L328-347]:
    /// `DLET PAUTSUM0`. The foreign key of `pending_auth_details` cascades,
    /// so every detail still under the summary goes with it, as an IMS delete
    /// of a parent deletes its dependents (P-CBPAUP0C-006). No row is
    /// [`PortError::NotFound`].
    async fn delete_summary(&mut self, id: AccountId) -> PortResult<()> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "DELETE FROM pending_auth_summaries WHERE pa_acct_id = $1",
            id.to_i64()
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// CBPAUP0C 2000-FIND-NEXT-AUTH-SUMMARY — P-CBPAUP0C-002
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L216-242]: the
    /// unqualified `GN PAUTSUM0` sweep, up to `limit` summaries whose account
    /// is greater than `after`, in `ACCNTID` order, from the first summary
    /// when `after` is `None`. The end of the database (`GB`) is an empty
    /// page.
    ///
    /// The two cases are separate statements, so each page is a range scan of
    /// the primary key, under custom and generic prepared plans alike, rather
    /// than a filtered scan from the first summary.
    async fn summaries_after(
        &mut self,
        after: Option<AccountId>,
        limit: u16,
    ) -> PortResult<Vec<PendingAuthSummary>> {
        let limit = limit_i64(limit)?;
        let mut sp = savepoint(self.tx).await?;
        let result = match after {
            None => {
                sqlx::query_as!(
                    SummaryRow,
                    r#"SELECT pa_acct_id AS acct_id, pa_cust_id AS cust_id,
                              pa_auth_status AS auth_status, pa_account_status AS account_status,
                              pa_credit_limit AS credit_limit, pa_cash_limit AS cash_limit,
                              pa_credit_balance AS credit_balance, pa_cash_balance AS cash_balance,
                              pa_approved_auth_cnt AS approved_auth_cnt,
                              pa_declined_auth_cnt AS declined_auth_cnt,
                              pa_approved_auth_amt AS approved_auth_amt,
                              pa_declined_auth_amt AS declined_auth_amt
                         FROM pending_auth_summaries
                        ORDER BY pa_acct_id
                        LIMIT $1"#,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
            Some(after) => {
                sqlx::query_as!(
                    SummaryRow,
                    r#"SELECT pa_acct_id AS acct_id, pa_cust_id AS cust_id,
                              pa_auth_status AS auth_status, pa_account_status AS account_status,
                              pa_credit_limit AS credit_limit, pa_cash_limit AS cash_limit,
                              pa_credit_balance AS credit_balance, pa_cash_balance AS cash_balance,
                              pa_approved_auth_cnt AS approved_auth_cnt,
                              pa_declined_auth_cnt AS declined_auth_cnt,
                              pa_approved_auth_amt AS approved_auth_amt,
                              pa_declined_auth_amt AS declined_auth_amt
                         FROM pending_auth_summaries
                        WHERE pa_acct_id > $1
                        ORDER BY pa_acct_id
                        LIMIT $2"#,
                    after.to_i64(),
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
        };
        let rows = settle(sp, result).await?;
        rows.into_iter()
            .map(|row| summary_from_row(row).map_err(PortError::from))
            .collect()
    }

    /// COPAUA0C 8500-INSERT-AUTH — P-COPAUA0C-014
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L911-930]:
    /// `ISRT PAUTSUM0 WHERE (ACCNTID = PA-ACCT-ID) PAUTDTL1`, the detail under
    /// the summary of `detail.pa_acct_id`, returning the identity the REST
    /// resource uses.
    ///
    /// A second detail with the same `PAUT9CTS` key under the summary violates
    /// `pautdtl1_key`, SQLSTATE 23505, the IMS `II` status:
    /// [`PortError::Duplicate`]. A missing summary violates the parent foreign
    /// key, SQLSTATE 23503, which the port reports as the `GE` of a parent
    /// that is not found: [`PortError::NotFound`], not the `Restricted` the
    /// shared mapping gives a blocked delete.
    async fn insert_detail(&mut self, detail: NewPendingAuthDetail) -> PortResult<AuthId> {
        let c = DetailColumns::new(&detail)?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_scalar!(
            r#"INSERT INTO pending_auth_details
                   (pa_acct_id, pa_auth_date_9c, pa_auth_time_9c,
                    pa_auth_orig_date, pa_auth_orig_time, pa_card_num, pa_auth_type,
                    pa_card_expiry_date, pa_message_type, pa_message_source,
                    pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
                    pa_processing_code, pa_transaction_amt, pa_approved_amt,
                    pa_merchant_catagory_code, pa_acqr_country_code, pa_pos_entry_mode,
                    pa_merchant_id, pa_merchant_name, pa_merchant_city, pa_merchant_state,
                    pa_merchant_zip, pa_transaction_id, pa_match_status, pa_auth_fraud,
                    pa_fraud_rpt_date)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                       $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
               RETURNING auth_id"#,
            c.acct_id,
            c.auth_date_9c,
            c.auth_time_9c,
            c.auth_orig_date,
            c.auth_orig_time,
            c.card_num,
            c.auth_type,
            c.card_expiry_date,
            c.message_type,
            c.message_source,
            c.auth_id_code,
            c.auth_resp_code,
            c.auth_resp_reason,
            c.processing_code,
            c.transaction_amt,
            c.approved_amt,
            c.merchant_catagory_code,
            c.acqr_country_code,
            c.pos_entry_mode,
            c.merchant_id,
            c.merchant_name,
            c.merchant_city,
            c.merchant_state,
            c.merchant_zip,
            c.transaction_id,
            c.match_status,
            c.auth_fraud,
            c.fraud_rpt_date
        )
        .fetch_one(&mut *sp)
        .await;
        settle(sp, result)
            .await
            .map(AuthId::from)
            .map_err(parent_not_found)
    }

    /// COPAUS1C READ-AUTH-RECORD — P-COPAUS1C-003
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L465-470]: the
    /// detail `GNP PAUTDTL1 WHERE (PAUT9CTS = PA-AUTHORIZATION-KEY)` reaches,
    /// addressed by its identity. No row is the `GE` status.
    async fn read_detail(&mut self, id: AuthId) -> PortResult<PendingAuthDetail> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            DetailRow,
            r#"SELECT auth_id, pa_acct_id, pa_auth_date_9c, pa_auth_time_9c,
                      pa_auth_orig_date, pa_auth_orig_time, pa_card_num, pa_auth_type,
                      pa_card_expiry_date, pa_message_type, pa_message_source,
                      pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
                      pa_processing_code, pa_transaction_amt, pa_approved_amt,
                      pa_merchant_catagory_code, pa_acqr_country_code, pa_pos_entry_mode,
                      pa_merchant_id, pa_merchant_name, pa_merchant_city, pa_merchant_state,
                      pa_merchant_zip, pa_transaction_id, pa_match_status, pa_auth_fraud,
                      pa_fraud_rpt_date
                 FROM pending_auth_details
                WHERE auth_id = $1"#,
            id.get()
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(detail_from_row(&row)?)
    }

    /// COPAUS1C MARK-AUTH-FRAUD, READ-AUTH-RECORD — P-COPAUS1C-003,
    /// P-COPAUS1C-006 [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L230-242]
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L465-470]: the
    /// detail PF5 toggles, read with `SELECT … FOR UPDATE`.
    ///
    /// The row lock is held until the caller's transaction ends, through the
    /// `AUTHFRDS` write, the detail replace and the commit or rollback, so a
    /// second fraud mark of the same authorization waits here and then reads
    /// the flag the first one committed. The statement runs under its own
    /// savepoint, so a lock timeout ([`PortError::Contention`]) or any other
    /// failure leaves the transaction usable; a released savepoint keeps the
    /// lock in the transaction. No row is the `GE` status.
    async fn read_detail_for_update(&mut self, id: AuthId) -> PortResult<PendingAuthDetail> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            DetailRow,
            r#"SELECT auth_id, pa_acct_id, pa_auth_date_9c, pa_auth_time_9c,
                      pa_auth_orig_date, pa_auth_orig_time, pa_card_num, pa_auth_type,
                      pa_card_expiry_date, pa_message_type, pa_message_source,
                      pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
                      pa_processing_code, pa_transaction_amt, pa_approved_amt,
                      pa_merchant_catagory_code, pa_acqr_country_code, pa_pos_entry_mode,
                      pa_merchant_id, pa_merchant_name, pa_merchant_city, pa_merchant_state,
                      pa_merchant_zip, pa_transaction_id, pa_match_status, pa_auth_fraud,
                      pa_fraud_rpt_date
                 FROM pending_auth_details
                WHERE auth_id = $1
                  FOR UPDATE"#,
            id.get()
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(detail_from_row(&row)?)
    }

    /// COPAUS0C PROCESS-PAGE-FORWARD and REPOSITION-AUTHORIZATIONS —
    /// P-COPAUS0C-010 [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L415-454]
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L488-519]; CBPAUP0C
    /// 3000-FIND-NEXT-AUTH-DTL — P-CBPAUP0C-003
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L248-272]: the
    /// `GNP PAUTDTL1` loop under the summary of `id`, in twin order.
    ///
    /// A start key positions at its own `PAUT9CTS` key, inclusive, as the
    /// qualified `GNP … WHERE (PAUT9CTS = key)` does, and only when it names a
    /// detail of this account; otherwise nothing is found, as that `GNP` ends
    /// with `GE`. Forward reads ascending keys from there, newest first;
    /// backward reads descending keys. No detail, or the end of the twins, is
    /// an empty page.
    async fn details_for_account(
        &mut self,
        id: AccountId,
        browse: Browse<AuthId>,
    ) -> PortResult<Vec<PendingAuthDetail>> {
        let start = browse.start_key.map(AuthId::get);
        let limit = limit_i64(browse.limit)?;
        details_page(self.tx, id.to_i64(), start, browse.direction, limit).await
    }

    /// COPAUS1C READ-NEXT-AUTH-RECORD — P-COPAUS1C-014, P-COPAUS1C-009
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L493-518]: the
    /// unqualified `GNP PAUTDTL1` from the `PAUT9CTS` key (`auth_date_9c`,
    /// `auth_time_9c`) under the summary of `account`, the next twin, or for
    /// [`Direction::Backward`] the previous one.
    ///
    /// One statement on the unique index `pautdtl1_key` (`pa_acct_id`,
    /// `pa_auth_date_9c`, `pa_auth_time_9c`): the first row past the key in
    /// the direction's order. The key is excluded, as it is unique under the
    /// summary and belongs to the detail already read. Both key parts are
    /// `INTEGER`, so the row comparison orders as the packed `PAUT9CTS` bytes
    /// do. `None` when no twin follows (`GE` or `GB`).
    async fn adjacent_detail(
        &mut self,
        account: AccountId,
        auth_date_9c: i32,
        auth_time_9c: i32,
        direction: Direction,
    ) -> PortResult<Option<PendingAuthDetail>> {
        let mut sp = savepoint(self.tx).await?;
        let result = match direction {
            Direction::Forward => {
                sqlx::query_as!(
                    DetailRow,
                    r#"SELECT auth_id, pa_acct_id, pa_auth_date_9c, pa_auth_time_9c,
                              pa_auth_orig_date, pa_auth_orig_time, pa_card_num, pa_auth_type,
                              pa_card_expiry_date, pa_message_type, pa_message_source,
                              pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
                              pa_processing_code, pa_transaction_amt, pa_approved_amt,
                              pa_merchant_catagory_code, pa_acqr_country_code,
                              pa_pos_entry_mode, pa_merchant_id, pa_merchant_name,
                              pa_merchant_city, pa_merchant_state, pa_merchant_zip,
                              pa_transaction_id, pa_match_status, pa_auth_fraud,
                              pa_fraud_rpt_date
                         FROM pending_auth_details
                        WHERE pa_acct_id = $1
                          AND (pa_auth_date_9c, pa_auth_time_9c) > ($2, $3)
                        ORDER BY pa_auth_date_9c, pa_auth_time_9c
                        LIMIT 1"#,
                    account.to_i64(),
                    auth_date_9c,
                    auth_time_9c
                )
                .fetch_optional(&mut *sp)
                .await
            }
            Direction::Backward => {
                sqlx::query_as!(
                    DetailRow,
                    r#"SELECT auth_id, pa_acct_id, pa_auth_date_9c, pa_auth_time_9c,
                              pa_auth_orig_date, pa_auth_orig_time, pa_card_num, pa_auth_type,
                              pa_card_expiry_date, pa_message_type, pa_message_source,
                              pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason,
                              pa_processing_code, pa_transaction_amt, pa_approved_amt,
                              pa_merchant_catagory_code, pa_acqr_country_code,
                              pa_pos_entry_mode, pa_merchant_id, pa_merchant_name,
                              pa_merchant_city, pa_merchant_state, pa_merchant_zip,
                              pa_transaction_id, pa_match_status, pa_auth_fraud,
                              pa_fraud_rpt_date
                         FROM pending_auth_details
                        WHERE pa_acct_id = $1
                          AND (pa_auth_date_9c, pa_auth_time_9c) < ($2, $3)
                        ORDER BY pa_auth_date_9c DESC, pa_auth_time_9c DESC
                        LIMIT 1"#,
                    account.to_i64(),
                    auth_date_9c,
                    auth_time_9c
                )
                .fetch_optional(&mut *sp)
                .await
            }
        };
        let row = settle(sp, result).await?;
        row.as_ref()
            .map(detail_from_row)
            .transpose()
            .map_err(PortError::from)
    }

    /// COPAUS1C UPDATE-AUTH-DETAILS — P-COPAUS1C-008
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L520-552]:
    /// `REPL PAUTDTL1` after the fraud marking, every field but the keys
    /// replaced.
    ///
    /// A `REPL` can neither move a segment to another parent nor change its
    /// sequence field, so the row is replaced only when its parent account
    /// and `PAUT9CTS` key equal those of `detail`. No such row with the
    /// identity is [`PortError::NotFound`].
    async fn update_detail(&mut self, detail: PendingAuthDetail) -> PortResult<()> {
        let c = DetailColumns::new(&detail.to_new())?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            r#"UPDATE pending_auth_details
                  SET pa_auth_orig_date = $5,
                      pa_auth_orig_time = $6,
                      pa_card_num = $7,
                      pa_auth_type = $8,
                      pa_card_expiry_date = $9,
                      pa_message_type = $10,
                      pa_message_source = $11,
                      pa_auth_id_code = $12,
                      pa_auth_resp_code = $13,
                      pa_auth_resp_reason = $14,
                      pa_processing_code = $15,
                      pa_transaction_amt = $16,
                      pa_approved_amt = $17,
                      pa_merchant_catagory_code = $18,
                      pa_acqr_country_code = $19,
                      pa_pos_entry_mode = $20,
                      pa_merchant_id = $21,
                      pa_merchant_name = $22,
                      pa_merchant_city = $23,
                      pa_merchant_state = $24,
                      pa_merchant_zip = $25,
                      pa_transaction_id = $26,
                      pa_match_status = $27,
                      pa_auth_fraud = $28,
                      pa_fraud_rpt_date = $29
                WHERE auth_id = $1
                  AND pa_acct_id = $2
                  AND pa_auth_date_9c = $3
                  AND pa_auth_time_9c = $4"#,
            detail.auth_id.get(),
            c.acct_id,
            c.auth_date_9c,
            c.auth_time_9c,
            c.auth_orig_date,
            c.auth_orig_time,
            c.card_num,
            c.auth_type,
            c.card_expiry_date,
            c.message_type,
            c.message_source,
            c.auth_id_code,
            c.auth_resp_code,
            c.auth_resp_reason,
            c.processing_code,
            c.transaction_amt,
            c.approved_amt,
            c.merchant_catagory_code,
            c.acqr_country_code,
            c.pos_entry_mode,
            c.merchant_id,
            c.merchant_name,
            c.merchant_city,
            c.merchant_state,
            c.merchant_zip,
            c.transaction_id,
            c.match_status,
            c.auth_fraud,
            c.fraud_rpt_date
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// CBPAUP0C 5000-DELETE-AUTH-DTL — P-CBPAUP0C-005
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L303-323]:
    /// `DLET PAUTDTL1` of an expired detail. No row is
    /// [`PortError::NotFound`].
    async fn delete_detail(&mut self, id: AuthId) -> PortResult<()> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "DELETE FROM pending_auth_details WHERE auth_id = $1",
            id.get()
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// COPAUS2C MAIN-PARA — P-COPAUS2C-002, P-COPAUS2C-003
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L141-202]:
    /// `INSERT INTO CARDDEMO.AUTHFRDS`, every column as `fraud` holds it,
    /// `FRAUD_RPT_DATE` included: the rule fills it from the processing clock
    /// where the COBOL writes Db2 `CURRENT DATE`. A report with the same
    /// `CARD_NUM` and `AUTH_TS` violates the primary key, SQLSTATE 23505, the
    /// `SQLCODE` -803 that sends the rule to `FRAUD-UPDATE`:
    /// [`PortError::Duplicate`].
    async fn insert_fraud(&mut self, fraud: AuthFraud) -> PortResult<()> {
        let pos_entry_mode = fraud
            .pos_entry_mode
            .map(|code| narrow::<_, i16>(code.get(), "pos_entry_mode"))
            .transpose()?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            r#"INSERT INTO auth_fraud
                   (card_num, auth_ts, auth_type, card_expiry_date, message_type,
                    message_source, auth_id_code, auth_resp_code, auth_resp_reason,
                    processing_code, transaction_amt, approved_amt, merchant_catagory_code,
                    acqr_country_code, pos_entry_mode, merchant_id, merchant_name,
                    merchant_city, merchant_state, merchant_zip, transaction_id,
                    match_status, auth_fraud, fraud_rpt_date, acct_id, cust_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                       $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26)"#,
            fraud.card_num.trimmed().into_owned(),
            fraud.auth_ts,
            opt_text(fraud.auth_type.as_ref()),
            opt_text(fraud.card_expiry_date.as_ref()),
            opt_text(fraud.message_type.as_ref()),
            opt_text(fraud.message_source.as_ref()),
            opt_text(fraud.auth_id_code.as_ref()),
            opt_text(fraud.auth_resp_code.as_ref()),
            opt_text(fraud.auth_resp_reason.as_ref()),
            opt_text(fraud.processing_code.as_ref()),
            fraud.transaction_amt.map(Money::to_decimal),
            fraud.approved_amt.map(Money::to_decimal),
            opt_text(fraud.merchant_catagory_code.as_ref()),
            opt_text(fraud.acqr_country_code.as_ref()),
            pos_entry_mode,
            opt_text(fraud.merchant_id.as_ref()),
            opt_text(fraud.merchant_name.as_ref()),
            opt_text(fraud.merchant_city.as_ref()),
            opt_text(fraud.merchant_state.as_ref()),
            opt_text(fraud.merchant_zip.as_ref()),
            opt_text(fraud.transaction_id.as_ref()),
            opt_text(fraud.match_status.as_ref()),
            opt_text(fraud.auth_fraud.as_ref()),
            fraud.fraud_rpt_date,
            fraud.acct_id.map(AccountId::to_i64),
            fraud.cust_id.map(CustomerId::to_i64)
        )
        .execute(&mut *sp)
        .await;
        settle(sp, result).await.map(|_| ())
    }

    /// COPAUS2C FRAUD-UPDATE — P-COPAUS2C-004
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L221-233]:
    /// `UPDATE CARDDEMO.AUTHFRDS SET AUTH_FRAUD, FRAUD_RPT_DATE` of the report
    /// keyed by `fraud.card_num` and `fraud.auth_ts`. Both values come from
    /// `fraud`; the report date is the processing clock's date the rule
    /// passes, never the database's current date. No row is the `SQLCODE`
    /// +100: [`PortError::NotFound`].
    async fn update_fraud(&mut self, fraud: AuthFraud) -> PortResult<()> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            r#"UPDATE auth_fraud
                  SET auth_fraud = $3,
                      fraud_rpt_date = $4
                WHERE card_num = $1
                  AND auth_ts = $2"#,
            fraud.card_num.trimmed().into_owned(),
            fraud.auth_ts,
            opt_text(fraud.auth_fraud.as_ref()),
            fraud.fraud_rpt_date
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

impl PgStore<'_, '_> {
    /// The `AUTHFRDS` report of the authorization keyed by `card_num` and
    /// `auth_ts`, the primary key and the unique index `XAUTHFRD`
    /// [app/app-authorization-ims-db2-mq/ddl/XAUTHFRD.ddl:L1-4].
    ///
    /// No carried program reads the table back: `COPAUS2C` only inserts and
    /// updates it (P-COPAUS2C-003, P-COPAUS2C-004). The read lets the CPVD
    /// adapter and its tests see the row those writes left, each column held
    /// to its section A row (P-AUTHFRDS-001 to P-AUTHFRDS-026).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no report has the key;
    /// [`PortError::Unavailable`] on any other failure, a stored value that
    /// does not fit its domain type included.
    pub async fn read_fraud(
        &mut self,
        card_num: CardNumber,
        auth_ts: NaiveDateTime,
    ) -> PortResult<AuthFraud> {
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            FraudRow,
            r#"SELECT card_num, auth_ts, auth_type, card_expiry_date, message_type,
                      message_source, auth_id_code, auth_resp_code, auth_resp_reason,
                      processing_code, transaction_amt, approved_amt, merchant_catagory_code,
                      acqr_country_code, pos_entry_mode, merchant_id, merchant_name,
                      merchant_city, merchant_state, merchant_zip, transaction_id,
                      match_status, auth_fraud, fraud_rpt_date, acct_id, cust_id
                 FROM auth_fraud
                WHERE card_num = $1
                  AND auth_ts = $2"#,
            card_num.trimmed().into_owned(),
            auth_ts
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(fraud_from_row(&row)?)
    }
}

/// One page of the details of the account `acct_id` in twin order, read under
/// a savepoint of its own in `tx`: [`AuthorizationStore::details_for_account`],
/// the `GNP PAUTDTL1` loop of COPAUS0C PROCESS-PAGE-FORWARD — P-COPAUS0C-010
/// and CBPAUP0C 3000-FIND-NEXT-AUTH-DTL — P-CBPAUP0C-003.
///
/// `start`, when given, is the identity of a detail of the account, and the
/// page begins at its key inclusive; an identity that names no detail of the
/// account leaves the start-row sub-select empty, which makes the row
/// comparison `NULL`, so the page is empty, as the qualified `GNP` ends with
/// `GE`. Without `start` the page begins at the first twin, or for backward
/// at the last. [`Direction::Forward`] reads ascending (`pa_auth_date_9c`,
/// `pa_auth_time_9c`), the twin order; backward reads descending. Both key
/// parts are `INTEGER`, so the row comparison orders as the packed
/// `PAUT9CTS` bytes do.
///
/// Each of the four cases, with or without `start` in either direction, is
/// its own statement, so no optional-parameter `OR` keeps the start key out
/// of the plan. The start-row sub-select is uncorrelated and runs once,
/// before the scan, and its key then bounds the scan of `pautdtl1_key` as an
/// index condition, under custom and generic prepared plans alike, instead
/// of filtering out every twin that precedes it.
async fn details_page(
    tx: &mut Transaction<'_, Postgres>,
    acct_id: i64,
    start: Option<i64>,
    direction: Direction,
    limit: i64,
) -> PortResult<Vec<PendingAuthDetail>> {
    let mut sp = savepoint(tx).await?;
    let result = match direction {
        Direction::Forward => details_forward(&mut sp, acct_id, start, limit).await,
        Direction::Backward => details_backward(&mut sp, acct_id, start, limit).await,
    };
    let rows = settle(sp, result).await?;
    rows.iter()
        .map(|row| detail_from_row(row).map_err(PortError::from))
        .collect()
}

/// COPAUS0C PROCESS-PAGE-FORWARD and REPOSITION-AUTHORIZATIONS —
/// P-COPAUS0C-010 [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L415-454]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L488-519]; CBPAUP0C
/// 3000-FIND-NEXT-AUTH-DTL — P-CBPAUP0C-003
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L248-272]: the
/// `GNP PAUTDTL1` reads in twin order.
///
/// The rows of a [`Direction::Forward`] [`details_page`]: up to `limit`
/// twins of `acct_id` in ascending key order, from the first twin, or from
/// the key of the detail `start` inclusive.
async fn details_forward(
    conn: &mut sqlx::PgConnection,
    acct_id: i64,
    start: Option<i64>,
    limit: i64,
) -> Result<Vec<DetailRow>, sqlx::Error> {
    match start {
        None => {
            sqlx::query_as!(
                DetailRow,
                r#"SELECT d.auth_id, d.pa_acct_id, d.pa_auth_date_9c, d.pa_auth_time_9c,
                          d.pa_auth_orig_date, d.pa_auth_orig_time, d.pa_card_num,
                          d.pa_auth_type, d.pa_card_expiry_date, d.pa_message_type,
                          d.pa_message_source, d.pa_auth_id_code, d.pa_auth_resp_code,
                          d.pa_auth_resp_reason, d.pa_processing_code, d.pa_transaction_amt,
                          d.pa_approved_amt, d.pa_merchant_catagory_code,
                          d.pa_acqr_country_code, d.pa_pos_entry_mode, d.pa_merchant_id,
                          d.pa_merchant_name, d.pa_merchant_city, d.pa_merchant_state,
                          d.pa_merchant_zip, d.pa_transaction_id, d.pa_match_status,
                          d.pa_auth_fraud, d.pa_fraud_rpt_date
                     FROM pending_auth_details d
                    WHERE d.pa_acct_id = $1
                    ORDER BY d.pa_auth_date_9c, d.pa_auth_time_9c
                    LIMIT $2"#,
                acct_id,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        Some(start) => {
            sqlx::query_as!(
                DetailRow,
                r#"SELECT d.auth_id, d.pa_acct_id, d.pa_auth_date_9c, d.pa_auth_time_9c,
                          d.pa_auth_orig_date, d.pa_auth_orig_time, d.pa_card_num,
                          d.pa_auth_type, d.pa_card_expiry_date, d.pa_message_type,
                          d.pa_message_source, d.pa_auth_id_code, d.pa_auth_resp_code,
                          d.pa_auth_resp_reason, d.pa_processing_code, d.pa_transaction_amt,
                          d.pa_approved_amt, d.pa_merchant_catagory_code,
                          d.pa_acqr_country_code, d.pa_pos_entry_mode, d.pa_merchant_id,
                          d.pa_merchant_name, d.pa_merchant_city, d.pa_merchant_state,
                          d.pa_merchant_zip, d.pa_transaction_id, d.pa_match_status,
                          d.pa_auth_fraud, d.pa_fraud_rpt_date
                     FROM pending_auth_details d
                    WHERE d.pa_acct_id = $1
                      AND (d.pa_auth_date_9c, d.pa_auth_time_9c) >= (
                              SELECT s.pa_auth_date_9c, s.pa_auth_time_9c
                                FROM pending_auth_details s
                               WHERE s.auth_id = $2 AND s.pa_acct_id = $1)
                    ORDER BY d.pa_auth_date_9c, d.pa_auth_time_9c
                    LIMIT $3"#,
                acct_id,
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
    }
}

/// COPAUS0C PROCESS-PF7-KEY — P-COPAUS0C-011
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L362-385], with
/// REPOSITION-AUTHORIZATIONS — P-COPAUS0C-010
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L488-519], for the
/// previous page, read back from the first key shown instead of
/// repositioning on the first key of the previous page.
///
/// The rows of a [`Direction::Backward`] [`details_page`]: up to `limit`
/// twins of `acct_id` in descending key order, from the last twin, or from
/// the key of the detail `start` inclusive.
async fn details_backward(
    conn: &mut sqlx::PgConnection,
    acct_id: i64,
    start: Option<i64>,
    limit: i64,
) -> Result<Vec<DetailRow>, sqlx::Error> {
    match start {
        None => {
            sqlx::query_as!(
                DetailRow,
                r#"SELECT d.auth_id, d.pa_acct_id, d.pa_auth_date_9c, d.pa_auth_time_9c,
                          d.pa_auth_orig_date, d.pa_auth_orig_time, d.pa_card_num,
                          d.pa_auth_type, d.pa_card_expiry_date, d.pa_message_type,
                          d.pa_message_source, d.pa_auth_id_code, d.pa_auth_resp_code,
                          d.pa_auth_resp_reason, d.pa_processing_code, d.pa_transaction_amt,
                          d.pa_approved_amt, d.pa_merchant_catagory_code,
                          d.pa_acqr_country_code, d.pa_pos_entry_mode, d.pa_merchant_id,
                          d.pa_merchant_name, d.pa_merchant_city, d.pa_merchant_state,
                          d.pa_merchant_zip, d.pa_transaction_id, d.pa_match_status,
                          d.pa_auth_fraud, d.pa_fraud_rpt_date
                     FROM pending_auth_details d
                    WHERE d.pa_acct_id = $1
                    ORDER BY d.pa_auth_date_9c DESC, d.pa_auth_time_9c DESC
                    LIMIT $2"#,
                acct_id,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        Some(start) => {
            sqlx::query_as!(
                DetailRow,
                r#"SELECT d.auth_id, d.pa_acct_id, d.pa_auth_date_9c, d.pa_auth_time_9c,
                          d.pa_auth_orig_date, d.pa_auth_orig_time, d.pa_card_num,
                          d.pa_auth_type, d.pa_card_expiry_date, d.pa_message_type,
                          d.pa_message_source, d.pa_auth_id_code, d.pa_auth_resp_code,
                          d.pa_auth_resp_reason, d.pa_processing_code, d.pa_transaction_amt,
                          d.pa_approved_amt, d.pa_merchant_catagory_code,
                          d.pa_acqr_country_code, d.pa_pos_entry_mode, d.pa_merchant_id,
                          d.pa_merchant_name, d.pa_merchant_city, d.pa_merchant_state,
                          d.pa_merchant_zip, d.pa_transaction_id, d.pa_match_status,
                          d.pa_auth_fraud, d.pa_fraud_rpt_date
                     FROM pending_auth_details d
                    WHERE d.pa_acct_id = $1
                      AND (d.pa_auth_date_9c, d.pa_auth_time_9c) <= (
                              SELECT s.pa_auth_date_9c, s.pa_auth_time_9c
                                FROM pending_auth_details s
                               WHERE s.auth_id = $2 AND s.pa_acct_id = $1)
                    ORDER BY d.pa_auth_date_9c DESC, d.pa_auth_time_9c DESC
                    LIMIT $3"#,
                acct_id,
                start,
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
    }
}

/// The summary a stored row holds, each column held to its section A row
/// (P-CIPAUSMY-001 to P-CIPAUSMY-012).
fn summary_from_row(row: SummaryRow) -> Result<PendingAuthSummary, PersistenceError> {
    Ok(PendingAuthSummary {
        pa_acct_id: account_id(row.acct_id, SUMMARIES, "pa_acct_id")?,
        pa_cust_id: customer_id(row.cust_id, SUMMARIES, "pa_cust_id")?,
        pa_auth_status: alnum(&row.auth_status, SUMMARIES, "pa_auth_status")?,
        pa_account_status: account_statuses(row.account_status)?,
        pa_credit_limit: money(row.credit_limit, SUMMARIES, "pa_credit_limit")?,
        pa_cash_limit: money(row.cash_limit, SUMMARIES, "pa_cash_limit")?,
        pa_credit_balance: money(row.credit_balance, SUMMARIES, "pa_credit_balance")?,
        pa_cash_balance: money(row.cash_balance, SUMMARIES, "pa_cash_balance")?,
        pa_approved_auth_cnt: auth_count(row.approved_auth_cnt, SUMMARIES, "pa_approved_auth_cnt")?,
        pa_declined_auth_cnt: auth_count(row.declined_auth_cnt, SUMMARIES, "pa_declined_auth_cnt")?,
        pa_approved_auth_amt: money(row.approved_auth_amt, SUMMARIES, "pa_approved_auth_amt")?,
        pa_declined_auth_amt: money(row.declined_auth_amt, SUMMARIES, "pa_declined_auth_amt")?,
    })
}

/// `PA-ACCOUNT-STATUS X(02) OCCURS 5 TIMES` from its `VARCHAR(2)[5]` column
/// (P-CIPAUSMY-004): element `i` is occurrence `i + 1`. An array of another
/// length is [`PersistenceError::Corrupt`], although the column's check
/// already admits only five elements.
fn account_statuses(
    stored: Vec<String>,
) -> Result<[Alnum<2>; PA_ACCOUNT_STATUS_OCCURS], PersistenceError> {
    let occurrences = <[String; PA_ACCOUNT_STATUS_OCCURS]>::try_from(stored).map_err(|stored| {
        PersistenceError::Corrupt {
            table: SUMMARIES,
            column: ACCOUNT_STATUS,
            reason: format!(
                "expected {PA_ACCOUNT_STATUS_OCCURS} occurrences, got {}",
                stored.len()
            ),
        }
    })?;
    let mut statuses = [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS];
    for (status, occurrence) in statuses.iter_mut().zip(&occurrences) {
        *status = alnum(occurrence, SUMMARIES, ACCOUNT_STATUS)?;
    }
    Ok(statuses)
}

/// The detail a stored row holds, each column held to its section A row
/// (P-CIPAUDTY-001 to P-CIPAUDTY-027). The match status and the fraud flag
/// keep whatever byte they hold, an unnamed one included.
fn detail_from_row(row: &DetailRow) -> Result<PendingAuthDetail, PersistenceError> {
    Ok(PendingAuthDetail {
        auth_id: AuthId::from(row.auth_id),
        pa_acct_id: account_id(row.pa_acct_id, DETAILS, "pa_acct_id")?,
        pa_auth_date_9c: row.pa_auth_date_9c,
        pa_auth_time_9c: row.pa_auth_time_9c,
        pa_auth_orig_date: alnum(&row.pa_auth_orig_date, DETAILS, "pa_auth_orig_date")?,
        pa_auth_orig_time: alnum(&row.pa_auth_orig_time, DETAILS, "pa_auth_orig_time")?,
        pa_card_num: card_number(&row.pa_card_num, DETAILS, "pa_card_num")?,
        pa_auth_type: alnum(&row.pa_auth_type, DETAILS, "pa_auth_type")?,
        pa_card_expiry_date: alnum(&row.pa_card_expiry_date, DETAILS, "pa_card_expiry_date")?,
        pa_message_type: alnum(&row.pa_message_type, DETAILS, "pa_message_type")?,
        pa_message_source: alnum(&row.pa_message_source, DETAILS, "pa_message_source")?,
        pa_auth_id_code: alnum(&row.pa_auth_id_code, DETAILS, "pa_auth_id_code")?,
        pa_auth_resp_code: alnum(&row.pa_auth_resp_code, DETAILS, "pa_auth_resp_code")?,
        pa_auth_resp_reason: alnum(&row.pa_auth_resp_reason, DETAILS, "pa_auth_resp_reason")?,
        pa_processing_code: numeric_code(row.pa_processing_code, DETAILS, "pa_processing_code")?,
        pa_transaction_amt: money(row.pa_transaction_amt, DETAILS, "pa_transaction_amt")?,
        pa_approved_amt: money(row.pa_approved_amt, DETAILS, "pa_approved_amt")?,
        pa_merchant_catagory_code: alnum(
            &row.pa_merchant_catagory_code,
            DETAILS,
            "pa_merchant_catagory_code",
        )?,
        pa_acqr_country_code: alnum(&row.pa_acqr_country_code, DETAILS, "pa_acqr_country_code")?,
        pa_pos_entry_mode: numeric_code(row.pa_pos_entry_mode, DETAILS, "pa_pos_entry_mode")?,
        pa_merchant_id: alnum(&row.pa_merchant_id, DETAILS, "pa_merchant_id")?,
        pa_merchant_name: alnum(&row.pa_merchant_name, DETAILS, "pa_merchant_name")?,
        pa_merchant_city: alnum(&row.pa_merchant_city, DETAILS, "pa_merchant_city")?,
        pa_merchant_state: alnum(&row.pa_merchant_state, DETAILS, "pa_merchant_state")?,
        pa_merchant_zip: alnum(&row.pa_merchant_zip, DETAILS, "pa_merchant_zip")?,
        pa_transaction_id: alnum(&row.pa_transaction_id, DETAILS, "pa_transaction_id")?,
        pa_match_status: MatchStatus::from_alnum(alnum(
            &row.pa_match_status,
            DETAILS,
            "pa_match_status",
        )?),
        pa_auth_fraud: FraudFlag::from_alnum(alnum(&row.pa_auth_fraud, DETAILS, "pa_auth_fraud")?),
        pa_fraud_rpt_date: alnum(&row.pa_fraud_rpt_date, DETAILS, "pa_fraud_rpt_date")?,
    })
}

/// The fraud report a stored row holds, each column held to its section A
/// row (P-AUTHFRDS-001 to P-AUTHFRDS-026); `NULL` stays `None`.
fn fraud_from_row(row: &FraudRow) -> Result<AuthFraud, PersistenceError> {
    Ok(AuthFraud {
        card_num: card_number(&row.card_num, FRAUD, "card_num")?,
        auth_ts: row.auth_ts,
        auth_type: opt_alnum(row.auth_type.as_deref(), FRAUD, "auth_type")?,
        card_expiry_date: opt_alnum(row.card_expiry_date.as_deref(), FRAUD, "card_expiry_date")?,
        message_type: opt_alnum(row.message_type.as_deref(), FRAUD, "message_type")?,
        message_source: opt_alnum(row.message_source.as_deref(), FRAUD, "message_source")?,
        auth_id_code: opt_alnum(row.auth_id_code.as_deref(), FRAUD, "auth_id_code")?,
        auth_resp_code: opt_alnum(row.auth_resp_code.as_deref(), FRAUD, "auth_resp_code")?,
        auth_resp_reason: opt_alnum(row.auth_resp_reason.as_deref(), FRAUD, "auth_resp_reason")?,
        processing_code: opt_alnum(row.processing_code.as_deref(), FRAUD, "processing_code")?,
        transaction_amt: opt_money(row.transaction_amt, FRAUD, "transaction_amt")?,
        approved_amt: opt_money(row.approved_amt, FRAUD, "approved_amt")?,
        merchant_catagory_code: opt_alnum(
            row.merchant_catagory_code.as_deref(),
            FRAUD,
            "merchant_catagory_code",
        )?,
        acqr_country_code: opt_alnum(row.acqr_country_code.as_deref(), FRAUD, "acqr_country_code")?,
        pos_entry_mode: row
            .pos_entry_mode
            .map(|mode| numeric_code(mode, FRAUD, "pos_entry_mode"))
            .transpose()?,
        merchant_id: opt_alnum(row.merchant_id.as_deref(), FRAUD, "merchant_id")?,
        merchant_name: opt_alnum(row.merchant_name.as_deref(), FRAUD, "merchant_name")?,
        merchant_city: opt_alnum(row.merchant_city.as_deref(), FRAUD, "merchant_city")?,
        merchant_state: opt_alnum(row.merchant_state.as_deref(), FRAUD, "merchant_state")?,
        merchant_zip: opt_alnum(row.merchant_zip.as_deref(), FRAUD, "merchant_zip")?,
        transaction_id: opt_alnum(row.transaction_id.as_deref(), FRAUD, "transaction_id")?,
        match_status: opt_alnum(row.match_status.as_deref(), FRAUD, "match_status")?,
        auth_fraud: opt_alnum(row.auth_fraud.as_deref(), FRAUD, "auth_fraud")?,
        fraud_rpt_date: row.fraud_rpt_date,
        acct_id: row
            .acct_id
            .map(|id| account_id(id, FRAUD, "acct_id"))
            .transpose()?,
        cust_id: row
            .cust_id
            .map(|id| customer_id(id, FRAUD, "cust_id"))
            .transpose()?,
    })
}

/// An `X(n)` item as the right-trimmed text its `VARCHAR(n)` column stores
/// (AAP 0.5.3.4); reading back, [`alnum`] restores the padding.
fn text<const N: usize>(item: &Alnum<N>) -> String {
    item.trimmed().into_owned()
}

/// [`text`] for a nullable Db2 column of `auth_fraud`: `None` binds `NULL`.
fn opt_text<const N: usize>(item: Option<&Alnum<N>>) -> Option<String> {
    item.map(text)
}

/// `value` converted to the narrower integer type its column binds, such as
/// a nine-digit customer ID into `INTEGER` or a two-digit code into
/// `SMALLINT`.
///
/// Every value the domain types admit fits its column, so the conversion
/// cannot fail for a value they built. A value that would not fit is
/// reported as [`PortError::Unavailable`] rather than cut.
fn narrow<T, U>(value: T, column: &'static str) -> PortResult<U>
where
    U: TryFrom<T>,
{
    U::try_from(value)
        .map_err(|_| PortError::Unavailable(format!("{column} value exceeds its column type")))
}

/// The outcome of a detail `ISRT` whose summary is missing.
///
/// The shared SQLSTATE mapping reports the parent foreign key's 23503 as
/// [`PortError::Restricted`], the Db2 `SQLCODE` -532 of a blocked delete. An
/// insert under a missing parent is instead the IMS `GE` the port documents:
/// the only foreign key of `pending_auth_details` is the one to its summary,
/// so `Restricted` here always means that parent is absent.
fn parent_not_found(err: PortError) -> PortError {
    match err {
        PortError::Restricted => PortError::NotFound,
        other => other,
    }
}
