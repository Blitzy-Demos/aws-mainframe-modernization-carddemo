//! The customer master: VSAM `CUSTDAT`, `CVCUS01Y` `CUSTOMER-RECORD` (RECLN 500,
//! `KEYS(9 0)` [app/jcl/CUSTFILE.jcl:L50-51]), on the table `customers` of
//! `migrations/0002_master_data.sql`.
//!
//! | Access | COBOL | SQL |
//! |---|---|---|
//! | [`CustomerReader::read_customer`] | CICS `READ` by `CUST-ID`, or the batch keyed `READ`, in `COACTVWC`, `COACTUPC`, `COPAUA0C`, `COPAUS0C` and `CBSTM03B` | `SELECT ... FROM customers WHERE customer_id = $1` |
//! | [`CustomerReader::read_customer_for_update`] | CICS `READ ... UPDATE` in `COACTUPC` `9600-WRITE-PROCESSING` | the same `SELECT ... FOR UPDATE`, under a savepoint |
//! | [`CustomerWriter::rewrite_customer`] | CICS `REWRITE` in `COACTUPC` `9600-WRITE-PROCESSING` | `UPDATE customers SET <every non-key column> WHERE customer_id = $1`, under a savepoint |
//! | [`PgStore::read_customers`] | the `CBSTM03B` keyed `READ` of each customer the `CBSTM03A` cross-references name | one `SELECT ... FROM customers WHERE customer_id = ANY($1)` |
//!
//! # Columns
//!
//! One column per elementary item of `CVCUS01Y` [app/cpy/CVCUS01Y.cpy:L5-22], the
//! copybook name lower-snake-cased, as the reviewed section A rows P-CVCUS01Y-001
//! to P-CVCUS01Y-018 of `docs/parity/parity-oracle-audit.md` fix them. The key
//! `CUST-ID 9(09)` is `customer_id INTEGER`; `CUST-SSN 9(09)` and
//! `CUST-FICO-CREDIT-SCORE 9(03)` are `INTEGER`; every `X(n)` item is a
//! right-trimmed `VARCHAR(n)`. The trailing `FILLER X(168)` (P-CVCUS01Y-019) is not
//! stored.
//!
//! A stored value that does not fit its PIC clause (a number of too many digits,
//! a text longer than its `X(n)`) is `PersistenceError::Corrupt`, which the port
//! reports as `PortError::Unavailable`. It is never cut, and the error names the
//! column, never the value, which may be a social security number.
//!
//! # Writes
//!
//! `CUSTDAT` is `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L59]. `COACTUPC` rewrites
//! the account and then the customer, and a failed customer rewrite leaves the
//! account rewrite in place (P-COACTUPC-014) [app/cbl/COACTUPC.cbl:L4060-4102].
//! The rewrite, the locking read and the read each run under their own
//! savepoint through `crate::tx::savepoint` and `crate::tx::settle`: a failure
//! rolls back that statement alone and leaves the caller's transaction usable.

use std::collections::{BTreeMap, BTreeSet};

use carddemo_domain::entities::Customer;
use carddemo_domain::numeric::CustomerId;
use carddemo_domain::ports::{CustomerReader, CustomerWriter, PortError, PortResult};

use super::{alnum, cust_ssn, customer_id, expect_affected, fico_score, keyed_records};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table every statement of this module names, and every `Corrupt` error.
const TABLE: &str = "customers";

/// One `customers` row as the `SELECT`s return it, every column `NOT NULL`.
///
/// No `Debug`: the row holds the social security number in the clear, and
/// [`Customer`]'s own `Debug` is the redacting one.
struct CustomerRow {
    customer_id: i32,
    cust_first_name: String,
    cust_middle_name: String,
    cust_last_name: String,
    cust_addr_line_1: String,
    cust_addr_line_2: String,
    cust_addr_line_3: String,
    cust_addr_state_cd: String,
    cust_addr_country_cd: String,
    cust_addr_zip: String,
    cust_phone_num_1: String,
    cust_phone_num_2: String,
    cust_ssn: i32,
    cust_govt_issued_id: String,
    cust_dob_yyyy_mm_dd: String,
    cust_eft_account_id: String,
    cust_pri_card_holder_ind: String,
    cust_fico_credit_score: i32,
}

