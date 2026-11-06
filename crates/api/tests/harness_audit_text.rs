//! The contract of `common::audit_text`, the renderer that expands the reviewed
//! STRING-built message templates of the validation audit into the exact texts
//! the API error tests expect.
//!
//! The renderer is the oracle those tests hold the response to, so it must lay
//! out each picture as a COBOL `MOVE` does, cut at the receiving width without
//! padding, and refuse any binding or receiver the audit row contradicts. The
//! expected texts here are read from the audit cell or assembled from two
//! parts: the literal runs are the constants of the message catalog
//! `carddemo_domain::messages`, since every test imports its literal messages
//! from there, and the operand bytes are laid out by hand from the cited COBOL
//! pictures. No catalog composer is called, so the contract holds
//! independently of the production composers.
//!
//! It needs no container, database or runtime: only the audit under
//! `docs/parity/`.

pub mod common;

use std::panic::{self, AssertUnwindSafe};

use carddemo_audit::Document;
use carddemo_domain::messages;
use common::audit_text::{Operand, Picture, Template};
use common::validation_audit;

/// `WS-RETURN-MSG PIC X(75)` of COACTUPC [app/cbl/COACTUPC.cbl:L479] and
/// `WS-ERROR-MSG PIC X(75)` of COCRDLIC [app/cbl/COCRDLIC.cbl:L117].
const RETURN_MSG: usize = 75;

/// `WS-MESSAGE PIC X(80)` of COPAUS0C
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L37], COPAUS1C
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L37] and CORPT00C
/// [app/cbl/CORPT00C.cbl:L39].
const MESSAGE: usize = 80;

/// `WS-FRD-ACT-MSG PIC X(50)` of COPAUS2C
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L86].
const FRAUD_MSG: usize = 50;

/// The `ID` of the one row of a [`synthetic`] audit. It is not shaped like an
/// audit ID, so the coverage gates neither count nor reject it.
const SYNTHETIC: &str = "SYNTH-1";

/// The one-byte `'"'` literal CORPT00C strings ahead of `CONFIRMI`
/// [app/cbl/CORPT00C.cbl:L486]. The catalog keeps it private to its composer,
/// so it is written out here; the closing quote opens
/// [`messages::IS_NOT_A_VALID_VALUE_TO_CONFIRM`].
const CONFIRM_QUOTE: &str = "\"";

/// `ERROR-OPNAME PIC X(8)` holding `READ` [app/cbl/COACTUPC.cbl:L392-393].
fn opname() -> Operand {
    Operand::alphanumeric("ERROR-OPNAME", "READ", 8)
}

/// `ERROR-FILE PIC X(9)` holding the 8-byte file name `name`
/// [app/cbl/COACTUPC.cbl:L396-397].
fn file(name: &str) -> Operand {
    Operand::alphanumeric("ERROR-FILE", name, 9)
}

/// `ERROR-RESP PIC X(10)` [app/cbl/COACTUPC.cbl:L401-402], or `ERROR-RESP2`
/// [app/cbl/COACTUPC.cbl:L405-406], `name`, holding the 9-digit display of
/// `code` and a space.
fn resp(name: &str, code: u64) -> Operand {
    Operand::unsigned_to_alphanumeric(name, code, 9, 10)
}

/// The four bindings of the `WS-FILE-ERROR-MESSAGE` templates: `READ` of the
/// 8-byte file name `file_name` with RESP `code` and RESP2 `reason`.
fn file_error(file_name: &str, code: u64, reason: u64) -> Vec<Operand> {
    vec![
        opname(),
        file(file_name),
        resp("ERROR-RESP", code),
        resp("ERROR-RESP2", reason),
    ]
}

/// The `Message text` cell of the validation row `id`, as the audit parser
/// gives it.
fn message_cell<'d>(audit: &'d Document, id: &str) -> &'d str {
    let (table, row) = audit
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit defines no row {id}"));
    table
        .cell(row, "Message text")
        .unwrap_or_else(|| panic!("row {id} has no `Message text` cell"))
}

/// The worked text the note of row `id` quotes after `gives '`, up to the
/// next `'`, read from the cell rather than retyped.
fn worked_text(audit: &Document, id: &str) -> String {
    let cell = message_cell(audit, id);
    let (_, quoted) = cell
        .split_once("gives '")
        .unwrap_or_else(|| panic!("row {id} quotes no worked text: {cell}"));
    let (text, _) = quoted
        .split_once('\'')
        .unwrap_or_else(|| panic!("the worked text of row {id} is not closed: {cell}"));
    text.to_owned()
}

