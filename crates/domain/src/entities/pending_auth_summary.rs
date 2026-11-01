//! The pending-authorization summary of one account: copybook `CIPAUSMY`
//! [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L19-31], the root segment
//! `PAUTSUM0` of the IMS HIDAM database `DBPAUTP0`
//! [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L28-30]. Each summary is
//! one row of the table `pending_auth_summaries`, whose columns carry the field
//! names below.
//!
//! The segment is 100 bytes, sequenced by the packed account number `ACCNTID`.
//! No IMS segment passes through the codec, so the packed (`COMP-3`) and binary
//! (`COMP`) fields have no byte layout here: each is typed by its PIC digits, as
//! the section A rows of `docs/parity/parity-oracle-audit.md` record. The
//! trailing `FILLER X(34)` (P-CIPAUSMY-013) is not stored and not modelled.

use crate::numeric::{AccountId, Alnum, AuthCount, CustomerId, Money};

/// The number of occurrences of `PA-ACCOUNT-STATUS`: `OCCURS 5 TIMES`
/// (P-CIPAUSMY-004) [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L22].
pub const PA_ACCOUNT_STATUS_OCCURS: usize = 5;

/// `PENDING-AUTH-SUMMARY` [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L19-31],
/// the `PAUTSUM0` root segment of `DBPAUTP0`: per account, the running counts
/// and amounts of the authorizations `COPAUA0C` decided, under which the
/// `PAUTDTL1` details sit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAuthSummary {
    /// `PA-ACCT-ID PIC S9(11) COMP-3`, P-CIPAUSMY-001
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L19]: the segment key
    /// `ACCNTID` and the table's primary key.
    ///
    /// The PIC is signed, but section A types the field as the unsigned
    /// [`AccountId`]: `COPAUA0C` sets it only from the unsigned
    /// `XREF-ACCT-ID 9(11)`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L805], so no key is
    /// negative.
    pub pa_acct_id: AccountId,
    /// `PA-CUST-ID PIC 9(09)` (`DISPLAY`), P-CIPAUSMY-002
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L20].
    pub pa_cust_id: CustomerId,
    /// `PA-AUTH-STATUS PIC X(01)` (`DISPLAY`), P-CIPAUSMY-003
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L21]. No program of
    /// the authorization extension reads or moves to it.
    pub pa_auth_status: Alnum<1>,
    /// `PA-ACCOUNT-STATUS PIC X(02) OCCURS 5 TIMES` (`DISPLAY`), P-CIPAUSMY-004
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L22]: element `i` is
    /// COBOL subscript `i + 1`. Section A stores the table as the one column
    /// `pa_account_status VARCHAR(2)[5]`, in the same order. No program of the
    /// authorization extension reads or moves to it.
    pub pa_account_status: [Alnum<2>; PA_ACCOUNT_STATUS_OCCURS],
    /// `PA-CREDIT-LIMIT PIC S9(09)V99 COMP-3`, P-CIPAUSMY-005
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L23]. `COPAUA0C`
    /// declines an amount above this limit minus [`Self::pa_credit_balance`]
    /// (P-COPAUA0C-017) and refreshes it from `ACCT-CREDIT-LIMIT` in
    /// `8400-UPDATE-SUMMARY` (P-COPAUA0C-019).
    pub pa_credit_limit: Money<9>,
    /// `PA-CASH-LIMIT PIC S9(09)V99 COMP-3`, P-CIPAUSMY-006
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L24], refreshed from
    /// `ACCT-CASH-CREDIT-LIMIT` in `8400-UPDATE-SUMMARY` (P-COPAUA0C-019).
    pub pa_cash_limit: Money<9>,
    /// `PA-CREDIT-BALANCE PIC S9(09)V99 COMP-3`, P-CIPAUSMY-007
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L25]: the approved
    /// amounts added so far (P-COPAUA0C-019), subtracted from
    /// [`Self::pa_credit_limit`] for the available amount (P-COPAUA0C-017).
    pub pa_credit_balance: Money<9>,
    /// `PA-CASH-BALANCE PIC S9(09)V99 COMP-3`, P-CIPAUSMY-008
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L26], set to zero by
    /// every approval (P-COPAUA0C-019).
    pub pa_cash_balance: Money<9>,
    /// `PA-APPROVED-AUTH-CNT PIC S9(04) COMP`, P-CIPAUSMY-009
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L27].
    ///
    /// The only count the `CBPAUP0C` purge tests: its condition
    /// `IF PA-APPROVED-AUTH-CNT <= 0 AND PA-APPROVED-AUTH-CNT <= 0` names this
    /// field twice (P-CBPAUP0C-006, escalation E-16)
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L156]. The
    /// reproduction lives in `rules::auth_purge`.
    pub pa_approved_auth_cnt: AuthCount,
    /// `PA-DECLINED-AUTH-CNT PIC S9(04) COMP`, P-CIPAUSMY-010
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L28].
    ///
    /// Never tested by the `CBPAUP0C` purge, whose condition repeats the
    /// approved count instead (P-CBPAUP0C-006, escalation E-16)
    /// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L156]: a summary with
    /// declined authorizations is deleted once its approved count is at most
    /// zero. The reproduction lives in `rules::auth_purge`.
    pub pa_declined_auth_cnt: AuthCount,
    /// `PA-APPROVED-AUTH-AMT PIC S9(09)V99 COMP-3`, P-CIPAUSMY-011
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L29].
    pub pa_approved_auth_amt: Money<9>,
    /// `PA-DECLINED-AUTH-AMT PIC S9(09)V99 COMP-3`, P-CIPAUSMY-012
    /// [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:L30].
    pub pa_declined_auth_amt: Money<9>,
}