/// The `CUSTOMER-RECORD` a stored row stands for, each column held to its PIC
/// clause (P-CVCUS01Y-001 to P-CVCUS01Y-018) [app/cpy/CVCUS01Y.cpy:L5-22].
///
/// Text columns get their trailing spaces back, so every field holds the bytes
/// the COBOL record holds.
impl TryFrom<CustomerRow> for Customer {
    type Error = PersistenceError;

    fn try_from(row: CustomerRow) -> Result<Self, Self::Error> {
        Ok(Self {
            cust_id: customer_id(row.customer_id, TABLE, "customer_id")?,
            cust_first_name: alnum(&row.cust_first_name, TABLE, "cust_first_name")?,
            cust_middle_name: alnum(&row.cust_middle_name, TABLE, "cust_middle_name")?,
            cust_last_name: alnum(&row.cust_last_name, TABLE, "cust_last_name")?,
            cust_addr_line_1: alnum(&row.cust_addr_line_1, TABLE, "cust_addr_line_1")?,
            cust_addr_line_2: alnum(&row.cust_addr_line_2, TABLE, "cust_addr_line_2")?,
            cust_addr_line_3: alnum(&row.cust_addr_line_3, TABLE, "cust_addr_line_3")?,
            cust_addr_state_cd: alnum(&row.cust_addr_state_cd, TABLE, "cust_addr_state_cd")?,
            cust_addr_country_cd: alnum(&row.cust_addr_country_cd, TABLE, "cust_addr_country_cd")?,
            cust_addr_zip: alnum(&row.cust_addr_zip, TABLE, "cust_addr_zip")?,
            cust_phone_num_1: alnum(&row.cust_phone_num_1, TABLE, "cust_phone_num_1")?,
            cust_phone_num_2: alnum(&row.cust_phone_num_2, TABLE, "cust_phone_num_2")?,
            cust_ssn: cust_ssn(row.cust_ssn, TABLE, "cust_ssn")?,
            cust_govt_issued_id: alnum(&row.cust_govt_issued_id, TABLE, "cust_govt_issued_id")?,
            cust_dob_yyyy_mm_dd: alnum(&row.cust_dob_yyyy_mm_dd, TABLE, "cust_dob_yyyy_mm_dd")?,
            cust_eft_account_id: alnum(&row.cust_eft_account_id, TABLE, "cust_eft_account_id")?,
            cust_pri_card_holder_ind: alnum(
                &row.cust_pri_card_holder_ind,
                TABLE,
                "cust_pri_card_holder_ind",
            )?,
            cust_fico_credit_score: fico_score(
                row.cust_fico_credit_score,
                TABLE,
                "cust_fico_credit_score",
            )?,
        })
    }
}

/// An unsigned `9(09)` value, `CUST-ID` or `CUST-SSN`, as the `INTEGER` its
/// column binds.
///
/// Every such value is below `10^9` and fits; the checked conversion keeps the
/// bind free of an `as` cast and of a panic. A value that did not fit would be
/// [`PortError::Unavailable`] naming the column, never the value.
fn integer(value: i64, column: &'static str) -> PortResult<i32> {
    i32::try_from(value).map_err(|_| {
        PortError::Unavailable(format!("{TABLE}.{column} value exceeds the INTEGER range"))
    })
}