/// One operand built by a `MOVE`, with the bytes and picture it must hold.
struct Moved {
    /// What the case moves, for the failure text.
    case: &'static str,
    /// The operand under test.
    operand: Operand,
    /// The bytes the field must hold.
    bytes: &'static str,
    /// The picture the operand must carry.
    picture: Picture,
}

/// The case `case`: `operand` must hold `bytes` under `picture`.
fn moved(case: &'static str, operand: Operand, bytes: &'static str, picture: Picture) -> Moved {
    Moved {
        case,
        operand,
        bytes,
        picture,
    }
}

/// The layouts the `PIC X(n)` moves must give.
fn alphanumeric_moves() -> Vec<Moved> {
    vec![
        moved(
            "X(8) filled with spaces on the right",
            opname(),
            "READ    ",
            Picture::Alphanumeric(8),
        ),
        moved(
            "X(9) of an 8-byte name",
            file("CARDDAT "),
            "CARDDAT  ",
            Picture::Alphanumeric(9),
        ),
        moved(
            "X(4) cut at the right",
            Operand::alphanumeric("ERROR-FILE", "CXACAIX ", 4),
            "CXAC",
            Picture::Alphanumeric(4),
        ),
        moved(
            "X(1) of an empty value",
            Operand::alphanumeric("CONFIRMI", "", 1),
            " ",
            Picture::Alphanumeric(1),
        ),
        moved(
            "9(9) moved to X(10) keeps a trailing space",
            resp("ERROR-RESP", 13),
            "000000013 ",
            Picture::Alphanumeric(10),
        ),
        moved(
            "9(9) moved to X(5) is cut at the right",
            Operand::unsigned_to_alphanumeric("ERROR-RESP2", 80, 9, 5),
            "00000",
            Picture::Alphanumeric(5),
        ),
    ]
}

/// The layouts the `PIC 9(n)` and `PIC +9(n)` moves must give.
fn numeric_moves() -> Vec<Moved> {
    vec![
        moved(
            "9(9) filled with zeros on the left",
            Operand::unsigned("WS-RESP-CD-DIS", 13, 9),
            "000000013",
            Picture::Unsigned(9),
        ),
        moved(
            "9(4) of zero",
            Operand::unsigned("WS-REAS-CD-DIS", 0, 4),
            "0000",
            Picture::Unsigned(4),
        ),
        moved(
            "9(4) keeps the low-order digits",
            Operand::unsigned("WS-REAS-CD-DIS", 1_234_567, 4),
            "4567",
            Picture::Unsigned(4),
        ),
        moved(
            "9(20) of the largest u64",
            Operand::unsigned("WS-RESP-CD-DIS", u64::MAX, 20),
            "18446744073709551615",
            Picture::Unsigned(20),
        ),
        moved(
            "9(22), wider than every u64",
            Operand::unsigned("WS-RESP-CD-DIS", u64::MAX, 22),
            "0018446744073709551615",
            Picture::Unsigned(22),
        ),
        moved(
            "+9(6) of a positive value",
            Operand::signed_edited("WS-SQLCODE", 100, 6),
            "+000100",
            Picture::SignedEdited(6),
        ),
        moved(
            "+9(9) of a positive value",
            Operand::signed_edited("WS-SQLSTATE", 2000, 9),
            "+000002000",
            Picture::SignedEdited(9),
        ),
        moved(
            "+9(6) of a negative value",
            Operand::signed_edited("WS-SQLCODE", -803, 6),
            "-000803",
            Picture::SignedEdited(6),
        ),
        moved(
            "+9(6) of zero is signed +",
            Operand::signed_edited("WS-SQLCODE", 0, 6),
            "+000000",
            Picture::SignedEdited(6),
        ),
        moved(
            "+9(3) keeps the low-order digits of the magnitude",
            Operand::signed_edited("WS-SQLCODE", -1_234_567, 3),
            "-567",
            Picture::SignedEdited(3),
        ),
        moved(
            "+9(19) of the most negative i64",
            Operand::signed_edited("WS-SQLCODE", i64::MIN, 19),
            "-9223372036854775808",
            Picture::SignedEdited(19),
        ),
    ]
}

