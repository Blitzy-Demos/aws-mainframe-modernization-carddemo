//! The account record of the VSAM `ACCTDAT` file, `CVACT01Y` `ACCOUNT-RECORD`.
//!
//! Every field takes the type section A of `docs/parity/parity-oracle-audit.md`
//! gives its PIC clause (rows P-CVACT01Y-001 to P-CVACT01Y-013) and keeps the
//! copybook name lower-snake-cased, prefix and spelling included. The SQL table is
//! `accounts`, whose columns carry the same names except the key, `account_id`.
//! The record is pure data: the rules that read and change it live in
//! `crate::rules`.

use crate::numeric::{AccountGroupId, AccountId, Alnum, Money};

/// `CVACT01Y` `ACCOUNT-RECORD` [app/cpy/CVACT01Y.cpy:L4-17]: one 300-byte record
/// of the `ACCTDAT` KSDS, keyed by `ACCT-ID`.
///
/// The fields follow the copybook order. The trailing `FILLER X(178)`
/// (P-CVACT01Y-013) [app/cpy/CVACT01Y.cpy:L17] is not modelled: the codec
/// layout keeps its bytes raw, and no table stores it.
///
/// The three dates are `X(10)` text, as in the COBOL, so their comparisons are
/// the COBOL alphanumeric comparisons, for example the `CBTRN02C` expiry test of
/// `ACCT-EXPIRAION-DATE` against the transaction timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `ACCT-ID PIC 9(11)` [app/cpy/CVACT01Y.cpy:L5] — P-CVACT01Y-001.
    ///
    /// The record key; SQL `account_id BIGINT`.
    pub acct_id: AccountId,
    /// `ACCT-ACTIVE-STATUS PIC X(01)` [app/cpy/CVACT01Y.cpy:L6] — P-CVACT01Y-002.
    ///
    /// `Y` in every fixture record (P-CVACT01Y-017); SQL
    /// `acct_active_status VARCHAR(1)`.
    pub acct_active_status: Alnum<1>,
    /// `ACCT-CURR-BAL PIC S9(10)V99` [app/cpy/CVACT01Y.cpy:L7] — P-CVACT01Y-003.
    ///
    /// SQL `acct_curr_bal NUMERIC(12,2)`.
    pub acct_curr_bal: Money<10>,
    /// `ACCT-CREDIT-LIMIT PIC S9(10)V99` [app/cpy/CVACT01Y.cpy:L8] —
    /// P-CVACT01Y-004.
    ///
    /// SQL `acct_credit_limit NUMERIC(12,2)`.
    pub acct_credit_limit: Money<10>,
    /// `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99` [app/cpy/CVACT01Y.cpy:L9] —
    /// P-CVACT01Y-005.
    ///
    /// SQL `acct_cash_credit_limit NUMERIC(12,2)`.
    pub acct_cash_credit_limit: Money<10>,
    /// `ACCT-OPEN-DATE PIC X(10)` [app/cpy/CVACT01Y.cpy:L10] — P-CVACT01Y-006.
    ///
    /// SQL `acct_open_date VARCHAR(10)`.
    pub acct_open_date: Alnum<10>,
    /// `ACCT-EXPIRAION-DATE PIC X(10)` [app/cpy/CVACT01Y.cpy:L11] —
    /// P-CVACT01Y-007.
    ///
    /// The copybook's spelling is kept; SQL `acct_expiraion_date VARCHAR(10)`.
    pub acct_expiraion_date: Alnum<10>,
    /// `ACCT-REISSUE-DATE PIC X(10)` [app/cpy/CVACT01Y.cpy:L12] — P-CVACT01Y-008.
    ///
    /// SQL `acct_reissue_date VARCHAR(10)`.
    pub acct_reissue_date: Alnum<10>,
    /// `ACCT-CURR-CYC-CREDIT PIC S9(10)V99` [app/cpy/CVACT01Y.cpy:L13] —
    /// P-CVACT01Y-009.
    ///
    /// SQL `acct_curr_cyc_credit NUMERIC(12,2)`.
    pub acct_curr_cyc_credit: Money<10>,
    /// `ACCT-CURR-CYC-DEBIT PIC S9(10)V99` [app/cpy/CVACT01Y.cpy:L14] —
    /// P-CVACT01Y-010.
    ///
    /// SQL `acct_curr_cyc_debit NUMERIC(12,2)`.
    pub acct_curr_cyc_debit: Money<10>,
    /// `ACCT-ADDR-ZIP PIC X(10)` [app/cpy/CVACT01Y.cpy:L15] — P-CVACT01Y-011.
    ///
    /// Every fixture record holds `A000000000` here, read as the ZIP its offset
    /// says (E-06, P-CVACT01Y-019); SQL `acct_addr_zip VARCHAR(10)`.
    pub acct_addr_zip: Alnum<10>,
    /// `ACCT-GROUP-ID PIC X(10)` [app/cpy/CVACT01Y.cpy:L16] — P-CVACT01Y-012.
    ///
    /// The disclosure group: `CBACT04C` moves it to the `DIS-ACCT-GROUP-ID` key
    /// of the interest-rate read [app/cbl/CBACT04C.cbl:L210] and reads group
    /// `DEFAULT` when that key is not found [app/cbl/CBACT04C.cbl:L436-439]. It
    /// is blank in every fixture record (E-06, P-CVACT01Y-019), so the fixture
    /// accounts take the `DEFAULT` rates. SQL `acct_group_id VARCHAR(10)`.
    ///
    /// Row P-CVACT01Y-012 names the type `Alnum<10>`. The field is typed
    /// [`AccountGroupId`], the `X(10)` key type, because it is a lookup key; the
    /// key holds exactly that `Alnum<10>` ([`AccountGroupId::as_alnum`]), so the
    /// width and bytes are the ones the row fixes.
    pub acct_group_id: AccountGroupId,
}

#[cfg(test)]
mod tests {
    use super::Account;
    use crate::numeric::{AccountGroupId, AccountId, Alnum, Money, NumericError};

    /// The first `acctdata.txt` record [app/data/ASCII/acctdata.txt:L1], as its
    /// layout decodes it (P-CVACT01Y-015).
    fn first_fixture_account() -> Result<Account, NumericError> {
        Ok(Account {
            acct_id: AccountId::try_from(1_i64)?,
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::<10>::from_cents(19_400)?,
            acct_credit_limit: Money::<10>::from_cents(202_000)?,
            acct_cash_credit_limit: Money::<10>::from_cents(102_000)?,
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::<10>::ZERO,
            acct_curr_cyc_debit: Money::<10>::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        })
    }

    #[test]
    fn a_clone_equals_the_original_until_its_balance_changes() -> Result<(), NumericError> {
        let account = first_fixture_account()?;
        let mut copy = account.clone();
        assert_eq!(copy, account);

        copy.acct_curr_bal = Money::<10>::from_cents(19_401)?;
        assert_ne!(copy, account);
        Ok(())
    }

    #[test]
    fn the_record_holds_the_fixture_field_values() -> Result<(), NumericError> {
        let account = first_fixture_account()?;
        assert_eq!(account.acct_id.to_string(), "00000000001");
        assert_eq!(account.acct_curr_bal.to_string(), "194.00");
        assert_eq!(account.acct_expiraion_date.as_bytes(), b"2025-05-20");
        assert_eq!(account.acct_group_id.as_bytes(), &[b' '; 10]);
        Ok(())
    }
}
