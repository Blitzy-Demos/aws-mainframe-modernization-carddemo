//! The transaction master record: `CVTRA05Y TRAN-RECORD`, 350 bytes
//! [app/cpy/CVTRA05Y.cpy:L4-18], the record of the VSAM KSDS `TRANSACT`.
//!
//! Every field takes the type its reviewed section A row of
//! `docs/parity/parity-oracle-audit.md` names (P-CVTRA05Y-001 to
//! P-CVTRA05Y-013) and the name of its `transactions` column, the copybook name
//! in lower snake case. The trailing `FILLER X(20)` (P-CVTRA05Y-014)
//! [app/cpy/CVTRA05Y.cpy:L18] is not modelled: no column stores it, posting and
//! interest never set it (E-25), and the codec layout keeps its bytes.
//!
//! The record exists in two shapes because the replacement separates a row's
//! identity from its COBOL key (E-09):
//!
//! - [`NewTransaction`] is the record content alone: what CT02, CB00, POSTTRAN
//!   and INTCALC write, and what a backup or combined generation holds.
//! - [`Transaction`] is a stored row: the same content plus the `BIGINT`
//!   identity `transaction_id` that `reward_accruals.transaction_id` references.

use crate::numeric::{
    Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
};

/// The content of one `CVTRA05Y TRAN-RECORD` [app/cpy/CVTRA05Y.cpy:L4-18],
/// without a row identity.
///
/// The fields follow the copybook order. `CBTRN02C`
/// `2000-POST-TRANSACTION` fills them one by one from the daily record and
/// stamps `TRAN-PROC-TS` from the processing clock (P-CBTRN02C-013)
/// [app/cbl/CBTRN02C.cbl:L424-442].
///
/// Both timestamps stay text, so the COBOL alphanumeric comparisons and the
/// order of the alternate index on `TRAN-PROC-TS`, `KEYS(26 304)`
/// [app/jcl/TRANFILE.jcl:L84], are byte-exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTransaction {
    /// `TRAN-ID PIC X(16)`, offset 0 (P-CVTRA05Y-001)
    /// [app/cpy/CVTRA05Y.cpy:L5]: the KSDS key, column `tran_id`. It is unique
    /// among live rows only, and is separate from the row identity (E-09).
    pub tran_id: TranId,
    /// `TRAN-TYPE-CD PIC X(02)`, offset 16 (P-CVTRA05Y-002)
    /// [app/cpy/CVTRA05Y.cpy:L6]: column `tran_type_cd`.
    pub tran_type_cd: TranTypeCode,
    /// `TRAN-CAT-CD PIC 9(04)`, offset 18 (P-CVTRA05Y-003)
    /// [app/cpy/CVTRA05Y.cpy:L7]: column `tran_cat_cd`.
    pub tran_cat_cd: TranCategoryCode,
    /// `TRAN-SOURCE PIC X(10)`, offset 22 (P-CVTRA05Y-004)
    /// [app/cpy/CVTRA05Y.cpy:L8]: column `tran_source`.
    pub tran_source: Alnum<10>,
    /// `TRAN-DESC PIC X(100)`, offset 32 (P-CVTRA05Y-005)
    /// [app/cpy/CVTRA05Y.cpy:L9]: column `tran_desc`.
    pub tran_desc: Alnum<100>,
    /// `TRAN-AMT PIC S9(09)V99`, offset 132 (P-CVTRA05Y-006)
    /// [app/cpy/CVTRA05Y.cpy:L10]: column `tran_amt`, `NUMERIC(11,2)`.
    pub tran_amt: Money<9>,
    /// `TRAN-MERCHANT-ID PIC 9(09)`, offset 143 (P-CVTRA05Y-007)
    /// [app/cpy/CVTRA05Y.cpy:L11]: column `tran_merchant_id`.
    pub tran_merchant_id: MerchantId,
    /// `TRAN-MERCHANT-NAME PIC X(50)`, offset 152 (P-CVTRA05Y-008)
    /// [app/cpy/CVTRA05Y.cpy:L12]: column `tran_merchant_name`.
    pub tran_merchant_name: Alnum<50>,
    /// `TRAN-MERCHANT-CITY PIC X(50)`, offset 202 (P-CVTRA05Y-009)
    /// [app/cpy/CVTRA05Y.cpy:L13]: column `tran_merchant_city`.
    pub tran_merchant_city: Alnum<50>,
    /// `TRAN-MERCHANT-ZIP PIC X(10)`, offset 252 (P-CVTRA05Y-010)
    /// [app/cpy/CVTRA05Y.cpy:L14]: column `tran_merchant_zip`.
    pub tran_merchant_zip: Alnum<10>,
    /// `TRAN-CARD-NUM PIC X(16)`, offset 262 (P-CVTRA05Y-011)
    /// [app/cpy/CVTRA05Y.cpy:L15]: column `tran_card_num`.
    pub tran_card_num: CardNumber,
    /// `TRAN-ORIG-TS PIC X(26)`, offset 278 (P-CVTRA05Y-012)
    /// [app/cpy/CVTRA05Y.cpy:L16]: column `tran_orig_ts`, the text as its
    /// writer produced it.
    pub tran_orig_ts: Alnum<26>,
    /// `TRAN-PROC-TS PIC X(26)`, offset 304 (P-CVTRA05Y-013)
    /// [app/cpy/CVTRA05Y.cpy:L17]: column `tran_proc_ts`, the key of the
    /// `TRANIDX` alternate index [app/jcl/TRANFILE.jcl:L84].
    pub tran_proc_ts: Alnum<26>,
}