#[test]
fn each_picture_lays_out_the_moved_value_as_a_cobol_move_does() {
    for Moved {
        case,
        operand,
        bytes,
        picture,
    } in alphanumeric_moves().into_iter().chain(numeric_moves())
    {
        assert_eq!(operand.bytes(), bytes, "the bytes of {case}");
        assert_eq!(operand.picture(), picture, "the picture of {case}");
    }
}

#[test]
fn the_cross_reference_not_found_text_equals_the_worked_example_of_its_row() {
    let audit = validation_audit();
    let id = "V-COACTUPC-004";
    // P-CVACT01Y-016: account 99999999999 has no cross-reference, so the
    // CXACAIX read answers NOTFND, RESP 13 and RESP2 80.
    let text = Template::load(&audit, id).expand(
        &[
            Operand::alphanumeric("WS-CARD-RID-ACCT-ID-X", "99999999999", 11),
            resp("ERROR-RESP", 13),
            resp("ERROR-RESP2", 80),
        ],
        RETURN_MSG,
    );
    assert_eq!(text, worked_text(&audit, id), "{id}");
    assert_eq!(text.len(), RETURN_MSG, "{id}: {text:?}");
}

#[test]
fn the_card_browse_file_error_splits_into_its_literals_and_operands() {
    let audit = validation_audit();
    let template = Template::load(&audit, "V-COCRDLIC-008");
    assert_eq!(
        template.literals(),
        [
            messages::FILE_ERROR,
            messages::FILE_ERROR_ON,
            messages::FILE_ERROR_RETURNED_RESP,
            messages::FILE_ERROR_RESP2,
        ]
    );
    assert_eq!(
        template.operands(),
        ["ERROR-OPNAME", "ERROR-FILE", "ERROR-RESP", "ERROR-RESP2"]
    );
    let text = template.expand(&file_error("CARDDAT ", 17, 0), RETURN_MSG);
    assert_eq!(text.len(), RETURN_MSG, "{text:?}");
    let literal = Template::load(&audit, "V-COCRDLIC-001");
    assert!(literal.operands().is_empty(), "{:?}", literal.operands());
    assert_eq!(literal.literals(), [literal.text()]);
}

/// One expansion of a reviewed row with bindings its note accepts.
struct Expansion {
    /// The validation row.
    id: &'static str,
    /// The bindings of its operands.
    operands: Vec<Operand>,
    /// The width of the receiving field.
    receiver: usize,
    /// The text the COBOL builds: its literals from the catalog constants, its
    /// operands laid out from their pictures.
    expected: String,
}

/// Expansions of the `WS-FILE-ERROR-MESSAGE` group, whose notes state the
/// pictures of its fields and the receiver it is moved to.
fn file_error_expansions() -> Vec<Expansion> {
    vec![
        // The 80-byte group without its trailing FILLER X(5) is exactly the
        // 75 bytes of WS-ERROR-MSG [app/cbl/COCRDLIC.cbl:L153-171].
        Expansion {
            id: "V-COCRDLIC-008",
            operands: file_error("CARDDAT ", 17, 0),
            receiver: RETURN_MSG,
            expected: [
                messages::FILE_ERROR,
                "READ    ",
                messages::FILE_ERROR_ON,
                "CARDDAT  ",
                messages::FILE_ERROR_RETURNED_RESP,
                "000000017 ",
                messages::FILE_ERROR_RESP2,
                "000000000 ",
            ]
            .concat(),
        },
        Expansion {
            id: "V-COACTUPC-005",
            operands: file_error("CXACAIX ", 12, 14),
            receiver: RETURN_MSG,
            expected: [
                messages::FILE_ERROR,
                "READ    ",
                messages::FILE_ERROR_ON,
                "CXACAIX  ",
                messages::FILE_ERROR_RETURNED_RESP,
                "000000012 ",
                messages::FILE_ERROR_RESP2,
                "000000014 ",
            ]
            .concat(),
        },
    ]
}