impl CustomerReader for PgStore<'_, '_> {
    /// CICS `READ DATASET('CUSTDAT')` by the 9-byte customer key, and the batch
    /// keyed `READ` of `CUSTFILE`. No lock is taken; the read runs under its
    /// own savepoint, so a failure leaves the caller's transaction usable.
    ///
    /// - `COACTVWC` `9400-GETCUSTDATA-BYCUST` — P-COACTVWC-010
    ///   [app/cbl/COACTVWC.cbl:L825-870]
    /// - `COACTUPC` `9400-GETCUSTDATA-BYCUST` — P-COACTUPC-045
    ///   [app/cbl/COACTUPC.cbl:L3752-3796]
    /// - `COPAUA0C` `5300-READ-CUST-RECORD` — P-COPAUA0C-007
    ///   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L568-610]
    /// - `COPAUS0C` `GETCUSTDATA-BYCUST` — P-COPAUS0C-006, P-COPAUS0C-008
    ///   [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L915-963]
    /// - `CBSTM03A` `2000-CUSTFILE-GET` through `CBSTM03B` `3000-CUSTFILE-PROC`
    ///   — P-CBSTM03A-1258, P-CBSTM03B-004 [app/cbl/CBSTM03B.CBL:L181-204]
    ///
    /// A missing key is [`PortError::NotFound`], the `NOTFND` (status 23) each
    /// program branches on; any other failure, a stored value that does not fit
    /// its PIC clause included, is [`PortError::Unavailable`].
    async fn read_customer(&mut self, id: CustomerId) -> PortResult<Customer> {
        let key = integer(id.to_i64(), "customer_id")?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            CustomerRow,
            "SELECT customer_id, cust_first_name, cust_middle_name, cust_last_name, \
                    cust_addr_line_1, cust_addr_line_2, cust_addr_line_3, \
                    cust_addr_state_cd, cust_addr_country_cd, cust_addr_zip, \
                    cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
                    cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
                    cust_fico_credit_score \
             FROM customers \
             WHERE customer_id = $1",
            key,
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        Customer::try_from(row).map_err(PortError::from)
    }

    /// CICS `READ FILE('CUSTDAT') UPDATE`: `COACTUPC` `9600-WRITE-PROCESSING`
    /// locks the customer after the account, before its change check and the
    /// two rewrites — P-COACTUPC-011 [app/cbl/COACTUPC.cbl:L3916-3942].
    ///
    /// `SELECT ... FOR UPDATE` under its own savepoint. Releasing the savepoint
    /// keeps the row lock, so the customer stays locked until the caller's
    /// transaction ends, as the CICS lock lasts until the task's syncpoint. The
    /// statement waits for a lock another unit of work holds, as `READ UPDATE`
    /// waits.
    ///
    /// A missing key is [`PortError::NotFound`]; a lock wait that times out or
    /// deadlocks is [`PortError::Contention`]; `COACTUPC` answers both with its
    /// could-not-lock outcome. Any other failure is [`PortError::Unavailable`].
    async fn read_customer_for_update(&mut self, id: CustomerId) -> PortResult<Customer> {
        let key = integer(id.to_i64(), "customer_id")?;
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            CustomerRow,
            "SELECT customer_id, cust_first_name, cust_middle_name, cust_last_name, \
                    cust_addr_line_1, cust_addr_line_2, cust_addr_line_3, \
                    cust_addr_state_cd, cust_addr_country_cd, cust_addr_zip, \
                    cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
                    cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
                    cust_fico_credit_score \
             FROM customers \
             WHERE customer_id = $1 \
             FOR UPDATE",
            key,
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        Customer::try_from(row).map_err(PortError::from)
    }
}

