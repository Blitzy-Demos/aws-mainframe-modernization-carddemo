//! The account master: VSAM `ACCTDAT`, `CVACT01Y` `ACCOUNT-RECORD` (300
//! bytes, key `ACCT-ID` at offset 0 for 11 bytes,
//! [app/jcl/ACCTFILE.jcl:L40-41]), on table `accounts`.
//!
//! [`PgStore`] implements [`AccountReader`] and [`AccountWriter`] here. Every
//! access is by the record key, the primary key `account_id`:
//!
//! | COBOL access | SQL | Savepoint |
//! |---|---|---|
//! | `READ` by key | `SELECT` | its own |
//! | `READ ... UPDATE` | `SELECT ... FOR UPDATE` | its own |
//! | `REWRITE` | `UPDATE` of the eleven non-key columns | its own |
//! | `READ` by key of each account the `CBSTM03A` cross-references name ([`PgStore::read_accounts`]) | one `SELECT ... WHERE account_id = ANY($1)` | its own; no statement for an empty key set |
//!
//! # Columns
//!
//! The non-key columns carry the `CVACT01Y` field names lower-snake-cased,
//! `acct_expiraion_date` spelled as the copybook spells it (P-CVACT01Y-002 to
//! P-CVACT01Y-012); `FILLER X(178)` (P-CVACT01Y-013) is not stored. Text is
//! bound right-trimmed, the only form the column checks of
//! `migrations/0002_master_data.sql` admit, and padded back to its `X(n)`
//! width when read; money is `NUMERIC(12,2)` both ways. The blank
//! `ACCT-GROUP-ID` every fixture account holds is therefore stored as `''`
//! (E-06).
//!
//! # Scope
//!
//! The store moves whole records and decides nothing: the status, limit and
//! expiry tests the programs make on an account belong to the rules in
//! `carddemo_domain::rules`.

use std::collections::{BTreeMap, BTreeSet};

use carddemo_domain::entities::Account;
use carddemo_domain::numeric::AccountId;
use carddemo_domain::ports::{AccountReader, AccountWriter, PortError, PortResult};
use rust_decimal::Decimal;

use super::{account_group_id, account_id, alnum, expect_affected, keyed_records, money};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table every conversion error names.
const TABLE: &str = "accounts";

/// One `accounts` row in the column types sqlx decodes: the `BIGINT` key,
/// `VARCHAR` text and `NUMERIC(12,2)` money, every column `NOT NULL`.
///
/// The read by key and the locking read select the same columns into it, and
/// [`Account::try_from`] holds each value to its PIC clause. sqlx checks every
/// query text on its own, so each read spells the column list out.
struct AccountRow {
    account_id: i64,
    acct_active_status: String,
    acct_curr_bal: Decimal,
    acct_credit_limit: Decimal,
    acct_cash_credit_limit: Decimal,
    acct_open_date: String,
    acct_expiraion_date: String,
    acct_reissue_date: String,
    acct_curr_cyc_credit: Decimal,
    acct_curr_cyc_debit: Decimal,
    acct_addr_zip: String,
    acct_group_id: String,
}

impl TryFrom<AccountRow> for Account {
    type Error = PersistenceError;

