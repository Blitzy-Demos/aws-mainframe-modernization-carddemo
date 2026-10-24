//! The COBOL message texts and the abend work area that several online programs share through
//! the copybooks `CSMSG01Y` and `CSMSG02Y` (AAP 0.5.4).
//!
//! Every endpoint takes its messages from the audit of the program that owns it (AAP 0.1.2), so a
//! text that only one program moves is a constant of that program's module in `crate::online`.
//! This module holds the texts a shared copybook declares and the literals that the abend paths of
//! several programs move into its fields, each confirmed in those programs' audits.
//!
//! Every text is a byte-exact copy of its COBOL `VALUE` or literal, spelling, trailing dots and
//! trailing spaces included. A `PIC X(n)` constant is the field's whole `n`-byte value: a shorter
//! `VALUE` literal is padded with spaces on the right, as COBOL initialises the field. Where an
//! audit row says the screen shows the text without its padding, the owner renders it through
//! [`trimmed`].
//!
//! No text here carries a card number, SSN, CVV or password; a message that would interpolate one
//! is built by its owning module from the redacted forms of `crate::sensitive` (Cardholder Data
//! Redaction).

use copybook::PicX;

/// `CCDA-MSG-THANK-YOU PIC X(50)`, the session-end text (`CSMSG01Y.cpy:17-19`).
///
/// The 49-byte literal `'Thank you for using CardDemo application...      '` padded by its
/// `PIC X(50)` to 50 bytes, seven trailing spaces in all. Only `COSGN00C` moves it: on F3 it is
/// moved into `WS-MESSAGE PIC X(80)` and sent as plain text (`COSGN00C.cbl:88-90`), rule
/// `COSGN00C-004`, with the `F3=Exit` key of rule `COSGN00-047`.
pub const CCDA_MSG_THANK_YOU: &str = "Thank you for using CardDemo application...       ";

/// `CCDA-MSG-INVALID-KEY PIC X(50)`, the message for a key the screen does not serve
/// (`CSMSG01Y.cpy:17,20-21`).
///
/// The 49-byte literal `'Invalid key pressed. Please see below...         '` padded by its
/// `PIC X(50)` to 50 bytes, ten trailing spaces in all. Each program's `WHEN OTHER` of its key
/// `EVALUATE` moves it into `WS-MESSAGE`: rules `COSGN00C-005`, `COMEN01C-007`, `COADM01C-021`,
/// `COBIL00C-006`, `CORPT00C-005`, `COTRN00C-006`, `COTRN01C-007`, `COTRN02C-006`,
/// `COUSR00C-005`, `COUSR01C-009`, `COUSR02C-009`, `COUSR03C-009`, `COPAUS0C-004`,
/// `COPAUS1C-005` and the `F12=Exit` key of rule `COUSR01-038`. Endpoint 13 returns it as its 400
/// message for an `aid` the screen never sends (rule `COTRN02C-007`). `COUSR02C-009` and
/// `COUSR03C-009` show it without its trailing spaces ([`trimmed`]).
pub const CCDA_MSG_INVALID_KEY: &str = "Invalid key pressed. Please see below...          ";

const _: () = assert!(CCDA_MSG_THANK_YOU.len() == 50);
const _: () = assert!(CCDA_MSG_INVALID_KEY.len() == 50);

/// `text` without its trailing spaces; leading spaces and every other byte are kept.
///
/// For the responses whose audit row says the screen shows a padded `PIC X(n)` text trimmed, such
/// as `COUSR02C-009` and `COUSR03C-009` for [`CCDA_MSG_INVALID_KEY`].
#[must_use]
pub fn trimmed(text: &str) -> &str {
    text.trim_end_matches(' ')
}