impl CustomerWriter for PgStore<'_, '_> {
    /// CICS `REWRITE FILE('CUSTDAT') FROM(CUST-UPDATE-RECORD)`, the second write
    /// of `COACTUPC` `9600-WRITE-PROCESSING` — P-COACTUPC-014
    /// [app/cbl/COACTUPC.cbl:L4082-4104].
    ///
    /// Every non-key column is set from `customer`, keyed by its `cust_id`, as
    /// the `REWRITE` replaces the whole record. The statement runs under its own
    /// savepoint: when it fails, only this rewrite is undone, and the account
    /// rewrite before it stays in place, as `RECOVERY(NONE)` keeps it
    /// [app/csd/CARDDEMO.CSD:L59].
    ///
    /// No row with the key is [`PortError::NotFound`]; any other failure, such
    /// as a text the column constraints reject, is [`PortError::Unavailable`].
    async fn rewrite_customer(&mut self, customer: Customer) -> PortResult<()> {
        let key = integer(customer.cust_id.to_i64(), "customer_id")?;
        let ssn = integer(customer.cust_ssn.to_i64(), "cust_ssn")?;
        let fico = i32::from(customer.cust_fico_credit_score.get());
        let first_name = customer.cust_first_name.trimmed();
        let middle_name = customer.cust_middle_name.trimmed();
        let last_name = customer.cust_last_name.trimmed();
        let addr_line_1 = customer.cust_addr_line_1.trimmed();
        let addr_line_2 = customer.cust_addr_line_2.trimmed();
        let addr_line_3 = customer.cust_addr_line_3.trimmed();
        let state_cd = customer.cust_addr_state_cd.trimmed();
        let country_cd = customer.cust_addr_country_cd.trimmed();
        let zip = customer.cust_addr_zip.trimmed();
        let phone_num_1 = customer.cust_phone_num_1.trimmed();
        let phone_num_2 = customer.cust_phone_num_2.trimmed();
        let govt_issued_id = customer.cust_govt_issued_id.trimmed();
        let dob = customer.cust_dob_yyyy_mm_dd.trimmed();
        let eft_account_id = customer.cust_eft_account_id.trimmed();
        let pri_card_holder_ind = customer.cust_pri_card_holder_ind.trimmed();

        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "UPDATE customers \
             SET cust_first_name = $2, \
                 cust_middle_name = $3, \
                 cust_last_name = $4, \
                 cust_addr_line_1 = $5, \
                 cust_addr_line_2 = $6, \
                 cust_addr_line_3 = $7, \
                 cust_addr_state_cd = $8, \
                 cust_addr_country_cd = $9, \
                 cust_addr_zip = $10, \
                 cust_phone_num_1 = $11, \
                 cust_phone_num_2 = $12, \
                 cust_ssn = $13, \
                 cust_govt_issued_id = $14, \
                 cust_dob_yyyy_mm_dd = $15, \
                 cust_eft_account_id = $16, \
                 cust_pri_card_holder_ind = $17, \
                 cust_fico_credit_score = $18 \
             WHERE customer_id = $1",
            key,
            first_name.as_ref(),
            middle_name.as_ref(),
            last_name.as_ref(),
            addr_line_1.as_ref(),
            addr_line_2.as_ref(),
            addr_line_3.as_ref(),
            state_cd.as_ref(),
            country_cd.as_ref(),
            zip.as_ref(),
            phone_num_1.as_ref(),
            phone_num_2.as_ref(),
            ssn,
            govt_issued_id.as_ref(),
            dob.as_ref(),
            eft_account_id.as_ref(),
            pri_card_holder_ind.as_ref(),
            fico,
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

/// The batch read the ports do not declare: only `batch creastmt` calls it.
impl PgStore<'_, '_> {
    /// CBSTM03B 3000-CUSTFILE-PROC — P-CBSTM03B-004 [app/cbl/CBSTM03B.CBL:L181-204]
    /// for CBSTM03A 2000-CUSTFILE-GET — P-CBSTM03A-1258
    /// [app/cbl/CBSTM03A.CBL:L368-390]: the keyed `READ` of `CUSTFILE` for
    /// each of `ids`, all in one statement. No lock is taken; the statement
    /// runs under its own savepoint, as every store statement does.
    ///
    /// Each entry is what [`CustomerReader::read_customer`] returns for its ID:
    /// the record; [`PortError::NotFound`] when no customer has the ID; or
    /// [`PortError::Unavailable`] when the stored row does not fit its PIC
    /// clause, which fails that ID alone. No statement runs when `ids` is
    /// empty.
    ///
    /// # Errors
    ///
    /// The [`PortError`] of the statement when it fails as a whole; no entry is
    /// returned then.
    pub async fn read_customers(
        &mut self,
        ids: BTreeSet<CustomerId>,
    ) -> PortResult<BTreeMap<CustomerId, PortResult<Customer>>> {
        let binds: BTreeMap<CustomerId, PortResult<i32>> = ids
            .into_iter()
            .map(|id| (id, integer(id.to_i64(), "customer_id")))
            .collect();
        let keys: Vec<i32> = binds
            .values()
            .filter_map(|bind| bind.as_ref().ok().copied())
            .collect();
        let rows = if keys.is_empty() {
            Vec::new()
        } else {
            let mut sp = savepoint(self.tx).await?;
            let result = sqlx::query_as!(
                CustomerRow,
                "SELECT customer_id, cust_first_name, cust_middle_name, cust_last_name, \
                        cust_addr_line_1, cust_addr_line_2, cust_addr_line_3, \
                        cust_addr_state_cd, cust_addr_country_cd, cust_addr_zip, \
                        cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
                        cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
                        cust_fico_credit_score \
                 FROM customers \
                 WHERE customer_id = ANY($1)",
                keys.as_slice(),
            )
            .fetch_all(&mut *sp)
            .await;
            settle(sp, result).await?
        };
        Ok(keyed_records(
            binds,
            rows,
            |row| row.customer_id,
            Customer::try_from,
        ))
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::Customer;
    use carddemo_domain::numeric::{Alnum, CustSsn, CustomerId, FicoScore};
    use carddemo_domain::ports::PortError;

    use super::{CustomerRow, integer};
    use crate::PersistenceError;

    /// The stored row of the first fixture customer
    /// [app/data/ASCII/custdata.txt:L1] (P-CVCUS01Y-021), its text right-trimmed
    /// as the seed stores it.
    fn first_fixture_row() -> CustomerRow {
        CustomerRow {
            customer_id: 1,
            cust_first_name: "Immanuel".to_owned(),
            cust_middle_name: "Madeline".to_owned(),
            cust_last_name: "Kessler".to_owned(),
            cust_addr_line_1: "618 Deshaun Route".to_owned(),
            cust_addr_line_2: "Apt. 802".to_owned(),
            cust_addr_line_3: "Altenwerthshire".to_owned(),
            cust_addr_state_cd: "NC".to_owned(),
            cust_addr_country_cd: "USA".to_owned(),
            cust_addr_zip: "12546".to_owned(),
            cust_phone_num_1: "(908)119-8310".to_owned(),
            cust_phone_num_2: "(373)693-8684".to_owned(),
            cust_ssn: 20_973_888,
            cust_govt_issued_id: "00000000000049368437".to_owned(),
            cust_dob_yyyy_mm_dd: "1961-06-08".to_owned(),
            cust_eft_account_id: "0053581756".to_owned(),
            cust_pri_card_holder_ind: "Y".to_owned(),
            cust_fico_credit_score: 274,
        }
    }

    /// The column a conversion's `Corrupt` error names, checked to be
    /// `customers`, with the reason the domain constructor gave.
    fn corrupt_column(row: CustomerRow) -> (&'static str, String) {
        match Customer::try_from(row) {
            Err(PersistenceError::Corrupt {
                table,
                column,
                reason,
            }) => {
                assert_eq!(table, "customers");
                (column, reason)
            }
            Err(other) => panic!("expected a Corrupt error, got {other}"),
            Ok(customer) => panic!("a misfit row was accepted: {customer:?}"),
        }
    }

    #[test]
    fn a_stored_row_becomes_the_cobol_record_field_by_field() {
        let customer = Customer::try_from(first_fixture_row()).expect("the fixture row fits");
        let expected = Customer {
            cust_id: CustomerId::new(1).expect("1 fits 9(09)"),
            cust_first_name: Alnum::move_from("Immanuel"),
            cust_middle_name: Alnum::move_from("Madeline"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("020973888 fits 9(09)"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("274 fits 9(03)"),
        };
        assert_eq!(customer, expected);
        assert_eq!(customer.cust_id.to_string(), "000000001");
        assert_eq!(customer.cust_ssn.to_string(), "020973888");
        assert_eq!(
            customer.cust_first_name.as_bytes(),
            b"Immanuel                 "
        );
        assert_eq!(customer.cust_addr_zip.as_bytes(), b"12546     ");
    }

    #[test]
    fn a_blank_text_column_comes_back_as_spaces() {
        let mut row = first_fixture_row();
        row.cust_middle_name = String::new();
        row.cust_addr_line_2 = "Apt. 802   ".to_owned();
        let customer = Customer::try_from(row).expect("blank text fits");
        assert!(customer.cust_middle_name.is_blank());
        assert_eq!(customer.cust_addr_line_2.trimmed(), "Apt. 802");
    }

    #[test]
    fn a_value_that_does_not_fit_its_picture_is_corrupt_and_never_cut() {
        type Edit = fn(&mut CustomerRow);
        let cases: [(Edit, &str, &str); 7] = [
            (
                |row| row.customer_id = -1,
                "customer_id",
                "value must not be negative",
            ),
            (
                |row| row.customer_id = 1_000_000_000,
                "customer_id",
                "value is outside the range of the field",
            ),
            (
                |row| row.cust_ssn = 1_234_567_890,
                "cust_ssn",
                "value is outside the range of the field",
            ),
            (
                |row| row.cust_fico_credit_score = 1_000,
                "cust_fico_credit_score",
                "value is outside the range of the field",
            ),
            (
                |row| row.cust_addr_state_cd = "NCX".to_owned(),
                "cust_addr_state_cd",
                "expected at most 2 characters, got 3",
            ),
            (
                |row| row.cust_pri_card_holder_ind = "YN".to_owned(),
                "cust_pri_card_holder_ind",
                "expected at most 1 characters, got 2",
            ),
            (
                |row| row.cust_last_name = "K\u{100}ssler".to_owned(),
                "cust_last_name",
                "value has a character outside ISO 8859-1",
            ),
        ];
        for (edit, column, reason) in cases {
            let mut row = first_fixture_row();
            edit(&mut row);
            assert_eq!(corrupt_column(row), (column, reason.to_owned()), "{column}");
        }
    }

    #[test]
    fn a_misfit_reaches_the_port_without_the_stored_value() {
        let mut row = first_fixture_row();
        row.cust_ssn = 1_234_567_890;
        let error = match Customer::try_from(row) {
            Err(error) => PortError::from(error),
            Ok(customer) => panic!("a ten-digit SSN was accepted: {customer:?}"),
        };
        assert!(matches!(error, PortError::Unavailable(_)), "{error}");
        assert!(error.to_string().contains("customers.cust_ssn"), "{error}");
        assert!(!error.to_string().contains("1234567890"), "{error}");
    }

    #[test]
    fn nine_digit_values_bind_as_integer() {
        for (value, expected) in [
            (0, Ok(0)),
            (999_999_999, Ok(999_999_999)),
            (
                i64::from(i32::MAX) + 1,
                Err(PortError::Unavailable(
                    "customers.cust_ssn value exceeds the INTEGER range".to_owned(),
                )),
            ),
        ] {
            assert_eq!(integer(value, "cust_ssn"), expected, "{value}");
        }
        let id = CustomerId::new(999_999_999).expect("the largest 9(09) value");
        assert_eq!(integer(id.to_i64(), "customer_id"), Ok(999_999_999));
    }
}