/// Expansions of STRING-built rows whose notes state pictures or a receiver,
/// a template with `||` and one with `\"`, and a literal row.
fn string_expansions() -> Vec<Expansion> {
    vec![
        // WS-ACCT-ID X(11), WS-RESP-CD-DIS and WS-REAS-CD-DIS 9(09)
        // [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L44-50]; the
        // 73-byte text is not padded to the 80 bytes of WS-MESSAGE.
        Expansion {
            id: "V-COPAUS0C-004",
            operands: vec![
                Operand::alphanumeric("WS-ACCT-ID", "99999999999", 11),
                Operand::unsigned("WS-RESP-CD-DIS", 13, 9),
                Operand::unsigned("WS-REAS-CD-DIS", 80, 9),
            ],
            receiver: MESSAGE,
            expected: [
                messages::ACCOUNT_LABEL,
                "99999999999",
                messages::NOT_FOUND_IN_XREF_FILE_RESP,
                "000000013",
                messages::REAS,
                "000000080",
            ]
            .concat(),
        },
        // WS-SQLCODE +9(06) and WS-SQLSTATE +9(09)
        // [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L55-56]: a
        // negative SQLCODE fills WS-FRD-ACT-MSG to its last byte.
        Expansion {
            id: "V-COPAUS1C-009",
            operands: vec![
                Operand::signed_edited("WS-SQLCODE", -904, 6),
                Operand::signed_edited("WS-SQLSTATE", 57_011, 9),
            ],
            receiver: FRAUD_MSG,
            expected: [
                messages::SYSTEM_ERROR_DB2_CODE,
                "-000904",
                messages::DB2_STATE,
                "+000057011",
            ]
            .concat(),
        },
        Expansion {
            id: "V-COPAUS1C-010",
            operands: vec![
                Operand::signed_edited("WS-SQLCODE", 100, 6),
                Operand::signed_edited("WS-SQLSTATE", 2000, 9),
            ],
            receiver: FRAUD_MSG,
            expected: [
                messages::UPDT_ERROR_DB2_CODE,
                "+000100",
                messages::DB2_STATE,
                "+000002000",
            ]
            .concat(),
        },
        // IMS-RETURN-CODE X(02)
        // [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L79].
        Expansion {
            id: "V-COPAUS1C-011",
            operands: vec![Operand::alphanumeric("IMS-RETURN-CODE", "GE", 2)],
            receiver: MESSAGE,
            expected: [messages::SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK, "GE"].concat(),
        },
        // CONFIRMI X(1) [app/cpy-bms/CORPT00.CPY:L114] between the literal
        // double quotes [app/cbl/CORPT00C.cbl:L488].
        Expansion {
            id: "V-CORPT00C-018",
            operands: vec![Operand::alphanumeric("CONFIRMI", "X", 1)],
            receiver: MESSAGE,
            expected: [
                CONFIRM_QUOTE,
                "X",
                messages::IS_NOT_A_VALID_VALUE_TO_CONFIRM,
            ]
            .concat(),
        },
        // The literal of 2210-EDIT-ACCOUNT [app/cbl/COCRDLIC.cbl:L1022].
        Expansion {
            id: "V-COCRDLIC-001",
            operands: Vec::new(),
            receiver: RETURN_MSG,
            expected: messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT.to_owned(),
        },
    ]
}

#[test]
fn each_reviewed_template_expands_to_the_text_its_cobol_builds() {
    let audit = validation_audit();
    for Expansion {
        id,
        operands,
        receiver,
        expected,
    } in file_error_expansions()
        .into_iter()
        .chain(string_expansions())
    {
        let text = Template::load(&audit, id).expand(&operands, receiver);
        assert_eq!(text, expected, "{id}");
        assert!(
            text.len() <= receiver,
            "{id}: {text:?} exceeds X({receiver})"
        );
    }
}

/// A one-row validation audit whose row [`SYNTHETIC`] holds the
/// `Message text` `cell` and has the review-log `verdicts`.
fn synthetic(cell: &str, verdicts: &[&str]) -> Document {
    let mut lines = vec![
        "| ID | Message text |".to_owned(),
        "|---|---|".to_owned(),
        format!("| {SYNTHETIC} | {cell} |"),
        String::new(),
        "| Row ID | Verdict | Note | Reviewer |".to_owned(),
        "|---|---|---|---|".to_owned(),
    ];
    lines.extend(
        verdicts
            .iter()
            .map(|verdict| format!("| {SYNTHETIC} | {verdict} | note | reviewer |")),
    );
    Document::parse(&lines.join("\n"))
}

