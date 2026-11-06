//! The disclosure group record: `CVTRA02Y` `DIS-GROUP-RECORD`, one 50-byte
//! record of the VSAM file `DISCGRP`, and the `DEFAULT` group key `CBACT04C`
//! falls back to when an account's own group has no rate.

use crate::numeric::{AccountGroupId, Alnum, InterestRate, TranCategoryCode, TranTypeCode};

/// The group ID `CBACT04C` `1200-GET-INTEREST-RATE` retries with:
/// `MOVE 'DEFAULT' TO FD-DIS-ACCT-GROUP-ID` [app/cbl/CBACT04C.cbl:L437], the
/// literal left-justified and padded with spaces to the `X(10)` field
/// (P-CBACT04C-193).
pub const DEFAULT_GROUP_ID: [u8; 10] = *b"DEFAULT   ";

/// The key of a disclosure group: `CVTRA02Y` `DIS-GROUP-KEY`
/// [app/cpy/CVTRA02Y.cpy:L5-8], the 16-byte key `KEYS(16 0)`
/// [app/jcl/DISCGRP.jcl:L40] of the VSAM KSDS `DISCGRP`, and the primary key of
/// the table `disclosure_groups`.
///
/// The derived ordering compares the group, then the type, then the category.
/// Group and type compare by their stored bytes and the category's text is its
/// four zero-padded digits, so this is the byte order of the 16-byte key: the
/// KSDS key order under ASCII collation.
// No `Copy`: callers pass the key by value to the disclosure group port and
// keep it with `clone`, which clippy's `clone_on_copy` would reject on a
// `Copy` key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DisclosureGroupKey {
    /// `DIS-ACCT-GROUP-ID`, `PIC X(10)` [app/cpy/CVTRA02Y.cpy:L6]
    /// (P-CVTRA02Y-001): the disclosure group, which `CBACT04C` fills from the
    /// account's `ACCT-GROUP-ID` [app/cbl/CBACT04C.cbl:L210].
    ///
    /// The audit row types the field `Alnum<10>`. It is held as the key type
    /// [`AccountGroupId`] of `ACCT-GROUP-ID` it is matched with: the same ten
    /// stored bytes, which [`AccountGroupId::as_alnum`] yields as the row's
    /// `Alnum<10>`.
    pub dis_acct_group_id: AccountGroupId,
    /// `DIS-TRAN-TYPE-CD`, `PIC X(02)` [app/cpy/CVTRA02Y.cpy:L7]
    /// (P-CVTRA02Y-002): the transaction type, filled from `TRANCAT-TYPE-CD`
    /// [app/cbl/CBACT04C.cbl:L212].
    pub dis_tran_type_cd: TranTypeCode,
    /// `DIS-TRAN-CAT-CD`, `PIC 9(04)` [app/cpy/CVTRA02Y.cpy:L8]
    /// (P-CVTRA02Y-003): the transaction category, filled from `TRANCAT-CD`
    /// [app/cbl/CBACT04C.cbl:L211].
    pub dis_tran_cat_cd: TranCategoryCode,
}

impl DisclosureGroupKey {
    /// CBACT04C `1200-GET-INTEREST-RATE` — P-CBACT04C-004, P-CBACT04C-193:
    /// the key `1200-A-GET-DEFAULT-INT-RATE` reads after the account's own
    /// group ends with file status 23 [app/cbl/CBACT04C.cbl:L436-438].
    ///
    /// Only the group ID changes, to [`DEFAULT_GROUP_ID`]. The type and
    /// category stay those moved in before the first read
    /// [app/cbl/CBACT04C.cbl:L211-212], so a caller passes them unchanged.
    /// Every fixture `ACCT-GROUP-ID` is blank (E-06), so every INTCALC rate
    /// on the parity corpus comes through this key.
    #[must_use]
    pub const fn default_group(
        dis_tran_type_cd: TranTypeCode,
        dis_tran_cat_cd: TranCategoryCode,
    ) -> Self {
        Self {
            dis_acct_group_id: AccountGroupId::from_alnum(Alnum::from_array(DEFAULT_GROUP_ID)),
            dis_tran_type_cd,
            dis_tran_cat_cd,
        }
    }

    /// Whether the group ID holds exactly the ten bytes of
    /// [`DEFAULT_GROUP_ID`], the group [`Self::default_group`] sets.
    ///
    /// The comparison is byte for byte, as the VSAM key match is: a blank
    /// group, `default` in lower case or `DEFAULT` shifted right is not the
    /// default group.
    #[must_use]
    pub fn is_default_group(&self) -> bool {
        *self.dis_acct_group_id.as_bytes() == DEFAULT_GROUP_ID
    }
}