/// A stored row of the transaction master: a `CVTRA05Y TRAN-RECORD`
/// [app/cpy/CVTRA05Y.cpy:L4-18] with its row identity.
///
/// `transaction_id` is the `BIGINT` identity of the `transactions` table and
/// the target of `reward_accruals.transaction_id`. It is separate from the
/// COBOL key `tran_id`, which a row keeps after `batch tranbkp` takes it out of
/// the live master and which a later add may issue again (E-09).
///
/// The table's `in_master` flag is persistence-only and is not modelled: every
/// legacy read goes through the view `transaction_master`, which holds the live
/// rows alone, so every `Transaction` a rule sees is live.
///
/// Equality compares the identity as well as the content. Two rows with equal
/// content compare equal through [`Transaction::to_new`], which is how
/// `batch combtran` matches a backup record with a non-live row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    /// The row identity, column `transactions.transaction_id`: assigned by the
    /// database and never reused. No copybook defines it (E-09).
    pub transaction_id: TransactionRowId,
    /// `TRAN-ID PIC X(16)` (P-CVTRA05Y-001) [app/cpy/CVTRA05Y.cpy:L5]: the KSDS
    /// key, column `tran_id`, unique among live rows.
    pub tran_id: TranId,
    /// `TRAN-TYPE-CD PIC X(02)` (P-CVTRA05Y-002) [app/cpy/CVTRA05Y.cpy:L6].
    pub tran_type_cd: TranTypeCode,
    /// `TRAN-CAT-CD PIC 9(04)` (P-CVTRA05Y-003) [app/cpy/CVTRA05Y.cpy:L7].
    pub tran_cat_cd: TranCategoryCode,
    /// `TRAN-SOURCE PIC X(10)` (P-CVTRA05Y-004) [app/cpy/CVTRA05Y.cpy:L8].
    pub tran_source: Alnum<10>,
    /// `TRAN-DESC PIC X(100)` (P-CVTRA05Y-005) [app/cpy/CVTRA05Y.cpy:L9].
    pub tran_desc: Alnum<100>,
    /// `TRAN-AMT PIC S9(09)V99` (P-CVTRA05Y-006) [app/cpy/CVTRA05Y.cpy:L10].
    pub tran_amt: Money<9>,
    /// `TRAN-MERCHANT-ID PIC 9(09)` (P-CVTRA05Y-007) [app/cpy/CVTRA05Y.cpy:L11].
    pub tran_merchant_id: MerchantId,
    /// `TRAN-MERCHANT-NAME PIC X(50)` (P-CVTRA05Y-008)
    /// [app/cpy/CVTRA05Y.cpy:L12].
    pub tran_merchant_name: Alnum<50>,
    /// `TRAN-MERCHANT-CITY PIC X(50)` (P-CVTRA05Y-009)
    /// [app/cpy/CVTRA05Y.cpy:L13].
    pub tran_merchant_city: Alnum<50>,
    /// `TRAN-MERCHANT-ZIP PIC X(10)` (P-CVTRA05Y-010) [app/cpy/CVTRA05Y.cpy:L14].
    pub tran_merchant_zip: Alnum<10>,
    /// `TRAN-CARD-NUM PIC X(16)` (P-CVTRA05Y-011) [app/cpy/CVTRA05Y.cpy:L15].
    pub tran_card_num: CardNumber,
    /// `TRAN-ORIG-TS PIC X(26)` (P-CVTRA05Y-012) [app/cpy/CVTRA05Y.cpy:L16].
    pub tran_orig_ts: Alnum<26>,
    /// `TRAN-PROC-TS PIC X(26)` (P-CVTRA05Y-013) [app/cpy/CVTRA05Y.cpy:L17]:
    /// the key of the `TRANIDX` alternate index [app/jcl/TRANFILE.jcl:L84].
    pub tran_proc_ts: Alnum<26>,
}

