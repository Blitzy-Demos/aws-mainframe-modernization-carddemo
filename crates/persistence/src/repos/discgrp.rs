//! VSAM `DISCGRP` (`CVTRA02Y` `DIS-GROUP-RECORD`) on the table
//! `disclosure_groups`: the interest rate of an account group for one
//! transaction type and category, which `CBACT04C` reads for every balance
//! record of INTCALC.
//!
//! | Column | Field | PIC | Audit row |
//! |---|---|---|---|
//! | `dis_acct_group_id` | `DIS-ACCT-GROUP-ID` | `X(10)` | P-CVTRA02Y-001 |
//! | `dis_tran_type_cd` | `DIS-TRAN-TYPE-CD` | `X(02)` | P-CVTRA02Y-002 |
//! | `dis_tran_cat_cd` | `DIS-TRAN-CAT-CD` | `9(04)` | P-CVTRA02Y-003 |
//! | `dis_int_rate` | `DIS-INT-RATE` | `S9(04)V99` | P-CVTRA02Y-004 |
//!
//! The `FILLER X(28)` of `0` characters (P-CVTRA02Y-005) is not stored.
//!
//! The port is read-only: `CBACT04C` opens the file `INPUT`
//! [app/cbl/CBACT04C.cbl:L270-286], and `crate::seed` loads the 51 records of
//! `discgrp.txt` (P-CVTRA02Y-006). The seed reads the ASCII corpus, so record
//! 34 (`DEFAULT`, `07`, `0001`) holds the ASCII rate 0.00, not the rate of its
//! EBCDIC counterpart (E-05, P-CVTRA02Y-007).

use carddemo_domain::entities::{DisclosureGroup, DisclosureGroupKey};
use carddemo_domain::ports::{DisclosureGroupReader, PortError, PortResult};
use rust_decimal::Decimal;

use super::{account_group_id, interest_rate, tran_category_code, tran_type_code};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The table every `Corrupt` error of this module names.
const TABLE: &str = "disclosure_groups";

/// One `disclosure_groups` row as the columns hold it: the text keys
/// right-trimmed, the category as its integer, the rate as `NUMERIC(6,2)`.
///
/// The query aliases each `dis_*` column to its field name without the
/// `dis_` prefix: four fields sharing one prefix fail clippy's pedantic
/// `struct_field_names`, and the build denies every pedantic lint.
struct DisclosureGroupRow {
    acct_group_id: String,
    tran_type_cd: String,
    tran_cat_cd: i32,
    int_rate: Decimal,
}

impl TryFrom<DisclosureGroupRow> for DisclosureGroup {
    type Error = PersistenceError;

    /// `READ DISCGRP-FILE INTO DIS-GROUP-RECORD` — P-CVTRA02Y-001 to
    /// P-CVTRA02Y-004: each column rebuilt into the type of its copybook
    /// field. The blank group ID stored as `''` comes back as ten spaces (E-06),
    /// and a value its PIC clause cannot hold is
    /// [`PersistenceError::Corrupt`], never cut or rounded.
    fn try_from(row: DisclosureGroupRow) -> Result<Self, Self::Error> {
        Ok(Self {
            key: DisclosureGroupKey {
                dis_acct_group_id: account_group_id(
                    &row.acct_group_id,
                    TABLE,
                    "dis_acct_group_id",
                )?,
                dis_tran_type_cd: tran_type_code(&row.tran_type_cd, TABLE, "dis_tran_type_cd")?,
                dis_tran_cat_cd: tran_category_code(row.tran_cat_cd, TABLE, "dis_tran_cat_cd")?,
            },
            dis_int_rate: interest_rate(row.int_rate, TABLE, "dis_int_rate")?,
        })
    }
}

