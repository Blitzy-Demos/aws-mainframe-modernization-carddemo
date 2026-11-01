//! The daily transaction record: `CVTRA06Y DALYTRAN-RECORD`, 350 bytes
//! [app/cpy/CVTRA06Y.cpy:L4-18], the record of the sequential input `DALYTRAN`
//! that `POSTTRAN` posts [app/jcl/POSTTRAN.jcl:L30-31]. `batch posttran` reads it
//! from `--input`, by default `app/data/ASCII/dailytran.txt`.
//!
//! Every field takes the type its reviewed section A row of
//! `docs/parity/parity-oracle-audit.md` names (P-CVTRA06Y-001 to
//! P-CVTRA06Y-013), which is also the type of the transaction master field it
//! is moved to, and the copybook name in lower snake case. No table stores the
//! record: it exists only as input. The trailing `FILLER X(20)`
//! (P-CVTRA06Y-014) [app/cpy/CVTRA06Y.cpy:L18] is not modelled; posting never
//! reads it, and the codec layout keeps its bytes.

use super::transaction::NewTransaction;
use crate::numeric::{
    Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
};

/// One `CVTRA06Y DALYTRAN-RECORD` [app/cpy/CVTRA06Y.cpy:L4-18].
///
/// The fields follow the copybook order and mirror the `TRAN-RECORD` fields
/// of `CVTRA05Y` one for one, so [`DailyTran::to_new_transaction`] moves each
/// field without conversion. Both timestamps stay text: `CBTRN02C`
/// `1500-B-LOOKUP-ACCT` compares `DALYTRAN-ORIG-TS (1:10)` with the account's
/// expiration date as text [app/cbl/CBTRN02C.cbl:L414].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyTran {
    /// `DALYTRAN-ID PIC X(16)`, offset 0 (P-CVTRA06Y-001)
    /// [app/cpy/CVTRA06Y.cpy:L5]: the transaction ID posting copies to
    /// `TRAN-ID`, whatever it holds.
    pub dalytran_id: TranId,
    /// `DALYTRAN-TYPE-CD PIC X(02)`, offset 16 (P-CVTRA06Y-002)
    /// [app/cpy/CVTRA06Y.cpy:L6].
    pub dalytran_type_cd: TranTypeCode,
    /// `DALYTRAN-CAT-CD PIC 9(04)`, offset 18 (P-CVTRA06Y-003)
    /// [app/cpy/CVTRA06Y.cpy:L7].
    pub dalytran_cat_cd: TranCategoryCode,
    /// `DALYTRAN-SOURCE PIC X(10)`, offset 22 (P-CVTRA06Y-004)
    /// [app/cpy/CVTRA06Y.cpy:L8].
    pub dalytran_source: Alnum<10>,
    /// `DALYTRAN-DESC PIC X(100)`, offset 32 (P-CVTRA06Y-005)
    /// [app/cpy/CVTRA06Y.cpy:L9].
    pub dalytran_desc: Alnum<100>,
    /// `DALYTRAN-AMT PIC S9(09)V99`, offset 132 (P-CVTRA06Y-006)
    /// [app/cpy/CVTRA06Y.cpy:L10]: the signed amount, a trailing overpunch in
    /// the file.
    pub dalytran_amt: Money<9>,
    /// `DALYTRAN-MERCHANT-ID PIC 9(09)`, offset 143 (P-CVTRA06Y-007)
    /// [app/cpy/CVTRA06Y.cpy:L11].
    pub dalytran_merchant_id: MerchantId,
    /// `DALYTRAN-MERCHANT-NAME PIC X(50)`, offset 152 (P-CVTRA06Y-008)
    /// [app/cpy/CVTRA06Y.cpy:L12].
    pub dalytran_merchant_name: Alnum<50>,
    /// `DALYTRAN-MERCHANT-CITY PIC X(50)`, offset 202 (P-CVTRA06Y-009)
    /// [app/cpy/CVTRA06Y.cpy:L13].
    pub dalytran_merchant_city: Alnum<50>,
    /// `DALYTRAN-MERCHANT-ZIP PIC X(10)`, offset 252 (P-CVTRA06Y-010)
    /// [app/cpy/CVTRA06Y.cpy:L14].
    pub dalytran_merchant_zip: Alnum<10>,
    /// `DALYTRAN-CARD-NUM PIC X(16)`, offset 262 (P-CVTRA06Y-011)
    /// [app/cpy/CVTRA06Y.cpy:L15]: the card whose cross-reference
    /// `1500-A-LOOKUP-XREF` reads [app/cbl/CBTRN02C.cbl:L380-383].
    pub dalytran_card_num: CardNumber,
    /// `DALYTRAN-ORIG-TS PIC X(26)`, offset 278 (P-CVTRA06Y-012)
    /// [app/cpy/CVTRA06Y.cpy:L16]: the origination timestamp as its writer
    /// produced it.
    pub dalytran_orig_ts: Alnum<26>,
    /// `DALYTRAN-PROC-TS PIC X(26)`, offset 304 (P-CVTRA06Y-013)
    /// [app/cpy/CVTRA06Y.cpy:L17]: never read by posting, which stamps the
    /// processing timestamp from the clock instead. Every fixture record leaves
    /// it blank.
    pub dalytran_proc_ts: Alnum<26>,
}