#[cfg(test)]
mod tests {
    use super::{PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary};
    use crate::numeric::{AccountId, Alnum, AuthCount, CustomerId, Money, NumericError};

    /// A summary whose fields all hold distinct values.
    fn summary() -> Result<PendingAuthSummary, NumericError> {
        Ok(PendingAuthSummary {
            pa_acct_id: AccountId::new(1)?,
            pa_cust_id: CustomerId::new(1)?,
            pa_auth_status: Alnum::from_array(*b"A"),
            pa_account_status: [
                Alnum::from_array(*b"01"),
                Alnum::from_array(*b"02"),
                Alnum::from_array(*b"03"),
                Alnum::from_array(*b"04"),
                Alnum::from_array(*b"05"),
            ],
            pa_credit_limit: Money::from_cents(2_020_000)?,
            pa_cash_limit: Money::from_cents(500_000)?,
            pa_credit_balance: Money::from_cents(19_450)?,
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(2)?,
            pa_declined_auth_cnt: AuthCount::new(-1)?,
            pa_approved_auth_amt: Money::from_cents(19_450)?,
            pa_declined_auth_amt: Money::from_cents(-75_001)?,
        })
    }

    #[test]
    fn a_clone_equals_its_original_with_all_five_statuses() -> Result<(), NumericError> {
        let original = summary()?;
        let copy = original.clone();

        assert_eq!(copy, original);
        assert_eq!(PA_ACCOUNT_STATUS_OCCURS, 5);
        let statuses = copy.pa_account_status.map(|status| *status.as_bytes());
        assert_eq!(statuses, [*b"01", *b"02", *b"03", *b"04", *b"05"]);
        Ok(())
    }

    #[test]
    fn changing_any_one_status_occurrence_breaks_equality() -> Result<(), NumericError> {
        let original = summary()?;
        for occurrence in 0..PA_ACCOUNT_STATUS_OCCURS {
            let mut changed = original.clone();
            changed.pa_account_status[occurrence] = Alnum::from_array(*b"99");

            assert_ne!(changed, original, "occurrence {}", occurrence + 1);
        }
        Ok(())
    }
}