impl DisclosureGroupReader for PgStore<'_, '_> {
    /// CBACT04C 1200-GET-INTEREST-RATE, 1200-A-GET-DEFAULT-INT-RATE —
    /// P-CBACT04C-004, P-CBACT04C-005, P-CBACT04C-193: the random `READ` of
    /// `DISCGRP` by its full 16-byte key `KEYS(16 0)` [app/jcl/DISCGRP.jcl:L40],
    /// group `X(10)`, type `X(02)` and category `9(04)`
    /// [app/cbl/CBACT04C.cbl:L78-81] [app/cbl/CBACT04C.cbl:L415-460].
    ///
    /// The text keys bind right-trimmed, the stored form the column `CHECK`
    /// admits, so the equality matches exactly the rows whose padded bytes
    /// equal the key's: a blank group binds as `''`, the group of every
    /// fixture account (E-06), and `DEFAULT   ` binds as `DEFAULT`. The
    /// `COLLATE "C"` key columns compare byte for byte, as the VSAM key does.
    ///
    /// No fallback happens here. On [`PortError::NotFound`], the status 23 of
    /// the first `READ`, the interest rule moves `DEFAULT` into the group and
    /// calls this method again with `DisclosureGroupKey::default_group`, the
    /// second `READ` of 1200-A [app/cbl/CBACT04C.cbl:L436-438]
    /// [app/cbl/CBACT04C.cbl:L443-444].
    ///
    /// The read runs under its own savepoint: INTCALC reads the rate of an
    /// account after rewriting the previous one, and a failed read must leave
    /// that rewrite committable (P-CBACT04C-010).
    async fn read_disclosure_group(
        &mut self,
        key: DisclosureGroupKey,
    ) -> PortResult<DisclosureGroup> {
        let group = key.dis_acct_group_id.trimmed();
        let tran_type = key.dis_tran_type_cd.trimmed();
        let category = i32::from(key.dis_tran_cat_cd.get());
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query_as!(
            DisclosureGroupRow,
            "SELECT dis_acct_group_id AS acct_group_id, dis_tran_type_cd AS tran_type_cd, \
             dis_tran_cat_cd AS tran_cat_cd, dis_int_rate AS int_rate \
             FROM disclosure_groups \
             WHERE dis_acct_group_id = $1 AND dis_tran_type_cd = $2 AND dis_tran_cat_cd = $3",
            &*group,
            &*tran_type,
            category,
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        DisclosureGroup::try_from(row).map_err(PortError::from)
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::DisclosureGroup;
    use rust_decimal::Decimal;

    use super::{DisclosureGroupRow, TABLE};
    use crate::PersistenceError;

    fn row(group: &str, tran_type: &str, category: i32, rate: Decimal) -> DisclosureGroupRow {
        DisclosureGroupRow {
            acct_group_id: group.to_owned(),
            tran_type_cd: tran_type.to_owned(),
            tran_cat_cd: category,
            int_rate: rate,
        }
    }

    #[test]
    fn stored_rows_rebuild_the_padded_record() -> Result<(), PersistenceError> {
        // (group, type, category, rate in hundredths, padded group, is DEFAULT)
        let cases = [
            // discgrp.txt L18, the rate of every 01/0001 balance (P-CBACT04C-023).
            ("DEFAULT", "01", 1, 1_500, "DEFAULT   ", true),
            // discgrp.txt L1.
            ("A000000000", "01", 1, 1_500, "A000000000", false),
            // discgrp.txt L34, the ASCII rate of record 34 (E-05).
            ("DEFAULT", "07", 1, 0, "DEFAULT   ", true),
            // The blank group of every fixture account (E-06).
            ("", "02", 9_999, -1, "          ", false),
        ];
        for (group, tran_type, category, hundredths, padded, is_default) in cases {
            let stored = row(
                group,
                tran_type,
                category,
                Decimal::new(i64::from(hundredths), 2),
            );
            let record = DisclosureGroup::try_from(stored)?;
            assert_eq!(record.key.dis_acct_group_id.as_bytes(), padded.as_bytes());
            assert_eq!(record.key.is_default_group(), is_default, "{group:?}");
            assert_eq!(record.key.dis_tran_type_cd.trimmed(), tran_type);
            assert_eq!(i32::from(record.key.dis_tran_cat_cd.get()), category);
            assert_eq!(record.dis_int_rate.hundredths(), hundredths);
        }
        Ok(())
    }

    #[test]
    fn a_value_outside_its_picture_is_corrupt_in_its_column() {
        let rate = Decimal::new(1_500, 2);
        let cases = [
            (row("A0000000000", "01", 1, rate), "dis_acct_group_id"),
            (row("DEFAULT", "011", 1, rate), "dis_tran_type_cd"),
            (row("DEFAULT", "01", 10_000, rate), "dis_tran_cat_cd"),
            (row("DEFAULT", "01", -1, rate), "dis_tran_cat_cd"),
            (
                row("DEFAULT", "01", 1, Decimal::new(1_000_000, 2)),
                "dis_int_rate",
            ),
            (
                row("DEFAULT", "01", 1, Decimal::new(1_001, 3)),
                "dis_int_rate",
            ),
        ];
        for (stored, expected) in cases {
            match DisclosureGroup::try_from(stored) {
                Err(PersistenceError::Corrupt { table, column, .. }) => {
                    assert_eq!((table, column), (TABLE, expected));
                }
                other => panic!("expected a Corrupt {expected} error, got {other:?}"),
            }
        }
    }
}