/// The fallback `ABEND-ROUTINE` moves to `ABEND-MSG` when that field holds low-values
/// (`COACTVWC.cbl:918-919`, `COACTUPC.cbl:4205-4206`, `COCRDSLC.cbl:859-860`,
/// `COCRDUPC.cbl:1533-1534`, `COTRTUPC.cbl:1677-1678`).
///
/// `ABEND-MSG` starts as spaces (`CSMSG02Y.cpy:28-29`) and no statement moves low-values to it,
/// so the source never sends this text: the abend message is blank on every path but
/// [`ABEND_MSG_UNEXPECTED_DATA`]'s (rule `COTRTUPC-038`).
pub const ABEND_MSG_UNEXPECTED_ABEND: &str = "UNEXPECTED ABEND OCCURRED.";

/// The message of the `WHEN OTHER` branch of each program's main `EVALUATE`.
///
/// `COACTUPC` (`COACTUPC.cbl:2633-2640`), `COCRDUPC` (`COCRDUPC.cbl:1019-1026`) and `COTRTUPC`
/// (`COTRTUPC.cbl:1073-1080`) move it to `ABEND-MSG` and send [`AbendData`] (rule
/// `COTRTUPC-037`); `COACTVWC` (`COACTVWC.cbl:375-382`) and `COCRDSLC` (`COCRDSLC.cbl:373-380`)
/// move it to `WS-RETURN-MSG` and send it as plain text. The audits find the branch unreachable
/// in every program but `COTRTUPC`.
pub const ABEND_MSG_UNEXPECTED_DATA: &str = "UNEXPECTED DATA SCENARIO";

/// The `ABEND-CODE` the same `WHEN OTHER` branches move before the abend (`COACTVWC.cbl:377`,
/// `COACTUPC.cbl:2635`, `COCRDSLC.cbl:375`, `COCRDUPC.cbl:1021`, `COTRTUPC.cbl:1075`).
pub const ABEND_CODE_0001: &str = "0001";

/// The abend code of every `ABEND-ROUTINE`: the `ABCODE` of its `EXEC CICS ABEND`
/// (`COACTVWC.cbl:934-936`, `COACTUPC.cbl:4222-4224`, `COCRDSLC.cbl:875-877`,
/// `COCRDUPC.cbl:1550-1552`), and in `COTRTUPC` the `ABEND-CODE` it sends, replacing
/// [`ABEND_CODE_0001`] (`COTRTUPC.cbl:1682,1695-1697`, rule `COTRTUPC-037`).
pub const ABEND_CODE_9999: &str = "9999";

/// `ABEND-DATA`, the 134-byte abend work area `ABEND-ROUTINE` sends (`CSMSG02Y.cpy:21-29`).
///
/// [`Default`] is the copybook's initial value: every field `VALUE SPACES`. No endpoint returns
/// this record; an unexpected failure is a 500 with no COBOL text (rule `COTRTUPC-038`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AbendData {
    /// `ABEND-CODE PIC X(4)`.
    pub code: PicX<4>,
    /// `ABEND-CULPRIT PIC X(8)`, the program name.
    pub culprit: PicX<8>,
    /// `ABEND-REASON PIC X(50)`.
    pub reason: PicX<50>,
    /// `ABEND-MSG PIC X(72)`.
    pub msg: PicX<72>,
}

impl AbendData {
    /// `LENGTH OF ABEND-DATA`: the four fields in copybook order.
    pub const WIDTH: usize =
        PicX::<4>::WIDTH + PicX::<8>::WIDTH + PicX::<50>::WIDTH + PicX::<72>::WIDTH;

    /// The record after a COBOL alphanumeric `MOVE` of each text into its field: left-justified,
    /// space-padded on the right and truncated on the right at the field's width, byte by byte.
    /// Total for every input.
    #[must_use]
    pub fn new(code: &str, culprit: &str, reason: &str, msg: &str) -> Self {
        Self {
            code: PicX::moved_from(code.as_bytes()),
            culprit: PicX::moved_from(culprit.as_bytes()),
            reason: PicX::moved_from(reason.as_bytes()),
            msg: PicX::moved_from(msg.as_bytes()),
        }
    }