    /// The `CVACT01Y` record a row stores, each column through the conversion
    /// its section A row fixes (P-CVACT01Y-001 to P-CVACT01Y-012): the key as
    /// a `9(11)`, the five `S9(10)V99` amounts as `Money<10>`, the `X(1)` and
    /// `X(10)` text padded back to its width.
    ///
    /// A value that does not fit is [`PersistenceError::Corrupt`] naming
    /// `accounts` and its column, never cut or rounded to fit.
    fn try_from(row: AccountRow) -> Result<Self, Self::Error> {
        Ok(Self {
            acct_id: account_id(row.account_id, TABLE, "account_id")?,
            acct_active_status: alnum::<1>(&row.acct_active_status, TABLE, "acct_active_status")?,
            acct_curr_bal: money::<10>(row.acct_curr_bal, TABLE, "acct_curr_bal")?,
            acct_credit_limit: money::<10>(row.acct_credit_limit, TABLE, "acct_credit_limit")?,
            acct_cash_credit_limit: money::<10>(
                row.acct_cash_credit_limit,
                TABLE,
                "acct_cash_credit_limit",
            )?,
            acct_open_date: alnum::<10>(&row.acct_open_date, TABLE, "acct_open_date")?,
            acct_expiraion_date: alnum::<10>(
                &row.acct_expiraion_date,
                TABLE,
                "acct_expiraion_date",
            )?,
            acct_reissue_date: alnum::<10>(&row.acct_reissue_date, TABLE, "acct_reissue_date")?,
            acct_curr_cyc_credit: money::<10>(
                row.acct_curr_cyc_credit,
                TABLE,
                "acct_curr_cyc_credit",
            )?,
            acct_curr_cyc_debit: money::<10>(
                row.acct_curr_cyc_debit,
                TABLE,
                "acct_curr_cyc_debit",
            )?,
            acct_addr_zip: alnum::<10>(&row.acct_addr_zip, TABLE, "acct_addr_zip")?,
            acct_group_id: account_group_id(&row.acct_group_id, TABLE, "acct_group_id")?,
        })
    }
}

/// The outcome of a read by key: no row is the `NOTFND` (file status 23) the
/// programs branch on, and a row comes back as its record.
fn found(row: Option<AccountRow>) -> PortResult<Account> {
    let row = row.ok_or(PortError::NotFound)?;
    Account::try_from(row).map_err(PortError::from)
}

impl AccountReader for PgStore<'_, '_> {
    /// `COACTVWC` `9300-GETACCTDATA-BYACCT` [app/cbl/COACTVWC.cbl:L774-821];
    /// `COACTUPC` `9300-GETACCTDATA-BYACCT` [app/cbl/COACTUPC.cbl:L3701-3747];
    /// `COPAUS0C` `GETACCTDATA-BYACCT`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L865-912];
    /// `COPAUA0C` `5200-READ-ACCT-RECORD`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L520-562];
    /// `COACCT01` `4000-PROCESS-REQUEST-REPLY`
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L393-404]; `CBTRN02C`
    /// `1500-B-LOOKUP-ACCT` [app/cbl/CBTRN02C.cbl:L393-399]; `CBACT04C`
    /// `1100-GET-ACCT-DATA` [app/cbl/CBACT04C.cbl:L372-390]; `CBSTM03B`
    /// `4000-ACCTFILE-PROC` [app/cbl/CBSTM03B.CBL:L206-229] for `CBSTM03A`
    /// `3000-ACCTFILE-GET` — P-COACTVWC-009, P-COACTUPC-044, P-COPAUS0C-008,
    /// P-COPAUA0C-007, P-COACCT01-012, P-CBTRN02C-009, P-CBACT04C-002,
    /// P-CBSTM03B-005, P-CBSTM03A-1259.
    ///
    /// No lock is taken. The read runs under its own savepoint, so a failed
    /// statement leaves the caller's transaction usable: `COACTVWC` and
    /// `COACTUPC` still read `CUSTDAT` after an `ACCTDAT` failure
    /// (V-COACTVWC-006, V-COACTUPC-007), and INTCALC still commits the
    /// account rewrite made before it (P-CBACT04C-010).
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            AccountRow,
            r#"SELECT account_id, acct_active_status, acct_curr_bal, acct_credit_limit,
                      acct_cash_credit_limit, acct_open_date, acct_expiraion_date,
                      acct_reissue_date, acct_curr_cyc_credit, acct_curr_cyc_debit,
                      acct_addr_zip, acct_group_id
                 FROM accounts
                WHERE account_id = $1"#,
            id.to_i64()
        )
        .fetch_optional(&mut *sp)
        .await;
        found(settle(sp, result).await?)
    }

    /// `COACTUPC` `9600-WRITE-PROCESSING` `READ ... UPDATE`
    /// [app/cbl/COACTUPC.cbl:L3888-3915]; `COBIL00C` `READ-ACCTDAT-FILE`
    /// [app/cbl/COBIL00C.cbl:L343-372] — P-COACTUPC-010, P-COBIL00C-005,
    /// P-COBIL00C-009.
    ///
    /// The row lock is held until the caller's transaction ends, as the CICS
    /// lock lasts until the task's syncpoint. The statement waits for a lock
    /// another unit of work holds, as `READ UPDATE` waits on the record, and
    /// runs under its own savepoint, so a lock timeout or any other failure
    /// leaves the transaction usable for the writes that follow.
    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            AccountRow,
            r#"SELECT account_id, acct_active_status, acct_curr_bal, acct_credit_limit,
                      acct_cash_credit_limit, acct_open_date, acct_expiraion_date,
                      acct_reissue_date, acct_curr_cyc_credit, acct_curr_cyc_debit,
                      acct_addr_zip, acct_group_id
                 FROM accounts
                WHERE account_id = $1
                  FOR UPDATE"#,
            id.to_i64()
        )
        .fetch_optional(&mut *sp)
        .await;
        found(settle(sp, result).await?)
    }
}

