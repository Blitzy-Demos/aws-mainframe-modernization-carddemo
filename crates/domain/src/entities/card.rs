//! The card record of the VSAM `CARDDAT` file: `CVACT02Y` `CARD-RECORD`.
//!
//! Every field takes the type its section A row of
//! `docs/parity/parity-oracle-audit.md` assigns (`P-CVACT02Y-001` to
//! `P-CVACT02Y-006`, all reviewed pass), so the record re-encodes to the
//! 150 bytes it was decoded from once the codec restores the `FILLER`.

use crate::numeric::{AccountId, Alnum, CardNumber, Cvv};

/// A card: `CVACT02Y` `CARD-RECORD` [app/cpy/CVACT02Y.cpy:L4-11], `RECLN 150`.
///
/// Field names are the copybook names in lower snake case, prefix and spelling
/// kept (`card_expiraion_date`). The `cards` table names its key column
/// `card_number`; the field keeps the copybook's `card_num`. The trailing
/// `FILLER X(59)` (`P-CVACT02Y-007`) [app/cpy/CVACT02Y.cpy:L11] is not stored
/// and not modelled.
///
/// `Debug` shows the card number masked to its last four characters and the
/// verification code withheld, through the `Debug` of [`CardNumber`] and
/// [`Cvv`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// `CARD-NUM PIC X(16)` [app/cpy/CVACT02Y.cpy:L5], the `CARDDAT` key —
    /// `P-CVACT02Y-001`. SQL column `card_number`.
    pub card_num: CardNumber,
    /// `CARD-ACCT-ID PIC 9(11)` [app/cpy/CVACT02Y.cpy:L6], the key of the
    /// `CARDAIX` alternate index — `P-CVACT02Y-002`.
    pub card_acct_id: AccountId,
    /// `CARD-CVV-CD PIC 9(03)` [app/cpy/CVACT02Y.cpy:L7] — `P-CVACT02Y-003`.
    pub card_cvv_cd: Cvv,
    /// `CARD-EMBOSSED-NAME PIC X(50)` [app/cpy/CVACT02Y.cpy:L8] —
    /// `P-CVACT02Y-004`.
    pub card_embossed_name: Alnum<50>,
    /// `CARD-EXPIRAION-DATE PIC X(10)` [app/cpy/CVACT02Y.cpy:L9] —
    /// `P-CVACT02Y-005`. Text as stored, `YYYY-MM-DD` in the fixtures, so its
    /// comparisons are the COBOL alphanumeric ones.
    pub card_expiraion_date: Alnum<10>,
    /// `CARD-ACTIVE-STATUS PIC X(01)` [app/cpy/CVACT02Y.cpy:L10] —
    /// `P-CVACT02Y-006`.
    pub card_active_status: Alnum<1>,
}

#[cfg(test)]
mod tests {
    use super::Card;
    use crate::numeric::{AccountId, Alnum, CardNumber, Cvv, NumericError};

    /// The first fixture card, as `P-CVACT02Y-009` records it
    /// [app/data/ASCII/carddata.txt:L1], with an arbitrary verification code.
    fn fixture_card() -> Result<Card, NumericError> {
        Ok(Card {
            card_num: CardNumber::from_trimmed("0500024453765740")?,
            card_acct_id: AccountId::new(50)?,
            card_cvv_cd: Cvv::new(123)?,
            card_embossed_name: Alnum::move_from("Aniya Von"),
            card_expiraion_date: Alnum::move_from("2023-03-09"),
            card_active_status: Alnum::move_from("Y"),
        })
    }

    #[test]
    fn a_clone_is_equal_and_a_changed_status_is_not() -> Result<(), NumericError> {
        let card = fixture_card()?;
        let copy = card.clone();
        assert_eq!(copy, card);

        let inactive = Card {
            card_active_status: Alnum::move_from("N"),
            ..card.clone()
        };
        assert_ne!(inactive, card);
        Ok(())
    }

    #[test]
    fn debug_masks_the_card_number_and_withholds_the_cvv() -> Result<(), NumericError> {
        let shown = format!("{:?}", fixture_card()?);
        assert!(shown.contains("************5740"), "{shown}");
        assert!(shown.contains("Cvv(<redacted>)"), "{shown}");
        assert!(!shown.contains("0500024453765740"), "{shown}");
        assert!(!shown.contains("123"), "{shown}");
        Ok(())
    }
}