    /// The record as `EXEC CICS SEND FROM (ABEND-DATA)` sends it: code, culprit, reason and
    /// message, each at its full width.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::WIDTH] {
        let mut out = [b' '; Self::WIDTH];
        let fields = self
            .code
            .as_bytes()
            .iter()
            .chain(self.culprit.as_bytes())
            .chain(self.reason.as_bytes())
            .chain(self.msg.as_bytes());
        for (target, &byte) in out.iter_mut().zip(fields) {
            *target = byte;
        }
        out
    }
}

const _: () = assert!(AbendData::WIDTH == 134);

#[cfg(test)]
mod tests {
    use super::{
        ABEND_CODE_0001, ABEND_CODE_9999, ABEND_MSG_UNEXPECTED_ABEND, ABEND_MSG_UNEXPECTED_DATA,
        AbendData, CCDA_MSG_INVALID_KEY, CCDA_MSG_THANK_YOU, trimmed,
    };

    /// `text` space-padded on the right to `width` bytes.
    fn padded(text: &str, width: usize) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.resize(width, b' ');
        bytes
    }

    #[test]
    fn shared_texts_are_their_fifty_byte_field_values() {
        for (text, shown) in [
            (
                CCDA_MSG_THANK_YOU,
                "Thank you for using CardDemo application...",
            ),
            (
                CCDA_MSG_INVALID_KEY,
                "Invalid key pressed. Please see below...",
            ),
        ] {
            assert_eq!(text.len(), 50);
            assert_eq!(trimmed(text), shown);
            assert_eq!(text.as_bytes(), padded(shown, 50).as_slice());
        }
    }

    #[test]
    fn trimmed_drops_only_trailing_spaces() {
        for (text, expected) in [
            ("", ""),
            ("     ", ""),
            ("  lead  ", "  lead"),
            ("tab\t ", "tab\t"),
            ("inner  space", "inner  space"),
        ] {
            assert_eq!(trimmed(text), expected);
        }
    }

    #[test]
    fn abend_literals_are_the_cobol_texts() {
        assert_eq!(ABEND_MSG_UNEXPECTED_ABEND, "UNEXPECTED ABEND OCCURRED.");
        assert_eq!(ABEND_MSG_UNEXPECTED_DATA, "UNEXPECTED DATA SCENARIO");
        assert_eq!(ABEND_CODE_0001, "0001");
        assert_eq!(ABEND_CODE_9999, "9999");
    }

    #[test]
    fn abend_data_is_sent_as_its_134_byte_layout() {
        let data = AbendData::new(ABEND_CODE_0001, "COACTUPC", "", ABEND_MSG_UNEXPECTED_DATA);
        let bytes = data.to_bytes();
        assert_eq!(bytes.len(), 134);
        let mut expected = b"0001COACTUPC".to_vec();
        expected.extend(padded("", 50));
        expected.extend(padded(ABEND_MSG_UNEXPECTED_DATA, 72));
        assert_eq!(bytes.as_slice(), expected.as_slice());
        assert_eq!(
            data.msg.as_bytes().as_slice(),
            padded(ABEND_MSG_UNEXPECTED_DATA, 72)
        );
    }

    #[test]
    fn abend_data_moves_truncate_on_the_right() {
        let long_reason = "R".repeat(60);
        let long_msg = "M".repeat(80);
        let data = AbendData::new("99999", "COTRTUPCX", &long_reason, &long_msg);
        let mut expected = b"9999COTRTUPC".to_vec();
        expected.extend(padded(&"R".repeat(50), 50));
        expected.extend(padded(&"M".repeat(72), 72));
        assert_eq!(data.to_bytes().as_slice(), expected.as_slice());
    }

    #[test]
    fn default_is_the_copybook_value_spaces() {
        let initial = AbendData::default();
        assert_eq!(initial.to_bytes(), [b' '; AbendData::WIDTH]);
        assert_eq!(initial, AbendData::new("", "", "", ""));
        assert!(initial.msg.is_spaces());
    }
}
