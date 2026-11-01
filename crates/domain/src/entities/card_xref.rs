//! The card cross-reference record: `CVACT03Y` `CARD-XREF-RECORD`, one 50-byte
//! record of the VSAM file `CCXREF`.

use crate::numeric::{AccountId, CardNumber, CustomerId};

/// A card cross-reference: `CVACT03Y` `CARD-XREF-RECORD`
/// [app/cpy/CVACT03Y.cpy:L4-8], one record of the VSAM KSDS `CCXREF`, keyed by
/// `XREF-CARD-NUM` (`KEYS(16 0)` [app/jcl/XREFFILE.jcl:L43]).
///
/// It links a card to the customer and the account it belongs to. The
/// non-unique alternate index `CXACAIX`, `KEYS(11,25)` on `XREF-ACCT-ID`
/// [app/jcl/XREFFILE.jcl:L74], reads it by account. The table `card_xref`
/// holds it, one column per field under the field's name, with the index
/// `cxacaix` on `xref_acct_id` in place of the alternate index.
///
/// The trailing `FILLER X(14)` [app/cpy/CVACT03Y.cpy:L8] (P-CVACT03Y-004)
/// holds no data and is not modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardXref {
    /// `XREF-CARD-NUM`, `PIC X(16)` [app/cpy/CVACT03Y.cpy:L5] (P-CVACT03Y-001):
    /// the card number, the KSDS key.
    pub xref_card_num: CardNumber,
    /// `XREF-CUST-ID`, `PIC 9(09)` [app/cpy/CVACT03Y.cpy:L6] (P-CVACT03Y-002):
    /// the customer the card is issued to.
    pub xref_cust_id: CustomerId,
    /// `XREF-ACCT-ID`, `PIC 9(11)` [app/cpy/CVACT03Y.cpy:L7] (P-CVACT03Y-003):
    /// the account the card draws on, the key of the alternate index `CXACAIX`.
    pub xref_acct_id: AccountId,
}

#[cfg(test)]
mod tests {
    use super::CardXref;
    use crate::numeric::{AccountId, CardNumber, CustomerId, NumericError};

    /// The first `cardxref.txt` record (P-CVACT03Y-006).
    fn first_fixture_record() -> Result<CardXref, NumericError> {
        Ok(CardXref {
            xref_card_num: CardNumber::from_trimmed("0500024453765740")?,
            xref_cust_id: CustomerId::new(50)?,
            xref_acct_id: AccountId::new(50)?,
        })
    }

    #[test]
    fn holds_the_fields_of_the_first_fixture_record() -> Result<(), NumericError> {
        let record = first_fixture_record()?;
        assert_eq!(record.xref_card_num.to_string(), "0500024453765740");
        assert_eq!(record.xref_cust_id.to_string(), "000000050");
        assert_eq!(record.xref_acct_id.to_string(), "00000000050");
        // The derived `Debug` goes through the masking `Debug` of `CardNumber`.
        let debug = format!("{record:?}");
        assert!(!debug.contains("0500024453765740"), "{debug}");
        assert!(debug.contains("************5740"), "{debug}");
        Ok(())
    }

    #[test]
    fn a_clone_equals_its_original() -> Result<(), NumericError> {
        let record = first_fixture_record()?;
        assert_eq!(record.clone(), record);
        Ok(())
    }

    #[test]
    fn records_differing_in_any_one_field_are_unequal() -> Result<(), NumericError> {
        let record = first_fixture_record()?;
        let variants = [
            (
                "xref_card_num",
                CardXref {
                    xref_card_num: CardNumber::from_trimmed("0500024453765741")?,
                    ..record.clone()
                },
            ),
            (
                "xref_cust_id",
                CardXref {
                    xref_cust_id: CustomerId::new(49)?,
                    ..record.clone()
                },
            ),
            (
                "xref_acct_id",
                CardXref {
                    xref_acct_id: AccountId::new(49)?,
                    ..record.clone()
                },
            ),
        ];
        for (field, variant) in variants {
            assert_ne!(variant, record, "{field}");
        }
        Ok(())
    }
}
