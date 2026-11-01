//! The transaction category balance record of the VSAM file `TCATBALF`.
//!
//! `CVTRA01Y` `TRAN-CAT-BAL-RECORD` [app/cpy/CVTRA01Y.cpy:L4-10], `RECLN` 50,
//! a KSDS keyed on its first 17 bytes, `KEYS(17 0)` [app/jcl/TCATBALF.jcl:L40].
//! The replacement table is `transaction_category_balances`, whose columns are
//! the field names below.
//!
//! The trailing `FILLER PIC X(22)` (P-CVTRA01Y-005) [app/cpy/CVTRA01Y.cpy:L10]
//! is not modelled: it is not stored, and the fixture filler, which holds `0`
//! characters, is kept byte for byte by the codec layout alone.

use crate::numeric::{AccountId, Money, TranCategoryCode, TranTypeCode};

/// `TRAN-CAT-KEY` [app/cpy/CVTRA01Y.cpy:L5-8]: account, transaction type and
/// transaction category, the 17-byte key of `TCATBALF`.
///
/// The fields are declared in key order, so the derived `Ord` is the order in
/// which the KSDS holds its records: by account, then type, then category.
/// The two `9(n)` fields are fixed-width zero-padded digits, whose numeric
/// order is their byte order, and the type code compares by its stored bytes.
/// `CBACT04C` `1000-TCATBALF-GET-NEXT` reads the file sequentially in that
/// order (P-CBACT04C-182) [app/cbl/CBACT04C.cbl:L325-348], and `CBTRN02C`
/// `2700-UPDATE-TCATBAL` reads one record by this key (P-CBTRN02C-015)
/// [app/cbl/CBTRN02C.cbl:L467-499].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TranCatBalanceKey {
    /// `TRANCAT-ACCT-ID PIC 9(11)` (P-CVTRA01Y-001) [app/cpy/CVTRA01Y.cpy:L6].
    pub trancat_acct_id: AccountId,
    /// `TRANCAT-TYPE-CD PIC X(02)` (P-CVTRA01Y-002) [app/cpy/CVTRA01Y.cpy:L7].
    pub trancat_type_cd: TranTypeCode,
    /// `TRANCAT-CD PIC 9(04)` (P-CVTRA01Y-003) [app/cpy/CVTRA01Y.cpy:L8].
    pub trancat_cd: TranCategoryCode,
}

/// `CVTRA01Y` `TRAN-CAT-BAL-RECORD` [app/cpy/CVTRA01Y.cpy:L4-10]: the running
/// balance of one account's transactions of one type and category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranCatBalance {
    /// The group `TRAN-CAT-KEY` [app/cpy/CVTRA01Y.cpy:L5].
    pub key: TranCatBalanceKey,
    /// `TRAN-CAT-BAL PIC S9(09)V99` (P-CVTRA01Y-004) [app/cpy/CVTRA01Y.cpy:L9].
    ///
    /// `CBTRN02C` `2700-UPDATE-TCATBAL` adds `DALYTRAN-AMT` here for every
    /// posted transaction, and creates the record from a zero balance when its
    /// key is missing (status 23) [app/cbl/CBTRN02C.cbl:L440]
    /// [app/cbl/CBTRN02C.cbl:L467-543]. `CBACT04C` `1300-COMPUTE-INTEREST`
    /// computes the monthly interest from it (P-CBACT04C-012). The arithmetic
    /// itself lives in `rules::posting` and `rules::interest`.
    pub tran_cat_bal: Money<9>,
}

#[cfg(test)]
mod tests {
    use super::{TranCatBalance, TranCatBalanceKey};
    use crate::numeric::{AccountId, Money, NumericError, TranCategoryCode, TranTypeCode};

    /// The key of account `acct_id`, type `type_cd` and category `cat_cd`.
    fn key(acct_id: u64, type_cd: &str, cat_cd: u16) -> Result<TranCatBalanceKey, NumericError> {
        Ok(TranCatBalanceKey {
            trancat_acct_id: AccountId::new(acct_id)?,
            trancat_type_cd: TranTypeCode::from_trimmed(type_cd)?,
            trancat_cd: TranCategoryCode::new(cat_cd)?,
        })
    }

    #[test]
    fn keys_sort_by_account_then_type_then_category() -> Result<(), NumericError> {
        let expected = vec![
            key(1, "01", 1)?,
            key(1, "01", 2)?,
            key(1, "02", 1)?,
            key(2, "01", 1)?,
            key(99_999_999_999, "01", 1)?,
        ];
        let mut keys = vec![
            key(2, "01", 1)?,
            key(1, "02", 1)?,
            key(99_999_999_999, "01", 1)?,
            key(1, "01", 2)?,
            key(1, "01", 1)?,
        ];
        keys.sort();
        assert_eq!(keys, expected);
        assert!(key(1, "01", 2)? < key(1, "02", 1)?);
        assert!(key(1, "02", 1)? < key(2, "01", 1)?);
        Ok(())
    }

    #[test]
    fn records_compare_by_key_and_balance_and_clone_equal() -> Result<(), NumericError> {
        let record = TranCatBalance {
            key: key(1, "01", 1)?,
            tran_cat_bal: Money::from_cents(116_487)?,
        };
        assert_eq!(record.clone(), record);

        let other_balance = TranCatBalance {
            tran_cat_bal: Money::from_cents(-7_077)?,
            ..record.clone()
        };
        assert_ne!(other_balance, record);

        let other_key = TranCatBalance {
            key: key(1, "03", 1)?,
            ..record.clone()
        };
        assert_ne!(other_key, record);
        Ok(())
    }
}