/// The synthetic row with the `Message text` `cell`, reviewed `pass`, loaded.
fn synthetic_template(cell: &str) -> Template {
    Template::load(&synthetic(cell, &["pass"]), SYNTHETIC)
}

/// One misuse the renderer must refuse, with texts its panic must carry.
struct Refused {
    /// What the case does wrong, for the failure text.
    case: &'static str,
    /// The misuse, given the parsed validation audit.
    misuse: fn(&Document),
    /// Texts the panic must contain: the row it names and what differs.
    names: &'static [&'static str],
}

/// The bindings and receivers `Template::expand` and `Operand` must refuse
/// whatever the note states.
fn misbound_cases() -> Vec<Refused> {
    vec![
        Refused {
            case: "an operand without a binding",
            misuse: |audit| {
                let mut operands = file_error("CXACAIX ", 12, 14);
                operands.pop();
                let _ = Template::load(audit, "V-COACTUPC-005").expand(&operands, RETURN_MSG);
            },
            names: &["V-COACTUPC-005", "{ERROR-RESP2} has no binding"],
        },
        Refused {
            case: "a binding no operand uses",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COCRDLIC-001").expand(&[opname()], RETURN_MSG);
            },
            names: &["V-COCRDLIC-001", "ERROR-OPNAME is used by no operand"],
        },
        Refused {
            case: "two bindings of one name",
            misuse: |audit| {
                let mut operands = file_error("CXACAIX ", 12, 14);
                operands.push(resp("ERROR-RESP", 13));
                let _ = Template::load(audit, "V-COACTUPC-005").expand(&operands, RETURN_MSG);
            },
            names: &["V-COACTUPC-005", "ERROR-RESP is bound twice"],
        },
        Refused {
            case: "a receiver of no bytes",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COCRDLIC-001").expand(&[], 0);
            },
            names: &["V-COCRDLIC-001", "PIC X(0) is not a COBOL field"],
        },
        Refused {
            case: "a value that is not ASCII",
            misuse: |_| {
                let _ = Operand::alphanumeric("ERROR-FILE", "CARDD\u{c4}T", 9);
            },
            names: &["operand ERROR-FILE", "is not ASCII"],
        },
        Refused {
            case: "a picture of no digits",
            misuse: |_| {
                let _ = Operand::unsigned("WS-RESP-CD-DIS", 13, 0);
            },
            names: &["operand WS-RESP-CD-DIS", "PIC 9(0) is not a COBOL picture"],
        },
        Refused {
            case: "a template that is not ASCII",
            misuse: |_| {
                let _ = synthetic_template("`\"\u{c4}nderung {ERROR-RESP}\"`")
                    .expand(&[resp("ERROR-RESP", 13)], RETURN_MSG);
            },
            names: &[SYNTHETIC, "the template is not ASCII"],
        },
    ]
}