/// A disclosure group: `CVTRA02Y` `DIS-GROUP-RECORD`
/// [app/cpy/CVTRA02Y.cpy:L4-10], one record of the VSAM KSDS `DISCGRP`: the
/// annual interest rate of an account group for one transaction type and
/// category. The table `disclosure_groups` holds it, one column per field
/// under the field's name, seeded with the 51 records of `discgrp.txt`
/// (P-CVTRA02Y-006).
///
/// The trailing `FILLER X(28)` [app/cpy/CVTRA02Y.cpy:L10] (P-CVTRA02Y-005) is
/// not modelled. The fixture holds `0` characters there
/// [app/data/ASCII/discgrp.txt:L1], which the record codec keeps as raw bytes.
///
/// The ASCII and EBCDIC fixtures differ in the rate of record 34, group
/// `DEFAULT`, type `07`, category `0001`: the ASCII record holds 0.00
/// [app/data/ASCII/discgrp.txt:L34] and its EBCDIC counterpart a non-zero rate
/// (E-05, P-CVTRA02Y-007). The ASCII corpus seeds the table and drives parity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureGroup {
    /// `DIS-GROUP-KEY` [app/cpy/CVTRA02Y.cpy:L5-8]: the record key.
    pub key: DisclosureGroupKey,
    /// `DIS-INT-RATE`, `PIC S9(04)V99` [app/cpy/CVTRA02Y.cpy:L9]
    /// (P-CVTRA02Y-004): the annual rate in percent, the rate operand of
    /// `CBACT04C` `1300-COMPUTE-INTEREST` (P-CBACT04C-012). A zero rate computes
    /// no interest (P-CBACT04C-006).
    pub dis_int_rate: InterestRate,
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_GROUP_ID, DisclosureGroup, DisclosureGroupKey};
    use crate::numeric::{
        AccountGroupId, InterestRate, NumericError, TranCategoryCode, TranTypeCode,
    };

    /// The key whose fields hold the right-trimmed column texts and category,
    /// as persistence rebuilds a key from its `disclosure_groups` row.
    fn key(
        group: &str,
        tran_type: &str,
        category: u16,
    ) -> Result<DisclosureGroupKey, NumericError> {
        Ok(DisclosureGroupKey {
            dis_acct_group_id: AccountGroupId::from_trimmed(group)?,
            dis_tran_type_cd: TranTypeCode::from_trimmed(tran_type)?,
            dis_tran_cat_cd: TranCategoryCode::new(category)?,
        })
    }

    /// The 16 bytes of the VSAM key: group, type and the category's digits.
    fn key_bytes(key: &DisclosureGroupKey) -> Vec<u8> {
        let mut bytes = key.dis_acct_group_id.as_bytes().to_vec();
        bytes.extend_from_slice(key.dis_tran_type_cd.as_bytes());
        bytes.extend_from_slice(key.dis_tran_cat_cd.to_string().as_bytes());
        bytes
    }

    #[test]
    fn default_group_moves_only_the_group_id() -> Result<(), NumericError> {
        let tran_type = TranTypeCode::move_from("07");
        let category = TranCategoryCode::new(1)?;
        let default = DisclosureGroupKey::default_group(tran_type, category);

        assert_eq!(default.dis_acct_group_id.as_bytes(), b"DEFAULT   ");
        assert_eq!(*default.dis_acct_group_id.as_bytes(), DEFAULT_GROUP_ID);
        assert_eq!(default.dis_tran_type_cd, tran_type);
        assert_eq!(default.dis_tran_cat_cd, category);
        assert!(default.is_default_group());
        // The `disclosure_groups` column holds `DEFAULT` right-trimmed; the
        // key rebuilt from it is the fallback key.
        assert_eq!(key("DEFAULT", "07", 1)?, default);
        Ok(())
    }

    #[test]
    fn only_the_padded_default_literal_is_the_default_group() -> Result<(), NumericError> {
        // (group text, is the default group)
        let cases = [
            ("DEFAULT", true),
            // E-06: the blank group every fixture account carries.
            ("", false),
            ("A000000000", false),
            ("default", false),
            (" DEFAULT", false),
            ("DEFAULTS", false),
        ];
        for (group, expected) in cases {
            assert_eq!(
                key(group, "01", 1)?.is_default_group(),
                expected,
                "{group:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn keys_order_by_group_then_type_then_category() -> Result<(), NumericError> {
        let ascending = [
            key("A000000000", "01", 1)?,
            key("A000000000", "01", 2)?,
            key("A000000000", "02", 1)?,
            key("A000000000", "07", 9_999)?,
            key("DEFAULT", "01", 9)?,
            key("DEFAULT", "01", 10)?,
            key("DEFAULT", "07", 1)?,
            key("ZEROAPR", "01", 1)?,
        ];
        for (low, high) in ascending.iter().zip(ascending.iter().skip(1)) {
            assert!(low < high, "{low:?} < {high:?}");
        }

        let mut by_ord = ascending.to_vec();
        by_ord.reverse();
        by_ord.sort();
        let mut by_bytes = ascending.to_vec();
        by_bytes.reverse();
        by_bytes.sort_by_key(key_bytes);
        assert_eq!(by_ord, ascending);
        assert_eq!(by_bytes, ascending);
        Ok(())
    }

    #[test]
    fn the_default_fallback_key_finds_the_fixture_default_record() -> Result<(), NumericError> {
        // discgrp.txt L18: `DEFAULT   ` `01` `0001` `00150{`, the rate of
        // every 01/0001 balance on the parity corpus (P-CBACT04C-023).
        let record = DisclosureGroup {
            key: key("DEFAULT", "01", 1)?,
            dis_int_rate: InterestRate::from_hundredths(1_500)?,
        };
        let lookup = DisclosureGroupKey::default_group(
            TranTypeCode::move_from("01"),
            TranCategoryCode::new(1)?,
        );
        assert_eq!(record.key, lookup);
        assert_eq!(record.dis_int_rate.to_string(), "15.00");

        let other_rate = DisclosureGroup {
            dis_int_rate: InterestRate::ZERO,
            ..record.clone()
        };
        assert_ne!(other_rate, record);
        assert_eq!(record.clone(), record);
        Ok(())
    }
}