impl Transaction {
    /// The stored row with identity `transaction_id` holding the given record
    /// content, whose fields move across unchanged.
    // The content is destructured in the parameter rather than bound to a name:
    // every field is `Copy`, so a named parameter destructured in the body only
    // counts as borrowed, and clippy's `needless_pass_by_value` would ask for a
    // reference where callers hand the record over by value.
    #[must_use]
    pub fn from_new(
        transaction_id: TransactionRowId,
        NewTransaction {
            tran_id,
            tran_type_cd,
            tran_cat_cd,
            tran_source,
            tran_desc,
            tran_amt,
            tran_merchant_id,
            tran_merchant_name,
            tran_merchant_city,
            tran_merchant_zip,
            tran_card_num,
            tran_orig_ts,
            tran_proc_ts,
        }: NewTransaction,
    ) -> Self {
        Self {
            transaction_id,
            tran_id,
            tran_type_cd,
            tran_cat_cd,
            tran_source,
            tran_desc,
            tran_amt,
            tran_merchant_id,
            tran_merchant_name,
            tran_merchant_city,
            tran_merchant_zip,
            tran_card_num,
            tran_orig_ts,
            tran_proc_ts,
        }
    }

    /// The record content without the row identity.
    ///
    /// `batch combtran` compares it with a record of the combined generation to
    /// decide whether a non-live row with the same `tran_id` is made live again.
    #[must_use]
    pub fn to_new(&self) -> NewTransaction {
        NewTransaction {
            tran_id: self.tran_id,
            tran_type_cd: self.tran_type_cd,
            tran_cat_cd: self.tran_cat_cd,
            tran_source: self.tran_source,
            tran_desc: self.tran_desc,
            tran_amt: self.tran_amt,
            tran_merchant_id: self.tran_merchant_id,
            tran_merchant_name: self.tran_merchant_name,
            tran_merchant_city: self.tran_merchant_city,
            tran_merchant_zip: self.tran_merchant_zip,
            tran_card_num: self.tran_card_num,
            tran_orig_ts: self.tran_orig_ts,
            tran_proc_ts: self.tran_proc_ts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NewTransaction, Transaction};
    use crate::numeric::{
        Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
        TransactionRowId,
    };

    /// The first record of `app/data/ASCII/dailytran.txt` as POSTTRAN posts it,
    /// stamped with the pinned processing clock (P-CBTRN02C-216).
    fn first_posted_record() -> NewTransaction {
        NewTransaction {
            tran_id: TranId::move_from("0000000000683580"),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: TranCategoryCode::new(1).expect("1 fits 9(04)"),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: Money::from_cents(50_477).expect("504.77 fits S9(09)V99"),
            tran_merchant_id: MerchantId::new(800_000_000).expect("fits 9(09)"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from("4859452612877065"),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from("2022-07-01-12.00.00.000000"),
        }
    }

    #[test]
    fn from_new_keeps_the_identity_and_to_new_returns_the_content() {
        let new = first_posted_record();
        let row = Transaction::from_new(TransactionRowId::from(42), new.clone());

        assert_eq!(row.transaction_id, TransactionRowId::from(42));
        assert_eq!(row.tran_id, new.tran_id);
        assert_eq!(row.tran_proc_ts, new.tran_proc_ts);
        assert_eq!(row.to_new(), new);
    }

    #[test]
    fn rows_with_equal_content_and_different_identities_differ() {
        let first = Transaction::from_new(TransactionRowId::from(1), first_posted_record());
        let second = Transaction::from_new(TransactionRowId::from(2), first_posted_record());

        assert_ne!(first, second);
        assert_eq!(first.to_new(), second.to_new());
        assert_eq!(
            first,
            Transaction::from_new(TransactionRowId::from(1), first_posted_record())
        );
    }

    #[test]
    fn content_differing_in_one_field_is_not_equal() {
        let new = first_posted_record();
        let changed = NewTransaction {
            tran_amt: Money::from_cents(-50_477).expect("-504.77 fits S9(09)V99"),
            ..new.clone()
        };

        assert_ne!(changed, new);
        assert_ne!(
            Transaction::from_new(TransactionRowId::from(7), changed),
            Transaction::from_new(TransactionRowId::from(7), new)
        );
    }
}