/// The bindings and receivers `Template::expand` must refuse because the note
/// of the row states otherwise.
fn contradicted_cases() -> Vec<Refused> {
    vec![
        Refused {
            case: "a binding narrower than the X(8) the note states",
            misuse: |audit| {
                let mut operands = file_error("CXACAIX ", 12, 14);
                operands[0] = Operand::alphanumeric("ERROR-OPNAME", "READ", 7);
                let _ = Template::load(audit, "V-COACTUPC-005").expand(&operands, RETURN_MSG);
            },
            names: &[
                "V-COACTUPC-005",
                "ERROR-OPNAME is bound as PIC X(7), but the note states PIC X(8)",
            ],
        },
        Refused {
            case: "a 9(9) binding where the note states ERROR-RESP and ERROR-RESP2 PIC X(10)",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COACTUPC-004").expand(
                    &[
                        Operand::alphanumeric("WS-CARD-RID-ACCT-ID-X", "99999999999", 11),
                        Operand::unsigned("ERROR-RESP", 13, 9),
                        resp("ERROR-RESP2", 80),
                    ],
                    RETURN_MSG,
                );
            },
            names: &[
                "V-COACTUPC-004",
                "ERROR-RESP is bound as PIC 9(9), but the note states PIC X(10)",
            ],
        },
        Refused {
            case: "a +9(6) binding where the note states PIC +9(09)",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COPAUS1C-009").expand(
                    &[
                        Operand::signed_edited("WS-SQLCODE", -904, 6),
                        Operand::signed_edited("WS-SQLSTATE", 57_011, 6),
                    ],
                    FRAUD_MSG,
                );
            },
            names: &[
                "V-COPAUS1C-009",
                "WS-SQLSTATE is bound as PIC +9(6), but the note states PIC +9(9)",
            ],
        },
        Refused {
            case: "a receiver other than the X(75) the group is moved to",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COACTUPC-005")
                    .expand(&file_error("CXACAIX ", 12, 14), RETURN_MSG - 1);
            },
            names: &[
                "V-COACTUPC-005",
                "the receiver is PIC X(74), but the note states X(75)",
            ],
        },
        Refused {
            case: "a receiver other than the 75 bytes the text is cut at",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COACTUPC-004").expand(
                    &[
                        Operand::alphanumeric("WS-CARD-RID-ACCT-ID-X", "99999999999", 11),
                        resp("ERROR-RESP", 13),
                        resp("ERROR-RESP2", 80),
                    ],
                    MESSAGE,
                );
            },
            names: &[
                "V-COACTUPC-004",
                "the receiver is PIC X(80), but the note states X(75)",
            ],
        },
        Refused {
            case: "a receiver other than the WS-FRD-ACT-MSG X(50) the text is built into",
            misuse: |audit| {
                let _ = Template::load(audit, "V-COPAUS1C-010").expand(
                    &[
                        Operand::signed_edited("WS-SQLCODE", 100, 6),
                        Operand::signed_edited("WS-SQLSTATE", 2000, 9),
                    ],
                    MESSAGE,
                );
            },
            names: &[
                "V-COPAUS1C-010",
                "the receiver is PIC X(80), but the note states X(50)",
            ],
        },
    ]
}

/// The rows `Template::load` must refuse: missing, unreviewed or holding a
/// malformed template.
fn malformed_cases() -> Vec<Refused> {
    vec![
        Refused {
            case: "a row the audit does not define",
            misuse: |audit| {
                let _ = Template::load(audit, "NO-SUCH-ROW");
            },
            names: &["defines no data row NO-SUCH-ROW"],
        },
        Refused {
            case: "a row without a review verdict",
            misuse: |_| {
                let _ = Template::load(&synthetic("`\"Text.\"`", &[]), SYNTHETIC);
            },
            names: &[SYNTHETIC, "has not passed its review"],
        },
        Refused {
            case: "a row with a failing verdict besides a passing one",
            misuse: |_| {
                let _ = Template::load(&synthetic("`\"Text.\"`", &["pass", "fail"]), SYNTHETIC);
            },
            names: &[SYNTHETIC, "has not passed its review"],
        },
        Refused {
            case: "a message text that is not double-quoted",
            misuse: |_| {
                let _ = synthetic_template("`Text.`");
            },
            names: &[SYNTHETIC, "is not a double-quoted string"],
        },
        Refused {
            case: "an escape other than \\\" and \\\\",
            misuse: |_| {
                let _ = synthetic_template("`\"Line\\nbreak\"`");
            },
            names: &[SYNTHETIC, "holds an escape other than"],
        },
        Refused {
            case: "a double quote not written \\\"",
            misuse: |_| {
                let _ = synthetic_template("`\"Say \"hi\"\"`");
            },
            names: &[SYNTHETIC, "holds a double quote not written"],
        },
        Refused {
            case: "a { without a }",
            misuse: |_| {
                let _ = synthetic_template("`\"Resp:{ERROR-RESP\"`");
            },
            names: &[SYNTHETIC, "the `{` at byte 5 has no `}`"],
        },
        Refused {
            case: "braces enclosing no field name",
            misuse: |_| {
                let _ = synthetic_template("`\"Resp:{}\"`");
            },
            names: &[SYNTHETIC, "`{}` at byte 5 encloses no COBOL field name"],
        },
    ]
}

#[test]
fn each_misuse_panics_naming_the_row_and_what_differs() {
    let audit = validation_audit();
    for Refused {
        case,
        misuse,
        names,
    } in misbound_cases()
        .into_iter()
        .chain(contradicted_cases())
        .chain(malformed_cases())
    {
        let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| misuse(&audit))) else {
            panic!("the renderer accepted {case}");
        };
        let text = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_else(|| panic!("the panic for {case} carries no text"));
        for name in names {
            assert!(
                text.contains(name),
                "the panic for {case} must name {name:?}: {text}"
            );
        }
    }
}