impl AccountWriter for PgStore<'_, '_> {
    /// `COACTUPC` `9600-WRITE-PROCESSING` `REWRITE`
    /// [app/cbl/COACTUPC.cbl:L4060-4081]; `COBIL00C` `UPDATE-ACCTDAT-FILE`
    /// [app/cbl/COBIL00C.cbl:L377-403]; `CBTRN02C` `2800-UPDATE-ACCOUNT-REC`
    /// [app/cbl/CBTRN02C.cbl:L545-560]; `CBACT04C` `1050-UPDATE-ACCOUNT`
    /// [app/cbl/CBACT04C.cbl:L350-370] — P-COACTUPC-013, P-COACTUPC-014,
    /// P-COBIL00C-018, P-COBIL00C-019, P-CBTRN02C-018, P-CBACT04C-010.
    ///
    /// Replaces all eleven non-key columns, as `REWRITE` replaces the whole
    /// record. The statement runs under its own savepoint and stays in place
    /// after a later write of the same task fails: `ACCTDAT` is
    /// `RECOVERY(NONE)`, so the `COACTUPC` account rewrite survives a failed
    /// customer rewrite and its `SYNCPOINT ROLLBACK`
    /// [app/cbl/COACTUPC.cbl:L4082-4104]. A key that names no row is
    /// [`PortError::NotFound`], the `REWRITE` `NOTFND` or `INVALID KEY`.
    async fn rewrite_account(&mut self, account: Account) -> PortResult<()> {
        let status = account.acct_active_status.trimmed();
        let open_date = account.acct_open_date.trimmed();
        let expiraion_date = account.acct_expiraion_date.trimmed();
        let reissue_date = account.acct_reissue_date.trimmed();
        let addr_zip = account.acct_addr_zip.trimmed();
        let group_id = account.acct_group_id.trimmed();
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query!(
            r#"UPDATE accounts
                  SET acct_active_status     = $2,
                      acct_curr_bal          = $3,
                      acct_credit_limit      = $4,
                      acct_cash_credit_limit = $5,
                      acct_open_date         = $6,
                      acct_expiraion_date    = $7,
                      acct_reissue_date      = $8,
                      acct_curr_cyc_credit   = $9,
                      acct_curr_cyc_debit    = $10,
                      acct_addr_zip          = $11,
                      acct_group_id          = $12
                WHERE account_id = $1"#,
            account.acct_id.to_i64(),
            &*status,
            account.acct_curr_bal.to_decimal(),
            account.acct_credit_limit.to_decimal(),
            account.acct_cash_credit_limit.to_decimal(),
            &*open_date,
            &*expiraion_date,
            &*reissue_date,
            account.acct_curr_cyc_credit.to_decimal(),
            account.acct_curr_cyc_debit.to_decimal(),
            &*addr_zip,
            &*group_id
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

/// The batch read the ports do not declare: only `batch creastmt` calls it.
impl PgStore<'_, '_> {
    /// CBSTM03B 4000-ACCTFILE-PROC — P-CBSTM03B-005 [app/cbl/CBSTM03B.CBL:L206-229]
    /// for CBSTM03A 3000-ACCTFILE-GET — P-CBSTM03A-1259
    /// [app/cbl/CBSTM03A.CBL:L392-414]: the keyed `READ` of `ACCTFILE` for
    /// each of `ids`, all in one statement. No lock is taken; the statement
    /// runs under its own savepoint, as every store statement does.
    ///
    /// Each entry is what [`AccountReader::read_account`] returns for its ID:
    /// the record; [`PortError::NotFound`] when no account has the ID; or
    /// [`PortError::Unavailable`] when the stored row does not fit its PIC
    /// clause, which fails that ID alone. No statement runs when `ids` is
    /// empty.
    ///
    /// # Errors
    ///
    /// The [`PortError`] of the statement when it fails as a whole; no entry is
    /// returned then.
    pub async fn read_accounts(
        &mut self,
        ids: BTreeSet<AccountId>,
    ) -> PortResult<BTreeMap<AccountId, PortResult<Account>>> {
        let keys: Vec<i64> = ids.iter().copied().map(AccountId::to_i64).collect();
        let rows = if keys.is_empty() {
            Vec::new()
        } else {
            let mut sp = savepoint(self.tx, &mut self.ledger).await?;
            let result = sqlx::query_as!(
                AccountRow,
                r#"SELECT account_id, acct_active_status, acct_curr_bal, acct_credit_limit,
                          acct_cash_credit_limit, acct_open_date, acct_expiraion_date,
                          acct_reissue_date, acct_curr_cyc_credit, acct_curr_cyc_debit,
                          acct_addr_zip, acct_group_id
                     FROM accounts
                    WHERE account_id = ANY($1)"#,
                keys.as_slice()
            )
            .fetch_all(&mut *sp)
            .await;
            settle(sp, result).await?
        };
        let binds = ids.into_iter().map(|id| (id, Ok(id.to_i64()))).collect();
        Ok(keyed_records(
            binds,
            rows,
            |row| row.account_id,
            Account::try_from,
        ))
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::Account;
    use carddemo_domain::numeric::{AccountGroupId, AccountId, Alnum, Money, NumericError};
    use rust_decimal::Decimal;

    use super::{AccountRow, PersistenceError, PortError, TABLE, found};

    /// The row `batch seed` stores for the first `acctdata.txt` record
    /// [app/data/ASCII/acctdata.txt:L1]: text right-trimmed, the blank group ID
    /// as `''` (E-06).
    fn first_fixture_row() -> AccountRow {
        AccountRow {
            account_id: 1,
            acct_active_status: "Y".to_owned(),
            acct_curr_bal: Decimal::new(19_400, 2),
            acct_credit_limit: Decimal::new(202_000, 2),
            acct_cash_credit_limit: Decimal::new(102_000, 2),
            acct_open_date: "2014-11-20".to_owned(),
            acct_expiraion_date: "2025-05-20".to_owned(),
            acct_reissue_date: "2025-05-20".to_owned(),
            acct_curr_cyc_credit: Decimal::new(0, 2),
            acct_curr_cyc_debit: Decimal::new(0, 2),
            acct_addr_zip: "A000000000".to_owned(),
            acct_group_id: String::new(),
        }
    }

    /// The `CVACT01Y` record the codec decodes from the same fixture line.
    fn first_fixture_account() -> Result<Account, NumericError> {
        Ok(Account {
            acct_id: AccountId::new(1)?,
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::from_cents(19_400)?,
            acct_credit_limit: Money::from_cents(202_000)?,
            acct_cash_credit_limit: Money::from_cents(102_000)?,
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        })
    }

    #[test]
    fn a_stored_row_reads_back_as_its_fixture_record() -> Result<(), NumericError> {
        let account = found(Some(first_fixture_row()));
        assert_eq!(account, Ok(first_fixture_account()?));
        let group = account.map(|record| *record.acct_group_id.as_bytes());
        assert_eq!(group, Ok([b' '; 10]), "the blank group ID is ten spaces");
        Ok(())
    }

    #[test]
    fn no_row_is_not_found() {
        assert_eq!(found(None), Err(PortError::NotFound));
    }

    #[test]
    fn the_rewrite_binds_read_back_as_the_same_record() -> Result<(), NumericError> {
        let account = first_fixture_account()?;
        let row = AccountRow {
            account_id: account.acct_id.to_i64(),
            acct_active_status: account.acct_active_status.trimmed().into_owned(),
            acct_curr_bal: account.acct_curr_bal.to_decimal(),
            acct_credit_limit: account.acct_credit_limit.to_decimal(),
            acct_cash_credit_limit: account.acct_cash_credit_limit.to_decimal(),
            acct_open_date: account.acct_open_date.trimmed().into_owned(),
            acct_expiraion_date: account.acct_expiraion_date.trimmed().into_owned(),
            acct_reissue_date: account.acct_reissue_date.trimmed().into_owned(),
            acct_curr_cyc_credit: account.acct_curr_cyc_credit.to_decimal(),
            acct_curr_cyc_debit: account.acct_curr_cyc_debit.to_decimal(),
            acct_addr_zip: account.acct_addr_zip.trimmed().into_owned(),
            acct_group_id: account.acct_group_id.trimmed().into_owned(),
        };
        assert_eq!(row.acct_group_id, "", "a blank group ID is stored as ''");
        assert_eq!(found(Some(row)), Ok(account));
        Ok(())
    }

    /// A change that puts one column of a row outside its picture.
    type Spoil = fn(&mut AccountRow);

    #[test]
    fn a_value_outside_its_picture_is_corrupt_and_names_its_column() {
        let cases: [(&str, Spoil); 12] = [
            ("account_id", |row| {
                row.account_id = 100_000_000_000;
            }),
            ("acct_active_status", |row| {
                row.acct_active_status = "YN".to_owned();
            }),
            ("acct_curr_bal", |row| {
                row.acct_curr_bal = Decimal::new(1, 3);
            }),
            ("acct_credit_limit", |row| {
                row.acct_credit_limit = Decimal::new(-10_000_000_000, 0);
            }),
            ("acct_cash_credit_limit", |row| {
                row.acct_cash_credit_limit = Decimal::new(10_000_000_000, 0);
            }),
            ("acct_open_date", |row| {
                row.acct_open_date = "2014-11-200".to_owned();
            }),
            ("acct_expiraion_date", |row| {
                row.acct_expiraion_date = "2025-05-2\u{20AC}".to_owned();
            }),
            ("acct_reissue_date", |row| {
                row.acct_reissue_date = "2025-05-20 X".to_owned();
            }),
            ("acct_curr_cyc_credit", |row| {
                row.acct_curr_cyc_credit = Decimal::new(5, 3);
            }),
            ("acct_curr_cyc_debit", |row| {
                row.acct_curr_cyc_debit = Decimal::new(-100_000_000_000_000, 4);
            }),
            ("acct_addr_zip", |row| {
                row.acct_addr_zip = "A0000000000".to_owned();
            }),
            ("acct_group_id", |row| {
                row.acct_group_id = "DEFAULTGRP1".to_owned();
            }),
        ];
        for (column, spoil) in cases {
            let mut row = first_fixture_row();
            spoil(&mut row);
            match Account::try_from(row) {
                Err(PersistenceError::Corrupt {
                    table,
                    column: named,
                    ..
                }) => assert_eq!((table, named), (TABLE, column)),
                other => panic!("{column}: expected a Corrupt error, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_corrupt_row_reaches_the_port_as_unavailable() {
        let mut row = first_fixture_row();
        row.acct_curr_bal = Decimal::new(1, 3);
        let expected = "stored accounts.acct_curr_bal value does not fit its domain type: \
                        value has more decimal places than the field allows";
        assert_eq!(
            found(Some(row)),
            Err(PortError::Unavailable(expected.to_owned()))
        );
    }
}