impl DailyTran {
    /// `CBTRN02C 2000-POST-TRANSACTION` — P-CBTRN02C-013, P-CBTRN02C-014.
    ///
    /// The transaction master record built from this daily record
    /// [app/cbl/CBTRN02C.cbl:L424-438]: `DALYTRAN-ID` through
    /// `DALYTRAN-ORIG-TS` move one by one to `TRAN-ID` through `TRAN-ORIG-TS`
    /// (L425-436), and `TRAN-PROC-TS` is `tran_proc_ts` (L437-438).
    ///
    /// The caller fills `tran_proc_ts` from `Z-GET-DB2-FORMAT-TIMESTAMP`
    /// [app/cbl/CBTRN02C.cbl:L692-705], that is
    /// [`db2_timestamp`](crate::clock::db2_timestamp) of the processing clock,
    /// so this function reads no clock. `DALYTRAN-PROC-TS` is not carried: the
    /// COBOL never moves it. The paragraph's category balance, account and
    /// transaction-file updates (L440-442) belong to the posting rule, not
    /// here.
    #[must_use]
    pub fn to_new_transaction(&self, tran_proc_ts: Alnum<26>) -> NewTransaction {
        NewTransaction {
            tran_id: self.dalytran_id,
            tran_type_cd: self.dalytran_type_cd,
            tran_cat_cd: self.dalytran_cat_cd,
            tran_source: self.dalytran_source,
            tran_desc: self.dalytran_desc,
            tran_amt: self.dalytran_amt,
            tran_merchant_id: self.dalytran_merchant_id,
            tran_merchant_name: self.dalytran_merchant_name,
            tran_merchant_city: self.dalytran_merchant_city,
            tran_merchant_zip: self.dalytran_merchant_zip,
            tran_card_num: self.dalytran_card_num,
            tran_orig_ts: self.dalytran_orig_ts,
            tran_proc_ts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DailyTran;
    use crate::entities::transaction::NewTransaction;
    use crate::numeric::{
        Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
    };

    /// The processing timestamp the parity tests pin (P-CBTRN02C-216).
    const PINNED_PROC_TS: &str = "2022-07-01-12.00.00.000000";

    /// Records 1 and 2 of `app/data/ASCII/dailytran.txt`, a purchase and a
    /// return: a positive and a negative amount (`0000005047G`, `0000009190}`).
    fn fixture_records() -> [DailyTran; 2] {
        [
            DailyTran {
                dalytran_id: TranId::move_from("0000000000683580"),
                dalytran_type_cd: TranTypeCode::move_from("01"),
                dalytran_cat_cd: TranCategoryCode::new(1).expect("1 fits 9(04)"),
                dalytran_source: Alnum::move_from("POS TERM"),
                dalytran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
                dalytran_amt: Money::from_cents(50_477).expect("504.77 fits S9(09)V99"),
                dalytran_merchant_id: MerchantId::new(800_000_000).expect("fits 9(09)"),
                dalytran_merchant_name: Alnum::move_from("Abshire-Lowe"),
                dalytran_merchant_city: Alnum::move_from("North Enoshaven"),
                dalytran_merchant_zip: Alnum::move_from("72112"),
                dalytran_card_num: CardNumber::move_from("4859452612877065"),
                dalytran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
                dalytran_proc_ts: Alnum::spaces(),
            },
            DailyTran {
                dalytran_id: TranId::move_from("0000000001774260"),
                dalytran_type_cd: TranTypeCode::move_from("03"),
                dalytran_cat_cd: TranCategoryCode::new(1).expect("1 fits 9(04)"),
                dalytran_source: Alnum::move_from("OPERATOR"),
                dalytran_desc: Alnum::move_from("Return item at Nitzsche, Nicolas and Lowe"),
                dalytran_amt: Money::from_cents(-91_900).expect("-919.00 fits S9(09)V99"),
                dalytran_merchant_id: MerchantId::new(800_000_000).expect("fits 9(09)"),
                dalytran_merchant_name: Alnum::move_from("Nitzsche, Nicolas and Lowe"),
                dalytran_merchant_city: Alnum::move_from("Fidelshire"),
                dalytran_merchant_zip: Alnum::move_from("53378"),
                dalytran_card_num: CardNumber::move_from("0927987108636232"),
                dalytran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
                dalytran_proc_ts: Alnum::spaces(),
            },
        ]
    }

    #[test]
    fn every_field_moves_to_its_tran_counterpart() {
        let proc_ts = Alnum::move_from(PINNED_PROC_TS);
        for daily in fixture_records() {
            let posted = daily.to_new_transaction(proc_ts);

            assert_eq!(
                posted,
                NewTransaction {
                    tran_id: daily.dalytran_id,
                    tran_type_cd: daily.dalytran_type_cd,
                    tran_cat_cd: daily.dalytran_cat_cd,
                    tran_source: daily.dalytran_source,
                    tran_desc: daily.dalytran_desc,
                    tran_amt: daily.dalytran_amt,
                    tran_merchant_id: daily.dalytran_merchant_id,
                    tran_merchant_name: daily.dalytran_merchant_name,
                    tran_merchant_city: daily.dalytran_merchant_city,
                    tran_merchant_zip: daily.dalytran_merchant_zip,
                    tran_card_num: daily.dalytran_card_num,
                    tran_orig_ts: daily.dalytran_orig_ts,
                    tran_proc_ts: proc_ts,
                }
            );
        }
    }

    #[test]
    fn the_processing_timestamp_is_the_argument_not_the_daily_field() {
        let [first, _] = fixture_records();
        let daily = DailyTran {
            dalytran_proc_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            ..first
        };
        let proc_ts = Alnum::move_from(PINNED_PROC_TS);

        let posted = daily.to_new_transaction(proc_ts);

        assert_eq!(posted.tran_proc_ts, proc_ts);
        assert_ne!(posted.tran_proc_ts, daily.dalytran_proc_ts);
        assert_eq!(posted.tran_orig_ts, daily.dalytran_orig_ts);
    }

    #[test]
    fn the_daily_record_is_unchanged() {
        for daily in fixture_records() {
            let before = daily.clone();

            let _ = daily.to_new_transaction(Alnum::move_from(PINNED_PROC_TS));

            assert_eq!(daily, before);
        }
    }
}
