//! The message catalog: every message text the carried COBOL programs show,
//! return, log, `DISPLAY` or write to a reject or error record, held once and
//! byte-identical to the audited text, plus the texts the request itself writes
//! for authentication, rewards and the unexpected-error fallback. Rules,
//! handlers, flows, batch jobs and tests take their texts from here and never
//! spell one out themselves.
//!
//! # Sources and gate
//!
//! A constant exists only for a text a passing audit row records:
//!
//! - every "Message text" of `docs/parity/validation-audit.md` (rows
//!   `V-<PROGRAM>-<nnn>`). A `no-message` row, such as the missing `version`
//!   token of E-24, records no text and gets no constant;
//! - every message quoted in sections B to F of
//!   `docs/parity/parity-oracle-audit.md` (rows `P-<KEY>-<nnn>`), and the
//!   members of a message family a row names by one member or by their common
//!   text when the lines the row cites hold those literals (the CBTRN02C close
//!   texts of P-CBTRN02C-020, the CBSTM03A open texts of P-CBSTM03A-002).
//!
//! Screen titles, PF-key legends, 3270 input prompts and cancel texts, menu
//! option texts (E-56), texts no path shows (E-67), file and program names,
//! record field values and output-layout labels are not messages; the layouts
//! live in [`crate::format`].
//!
//! # Byte rules
//!
//! - A constant is the COBOL `VALUE` or `MOVE` literal as declared, trailing
//!   spaces included, and is never padded to the field that receives it.
//!   [`CCDA_MSG_THANK_YOU`] keeps the 49 bytes of its `VALUE`, not the 50 of
//!   its `X(50)` field.
//! - Texts that differ in one byte are separate constants. A program suffix
//!   names each of two near-identical texts.
//! - Each doc comment cites the audit rows that use the text and the COBOL
//!   `path:line` that declares it.
//!
//! # Composers
//!
//! Some messages are built at run time by `STRING … DELIMITED BY … INTO`. Each
//! composer reproduces one such statement: operands in order, an operand
//! `DELIMITED BY SPACE` cut before its first space, a fixed-width field moved
//! to its `PIC X(n)` first, and the result cut at the receiver width. `STRING`
//! leaves the receiver bytes after the last one it writes unchanged, so a
//! composer returns only the bytes written; [`cobtupdt_return_msg`] lays them
//! over the one receiver the COBOL never clears (P-COBTUPDT-010).
//!
//! A `DISPLAY` of a literal and a field prints the field's bytes directly
//! after the literal. [`file_status_message`] and the CBTRN02C counter lines
//! are the displays whose field needs editing.

use rust_decimal::Decimal;

use crate::format::{
    EditedPicture, delimited_by, edit, edit_sqlcode, move_alnum, unsigned_display,
};

// --- Receivers and edited fields of the composed messages

/// `WS-RETURN-MSG PIC X(75)` of COACTVWC, COACTUPC, COCRDSLC, COCRDUPC,
/// COTRTUPC and COTRTLIC, and `WS-ERROR-MSG PIC X(75)` of COCRDLIC.
const RETURN_MSG_LEN: usize = 75;

/// `WS-MESSAGE PIC X(80)` of COSGN00C, COTRN00C to COTRN02C, CORPT00C,
/// COBIL00C, COUSR00C to COUSR03C, COPAUS0C and COPAUS1C.
const WS_MESSAGE_LEN: usize = 80;

/// COPAUS2C `WS-FRD-ACT-MSG PIC X(50)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L86].
const FRD_ACT_MSG_LEN: usize = 50;

/// COBTUPDT `WS-RETURN-MSG PIC X(80)`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L61].
const COBTUPDT_RETURN_MSG_LEN: usize = 80;

/// `WS-EDIT-VARIABLE-NAME PIC X(25)`: a longer field label loses its tail
/// before `FUNCTION TRIM` sees it (E-45).
const EDIT_VARIABLE_NAME_LEN: usize = 25;

/// COTRTLIC `WS-DB2-CURRENT-ACTION PIC X(72)`, trimmed by 9999-FORMAT-DB2-MESSAGE.
const DB2_CURRENT_ACTION_LEN: usize = 72;

/// `WS-CARD-RID-ACCT-ID-X` and `WS-ACCT-ID`, both `PIC X(11)`.
const ACCOUNT_ID_LEN: usize = 11;

/// `WS-CARD-RID-CUST-ID-X PIC X(9)`.
const CUSTOMER_ID_LEN: usize = 9;

/// `TRAN-ID PIC X(16)` [app/cpy/CVTRA05Y.cpy].
const TRAN_ID_LEN: usize = 16;

/// `SEC-USR-ID PIC X(08)` [app/cpy/CSUSR01Y.cpy].
const USER_ID_LEN: usize = 8;

/// CORPT00C `WS-REPORT-NAME PIC X(10)` [app/cbl/CORPT00C.cbl:L58].
const REPORT_NAME_LEN: usize = 10;

/// CORPT00 `CONFIRMI PIC X(1)` [app/cpy-bms/CORPT00.CPY:L114].
const CONFIRM_LEN: usize = 1;

/// `IMS-RETURN-CODE PIC X(02)` of COPAUS0C and COPAUS1C.
const IMS_RETURN_CODE_LEN: usize = 2;

/// COACCT01 `WS-FUNC PIC X(04)` [app/app-vsam-mq/cbl/COACCT01.cbl:L110].
const CDRA_FUNCTION_LEN: usize = 4;

/// COBTUPDT `WS-INPUT-REC`: type `X(1)`, code `X(2)`, description `X(50)`
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L71-77].
const COBTUPDT_INPUT_REC_LEN: usize = 53;

/// The digits of a `DISPLAY` copy of a CICS `RESP` or `RESP2` fullword:
/// `ERROR-RESP`, `WS-RESP-CD-DIS PIC 9(09)`.
const RESP_DIGITS: usize = 9;

/// `ERROR-RESP` and `ERROR-RESP2 PIC X(10)` of `WS-FILE-ERROR-MESSAGE`.
const ERROR_RESP_LEN: usize = 10;

/// `WS-TRANSACTION-COUNT` and `WS-REJECT-COUNT PIC 9(09)`
/// [app/cbl/CBTRN02C.cbl:L185-186].
const COUNT_DIGITS: usize = 9;

/// COPAUS2C `WS-SQLCODE PIC +9(06)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L55].
const PIC_FRAUD_SQLCODE: EditedPicture = EditedPicture::new("+9(06)");

/// COPAUS2C `WS-SQLSTATE PIC +9(09)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L56].
const PIC_FRAUD_SQLSTATE: EditedPicture = EditedPicture::new("+9(09)");

/// The `':'` COTRTUPC strings between the edited SQLCODE and `SQLERRM`
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1503].
const SQLERRM_SEPARATOR: &str = ":";

/// The `' '` 9999-FORMAT-DB2-MESSAGE strings before the DSNTIAC text
/// [app/app-transaction-type-db2/cpy/CSDB2RPY.cpy:L78].
const DSNTIAC_SEPARATOR: &str = " ";

/// The `'.'` that ends the CT02 and CB00 success texts
/// [app/cbl/COTRN02C.cbl:L732] [app/cbl/COBIL00C.cbl:L530].
const TRAN_ID_TERMINATOR: &str = ".";

/// The `'"'` CORPT00C strings around the rejected confirmation value
/// [app/cbl/CORPT00C.cbl:L486].
const CONFIRM_QUOTE: &str = "\"";

// --- Common (CSMSG01Y). CSMSG02Y declares no message literal: its abend text
// is set by the programs and never shown (E-67).

/// The `CCDA-MSG-THANK-YOU` `VALUE` literal: 49 bytes as declared and as the
/// audit quotes it, so the constant is not padded to the 50 bytes of its
/// `X(50)` field; [`move_alnum`] to 50 gives the field. COSGN00C sends the field
/// in its 80-byte `WS-MESSAGE`, 30 spaces after it, and `POST /auth/logout`
/// returns it — P-COSGN00C-003, P-COSGN00C-016 · app/cpy/CSMSG01Y.cpy:L19
pub const CCDA_MSG_THANK_YOU: &str = "Thank you for using CardDemo application...      ";

/// The `CCDA-MSG-INVALID-KEY` `VALUE` literal: 49 bytes as declared, not padded
/// to its `X(50)` field — P-COADM01C-004, P-COBIL00C-002,
/// P-COMEN01C-005, P-COPAUS0C-002, P-COPAUS1C-002, P-CORPT00C-001,
/// P-COSGN00C-004, P-COTRN00C-001, P-COTRN01C-002, P-COTRN02C-002,
/// P-COUSR00C-001, P-COUSR01C-001, P-COUSR02C-001, P-COUSR03C-001 ·
/// app/cpy/CSMSG01Y.cpy:L21
pub const CCDA_MSG_INVALID_KEY: &str = "Invalid key pressed. Please see below...         ";

// --- Sign-on (COSGN00C)

/// V-COSGN00C-001, P-COSGN00C-005 · app/cbl/COSGN00C.cbl:L120
pub const PLEASE_ENTER_USER_ID: &str = "Please enter User ID ...";

/// V-COSGN00C-002, P-COSGN00C-006 · app/cbl/COSGN00C.cbl:L125
pub const PLEASE_ENTER_PASSWORD: &str = "Please enter Password ...";

/// V-COSGN00C-005, P-COSGN00C-010 · app/cbl/COSGN00C.cbl:L242
pub const WRONG_PASSWORD_TRY_AGAIN: &str = "Wrong Password. Try again ...";

/// V-COSGN00C-003, P-COSGN00C-011 · app/cbl/COSGN00C.cbl:L249
pub const USER_NOT_FOUND_TRY_AGAIN: &str = "User not found. Try again ...";

/// V-COSGN00C-004, P-COSGN00C-012 · app/cbl/COSGN00C.cbl:L254
pub const UNABLE_TO_VERIFY_THE_USER: &str = "Unable to verify the User ...";

// --- Accounts (COACTVWC, COACTUPC with CSUTLDPY); the key texts are shared
// with the card programs

/// V-COACTUPC-002, V-COACTVWC-001, V-COCRDSLC-005, V-COCRDUPC-006,
/// P-COACTUPC-003, P-COACTVWC-021, P-COCRDSLC-015 · app/cbl/COACTUPC.cbl:L490,
/// app/cbl/COACTVWC.cbl:L124, app/cbl/COCRDSLC.cbl:L143, app/cbl/COCRDUPC.cbl:L186
pub const NO_INPUT_RECEIVED: &str = "No input received";

/// V-COCRDSLC-001, V-COCRDUPC-002, P-COACTUPC-003, P-COACTVWC-004,
/// P-COACTVWC-021, P-COCRDSLC-002, P-COCRDSLC-015, P-COCRDUPC-011 ·
/// app/cbl/COACTUPC.cbl:L484, app/cbl/COACTVWC.cbl:L122, app/cbl/COCRDSLC.cbl:L139,
/// app/cbl/COCRDUPC.cbl:L178
pub const ACCOUNT_NUMBER_NOT_PROVIDED: &str = "Account number not provided";

/// The two literals 1210-EDIT-ACCOUNT strings together (E-45) — V-COACTUPC-003,
/// P-COACTUPC-003 · app/cbl/COACTUPC.cbl:L1807-1808
pub const ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO: &str =
    "Account Number if supplied must be a 11 digit Non-Zero Number";

/// Two spaces after "must", as declared — V-COACTVWC-002, P-COACTVWC-005 ·
/// app/cbl/COACTVWC.cbl:L672
pub const ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT: &str =
    "Account Filter must  be a non-zero 11 digit number";

/// Head of the COACTVWC and COACTUPC not-found texts and of the COPAUS0C
/// account lookup texts — V-COACTUPC-004, V-COACTUPC-006, V-COACTVWC-003,
/// V-COACTVWC-005, V-COPAUS0C-004 to V-COPAUS0C-007, P-COACTUPC-043,
/// P-COACTUPC-044, P-COACTVWC-007, P-COACTVWC-009, P-COPAUS0C-007 ·
/// app/cbl/COACTUPC.cbl:L3675, app/cbl/COACTUPC.cbl:L3724, app/cbl/COACTVWC.cbl:L748,
/// app/cbl/COACTVWC.cbl:L797, app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L837
pub const ACCOUNT_LABEL: &str = "Account:";

/// V-COACTUPC-004, V-COACTUPC-006, V-COACTVWC-003, V-COACTVWC-005,
/// P-COACTUPC-043, P-COACTUPC-044, P-COACTVWC-007, P-COACTVWC-009 ·
/// app/cbl/COACTUPC.cbl:L3677, app/cbl/COACTUPC.cbl:L3726, app/cbl/COACTVWC.cbl:L750,
/// app/cbl/COACTVWC.cbl:L799
pub const NOT_FOUND_IN: &str = " not found in";

/// V-COACTUPC-004, V-COACTVWC-003, P-COACTUPC-043, P-COACTVWC-007 ·
/// app/cbl/COACTUPC.cbl:L3678, app/cbl/COACTVWC.cbl:L751
pub const CROSS_REF_FILE_RESP: &str = " Cross ref file.  Resp:";

/// V-COACTUPC-006, V-COACTVWC-005, P-COACTUPC-044, P-COACTVWC-009 ·
/// app/cbl/COACTUPC.cbl:L3727, app/cbl/COACTVWC.cbl:L800
pub const ACCT_MASTER_FILE_RESP: &str = " Acct Master file.Resp:";

/// V-COACTUPC-004, V-COACTUPC-006, V-COACTVWC-003, V-COACTVWC-005,
/// V-COPAUS0C-004 to V-COPAUS0C-009, P-COACTUPC-043, P-COACTUPC-044,
/// P-COACTVWC-007, P-COACTVWC-009, P-COPAUS0C-007 · app/cbl/COACTUPC.cbl:L3680,
/// app/cbl/COACTUPC.cbl:L3729, app/cbl/COACTVWC.cbl:L753, app/cbl/COACTVWC.cbl:L802,
/// app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L840
pub const REAS: &str = " Reas:";

/// V-COACTUPC-008, V-COACTVWC-007, P-COACTUPC-045, P-COACTVWC-010 ·
/// app/cbl/COACTUPC.cbl:L3774, app/cbl/COACTVWC.cbl:L847
pub const CUST_ID_LABEL: &str = "CustId:";

/// V-COACTUPC-008, V-COACTVWC-007, P-COACTUPC-045, P-COACTVWC-010 ·
/// app/cbl/COACTUPC.cbl:L3776, app/cbl/COACTVWC.cbl:L849
pub const NOT_FOUND: &str = " not found";

/// V-COACTUPC-008, V-COACTVWC-007, P-COACTUPC-045, P-COACTVWC-010 ·
/// app/cbl/COACTUPC.cbl:L3777, app/cbl/COACTVWC.cbl:L850
pub const IN_CUSTOMER_MASTER_RESP: &str = " in customer master.Resp: ";

/// V-COACTUPC-008, V-COACTVWC-007, P-COACTUPC-045 · app/cbl/COACTUPC.cbl:L3779,
/// app/cbl/COACTVWC.cbl:L852
pub const REAS_UPPER: &str = " REAS:";

/// The first `FILLER X(12)` of `WS-FILE-ERROR-MESSAGE`; COCRDLIC declares
/// `'File Error:'`, which its `X(12)` pads to these bytes — V-COACTUPC-005,
/// V-COACTUPC-007, V-COACTUPC-009, V-COACTVWC-004, V-COACTVWC-006,
/// V-COACTVWC-008, V-COCRDLIC-008, V-COCRDLIC-010, V-COCRDSLC-007,
/// V-COCRDUPC-008, P-COACTVWC-008, P-COCRDSLC-005, P-COCRDUPC-023 ·
/// app/cbl/COACTUPC.cbl:L391, app/cbl/COACTVWC.cbl:L88, app/cbl/COCRDLIC.cbl:L155,
/// app/cbl/COCRDSLC.cbl:L104, app/cbl/COCRDUPC.cbl:L135
pub const FILE_ERROR: &str = "File Error: ";

/// `FILLER X(4)` of `WS-FILE-ERROR-MESSAGE` — the [`FILE_ERROR`] rows ·
/// app/cbl/COACTUPC.cbl:L395, app/cbl/COCRDLIC.cbl:L159
pub const FILE_ERROR_ON: &str = " on ";

/// `FILLER X(15)` of `WS-FILE-ERROR-MESSAGE` — the [`FILE_ERROR`] rows ·
/// app/cbl/COACTUPC.cbl:L400, app/cbl/COCRDLIC.cbl:L164
pub const FILE_ERROR_RETURNED_RESP: &str = " returned RESP ";

/// `FILLER X(7)` of `WS-FILE-ERROR-MESSAGE` — the [`FILE_ERROR`] rows ·
/// app/cbl/COACTUPC.cbl:L404, app/cbl/COCRDLIC.cbl:L168
pub const FILE_ERROR_RESP2: &str = ",RESP2 ";

/// V-COACTUPC-010, V-COCRDUPC-009, V-COTRTUPC-006, P-COACTUPC-005,
/// P-COTRTUPC-016 · app/cbl/COACTUPC.cbl:L492, app/cbl/COCRDUPC.cbl:L188,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L180
pub const NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED: &str =
    "No change detected with respect to values fetched.";

/// V-COACTUPC-123, P-COACTUPC-007 · app/cbl/COACTUPC.cbl:L2550
pub const INVALID_ZIP_CODE_FOR_STATE: &str = "Invalid zip code for state";

/// V-COACTUPC-124, P-COACTUPC-010 · app/cbl/COACTUPC.cbl:L518
pub const COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE: &str =
    "Could not lock account record for update";

/// The information text shown with a failed lock or rewrite — P-COACTUPC-010 ·
/// app/cbl/COACTUPC.cbl:L477
pub const CHANGES_UNSUCCESSFUL_PLEASE_TRY_AGAIN: &str = "Changes unsuccessful. Please try again";

/// V-COACTUPC-125, P-COACTUPC-011 · app/cbl/COACTUPC.cbl:L520
pub const COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE: &str =
    "Could not lock customer record for update";

/// `PUT /accounts/{id}` and `PUT /cards/{id}` answer it with 409 —
/// V-COACTUPC-126, V-COCRDUPC-019, P-COACTUPC-012, P-COCRDUPC-005 ·
/// app/cbl/COACTUPC.cbl:L522, app/cbl/COCRDUPC.cbl:L208
pub const RECORD_CHANGED_BY_SOME_ONE_ELSE: &str = "Record changed by some one else. Please review";

/// V-COACTUPC-127, V-COACTUPC-128, V-COCRDUPC-020, P-COACTUPC-013,
/// P-COACTUPC-014, P-COCRDUPC-006 · app/cbl/COACTUPC.cbl:L524, app/cbl/COCRDUPC.cbl:L210
pub const UPDATE_OF_RECORD_FAILED: &str = "Update of record failed";

/// The success text of the account, card and transaction-type updates —
/// P-COACTUPC-015, P-COCRDUPC-007, P-COTRTUPC-007, P-COTRTUPC-009,
/// P-COTRTUPC-024 · app/cbl/COACTUPC.cbl:L475, app/cbl/COCRDUPC.cbl:L169,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L163
pub const CHANGES_COMMITTED_TO_DATABASE: &str = "Changes committed to database";

// --- Field-edit texts: [`field_edit_message`] strings the trimmed field
// label before each of these (COACTUPC 1200 to 1280 edits, CSUTLDPY date
// edits, COTRTUPC and COTRTLIC key and description edits)

/// V-COACTUPC-011, V-COACTUPC-024, V-COACTUPC-037, V-COACTUPC-050,
/// V-COACTUPC-052, V-COACTUPC-054, V-COACTUPC-056, V-COACTUPC-060,
/// V-COACTUPC-063, V-COACTUPC-078, V-COACTUPC-082, V-COACTUPC-085,
/// V-COACTUPC-087, V-COACTUPC-088, V-COACTUPC-091, V-COACTUPC-094,
/// V-COACTUPC-096, V-COACTUPC-118, V-COACTUPC-121, V-COTRTUPC-001,
/// V-COTRTUPC-007, V-COTRTUPC-012, V-COTRTUPC-017, V-COTRTUPC-022,
/// V-COTRTLIC-013, P-COACTUPC-020 to P-COACTUPC-023, P-COACTUPC-025,
/// P-COACTUPC-026, P-COTRTLIC-021, P-COTRTUPC-017, P-COTRTUPC-018 ·
/// app/cbl/COACTUPC.cbl:L1841, app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L866,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1198
pub const MUST_BE_SUPPLIED: &str = " must be supplied.";

/// V-COACTUPC-012, V-COACTUPC-122, P-COACTUPC-021 · app/cbl/COACTUPC.cbl:L1886
pub const MUST_BE_Y_OR_N: &str = " must be Y or N.";

/// V-COACTUPC-013, V-COACTUPC-026, V-COACTUPC-039, V-COACTUPC-066,
/// P-CSUTLDPY-002 · app/cpy/CSUTLDPY.cpy:L37
pub const YEAR_MUST_BE_SUPPLIED: &str = " : Year must be supplied.";

/// V-COACTUPC-014, V-COACTUPC-027, V-COACTUPC-040, V-COACTUPC-067,
/// P-CSUTLDPY-003 · app/cpy/CSUTLDPY.cpy:L54
pub const MUST_BE_4_DIGIT_NUMBER: &str = " must be 4 digit number.";

/// V-COACTUPC-015, V-COACTUPC-028, V-COACTUPC-041, V-COACTUPC-068,
/// P-CSUTLDPY-004 · app/cpy/CSUTLDPY.cpy:L79
pub const CENTURY_NOT_VALID: &str = " : Century is not valid.";

/// V-COACTUPC-016, V-COACTUPC-029, V-COACTUPC-042, V-COACTUPC-069,
/// P-CSUTLDPY-005 · app/cpy/CSUTLDPY.cpy:L101
pub const MONTH_MUST_BE_SUPPLIED: &str = " : Month must be supplied.";

/// V-COACTUPC-017, V-COACTUPC-030, V-COACTUPC-043, V-COACTUPC-070,
/// P-CSUTLDPY-006 · app/cpy/CSUTLDPY.cpy:L119, app/cpy/CSUTLDPY.cpy:L136
pub const MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12: &str =
    ": Month must be a number between 1 and 12.";

/// V-COACTUPC-018, V-COACTUPC-031, V-COACTUPC-044, V-COACTUPC-071,
/// P-CSUTLDPY-007 · app/cpy/CSUTLDPY.cpy:L161
pub const DAY_MUST_BE_SUPPLIED: &str = " : Day must be supplied.";

/// V-COACTUPC-019, V-COACTUPC-032, V-COACTUPC-045, V-COACTUPC-072,
/// P-CSUTLDPY-008 · app/cpy/CSUTLDPY.cpy:L180, app/cpy/CSUTLDPY.cpy:L195
pub const DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31: &str = ":day must be a number between 1 and 31.";

/// V-COACTUPC-020, V-COACTUPC-033, V-COACTUPC-046, V-COACTUPC-073,
/// P-CSUTLDPY-009 · app/cpy/CSUTLDPY.cpy:L221
pub const CANNOT_HAVE_31_DAYS_IN_THIS_MONTH: &str = ":Cannot have 31 days in this month.";

/// V-COACTUPC-021, V-COACTUPC-034, V-COACTUPC-047, V-COACTUPC-074,
/// P-CSUTLDPY-010 · app/cpy/CSUTLDPY.cpy:L236
pub const CANNOT_HAVE_30_DAYS_IN_THIS_MONTH: &str = ":Cannot have 30 days in this month.";

/// V-COACTUPC-022, V-COACTUPC-035, V-COACTUPC-048, V-COACTUPC-075,
/// P-CSUTLDPY-011 · app/cpy/CSUTLDPY.cpy:L266
pub const NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS: &str =
    ":Not a leap year.Cannot have 29 days in this month.";

/// The text before the CSUTLDTC severity in [`date_validation_error_message`] —
/// V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049, V-COACTUPC-076,
/// P-CSUTLDPY-012 · app/cpy/CSUTLDPY.cpy:L308
pub const VALIDATION_ERROR_SEV_CODE: &str = " validation error Sev code: ";

/// The text before the CSUTLDTC message number in
/// [`date_validation_error_message`] — the [`VALIDATION_ERROR_SEV_CODE`] rows ·
/// app/cpy/CSUTLDPY.cpy:L310
pub const MESSAGE_CODE: &str = " Message code: ";

/// The trailing space is declared — V-COACTUPC-077, P-CSUTLDPY-013 ·
/// app/cpy/CSUTLDPY.cpy:L363
pub const CANNOT_BE_IN_THE_FUTURE: &str = ":cannot be in the future ";

/// V-COACTUPC-025, V-COACTUPC-038, V-COACTUPC-051, V-COACTUPC-053,
/// V-COACTUPC-055, P-COACTUPC-026 · app/cbl/COACTUPC.cbl:L2209
pub const IS_NOT_VALID: &str = " is not valid";

/// V-COACTUPC-057, V-COACTUPC-061, V-COACTUPC-064, V-COACTUPC-079,
/// V-COACTUPC-092, V-COACTUPC-119, P-COACTUPC-025 · app/cbl/COACTUPC.cbl:L2146
pub const MUST_BE_ALL_NUMERIC: &str = " must be all numeric.";

/// V-COACTUPC-058, V-COACTUPC-062, V-COACTUPC-065, V-COACTUPC-080,
/// V-COACTUPC-093, V-COACTUPC-120, V-COTRTUPC-003, V-COTRTUPC-014,
/// V-COTRTUPC-024, P-COACTUPC-025, P-COTRTUPC-018 · app/cbl/COACTUPC.cbl:L2163,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L961
pub const MUST_NOT_BE_ZERO: &str = " must not be zero.";

/// V-COACTUPC-059, P-COACTUPC-031 · app/cbl/COACTUPC.cbl:L2457
pub const SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999: &str =
    ": should not be 000, 666, or between 900 and 999";

/// V-COACTUPC-081, P-COACTUPC-033 · app/cbl/COACTUPC.cbl:L2523
pub const SHOULD_BE_BETWEEN_300_AND_850: &str = ": should be between 300 and 850";

/// V-COACTUPC-083, V-COACTUPC-084, V-COACTUPC-086, V-COACTUPC-089,
/// V-COACTUPC-095, V-COACTUPC-097, P-COACTUPC-022, P-COACTUPC-024 ·
/// app/cbl/COACTUPC.cbl:L1941, app/cbl/COACTUPC.cbl:L2047
pub const CAN_HAVE_ALPHABETS_ONLY: &str = " can have alphabets only.";

/// V-COACTUPC-090, P-COACTUPC-032 · app/cbl/COACTUPC.cbl:L2503
pub const IS_NOT_A_VALID_STATE_CODE: &str = ": is not a valid state code";

/// V-COACTUPC-098, V-COACTUPC-108, P-COACTUPC-028 · app/cbl/COACTUPC.cbl:L2254
pub const AREA_CODE_MUST_BE_SUPPLIED: &str = ": Area code must be supplied.";

/// V-COACTUPC-099, V-COACTUPC-109, P-COACTUPC-028 · app/cbl/COACTUPC.cbl:L2272
pub const AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER: &str = ": Area code must be A 3 digit number.";

/// V-COACTUPC-100, V-COACTUPC-110, P-COACTUPC-028 · app/cbl/COACTUPC.cbl:L2286
pub const AREA_CODE_CANNOT_BE_ZERO: &str = ": Area code cannot be zero";

/// V-COACTUPC-101, V-COACTUPC-111, P-COACTUPC-028 · app/cbl/COACTUPC.cbl:L2306
pub const NOT_VALID_NORTH_AMERICA_AREA_CODE: &str =
    ": Not valid North America general purpose area code";

/// V-COACTUPC-102, V-COACTUPC-112, P-COACTUPC-029 · app/cbl/COACTUPC.cbl:L2325
pub const PREFIX_CODE_MUST_BE_SUPPLIED: &str = ": Prefix code must be supplied.";

/// V-COACTUPC-103, V-COACTUPC-113, P-COACTUPC-029 · app/cbl/COACTUPC.cbl:L2343
pub const PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER: &str = ": Prefix code must be A 3 digit number.";

/// V-COACTUPC-104, V-COACTUPC-114, P-COACTUPC-029 · app/cbl/COACTUPC.cbl:L2357
pub const PREFIX_CODE_CANNOT_BE_ZERO: &str = ": Prefix code cannot be zero";

/// V-COACTUPC-105, V-COACTUPC-115, P-COACTUPC-030 · app/cbl/COACTUPC.cbl:L2378
pub const LINE_NUMBER_CODE_MUST_BE_SUPPLIED: &str = ": Line number code must be supplied.";

/// V-COACTUPC-106, V-COACTUPC-116, P-COACTUPC-030 · app/cbl/COACTUPC.cbl:L2396
pub const LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER: &str =
    ": Line number code must be A 4 digit number.";

/// V-COACTUPC-107, V-COACTUPC-117, P-COACTUPC-030 · app/cbl/COACTUPC.cbl:L2410
pub const LINE_NUMBER_CODE_CANNOT_BE_ZERO: &str = ": Line number code cannot be zero";

/// V-COTRTUPC-002, V-COTRTUPC-013, V-COTRTUPC-023, P-COTRTUPC-018 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L943
pub const MUST_BE_NUMERIC: &str = " must be numeric.";

/// V-COTRTUPC-008, V-COTRTUPC-018, V-COTRTLIC-014, P-COACTUPC-023,
/// P-COTRTLIC-021, P-COTRTUPC-017 · app/cbl/COACTUPC.cbl:L1999,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L893,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1225
pub const CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY: &str = " can have numbers or alphabets only.";

// --- Field labels: the values `WS-EDIT-VARIABLE-NAME` receives before each
// edit, the first operand of [`field_edit_message`]

/// V-COACTUPC-011, V-COACTUPC-012 · app/cbl/COACTUPC.cbl:L1472
pub const FIELD_ACCOUNT_STATUS: &str = "Account Status";

/// V-COACTUPC-013 to V-COACTUPC-023 · app/cbl/COACTUPC.cbl:L1478
pub const FIELD_OPEN_DATE: &str = "Open Date";

/// V-COACTUPC-024, V-COACTUPC-025 · app/cbl/COACTUPC.cbl:L1484
pub const FIELD_CREDIT_LIMIT: &str = "Credit Limit";

/// V-COACTUPC-026 to V-COACTUPC-036 · app/cbl/COACTUPC.cbl:L1490
pub const FIELD_EXPIRY_DATE: &str = "Expiry Date";

/// V-COACTUPC-037, V-COACTUPC-038 · app/cbl/COACTUPC.cbl:L1496
pub const FIELD_CASH_CREDIT_LIMIT: &str = "Cash Credit Limit";

/// V-COACTUPC-039 to V-COACTUPC-049 · app/cbl/COACTUPC.cbl:L1503
pub const FIELD_REISSUE_DATE: &str = "Reissue Date";

/// V-COACTUPC-050, V-COACTUPC-051 · app/cbl/COACTUPC.cbl:L1509
pub const FIELD_CURRENT_BALANCE: &str = "Current Balance";

/// 26 bytes: the `X(25)` `WS-EDIT-VARIABLE-NAME` keeps "Current Cycle Credit
/// Limi", which [`field_edit_message`] reproduces (E-45) — V-COACTUPC-052,
/// V-COACTUPC-053, P-COACTUPC-026 · app/cbl/COACTUPC.cbl:L1515
pub const FIELD_CURRENT_CYCLE_CREDIT_LIMIT: &str = "Current Cycle Credit Limit";

/// V-COACTUPC-054, V-COACTUPC-055 · app/cbl/COACTUPC.cbl:L1522
pub const FIELD_CURRENT_CYCLE_DEBIT_LIMIT: &str = "Current Cycle Debit Limit";

/// V-COACTUPC-056 to V-COACTUPC-059 · app/cbl/COACTUPC.cbl:L2439
pub const FIELD_SSN_FIRST_3_CHARS: &str = "SSN: First 3 chars";

/// V-COACTUPC-060 to V-COACTUPC-062 · app/cbl/COACTUPC.cbl:L2469
pub const FIELD_SSN_4TH_AND_5TH_CHARS: &str = "SSN 4th & 5th chars";

/// V-COACTUPC-063 to V-COACTUPC-065 · app/cbl/COACTUPC.cbl:L2481
pub const FIELD_SSN_LAST_4_CHARS: &str = "SSN Last 4 chars";

/// V-COACTUPC-066 to V-COACTUPC-077 · app/cbl/COACTUPC.cbl:L1533
pub const FIELD_DATE_OF_BIRTH: &str = "Date of Birth";

/// V-COACTUPC-078 to V-COACTUPC-081 · app/cbl/COACTUPC.cbl:L1545
pub const FIELD_FICO_SCORE: &str = "FICO Score";

/// V-COACTUPC-082, V-COACTUPC-083 · app/cbl/COACTUPC.cbl:L1560
pub const FIELD_FIRST_NAME: &str = "First Name";

/// V-COACTUPC-084 · app/cbl/COACTUPC.cbl:L1568
pub const FIELD_MIDDLE_NAME: &str = "Middle Name";

/// V-COACTUPC-085, V-COACTUPC-086 · app/cbl/COACTUPC.cbl:L1576
pub const FIELD_LAST_NAME: &str = "Last Name";

/// V-COACTUPC-087 · app/cbl/COACTUPC.cbl:L1584
pub const FIELD_ADDRESS_LINE_1: &str = "Address Line 1";

/// V-COACTUPC-088 to V-COACTUPC-090 · app/cbl/COACTUPC.cbl:L1592
pub const FIELD_STATE: &str = "State";

/// V-COACTUPC-091 to V-COACTUPC-093 · app/cbl/COACTUPC.cbl:L1605
pub const FIELD_ZIP: &str = "Zip";

/// V-COACTUPC-094, V-COACTUPC-095 · app/cbl/COACTUPC.cbl:L1615
pub const FIELD_CITY: &str = "City";

/// V-COACTUPC-096, V-COACTUPC-097 · app/cbl/COACTUPC.cbl:L1623
pub const FIELD_COUNTRY: &str = "Country";

/// V-COACTUPC-098 to V-COACTUPC-107 · app/cbl/COACTUPC.cbl:L1632
pub const FIELD_PHONE_NUMBER_1: &str = "Phone Number 1";

/// V-COACTUPC-108 to V-COACTUPC-117 · app/cbl/COACTUPC.cbl:L1640
pub const FIELD_PHONE_NUMBER_2: &str = "Phone Number 2";

/// V-COACTUPC-118 to V-COACTUPC-120 · app/cbl/COACTUPC.cbl:L1648
pub const FIELD_EFT_ACCOUNT_ID: &str = "EFT Account Id";

/// V-COACTUPC-121, V-COACTUPC-122 · app/cbl/COACTUPC.cbl:L1657
pub const FIELD_PRIMARY_CARD_HOLDER: &str = "Primary Card Holder";

/// V-COTRTUPC-007, V-COTRTUPC-008, V-COTRTUPC-017, V-COTRTUPC-018,
/// V-COTRTLIC-013, V-COTRTLIC-014, P-COTRTLIC-017 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L758,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1083
pub const FIELD_TRANSACTION_DESC: &str = "Transaction Desc";

/// V-COTRTUPC-001 to V-COTRTUPC-003, V-COTRTUPC-012 to V-COTRTUPC-014,
/// V-COTRTUPC-022 to V-COTRTUPC-024 · app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L826
pub const FIELD_TRAN_TYPE_CODE: &str = "Tran Type code";

// --- Cards (COCRDLIC, COCRDSLC, COCRDUPC)

/// V-COCRDLIC-001, V-COCRDSLC-002, V-COCRDUPC-003, P-COCRDLIC-005,
/// P-COCRDSLC-002, P-COCRDUPC-011 · app/cbl/COCRDLIC.cbl:L1022,
/// app/cbl/COCRDSLC.cbl:L670, app/cbl/COCRDUPC.cbl:L745
pub const ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT: &str =
    "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";

/// V-COCRDSLC-003, V-COCRDUPC-004, P-COCRDSLC-003, P-COCRDUPC-012 ·
/// app/cbl/COCRDSLC.cbl:L141, app/cbl/COCRDUPC.cbl:L180
pub const CARD_NUMBER_NOT_PROVIDED: &str = "Card number not provided";

/// V-COCRDLIC-002, V-COCRDSLC-004, V-COCRDUPC-005, P-COCRDLIC-006,
/// P-COCRDSLC-003, P-COCRDUPC-012 · app/cbl/COCRDLIC.cbl:L1058,
/// app/cbl/COCRDSLC.cbl:L711, app/cbl/COCRDUPC.cbl:L789
pub const CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT: &str =
    "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

/// V-COCRDSLC-006, V-COCRDUPC-007, P-COCRDSLC-004, P-COCRDUPC-023 ·
/// app/cbl/COCRDSLC.cbl:L154, app/cbl/COCRDUPC.cbl:L204
pub const DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION: &str =
    "Did not find cards for this search condition";

/// The three leading spaces are declared: the information text of a
/// successful card read — P-COCRDSLC-010 · app/cbl/COCRDSLC.cbl:L130
pub const DISPLAYING_REQUESTED_DETAILS: &str = "   Displaying requested details";

/// V-COCRDUPC-010, P-COCRDUPC-013 · app/cbl/COCRDUPC.cbl:L182
pub const CARD_NAME_NOT_PROVIDED: &str = "Card name not provided";

/// V-COCRDUPC-011, P-COCRDUPC-013 · app/cbl/COCRDUPC.cbl:L184
pub const CARD_NAME_CAN_ONLY_CONTAIN_ALPHABETS_AND_SPACES: &str =
    "Card name can only contain alphabets and spaces";

/// V-COCRDUPC-012, V-COCRDUPC-013, P-COCRDUPC-014 · app/cbl/COCRDUPC.cbl:L196
pub const CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N: &str = "Card Active Status must be Y or N";

/// V-COCRDUPC-014, V-COCRDUPC-015, P-COCRDUPC-015 · app/cbl/COCRDUPC.cbl:L198
pub const CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12: &str =
    "Card expiry month must be between 1 and 12";

/// V-COCRDUPC-016, V-COCRDUPC-017, P-COCRDUPC-016 · app/cbl/COCRDUPC.cbl:L200
pub const INVALID_CARD_EXPIRY_YEAR: &str = "Invalid card expiry year";

/// V-COCRDUPC-018, V-COTRTUPC-009, V-COTRTUPC-019, P-COCRDUPC-004,
/// P-COTRTUPC-008 · app/cbl/COCRDUPC.cbl:L206,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L182
pub const COULD_NOT_LOCK_RECORD_FOR_UPDATE: &str = "Could not lock record for update";

/// V-COCRDLIC-003, P-COCRDLIC-007 · app/cbl/COCRDLIC.cbl:L124
pub const PLEASE_SELECT_ONLY_ONE_RECORD_TO_VIEW_OR_UPDATE: &str =
    "PLEASE SELECT ONLY ONE RECORD TO VIEW OR UPDATE";

/// V-COCRDLIC-004, P-COCRDLIC-008 · app/cbl/COCRDLIC.cbl:L126
pub const INVALID_ACTION_CODE: &str = "INVALID ACTION CODE";

/// Upper case, unlike [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`] —
/// V-COCRDLIC-009, P-COCRDLIC-011 · app/cbl/COCRDLIC.cbl:L903
pub const NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC: &str = "NO PREVIOUS PAGES TO DISPLAY";

/// Upper case, unlike [`NO_MORE_PAGES_TO_DISPLAY_COTRTLIC`] — V-COCRDLIC-005,
/// P-COCRDLIC-011 · app/cbl/COCRDLIC.cbl:L908
pub const NO_MORE_PAGES_TO_DISPLAY_COCRDLIC: &str = "NO MORE PAGES TO DISPLAY";

/// V-COCRDLIC-007, P-COCRDLIC-012 · app/cbl/COCRDLIC.cbl:L1219,
/// app/cbl/COCRDLIC.cbl:L1239
pub const NO_MORE_RECORDS_TO_SHOW: &str = "NO MORE RECORDS TO SHOW";

/// Upper case with a full stop, unlike
/// [`NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC`] — V-COCRDLIC-006,
/// P-COCRDLIC-012 · app/cbl/COCRDLIC.cbl:L122
pub const NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC: &str =
    "NO RECORDS FOUND FOR THIS SEARCH CONDITION.";

// --- Transactions (COTRN00C, COTRN01C, COTRN02C); texts COBIL00C shares with
// COTRN02C are here too

/// V-COTRN00C-002, V-COPAUS0C-003, P-COPAUS0C-005, P-COTRN00C-004 ·
/// app/cbl/COTRN00C.cbl:L199, app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L328
pub const INVALID_SELECTION_VALID_VALUE_IS_S: &str = "Invalid selection. Valid value is S";

/// V-COTRN00C-004, P-COTRN00C-005 · app/cbl/COTRN00C.cbl:L214
pub const TRAN_ID_MUST_BE_NUMERIC: &str = "Tran ID must be Numeric ...";

/// V-COTRN00C-009, V-COUSR00C-007, V-COPAUS0C-014, P-COPAUS0C-011,
/// P-COTRN00C-009, P-COUSR00C-004 · app/cbl/COTRN00C.cbl:L248,
/// app/cbl/COUSR00C.cbl:L251, app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L381
pub const YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE: &str =
    "You are already at the top of the page...";

/// V-COTRN00C-003, V-COUSR00C-002, V-COPAUS0C-013, P-COPAUS0C-012,
/// P-COTRN00C-010, P-COUSR00C-005 · app/cbl/COTRN00C.cbl:L270,
/// app/cbl/COUSR00C.cbl:L273, app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L409
pub const YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE: &str =
    "You are already at the bottom of the page...";

/// V-COTRN00C-005, V-COTRN00C-010, V-COUSR00C-003, P-COTRN00C-011,
/// P-COUSR00C-006, P-COUSR00C-016 · app/cbl/COTRN00C.cbl:L608, app/cbl/COUSR00C.cbl:L603
pub const YOU_ARE_AT_THE_TOP_OF_THE_PAGE: &str = "You are at the top of the page...";

/// V-COTRN00C-007, V-COUSR00C-005, P-COTRN00C-013, P-COUSR00C-007 ·
/// app/cbl/COTRN00C.cbl:L642, app/cbl/COUSR00C.cbl:L637
pub const YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE: &str =
    "You have reached the bottom of the page...";

/// V-COTRN00C-012, V-COUSR00C-008, P-COTRN00C-014, P-COUSR00C-008,
/// P-COUSR00C-009 · app/cbl/COTRN00C.cbl:L676, app/cbl/COUSR00C.cbl:L671
pub const YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE: &str = "You have reached the top of the page...";

/// Lower-case "transaction", unlike [`UNABLE_TO_LOOKUP_TRANSACTION`] —
/// V-COTRN00C-006, V-COTRN00C-008, V-COTRN00C-011, V-COTRN00C-013,
/// P-COTRN00C-012 to P-COTRN00C-014 · app/cbl/COTRN00C.cbl:L615,
/// app/cbl/COTRN00C.cbl:L649, app/cbl/COTRN00C.cbl:L683
pub const UNABLE_TO_LOOKUP_TRANSACTION_COTRN00C: &str = "Unable to lookup transaction...";

/// V-COTRN01C-001, P-COTRN01C-003 · app/cbl/COTRN01C.cbl:L149
pub const TRAN_ID_CAN_NOT_BE_EMPTY: &str = "Tran ID can NOT be empty...";

/// V-COTRN01C-002, V-COTRN02C-029, P-COBIL00C-013, P-COTRN01C-004,
/// P-COTRN02C-021 · app/cbl/COTRN01C.cbl:L285, app/cbl/COTRN02C.cbl:L657
pub const TRANSACTION_ID_NOT_FOUND: &str = "Transaction ID NOT found...";

/// Capital "Transaction", unlike [`UNABLE_TO_LOOKUP_TRANSACTION_COTRN00C`] —
/// V-COBIL00C-011 to V-COBIL00C-013, V-COTRN01C-003, V-COTRN02C-030,
/// V-COTRN02C-031, P-COBIL00C-013, P-COBIL00C-014, P-COTRN01C-004,
/// P-COTRN02C-021, P-COTRN02C-022 · app/cbl/COBIL00C.cbl:L463,
/// app/cbl/COBIL00C.cbl:L492, app/cbl/COTRN01C.cbl:L292, app/cbl/COTRN02C.cbl:L664
pub const UNABLE_TO_LOOKUP_TRANSACTION: &str = "Unable to lookup Transaction...";

/// V-COTRN02C-001, P-COTRN02C-005 · app/cbl/COTRN02C.cbl:L199
pub const ACCOUNT_ID_MUST_BE_NUMERIC: &str = "Account ID must be Numeric...";

/// V-COTRN02C-002, P-COTRN02C-006 · app/cbl/COTRN02C.cbl:L213
pub const CARD_NUMBER_MUST_BE_NUMERIC: &str = "Card Number must be Numeric...";

/// V-COTRN02C-003, P-COTRN02C-007 · app/cbl/COTRN02C.cbl:L226
pub const ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED: &str =
    "Account or Card Number must be entered...";

/// V-COBIL00C-004, V-COBIL00C-009, V-COBIL00C-016, V-COTRN02C-004,
/// P-COBIL00C-009, P-COBIL00C-011, P-COBIL00C-019, P-COTRN02C-008 ·
/// app/cbl/COBIL00C.cbl:L361, app/cbl/COBIL00C.cbl:L392, app/cbl/COBIL00C.cbl:L425,
/// app/cbl/COTRN02C.cbl:L593
pub const ACCOUNT_ID_NOT_FOUND: &str = "Account ID NOT found...";

/// V-COTRN02C-005, P-COTRN02C-008 · app/cbl/COTRN02C.cbl:L600
pub const UNABLE_TO_LOOKUP_ACCT_IN_XREF_AIX_FILE: &str =
    "Unable to lookup Acct in XREF AIX file...";

/// V-COTRN02C-006, P-COTRN02C-009 · app/cbl/COTRN02C.cbl:L626
pub const CARD_NUMBER_NOT_FOUND: &str = "Card Number NOT found...";

/// V-COTRN02C-007, P-COTRN02C-009 · app/cbl/COTRN02C.cbl:L633
pub const UNABLE_TO_LOOKUP_CARD_IN_XREF_FILE: &str = "Unable to lookup Card # in XREF file...";

/// V-COTRN02C-008, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L254
pub const TYPE_CD_CAN_NOT_BE_EMPTY: &str = "Type CD can NOT be empty...";

/// V-COTRN02C-009, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L260
pub const CATEGORY_CD_CAN_NOT_BE_EMPTY: &str = "Category CD can NOT be empty...";

/// V-COTRN02C-010, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L266
pub const SOURCE_CAN_NOT_BE_EMPTY: &str = "Source can NOT be empty...";

/// V-COTRN02C-011, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L272
pub const DESCRIPTION_CAN_NOT_BE_EMPTY: &str = "Description can NOT be empty...";

/// V-COTRN02C-012, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L278
pub const AMOUNT_CAN_NOT_BE_EMPTY: &str = "Amount can NOT be empty...";

/// V-COTRN02C-013, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L284
pub const ORIG_DATE_CAN_NOT_BE_EMPTY: &str = "Orig Date can NOT be empty...";

/// V-COTRN02C-014, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L290
pub const PROC_DATE_CAN_NOT_BE_EMPTY: &str = "Proc Date can NOT be empty...";

/// V-COTRN02C-015, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L296
pub const MERCHANT_ID_CAN_NOT_BE_EMPTY: &str = "Merchant ID can NOT be empty...";

/// V-COTRN02C-016, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L302
pub const MERCHANT_NAME_CAN_NOT_BE_EMPTY: &str = "Merchant Name can NOT be empty...";

/// V-COTRN02C-017, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L308
pub const MERCHANT_CITY_CAN_NOT_BE_EMPTY: &str = "Merchant City can NOT be empty...";

/// V-COTRN02C-018, P-COTRN02C-010 · app/cbl/COTRN02C.cbl:L314
pub const MERCHANT_ZIP_CAN_NOT_BE_EMPTY: &str = "Merchant Zip can NOT be empty...";

/// V-COTRN02C-019, P-COTRN02C-011 · app/cbl/COTRN02C.cbl:L325
pub const TYPE_CD_MUST_BE_NUMERIC: &str = "Type CD must be Numeric...";

/// V-COTRN02C-020, P-COTRN02C-011 · app/cbl/COTRN02C.cbl:L331
pub const CATEGORY_CD_MUST_BE_NUMERIC: &str = "Category CD must be Numeric...";

/// V-COTRN02C-021, P-COTRN02C-012 · app/cbl/COTRN02C.cbl:L345
pub const AMOUNT_SHOULD_BE_IN_FORMAT: &str = "Amount should be in format -99999999.99";

/// V-COTRN02C-022, P-COTRN02C-013 · app/cbl/COTRN02C.cbl:L360
pub const ORIG_DATE_SHOULD_BE_IN_FORMAT: &str = "Orig Date should be in format YYYY-MM-DD";

/// V-COTRN02C-023, P-COTRN02C-013 · app/cbl/COTRN02C.cbl:L375
pub const PROC_DATE_SHOULD_BE_IN_FORMAT: &str = "Proc Date should be in format YYYY-MM-DD";

/// V-COTRN02C-024, P-COTRN02C-015 · app/cbl/COTRN02C.cbl:L401
pub const ORIG_DATE_NOT_A_VALID_DATE: &str = "Orig Date - Not a valid date...";

/// V-COTRN02C-025, P-COTRN02C-015 · app/cbl/COTRN02C.cbl:L421
pub const PROC_DATE_NOT_A_VALID_DATE: &str = "Proc Date - Not a valid date...";

/// V-COTRN02C-026, P-COTRN02C-016 · app/cbl/COTRN02C.cbl:L432
pub const MERCHANT_ID_MUST_BE_NUMERIC: &str = "Merchant ID must be Numeric...";

/// V-COTRN02C-027, P-COTRN02C-018, P-COTRN02C-026 · app/cbl/COTRN02C.cbl:L178
pub const CONFIRM_TO_ADD_THIS_TRANSACTION: &str = "Confirm to add this transaction...";

/// V-COBIL00C-003, V-COTRN02C-028, P-COBIL00C-008, P-COTRN02C-019 ·
/// app/cbl/COBIL00C.cbl:L187, app/cbl/COTRN02C.cbl:L184
pub const INVALID_VALUE_VALID_VALUES_ARE_Y_N: &str = "Invalid value. Valid values are (Y/N)...";

/// V-COBIL00C-014, V-COTRN02C-032, P-COBIL00C-017, P-COTRN02C-025 ·
/// app/cbl/COBIL00C.cbl:L536, app/cbl/COTRN02C.cbl:L738
pub const TRAN_ID_ALREADY_EXIST: &str = "Tran ID already exist...";

/// V-COTRN02C-033, P-COTRN02C-025 · app/cbl/COTRN02C.cbl:L745
pub const UNABLE_TO_ADD_TRANSACTION: &str = "Unable to Add Transaction...";

/// The head of [`transaction_added_message`]; the trailing space is declared —
/// P-COTRN02C-024 · app/cbl/COTRN02C.cbl:L728
pub const TRANSACTION_ADDED_SUCCESSFULLY: &str = "Transaction added successfully. ";

/// P-COTRN02C-024 · app/cbl/COTRN02C.cbl:L730
pub const YOUR_TRAN_ID_IS: &str = " Your Tran ID is ";

// --- Reports (CORPT00C)

/// V-CORPT00C-001, P-CORPT00C-009 · app/cbl/CORPT00C.cbl:L438
pub const SELECT_A_REPORT_TYPE_TO_PRINT_REPORT: &str = "Select a report type to print report...";

/// V-CORPT00C-002, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L261
pub const START_DATE_MONTH_CAN_NOT_BE_EMPTY: &str = "Start Date - Month can NOT be empty...";

/// V-CORPT00C-003, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L268
pub const START_DATE_DAY_CAN_NOT_BE_EMPTY: &str = "Start Date - Day can NOT be empty...";

/// V-CORPT00C-004, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L275
pub const START_DATE_YEAR_CAN_NOT_BE_EMPTY: &str = "Start Date - Year can NOT be empty...";

/// V-CORPT00C-005, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L282
pub const END_DATE_MONTH_CAN_NOT_BE_EMPTY: &str = "End Date - Month can NOT be empty...";

/// V-CORPT00C-006, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L289
pub const END_DATE_DAY_CAN_NOT_BE_EMPTY: &str = "End Date - Day can NOT be empty...";

/// V-CORPT00C-007, P-CORPT00C-005 · app/cbl/CORPT00C.cbl:L296
pub const END_DATE_YEAR_CAN_NOT_BE_EMPTY: &str = "End Date - Year can NOT be empty...";

/// V-CORPT00C-008, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L331
pub const START_DATE_NOT_A_VALID_MONTH: &str = "Start Date - Not a valid Month...";

/// V-CORPT00C-009, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L340
pub const START_DATE_NOT_A_VALID_DAY: &str = "Start Date - Not a valid Day...";

/// V-CORPT00C-010, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L348
pub const START_DATE_NOT_A_VALID_YEAR: &str = "Start Date - Not a valid Year...";

/// V-CORPT00C-011, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L357
pub const END_DATE_NOT_A_VALID_MONTH: &str = "End Date - Not a valid Month...";

/// V-CORPT00C-012, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L366
pub const END_DATE_NOT_A_VALID_DAY: &str = "End Date - Not a valid Day...";

/// V-CORPT00C-013, P-CORPT00C-007 · app/cbl/CORPT00C.cbl:L374
pub const END_DATE_NOT_A_VALID_YEAR: &str = "End Date - Not a valid Year...";

/// V-CORPT00C-014, P-CORPT00C-008 · app/cbl/CORPT00C.cbl:L400
pub const START_DATE_NOT_A_VALID_DATE: &str = "Start Date - Not a valid date...";

/// V-CORPT00C-015, P-CORPT00C-008 · app/cbl/CORPT00C.cbl:L420
pub const END_DATE_NOT_A_VALID_DATE: &str = "End Date - Not a valid date...";

/// The head of [`confirm_print_report_message`] — V-CORPT00C-016,
/// P-CORPT00C-010 · app/cbl/CORPT00C.cbl:L466
pub const PLEASE_CONFIRM_TO_PRINT_THE: &str = "Please confirm to print the ";

/// The tail of [`confirm_print_report_message`] — V-CORPT00C-016,
/// P-CORPT00C-010 · app/cbl/CORPT00C.cbl:L469
pub const REPORT_ELLIPSIS: &str = " report...";

/// The tail of [`invalid_confirm_value_message`] — V-CORPT00C-018,
/// P-CORPT00C-011 · app/cbl/CORPT00C.cbl:L488
pub const IS_NOT_A_VALID_VALUE_TO_CONFIRM: &str = "\" is not a valid value to confirm...";

/// The tail of [`report_submitted_message`] — P-CORPT00C-014 ·
/// app/cbl/CORPT00C.cbl:L450
pub const REPORT_SUBMITTED_FOR_PRINTING: &str = " report submitted for printing ...";

/// The `WS-REPORT-NAME` of the monthly report, the name slot of
/// [`confirm_print_report_message`] and [`report_submitted_message`] —
/// V-CORPT00C-016 · app/cbl/CORPT00C.cbl:L214
pub const REPORT_NAME_MONTHLY: &str = "Monthly";

/// The `WS-REPORT-NAME` of the yearly report — V-CORPT00C-016 ·
/// app/cbl/CORPT00C.cbl:L240
pub const REPORT_NAME_YEARLY: &str = "Yearly";

/// The `WS-REPORT-NAME` of the custom-range report — V-CORPT00C-016 ·
/// app/cbl/CORPT00C.cbl:L433
pub const REPORT_NAME_CUSTOM: &str = "Custom";

// --- Bill payment (COBIL00C)

/// V-COBIL00C-001, P-COBIL00C-004 · app/cbl/COBIL00C.cbl:L161
pub const ACCT_ID_CAN_NOT_BE_EMPTY: &str = "Acct ID can NOT be empty...";

/// V-COBIL00C-005, P-COBIL00C-009 · app/cbl/COBIL00C.cbl:L368
pub const UNABLE_TO_LOOKUP_ACCOUNT: &str = "Unable to lookup Account...";

/// V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020 · app/cbl/COBIL00C.cbl:L201
pub const YOU_HAVE_NOTHING_TO_PAY: &str = "You have nothing to pay...";

/// The 200 answer to a blank confirmation, no write — V-COBIL00C-008,
/// P-COBIL00C-007 · app/cbl/COBIL00C.cbl:L237
pub const CONFIRM_TO_MAKE_A_BILL_PAYMENT: &str = "Confirm to make a bill payment...";

/// V-COBIL00C-010, P-COBIL00C-011 · app/cbl/COBIL00C.cbl:L432
pub const UNABLE_TO_LOOKUP_XREF_AIX_FILE: &str = "Unable to lookup XREF AIX file...";

/// V-COBIL00C-015, P-COBIL00C-017 · app/cbl/COBIL00C.cbl:L543
pub const UNABLE_TO_ADD_BILL_PAY_TRANSACTION: &str = "Unable to Add Bill pay Transaction...";

/// V-COBIL00C-017, P-COBIL00C-019 · app/cbl/COBIL00C.cbl:L399
pub const UNABLE_TO_UPDATE_ACCOUNT: &str = "Unable to Update Account...";

/// The head of [`payment_successful_message`]; the trailing space is
/// declared — P-COBIL00C-016 · app/cbl/COBIL00C.cbl:L527
pub const PAYMENT_SUCCESSFUL: &str = "Payment successful. ";

/// P-COBIL00C-016 · app/cbl/COBIL00C.cbl:L528
pub const YOUR_TRANSACTION_ID_IS: &str = " Your Transaction ID is ";

// --- Authorizations (COPAUS0C, COPAUS1C, COPAUS2C)

/// V-COPAUS0C-001, P-COPAUS0C-003 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L269
pub const PLEASE_ENTER_ACCT_ID: &str = "Please enter Acct Id...";

/// V-COPAUS0C-002, P-COPAUS0C-003 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L278
pub const ACCT_ID_MUST_BE_NUMERIC: &str = "Acct Id must be Numeric ...";

/// V-COPAUS0C-004, P-COPAUS0C-007 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L839
pub const NOT_FOUND_IN_XREF_FILE_RESP: &str = " not found in XREF file. Resp:";

/// V-COPAUS0C-005, P-COPAUS0C-007 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L854
pub const SYSTEM_ERROR_WHILE_READING_XREF_FILE_RESP: &str =
    " System error while reading XREF file. Resp:";

/// V-COPAUS0C-006, P-COPAUS0C-008 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L889
pub const NOT_FOUND_IN_ACCT_FILE_RESP: &str = " not found in ACCT file. Resp:";

/// V-COPAUS0C-007, P-COPAUS0C-008 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L904
pub const SYSTEM_ERROR_WHILE_READING_ACCT_FILE_RESP: &str =
    " System error while reading ACCT file. Resp:";

/// V-COPAUS0C-008, V-COPAUS0C-009 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L938,
/// app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L953
pub const CUSTOMER_LABEL: &str = "Customer:";

/// V-COPAUS0C-008, P-COPAUS0C-008 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L940
pub const NOT_FOUND_IN_CUST_FILE_RESP: &str = " not found in CUST file. Resp:";

/// V-COPAUS0C-009, P-COPAUS0C-008 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L955
pub const SYSTEM_ERROR_WHILE_READING_CUST_FILE_RESP: &str =
    " System error while reading CUST file. Resp:";

/// An [`ims_status_message`] head — V-COPAUS0C-010, V-COPAUS1C-002,
/// P-COPAUS0C-020, P-COPAUS1C-016 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L1023,
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L596
pub const SYSTEM_ERROR_WHILE_SCHEDULING_PSB: &str = " System error while scheduling PSB: Code:";

/// An [`ims_status_message`] head; upper-case "AUTH", unlike
/// [`SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C`] — V-COPAUS0C-011,
/// P-COPAUS0C-009 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L989
pub const SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS0C: &str =
    " System error while reading AUTH Summary: Code:";

/// An [`ims_status_message`] head; upper-case "AUTH", unlike
/// [`SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C`] — V-COPAUS0C-012,
/// P-COPAUS0C-014 · app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L477
pub const SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS0C: &str =
    " System error while reading AUTH Details: Code:";

/// An [`ims_status_message`] head — V-COPAUS0C-015, P-COPAUS0C-014 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L510
pub const SYSTEM_ERROR_WHILE_REPOS_AUTH_DETAILS: &str =
    " System error while repos. AUTH Details: Code:";

/// An [`ims_status_message`] head — V-COPAUS1C-003, P-COPAUS1C-004 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L456
pub const SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C: &str =
    " System error while reading Auth Summary: Code:";

/// An [`ims_status_message`] head — V-COPAUS1C-006, P-COPAUS1C-004 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L482
pub const SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C: &str =
    " System error while reading Auth Details: Code:";

/// An [`ims_status_message`] head; the two vertical bars belong to the
/// literal (E-44) — V-COPAUS1C-011, P-COPAUS1C-008 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L545
pub const SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK: &str =
    " System error while FRAUD Tagging, ROLLBACK||";

/// An [`ims_status_message`] head — V-COPAUS1C-013, P-COPAUS1C-014 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L511
pub const SYSTEM_ERROR_WHILE_READING_NEXT_AUTH: &str =
    " System error while reading next Auth: Code:";

/// V-COPAUS1C-012, P-COPAUS1C-009, P-COPAUS1C-014 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L283
pub const ALREADY_AT_THE_LAST_AUTHORIZATION: &str = "Already at the last Authorization...";

/// The success text of removing a fraud mark — P-COPAUS1C-008 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L535
pub const AUTH_FRAUD_REMOVED: &str = "AUTH FRAUD REMOVED...";

/// The success text of a fraud mark — P-COPAUS1C-008 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L537
pub const AUTH_MARKED_FRAUD: &str = "AUTH MARKED FRAUD...";

/// A [`fraud_db2_error_message`] head: the `AUTHFRDS` insert failed —
/// V-COPAUS1C-009, P-COPAUS2C-005 · app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L211
pub const SYSTEM_ERROR_DB2_CODE: &str = " SYSTEM ERROR DB2: CODE:";

/// A [`fraud_db2_error_message`] head: the `AUTHFRDS` update failed —
/// V-COPAUS1C-010, P-COPAUS2C-005 · app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L239
pub const UPDT_ERROR_DB2_CODE: &str = " UPDT ERROR DB2: CODE:";

/// V-COPAUS1C-009, V-COPAUS1C-010, P-COPAUS2C-005 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L212,
/// app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L240
pub const DB2_STATE: &str = ", STATE: ";

// --- Users (COUSR00C to COUSR03C)

/// V-COUSR00C-001, P-COUSR00C-002 · app/cbl/COUSR00C.cbl:L212
pub const INVALID_SELECTION_VALID_VALUES_ARE_U_AND_D: &str =
    "Invalid selection. Valid values are U and D";

/// V-COUSR00C-004, V-COUSR00C-006, V-COUSR00C-009, V-COUSR02C-003,
/// V-COUSR03C-003, P-COUSR00C-006 to P-COUSR00C-008, P-COUSR00C-016,
/// P-COUSR02C-003, P-COUSR03C-003 · app/cbl/COUSR00C.cbl:L610,
/// app/cbl/COUSR00C.cbl:L644, app/cbl/COUSR00C.cbl:L678, app/cbl/COUSR02C.cbl:L349,
/// app/cbl/COUSR03C.cbl:L296
pub const UNABLE_TO_LOOKUP_USER: &str = "Unable to lookup User...";

/// V-COUSR01C-001, V-COUSR02C-005, P-COUSR01C-002, P-COUSR02C-004 ·
/// app/cbl/COUSR01C.cbl:L120, app/cbl/COUSR02C.cbl:L188
pub const FIRST_NAME_CAN_NOT_BE_EMPTY: &str = "First Name can NOT be empty...";

/// V-COUSR01C-002, V-COUSR02C-006, P-COUSR01C-002, P-COUSR02C-004 ·
/// app/cbl/COUSR01C.cbl:L126, app/cbl/COUSR02C.cbl:L194
pub const LAST_NAME_CAN_NOT_BE_EMPTY: &str = "Last Name can NOT be empty...";

/// V-COUSR01C-003, V-COUSR02C-001, V-COUSR02C-004, V-COUSR03C-001,
/// V-COUSR03C-004, P-COUSR01C-002, P-COUSR02C-002, P-COUSR02C-004,
/// P-COUSR03C-002, P-COUSR03C-004 · app/cbl/COUSR01C.cbl:L132,
/// app/cbl/COUSR02C.cbl:L148, app/cbl/COUSR02C.cbl:L182, app/cbl/COUSR03C.cbl:L147,
/// app/cbl/COUSR03C.cbl:L179
pub const USER_ID_CAN_NOT_BE_EMPTY: &str = "User ID can NOT be empty...";

/// V-COUSR01C-004, V-COUSR02C-007, P-COUSR01C-002, P-COUSR02C-004 ·
/// app/cbl/COUSR01C.cbl:L138, app/cbl/COUSR02C.cbl:L200
pub const PASSWORD_CAN_NOT_BE_EMPTY: &str = "Password can NOT be empty...";

/// V-COUSR01C-005, V-COUSR02C-008, P-COUSR01C-002, P-COUSR02C-004 ·
/// app/cbl/COUSR01C.cbl:L144, app/cbl/COUSR02C.cbl:L206
pub const USER_TYPE_CAN_NOT_BE_EMPTY: &str = "User Type can NOT be empty...";

/// V-COUSR01C-006, P-COUSR01C-005 · app/cbl/COUSR01C.cbl:L263
pub const USER_ID_ALREADY_EXIST: &str = "User ID already exist...";

/// V-COUSR01C-007, P-COUSR01C-005 · app/cbl/COUSR01C.cbl:L270
pub const UNABLE_TO_ADD_USER: &str = "Unable to Add User...";

/// V-COUSR02C-002, V-COUSR02C-011, V-COUSR03C-002, V-COUSR03C-006,
/// P-COUSR02C-003, P-COUSR02C-006, P-COUSR03C-003, P-COUSR03C-005 ·
/// app/cbl/COUSR02C.cbl:L342, app/cbl/COUSR02C.cbl:L379, app/cbl/COUSR03C.cbl:L289,
/// app/cbl/COUSR03C.cbl:L325
pub const USER_ID_NOT_FOUND: &str = "User ID NOT found...";

/// V-COUSR02C-009, V-COUSR02C-012, V-COUSR03C-005, V-COUSR03C-007,
/// P-COUSR02C-006, P-COUSR03C-005 · app/cbl/COUSR02C.cbl:L386, app/cbl/COUSR03C.cbl:L332
pub const UNABLE_TO_UPDATE_USER: &str = "Unable to Update User...";

/// V-COUSR02C-010, P-COUSR02C-005 · app/cbl/COUSR02C.cbl:L239
pub const PLEASE_MODIFY_TO_UPDATE: &str = "Please modify to update ...";

/// The head of the user success texts — P-COUSR01C-004, P-COUSR01C-011,
/// P-COUSR02C-006, P-COUSR02C-013, P-COUSR03C-005, P-COUSR03C-011 ·
/// app/cbl/COUSR01C.cbl:L255, app/cbl/COUSR02C.cbl:L372, app/cbl/COUSR03C.cbl:L318
pub const USER_LABEL: &str = "User ";

/// The tail of [`user_added_message`] — P-COUSR01C-004, P-COUSR01C-011 ·
/// app/cbl/COUSR01C.cbl:L257
pub const HAS_BEEN_ADDED: &str = " has been added ...";

/// The tail of [`user_updated_message`] — P-COUSR02C-006, P-COUSR02C-013 ·
/// app/cbl/COUSR02C.cbl:L374
pub const HAS_BEEN_UPDATED: &str = " has been updated ...";

/// The tail of [`user_deleted_message`] — P-COUSR03C-005, P-COUSR03C-011 ·
/// app/cbl/COUSR03C.cbl:L320
pub const HAS_BEEN_DELETED: &str = " has been deleted ...";

// --- Transaction types (COTRTUPC, COTRTLIC with CSDB2RPY)

/// V-COTRTUPC-004, V-COTRTUPC-025, P-COTRTUPC-021 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L176
pub const NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE: &str =
    "No record found for this key in database";

/// The head of the COTRTUPC read-failure text and of the COBTUPDT SQL text
/// ([`TranTypeSqlFailure::Access`], [`tran_type_batch_sql_message`]) —
/// V-COTRTUPC-005, V-COTRTUPC-016, V-COTRTUPC-026, P-COBTUPDT-007 to
/// P-COBTUPDT-009, P-COTRTUPC-021 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1500,
/// app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L156
pub const ERROR_ACCESSING: &str = "Error accessing:";

/// Lower-case "table", unlike [`TRANSACTION_TYPE_TABLE_SQLCODE_ON_WRITE`] —
/// V-COTRTUPC-005, V-COTRTUPC-016, V-COTRTUPC-026, P-COBTUPDT-007,
/// P-COTRTUPC-021 · app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1501,
/// app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L157
pub const TRANSACTION_TYPE_TABLE_SQLCODE: &str = " TRANSACTION_TYPE table. SQLCODE:";

/// V-COTRTUPC-010, V-COTRTUPC-020, P-COTRTUPC-008 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1570
pub const ERROR_UPDATING: &str = "Error updating:";

/// The update and insert failures write "Table" — V-COTRTUPC-010,
/// V-COTRTUPC-011, V-COTRTUPC-020, V-COTRTUPC-021, P-COTRTUPC-008,
/// P-COTRTUPC-009 · app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1571,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1611
pub const TRANSACTION_TYPE_TABLE_SQLCODE_ON_WRITE: &str = " TRANSACTION_TYPE Table. SQLCODE:";

/// V-COTRTUPC-011, V-COTRTUPC-021, P-COTRTUPC-009 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1610
pub const ERROR_INSERTING_RECORD_INTO: &str = "Error inserting record into:";

/// The SQLCODE -532 delete text of both programs — V-COTRTUPC-027,
/// V-COTRTLIC-021, P-COTRTLIC-008, P-COTRTUPC-005 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1641,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1919
pub const PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST: &str =
    "Please delete associated child records first:";

/// The other delete-failure text of both programs — V-COTRTUPC-028,
/// V-COTRTLIC-022, V-COTRTLIC-023, P-COTRTLIC-008, P-COTRTUPC-005 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1654,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1929
pub const DELETE_FAILED_WITH_MESSAGE: &str = "Delete failed with message:";

/// The COTRTUPC delete-failure label, unlike the CSDB2RPY [`SQLCODE_LABEL`] —
/// V-COTRTUPC-027, V-COTRTUPC-028, P-COTRTUPC-005 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1642,
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1655
pub const SQLCODE_LABEL_COTRTUPC: &str = "SQLCODE :";

/// The success text of a COTRTUPC delete — P-COTRTUPC-004, P-COTRTUPC-024 ·
/// app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L154
pub const DELETE_SUCCESSFUL: &str = "Delete successful.";

/// The information text of a failed COTRTUPC update or delete —
/// P-COTRTUPC-024 · app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L165
pub const CHANGES_UNSUCCESSFUL: &str = "Changes unsuccessful";

/// V-COTRTLIC-001, P-COTRTLIC-018 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1116
pub const TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT: &str =
    "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER";

/// V-COTRTLIC-002, V-COTRTLIC-028, P-COTRTLIC-022, P-COTRTLIC-036,
/// P-COTRTLIC-045 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1264
pub const NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS: &str =
    "No Records found for these filter conditions";

/// Mixed case, unlike [`NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC`] —
/// V-COTRTLIC-004, P-COTRTLIC-004, P-COTRTLIC-028, P-COTRTLIC-030 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L254
pub const NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC: &str =
    "No records found for this search condition.";

/// V-COTRTLIC-005, P-COTRTLIC-030, P-COTRTLIC-031 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L256
pub const NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS: &str =
    "No more pages for these search conditions";

/// Mixed case, unlike [`NO_MORE_PAGES_TO_DISPLAY_COCRDLIC`] — V-COTRTLIC-008,
/// P-COTRTLIC-004 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1539
pub const NO_MORE_PAGES_TO_DISPLAY_COTRTLIC: &str = "No more pages to display";

/// Mixed case, unlike [`NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC`] —
/// V-COTRTLIC-009, P-COTRTLIC-004, P-COTRTLIC-044 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1534
pub const NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC: &str = "No previous pages to display";

/// V-COTRTLIC-011, V-COTRTLIC-019, P-COTRTLIC-005 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L260
pub const ACTION_CODE_SELECTED_IS_INVALID: &str = "Action code selected is invalid";

/// V-COTRTLIC-012, P-COTRTLIC-017, P-COTRTLIC-045 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L262
pub const NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES: &str =
    "No change detected with respect to database values.";

/// V-COTRTLIC-015, V-COTRTLIC-020, P-COTRTLIC-005 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L258
pub const PLEASE_SELECT_ONLY_1_ACTION: &str = "Please select only 1 action";

/// The success text of a COTRTLIC delete — P-COTRTLIC-007, P-COTRTLIC-028,
/// P-COTRTUPC-004 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L246
pub const HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE: &str =
    "HIGHLIGHTED row deleted.Hit Enter to continue";

/// The success text of a COTRTLIC inline update — P-COTRTLIC-009,
/// P-COTRTLIC-028 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L248
pub const HIGHLIGHTED_ROW_WAS_UPDATED: &str = "HIGHLIGHTED row was updated";

/// The label 9999-FORMAT-DB2-MESSAGE strings after the trimmed action in
/// [`tran_type_list_db2_message`] — V-COTRTLIC-003, V-COTRTLIC-006,
/// V-COTRTLIC-007, V-COTRTLIC-010, V-COTRTLIC-016 to V-COTRTLIC-018,
/// V-COTRTLIC-021 to V-COTRTLIC-027, P-COTRTLIC-008, P-COTRTLIC-031,
/// P-COTRTLIC-032, P-COTRTLIC-034, P-COTRTLIC-036 to P-COTRTLIC-040,
/// P-COTRTLIC-042 · app/app-transaction-type-db2/cpy/CSDB2RPY.cpy:L76
pub const SQLCODE_LABEL: &str = " SQLCODE:";

/// A [`tran_type_list_db2_message`] action; the trailing space is declared and
/// trimmed away — V-COTRTLIC-003, P-COTRTLIC-036 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1826
pub const ERROR_READING_TRANSACTION_TYPE_TABLE: &str = "Error reading TRANSACTION_TYPE table ";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-006, P-COTRTLIC-031 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1686
pub const C_TR_TYPE_FORWARD_FETCH: &str = "C-TR-TYPE-FORWARD fetch";

/// A [`tran_type_list_db2_message`] action, also used for a failed forward
/// page fetch (E-73) — V-COTRTLIC-007, V-COTRTLIC-025, P-COTRTLIC-032,
/// P-COTRTLIC-038, P-COTRTLIC-044 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1712,
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1986
pub const C_TR_TYPE_FORWARD_CLOSE: &str = "C-TR-TYPE-FORWARD close";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-010, P-COTRTLIC-034,
/// P-COTRTLIC-044 · app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1784
pub const ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD: &str =
    "Error on fetch Cursor C-TR-TYPE-BACKWARD";

/// A [`tran_type_list_db2_message`] action; the trailing space is declared and
/// trimmed away — V-COTRTLIC-016, P-COTRTLIC-010 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1864
pub const RECORD_NOT_FOUND_DELETED_BY_OTHERS: &str = "Record not found. Deleted by others ? ";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-017, P-COTRTLIC-010 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1874
pub const DEADLOCK_SOMEONE_ELSE_UPDATING: &str = "Deadlock. Someone else updating ?";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-018, P-COTRTLIC-010 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1883
pub const UPDATE_FAILED_WITH: &str = "Update failed with";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-024, P-COTRTLIC-037 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1958
pub const C_TR_TYPE_FORWARD_OPEN: &str = "C-TR-TYPE-FORWARD Open";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-026, P-COTRTLIC-039 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L2013
pub const C_TR_TYPE_BACKWARD_OPEN: &str = "C-TR-TYPE-BACKWARD Open";

/// A [`tran_type_list_db2_message`] action — V-COTRTLIC-027, P-COTRTLIC-040 ·
/// app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L2042
pub const C_TR_TYPE_BACKWARD_CLOSE: &str = "C-TR-TYPE-BACKWARD close";

/// A [`tran_type_list_db2_message`] action: the 9998-PRIMING-QUERY failure;
/// the trailing space is declared and trimmed away — P-COTRTLIC-042 ·
/// app/app-transaction-type-db2/cpy/CSDB2RPY.cpy:L40
pub const DB2_ACCESS_FAILURE: &str = "Db2 access failure. ";

// --- Date validation (CSUTLDTC): the `WS-RESULT X(15)` texts of the CEEDAYS
// feedback, as declared, and the labels of the 80-byte `WS-MESSAGE`

/// `FC-INVALID-DATE`, the zero feedback token — P-CSUTLDTC-002 ·
/// app/cbl/CSUTLDTC.cbl:L130
pub const DATE_IS_VALID: &str = "Date is valid";

/// `FC-INSUFFICIENT-DATA` — P-CSUTLDTC-003 · app/cbl/CSUTLDTC.cbl:L132
pub const INSUFFICIENT: &str = "Insufficient";

/// `FC-BAD-DATE-VALUE` — P-CSUTLDTC-003 · app/cbl/CSUTLDTC.cbl:L134
pub const DATEVALUE_ERROR: &str = "Datevalue error";

/// `FC-INVALID-ERA` — P-CSUTLDTC-007 · app/cbl/CSUTLDTC.cbl:L136
pub const INVALID_ERA: &str = "Invalid Era    ";

/// `FC-UNSUPP-RANGE` — P-CSUTLDTC-004 · app/cbl/CSUTLDTC.cbl:L138
pub const UNSUPP_RANGE: &str = "Unsupp. Range  ";

/// `FC-INVALID-MONTH` — P-CSUTLDTC-005 · app/cbl/CSUTLDTC.cbl:L140
pub const INVALID_MONTH: &str = "Invalid month  ";

/// `FC-BAD-PIC-STRING` — P-CSUTLDTC-007 · app/cbl/CSUTLDTC.cbl:L142
pub const BAD_PIC_STRING: &str = "Bad Pic String ";

/// `FC-NON-NUMERIC-DATA` — P-CSUTLDTC-006 · app/cbl/CSUTLDTC.cbl:L144
pub const NONNUMERIC_DATA: &str = "Nonnumeric data";

/// `FC-YEAR-IN-ERA-ZERO` — P-CSUTLDTC-007 · app/cbl/CSUTLDTC.cbl:L146
pub const YEAR_IN_ERA_IS_0: &str = "YearInEra is 0 ";

/// Every other feedback token — P-CSUTLDTC-007 · app/cbl/CSUTLDTC.cbl:L148
pub const DATE_IS_INVALID: &str = "Date is invalid";

/// The `FILLER X(11)` before `WS-MSG-NO` — P-CSUTLDTC-008 ·
/// app/cbl/CSUTLDTC.cbl:L45
pub const MESG_CODE: &str = "Mesg Code:";

/// The `FILLER X(09)` before `WS-DATE` — P-CSUTLDTC-008 · app/cbl/CSUTLDTC.cbl:L51
pub const TST_DATE: &str = "TstDate:";

/// The `FILLER X(10)` before `WS-DATE-FMT` — P-CSUTLDTC-008 ·
/// app/cbl/CSUTLDTC.cbl:L54
pub const MASK_USED: &str = "Mask used:";

// --- Batch: file, abend and status displays shared by CBTRN02C (POSTTRAN),
// CBACT04C (INTCALC), CBTRN03C (TRANREPT) and CBSTM03A (CREASTMT)

/// The display before every CEE3ABD abend 999 — P-CBTRN02C-004, P-CBTRN02C-005,
/// P-CBTRN02C-007, P-CBTRN02C-012, P-CBTRN02C-015 to P-CBTRN02C-017,
/// P-CBTRN02C-019 to P-CBTRN02C-021, P-CBTRN02C-221 to P-CBTRN02C-225,
/// P-CBACT04C-177 to P-CBACT04C-188, P-CBTRN03C-468 to P-CBTRN03C-474,
/// P-CBTRN03C-476, P-CBTRN03C-478 to P-CBTRN03C-484, P-CBSTM03A-011,
/// P-CBSTM03A-1257 to P-CBSTM03A-1263 · app/cbl/CBTRN02C.cbl:L708,
/// app/cbl/CBACT04C.cbl:L629, app/cbl/CBTRN03C.cbl:L627, app/cbl/CBSTM03A.CBL:L922
pub const ABENDING_PROGRAM: &str = "ABENDING PROGRAM";

/// The head of [`file_status_message`]: "NNNN" is part of the literal —
/// P-CBTRN02C-005, P-CBTRN02C-021, P-CBTRN02C-221 to P-CBTRN02C-225,
/// P-CBACT04C-177 to P-CBACT04C-189, P-CBTRN03C-468 to P-CBTRN03C-474,
/// P-CBTRN03C-476, P-CBTRN03C-478 to P-CBTRN03C-485 · app/cbl/CBTRN02C.cbl:L721,
/// app/cbl/CBACT04C.cbl:L642, app/cbl/CBTRN03C.cbl:L640
pub const FILE_STATUS_IS: &str = "FILE STATUS IS: NNNN";

/// P-CBTRN02C-004, P-CBTRN02C-005, P-CBACT04C-181 · app/cbl/CBTRN02C.cbl:L265,
/// app/cbl/CBACT04C.cbl:L318
pub const ERROR_OPENING_TRANSACTION_FILE: &str = "ERROR OPENING TRANSACTION FILE";

/// CBACT04C displays `XREFFILE-STATUS` right after it — P-CBTRN02C-222,
/// P-CBACT04C-178, P-CBTRN03C-470 · app/cbl/CBTRN02C.cbl:L284,
/// app/cbl/CBACT04C.cbl:L263, app/cbl/CBTRN03C.cbl:L423
pub const ERROR_OPENING_CROSS_REF_FILE: &str = "ERROR OPENING CROSS REF FILE";

/// CBACT04C also displays it for a failed disclosure group open (E-72) —
/// P-CBTRN02C-223, P-CBACT04C-179, P-CBACT04C-194 · app/cbl/CBTRN02C.cbl:L302,
/// app/cbl/CBACT04C.cbl:L281
pub const ERROR_OPENING_DALY_REJECTS_FILE: &str = "ERROR OPENING DALY REJECTS FILE";

/// P-CBTRN02C-224, P-CBACT04C-180 · app/cbl/CBTRN02C.cbl:L320,
/// app/cbl/CBACT04C.cbl:L300
pub const ERROR_OPENING_ACCOUNT_MASTER_FILE: &str = "ERROR OPENING ACCOUNT MASTER FILE";

/// P-CBTRN02C-020, P-CBACT04C-187 · app/cbl/CBTRN02C.cbl:L611,
/// app/cbl/CBACT04C.cbl:L606
pub const ERROR_CLOSING_TRANSACTION_FILE: &str = "ERROR CLOSING TRANSACTION FILE";

/// P-CBTRN02C-020, P-CBACT04C-184, P-CBTRN03C-480 · app/cbl/CBTRN02C.cbl:L630,
/// app/cbl/CBACT04C.cbl:L552, app/cbl/CBTRN03C.cbl:L562
pub const ERROR_CLOSING_CROSS_REF_FILE: &str = "ERROR CLOSING CROSS REF FILE";

/// P-CBTRN02C-020, P-CBACT04C-186 · app/cbl/CBTRN02C.cbl:L666,
/// app/cbl/CBACT04C.cbl:L588
pub const ERROR_CLOSING_ACCOUNT_FILE: &str = "ERROR CLOSING ACCOUNT FILE";

/// P-CBTRN02C-020, P-CBACT04C-183 · app/cbl/CBTRN02C.cbl:L685,
/// app/cbl/CBACT04C.cbl:L533
pub const ERROR_CLOSING_TRANSACTION_BALANCE_FILE: &str = "ERROR CLOSING TRANSACTION BALANCE FILE";

// --- POSTTRAN (CBTRN02C)

/// P-CBTRN02C-221 · app/cbl/CBTRN02C.cbl:L247
pub const ERROR_OPENING_DALYTRAN: &str = "ERROR OPENING DALYTRAN";

/// P-CBTRN02C-225 · app/cbl/CBTRN02C.cbl:L338
pub const ERROR_OPENING_TRANSACTION_BALANCE_FILE: &str = "ERROR OPENING TRANSACTION BALANCE FILE";

/// P-CBTRN02C-007 · app/cbl/CBTRN02C.cbl:L363
pub const ERROR_READING_DALYTRAN_FILE: &str = "ERROR READING DALYTRAN FILE";

/// Reject reason 100; the reject record's `X(76)` description pads it —
/// P-CBTRN02C-008 · app/cbl/CBTRN02C.cbl:L386
pub const INVALID_CARD_NUMBER_FOUND: &str = "INVALID CARD NUMBER FOUND";

/// Reject reason 101, and the 109 rewrite-failure text that is never written
/// to the rejects (E-15) — P-CBTRN02C-009, P-CBTRN02C-018 ·
/// app/cbl/CBTRN02C.cbl:L398, app/cbl/CBTRN02C.cbl:L557
pub const ACCOUNT_RECORD_NOT_FOUND: &str = "ACCOUNT RECORD NOT FOUND";

/// Reject reason 102; every fixture reject carries it — P-CBTRN02C-010 and the
/// section D.1 reject rows · app/cbl/CBTRN02C.cbl:L411
pub const OVERLIMIT_TRANSACTION: &str = "OVERLIMIT TRANSACTION";

/// Reject reason 103, which overwrites 102 (E-15) — P-CBTRN02C-011 ·
/// app/cbl/CBTRN02C.cbl:L418
pub const TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION: &str =
    "TRANSACTION RECEIVED AFTER ACCT EXPIRATION";

/// P-CBTRN02C-012 · app/cbl/CBTRN02C.cbl:L460
pub const ERROR_WRITING_TO_REJECTS_FILE: &str = "ERROR WRITING TO REJECTS FILE";

/// P-CBTRN02C-015 · app/cbl/CBTRN02C.cbl:L489
pub const ERROR_READING_TRANSACTION_BALANCE_FILE: &str = "ERROR READING TRANSACTION BALANCE FILE";

/// P-CBTRN02C-016 · app/cbl/CBTRN02C.cbl:L520
pub const ERROR_WRITING_TRANSACTION_BALANCE_FILE: &str = "ERROR WRITING TRANSACTION BALANCE FILE";

/// P-CBTRN02C-017 · app/cbl/CBTRN02C.cbl:L538
pub const ERROR_REWRITING_TRANSACTION_BALANCE_FILE: &str =
    "ERROR REWRITING TRANSACTION BALANCE FILE";

/// P-CBTRN02C-019 · app/cbl/CBTRN02C.cbl:L574
pub const ERROR_WRITING_TO_TRANSACTION_FILE: &str = "ERROR WRITING TO TRANSACTION FILE";

/// One of the file-specific close texts P-CBTRN02C-020 records ·
/// app/cbl/CBTRN02C.cbl:L593
pub const ERROR_CLOSING_DALYTRAN_FILE: &str = "ERROR CLOSING DALYTRAN FILE";

/// One of the file-specific close texts P-CBTRN02C-020 records ·
/// app/cbl/CBTRN02C.cbl:L648
pub const ERROR_CLOSING_DAILY_REJECTS_FILE: &str = "ERROR CLOSING DAILY REJECTS FILE";

/// The head of [`transactions_processed_message`] — P-CBTRN02C-020 ·
/// app/cbl/CBTRN02C.cbl:L227
pub const TRANSACTIONS_PROCESSED: &str = "TRANSACTIONS PROCESSED :";

/// The head of [`transactions_rejected_message`]; two spaces before the colon,
/// as declared — P-CBTRN02C-020 · app/cbl/CBTRN02C.cbl:L228
pub const TRANSACTIONS_REJECTED: &str = "TRANSACTIONS REJECTED  :";

// --- INTCALC (CBACT04C)

/// P-CBACT04C-177 · app/cbl/CBACT04C.cbl:L245
pub const ERROR_OPENING_TRANSACTION_CATEGORY_BALANCE: &str =
    "ERROR OPENING TRANSACTION CATEGORY BALANCE";

/// P-CBACT04C-182 · app/cbl/CBACT04C.cbl:L342
pub const ERROR_READING_TRANSACTION_CATEGORY_FILE: &str = "ERROR READING TRANSACTION CATEGORY FILE";

/// Followed by the account id the failed read used — P-CBACT04C-002 ·
/// app/cbl/CBACT04C.cbl:L375, app/cbl/CBACT04C.cbl:L397
pub const ACCOUNT_NOT_FOUND_CBACT04C: &str = "ACCOUNT NOT FOUND: ";

/// The account read's abend display, a member of the abend family of
/// P-CBACT04C-188 at the lines P-CBACT04C-002 cites ·
/// app/cbl/CBACT04C.cbl:L386
pub const ERROR_READING_ACCOUNT_FILE: &str = "ERROR READING ACCOUNT FILE";

/// The cross-reference read's abend display, a member of the abend family of
/// P-CBACT04C-188 at the lines P-CBACT04C-002 cites ·
/// app/cbl/CBACT04C.cbl:L408
pub const ERROR_READING_XREF_FILE: &str = "ERROR READING XREF FILE";

/// The disclosure group read's abend display for a status other than 00 and
/// 23, a member of the abend family of P-CBACT04C-188 at the lines
/// P-CBACT04C-004 cites · app/cbl/CBACT04C.cbl:L431
pub const ERROR_READING_DISCLOSURE_GROUP_FILE: &str = "ERROR READING DISCLOSURE GROUP FILE";

/// P-CBACT04C-010 · app/cbl/CBACT04C.cbl:L365
pub const ERROR_RE_WRITING_ACCOUNT_FILE: &str = "ERROR RE-WRITING ACCOUNT FILE";

/// Status 23 on the group read; the `DEFAULT` group is read next —
/// P-CBACT04C-004 · app/cbl/CBACT04C.cbl:L418
pub const DISCLOSURE_GROUP_RECORD_MISSING: &str = "DISCLOSURE GROUP RECORD MISSING";

/// P-CBACT04C-005 · app/cbl/CBACT04C.cbl:L455
pub const ERROR_READING_DEFAULT_DISCLOSURE_GROUP: &str = "ERROR READING DEFAULT DISCLOSURE GROUP";

/// P-CBACT04C-009 · app/cbl/CBACT04C.cbl:L510
pub const ERROR_WRITING_TRANSACTION_RECORD: &str = "ERROR WRITING TRANSACTION RECORD";

/// P-CBACT04C-185 · app/cbl/CBACT04C.cbl:L570
pub const ERROR_CLOSING_DISCLOSURE_GROUP_FILE: &str = "ERROR CLOSING DISCLOSURE GROUP FILE";

/// P-CBACT04C-187 · app/cbl/CBACT04C.cbl:L230
pub const END_OF_EXECUTION_OF_PROGRAM_CBACT04C: &str = "END OF EXECUTION OF PROGRAM CBACT04C";

// --- CREASTMT (CBSTM03A): each failure display is followed by
// [`RETURN_CODE`] and the two-byte CBSTM03B return code

/// The display after each CBSTM03A file failure text — P-CBSTM03A-002,
/// P-CBSTM03A-1257 to P-CBSTM03A-1263 · app/cbl/CBSTM03A.CBL:L360,
/// app/cbl/CBSTM03A.CBL:L740
pub const RETURN_CODE: &str = "RETURN CODE: ";

/// P-CBSTM03A-002 · app/cbl/CBSTM03A.CBL:L739
pub const ERROR_OPENING_TRNXFILE: &str = "ERROR OPENING TRNXFILE";

/// One of the open texts P-CBSTM03A-002 records by its TRNXFILE member ·
/// app/cbl/CBSTM03A.CBL:L774
pub const ERROR_OPENING_XREFFILE: &str = "ERROR OPENING XREFFILE";

/// One of the open texts P-CBSTM03A-002 records by its TRNXFILE member ·
/// app/cbl/CBSTM03A.CBL:L792
pub const ERROR_OPENING_CUSTFILE: &str = "ERROR OPENING CUSTFILE";

/// One of the open texts P-CBSTM03A-002 records by its TRNXFILE member ·
/// app/cbl/CBSTM03A.CBL:L810
pub const ERROR_OPENING_ACCTFILE: &str = "ERROR OPENING ACCTFILE";

/// P-CBSTM03A-003 · app/cbl/CBSTM03A.CBL:L751, app/cbl/CBSTM03A.CBL:L844
pub const ERROR_READING_TRNXFILE: &str = "ERROR READING TRNXFILE";

/// P-CBSTM03A-1257 · app/cbl/CBSTM03A.CBL:L359
pub const ERROR_READING_XREFFILE: &str = "ERROR READING XREFFILE";

/// P-CBSTM03A-1258 · app/cbl/CBSTM03A.CBL:L383
pub const ERROR_READING_CUSTFILE: &str = "ERROR READING CUSTFILE";

/// P-CBSTM03A-1259 · app/cbl/CBSTM03A.CBL:L407
pub const ERROR_READING_ACCTFILE: &str = "ERROR READING ACCTFILE";

/// P-CBSTM03A-1260 · app/cbl/CBSTM03A.CBL:L865
pub const ERROR_CLOSING_TRNXFILE: &str = "ERROR CLOSING TRNXFILE";

/// P-CBSTM03A-1261 · app/cbl/CBSTM03A.CBL:L882
pub const ERROR_CLOSING_XREFFILE: &str = "ERROR CLOSING XREFFILE";

/// P-CBSTM03A-1262 · app/cbl/CBSTM03A.CBL:L898
pub const ERROR_CLOSING_CUSTFILE: &str = "ERROR CLOSING CUSTFILE";

/// P-CBSTM03A-1263 · app/cbl/CBSTM03A.CBL:L914
pub const ERROR_CLOSING_ACCTFILE: &str = "ERROR CLOSING ACCTFILE";

// --- TRANREPT (CBTRN03C)

/// P-CBTRN03C-468 · app/cbl/CBTRN03C.cbl:L387
pub const ERROR_OPENING_TRANFILE: &str = "ERROR OPENING TRANFILE";

/// P-CBTRN03C-469 · app/cbl/CBTRN03C.cbl:L405
pub const ERROR_OPENING_REPTFILE: &str = "ERROR OPENING REPTFILE";

/// P-CBTRN03C-471 · app/cbl/CBTRN03C.cbl:L441
pub const ERROR_OPENING_TRANSACTION_TYPE_FILE: &str = "ERROR OPENING TRANSACTION TYPE FILE";

/// P-CBTRN03C-472 · app/cbl/CBTRN03C.cbl:L459
pub const ERROR_OPENING_TRANSACTION_CATG_FILE: &str = "ERROR OPENING TRANSACTION CATG FILE";

/// P-CBTRN03C-473 · app/cbl/CBTRN03C.cbl:L477
pub const ERROR_OPENING_DATE_PARM_FILE: &str = "ERROR OPENING DATE PARM FILE";

/// P-CBTRN03C-474 · app/cbl/CBTRN03C.cbl:L266
pub const ERROR_READING_TRANSACTION_FILE: &str = "ERROR READING TRANSACTION FILE";

/// Followed by the card number of the failed lookup — P-CBTRN03C-005 ·
/// app/cbl/CBTRN03C.cbl:L487
pub const INVALID_CARD_NUMBER: &str = "INVALID CARD NUMBER : ";

/// Followed by the type code of the failed lookup — P-CBTRN03C-005 ·
/// app/cbl/CBTRN03C.cbl:L497
pub const INVALID_TRANSACTION_TYPE: &str = "INVALID TRANSACTION TYPE : ";

/// Followed by the type and category key of the failed lookup —
/// P-CBTRN03C-005 · app/cbl/CBTRN03C.cbl:L507
pub const INVALID_TRAN_CATG_KEY: &str = "INVALID TRAN CATG KEY : ";

/// P-CBTRN03C-476 · app/cbl/CBTRN03C.cbl:L354
pub const ERROR_WRITING_REPTFILE: &str = "ERROR WRITING REPTFILE";

/// P-CBTRN03C-478 · app/cbl/CBTRN03C.cbl:L525
pub const ERROR_CLOSING_POSTED_TRANSACTION_FILE: &str = "ERROR CLOSING POSTED TRANSACTION FILE";

/// P-CBTRN03C-479 · app/cbl/CBTRN03C.cbl:L543
pub const ERROR_CLOSING_REPORT_FILE: &str = "ERROR CLOSING REPORT FILE";

/// P-CBTRN03C-481 · app/cbl/CBTRN03C.cbl:L580
pub const ERROR_CLOSING_TRANSACTION_TYPE_FILE: &str = "ERROR CLOSING TRANSACTION TYPE FILE";

/// P-CBTRN03C-482 · app/cbl/CBTRN03C.cbl:L598
pub const ERROR_CLOSING_TRANSACTION_CATG_FILE: &str = "ERROR CLOSING TRANSACTION CATG FILE";

/// P-CBTRN03C-483 · app/cbl/CBTRN03C.cbl:L616
pub const ERROR_CLOSING_DATE_PARM_FILE: &str = "ERROR CLOSING DATE PARM FILE";

/// P-CBTRN03C-483 · app/cbl/CBTRN03C.cbl:L215
pub const END_OF_EXECUTION_OF_PROGRAM_CBTRN03C: &str = "END OF EXECUTION OF PROGRAM CBTRN03C";

// --- CBPAUP0J (CBPAUP0C)

/// Followed by the IMS status; two spaces before the colon, as declared —
/// P-CBPAUP0C-002 · app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L237
pub const AUTH_SUMMARY_READ_FAILED: &str = "AUTH SUMMARY READ FAILED  :";

/// P-CBPAUP0C-009 · app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L380
pub const CBPAUP0C_ABENDING: &str = "CBPAUP0C ABENDING ...";

// --- MNTTRDB2 (COBTUPDT)

/// P-COBTUPDT-004 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L87
pub const OPEN_FILE_NOT_OK: &str = "OPEN FILE NOT OK";

/// The head of [`processing_record_message`]; three trailing spaces, as
/// declared — P-COBTUPDT-005 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L105
pub const PROCESSING: &str = "PROCESSING   ";

/// P-COBTUPDT-006 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L121
pub const IGNORING_COMMENTED_LINE: &str = "IGNORING COMMENTED LINE";

/// P-COBTUPDT-006 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L124
pub const ERROR_TYPE_NOT_VALID: &str = "ERROR: TYPE NOT VALID";

/// P-COBTUPDT-007 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L153
pub const RECORD_INSERTED_SUCCESSFULLY: &str = "RECORD INSERTED SUCCESSFULLY";

/// P-COBTUPDT-008 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L179
pub const RECORD_UPDATED_SUCCESSFULLY: &str = "RECORD UPDATED SUCCESSFULLY";

/// SQLCODE +100 on an update or delete — P-COBTUPDT-008, P-COBTUPDT-009 ·
/// app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L181,
/// app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L211
pub const NO_RECORDS_FOUND: &str = "No records found.";

/// P-COBTUPDT-009 · app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L209
pub const RECORD_DELETED_SUCCESSFULLY: &str = "RECORD DELETED SUCCESSFULLY";

// --- MQ: CDRD (CODATE01) and CDRA (COACCT01) error-record return messages,
// which `MQ-APPL-RETURN-MESSAGE X(25)` receives, and the CDRA reply texts

/// P-CODATE01-012 · app/app-vsam-mq/cbl/CODATE01.cbl:L333,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L384
pub const INP_MQGET_ERR: &str = "INP MQGET ERR:";

/// P-CODATE01-015, P-COACCT01-017 · app/app-vsam-mq/cbl/CODATE01.cbl:L400,
/// app/app-vsam-mq/cbl/CODATE01.cbl:L437, app/app-vsam-mq/cbl/COACCT01.cbl:L496,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L533
pub const MQPUT_ERR: &str = "MQPUT ERR";

/// P-CODATE01-022, P-COACCT01-030 · app/app-vsam-mq/cbl/CODATE01.cbl:L199,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L250
pub const INP_MQOPEN_ERR: &str = "INP MQOPEN ERR";

/// P-CODATE01-023, P-COACCT01-031 · app/app-vsam-mq/cbl/CODATE01.cbl:L233,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L284
pub const OUT_MQOPEN_ERR: &str = "OUT MQOPEN ERR";

/// The 28-byte literal as declared; moved into the `X(25)`
/// `MQ-APPL-RETURN-MESSAGE` it becomes "ERROR WHILE READING ACCTF", the text
/// P-COACCT01-015 records, so the error record gets the cut through
/// [`move_alnum`] — P-COACCT01-015 · app/app-vsam-mq/cbl/COACCT01.cbl:L442
pub const ERROR_WHILE_READING_ACCTFILE: &str = "ERROR WHILE READING ACCTFILE";

/// Not a message: the one request function code COACCT01 serves, kept with
/// the reply texts it selects — P-COACCT01-012 · app/app-vsam-mq/cbl/COACCT01.cbl:L393
pub const CDRA_FUNCTION_INQA: &str = "INQA";

/// The head of the CDRA not-found and invalid-request replies —
/// P-COACCT01-014, P-COACCT01-016 · app/app-vsam-mq/cbl/COACCT01.cbl:L429,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L449
pub const INVALID_REQUEST_PARAMETERS: &str = "INVALID REQUEST PARAMETERS ";

/// P-COACCT01-014, P-COACCT01-016 · app/app-vsam-mq/cbl/COACCT01.cbl:L430,
/// app/app-vsam-mq/cbl/COACCT01.cbl:L450
pub const ACCT_ID_LABEL: &str = "ACCT ID : ";

/// P-COACCT01-016 · app/app-vsam-mq/cbl/COACCT01.cbl:L451
pub const FUNCTION_LABEL: &str = "FUNCTION : ";

// --- MQ: CP00 (COPAUA0C) `ERR-MESSAGE` texts of the CCPAUERY error log

/// Location A001 warning; processing continues — P-COPAUA0C-006 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L497
pub const CARD_NOT_FOUND_IN_XREF: &str = "CARD NOT FOUND IN XREF";

/// Location C001 critical; the task ends before any reply — P-COPAUA0C-006 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L509
pub const FAILED_TO_READ_XREF_FILE: &str = "FAILED TO READ XREF FILE";

/// Location A002 warning — P-COPAUA0C-007 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L544
pub const ACCT_NOT_FOUND_IN_XREF: &str = "ACCT NOT FOUND IN XREF";

/// Location C002 critical; the task ends without a reply — P-COPAUA0C-007 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L557
pub const FAILED_TO_READ_ACCT_FILE: &str = "FAILED TO READ ACCT FILE";

/// Location A003 warning — P-COPAUA0C-007 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L592
pub const CUST_NOT_FOUND_IN_XREF: &str = "CUST NOT FOUND IN XREF";

/// Location C003 critical; the task ends without a reply — P-COPAUA0C-007 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L605
pub const FAILED_TO_READ_CUST_FILE: &str = "FAILED TO READ CUST FILE";

/// Location I002 critical; the task ends without a reply — P-COPAUA0C-008 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L637
pub const IMS_GET_SUMMARY_FAILED: &str = "IMS GET SUMMARY FAILED";

/// Location M003 critical; the task ends — P-COPAUA0C-002 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L426
pub const FAILED_TO_READ_REQUEST_MQ: &str = "FAILED TO READ REQUEST MQ";

/// Location M004 critical; no database write follows — P-COPAUA0C-012 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L775
pub const FAILED_TO_PUT_ON_REPLY_MQ: &str = "FAILED TO PUT ON REPLY MQ";

/// Location I003 critical — P-COPAUA0C-013, P-COPAUA0C-035 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L844
pub const IMS_UPDATE_SUMRY_FAILED: &str = "IMS UPDATE SUMRY FAILED";

/// Location I004 critical — P-COPAUA0C-014, P-COPAUA0C-035 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L929
pub const IMS_INSERT_DETL_FAILED: &str = "IMS INSERT DETL FAILED";

/// Location I001 critical — P-COPAUA0C-033 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L315
pub const IMS_SCHD_FAILED: &str = "IMS SCHD FAILED";

/// Location M005 warning — P-COPAUA0C-037 ·
/// app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L973
pub const FAILED_TO_CLOSE_REQUEST_MQ: &str = "FAILED TO CLOSE REQUEST MQ";

// --- Request-written texts: no COBOL counterpart exists, so the request's
// wording is the source

/// 401 for a missing or expired token on an authenticated route.
pub const AUTHENTICATION_REQUIRED: &str = "Authentication required";

/// 403 for a `usr_type` `U` token on `/admin/**`. The COBOL enforces the role
/// only through menu routing [app/cbl/COMEN01C.cbl:L127-143].
pub const ADMIN_ACCESS_REQUIRED: &str = "Admin access required";

/// 400 for a reward rule whose `reward_rate` is below 0.
pub const REWARD_RATE_NEGATIVE: &str = "reward_rate must not be negative";

/// 400 for a reward rule whose minimum amount exceeds its maximum.
pub const REWARD_MIN_EXCEEDS_MAX: &str =
    "min_transaction_amount must not exceed max_transaction_amount";

/// 400 for a reward rule whose `active_to` is before its `active_from`.
pub const REWARD_ACTIVE_TO_BEFORE_FROM: &str = "active_to must not precede active_from";

/// 404 for an unknown reward rule ID.
pub const REWARD_RULE_NOT_FOUND: &str = "Reward rule not found";

/// 404 of the reward balance and history endpoints for an unknown account.
/// The COBOL account texts are separate constants, such as
/// [`ACCOUNT_ID_NOT_FOUND`].
pub const ACCOUNT_NOT_FOUND: &str = "Account not found";

/// The 500 text when the audit records no COBOL message for the failure, and
/// the replacement P-COACTUPC-046 and P-COCRDUPC-025 record for the abend
/// texts no path shows. It never carries internal detail.
pub const UNEXPECTED_ERROR: &str = "Unexpected error";

// --- Composer helpers

/// The bytes a `STRING … INTO` writes into an `X(width)` receiver: `text` cut
/// at `width` bytes, a shorter text unchanged.
fn into_receiver(text: &str, width: usize) -> String {
    if text.len() <= width {
        text.to_owned()
    } else {
        move_alnum(text, width)
    }
}

/// `FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)` after `MOVE field_label TO` the
/// `X(25)` field: the label cut at 25 bytes, then leading and trailing spaces
/// removed.
fn edit_variable_name(field_label: &str) -> String {
    move_alnum(field_label, EDIT_VARIABLE_NAME_LEN)
        .trim_matches(' ')
        .to_owned()
}

/// A `STRING` operand `DELIMITED BY SPACE` held in an `X(width)` field: the
/// value moved to the field, up to its first space.
fn delimited_by_space(value: &str, width: usize) -> String {
    delimited_by(&move_alnum(value, width), " ").to_owned()
}

/// A CICS `RESP` or `RESP2` fullword moved to `PIC 9(09)`: its low-order nine
/// digits.
fn resp_digits(code: u32) -> String {
    unsigned_display(u64::from(code), RESP_DIGITS)
}

/// `MOVE WS-RESP-CD TO ERROR-RESP`: the nine `DISPLAY` digits left-justified in
/// `X(10)`, so a space follows them.
fn resp_field(code: u32) -> String {
    move_alnum(&resp_digits(code), ERROR_RESP_LEN)
}

// --- Composers: field and date edits (COACTUPC, CSUTLDPY, COTRTUPC, COTRTLIC)

/// The field-edit message: `STRING FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)`
/// and one field-edit text, both `DELIMITED BY SIZE`, `INTO WS-RETURN-MSG`.
///
/// `field_label` is a `FIELD_*` label and `suffix` a field-edit text such as
/// [`MUST_BE_SUPPLIED`]. The label is cut at 25 bytes before it is trimmed, so
/// [`FIELD_CURRENT_CYCLE_CREDIT_LIMIT`] gives "Current Cycle Credit Limi must
/// be supplied." (E-45); the result is cut at 75 bytes. COACTUPC 1215 to 1275
/// [app/cbl/COACTUPC.cbl:L1824-2531], COTRTUPC 1210 to 1245
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L820-974] and the COTRTLIC
/// description edit [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1198] build
/// it this way; the rows are those of the suffix constants.
#[must_use]
pub fn field_edit_message(field_label: &str, suffix: &str) -> String {
    let name = edit_variable_name(field_label);
    into_receiver(&[name.as_str(), suffix].concat(), RETURN_MSG_LEN)
}

/// The CSUTLDPY date-edit message of EDIT-YEAR-CCYY, EDIT-MONTH, EDIT-DAY,
/// EDIT-DAY-MONTH-YEAR and EDIT-DATE-OF-BIRTH [app/cpy/CSUTLDPY.cpy:L25-370]:
/// every paragraph strings the trimmed field label and its text `DELIMITED BY
/// SIZE`, the [`field_edit_message`] statement — P-CSUTLDPY-002 to
/// P-CSUTLDPY-011, P-CSUTLDPY-013.
///
/// `date_edit_message(FIELD_OPEN_DATE, CENTURY_NOT_VALID)` is
/// "Open Date : Century is not valid.".
#[must_use]
pub fn date_edit_message(field_label: &str, suffix: &str) -> String {
    field_edit_message(field_label, suffix)
}

/// CSUTLDPY EDIT-DATE-LE when CSUTLDTC returns a non-zero severity: the
/// trimmed field label, [`VALIDATION_ERROR_SEV_CODE`], `WS-SEVERITY X(04)`,
/// [`MESSAGE_CODE`] and `WS-MSG-NO X(04)`, all `DELIMITED BY SIZE`, cut at 75
/// bytes — V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049, V-COACTUPC-076,
/// P-CSUTLDPY-012 [app/cpy/CSUTLDPY.cpy:L306-313].
///
/// `severity` and `message_number` are the four-byte texts CSUTLDTC returns
/// in bytes 1-4 and 16-19 of its result; each is moved to its `X(04)`.
#[must_use]
pub fn date_validation_error_message(
    field_label: &str,
    severity: &str,
    message_number: &str,
) -> String {
    let name = edit_variable_name(field_label);
    let text = [
        name.as_str(),
        VALIDATION_ERROR_SEV_CODE,
        &move_alnum(severity, 4),
        MESSAGE_CODE,
        &move_alnum(message_number, 4),
    ]
    .concat();
    into_receiver(&text, RETURN_MSG_LEN)
}

// --- Composers: CICS read failures (COACTVWC, COACTUPC, COCRDLIC, COCRDSLC,
// COCRDUPC)

/// `WS-FILE-ERROR-MESSAGE` moved to the `X(75)` `WS-RETURN-MSG` (or COCRDLIC
/// `WS-ERROR-MSG`): [`FILE_ERROR`] `X(12)`, `ERROR-OPNAME X(8)`,
/// [`FILE_ERROR_ON`] `X(4)`, `ERROR-FILE X(9)`, [`FILE_ERROR_RETURNED_RESP`]
/// `X(15)`, `ERROR-RESP X(10)`, [`FILE_ERROR_RESP2`] `X(7)` and `ERROR-RESP2
/// X(10)`, which fill the 75 bytes exactly.
///
/// `operation` is the CICS command (`READ`) and `file` the dataset name
/// (`CXACAIX`); `resp` and `resp2` are the codes the slot carries (E-44).
/// V-COACTUPC-005, V-COACTUPC-007, V-COACTUPC-009, V-COACTVWC-004,
/// V-COACTVWC-006, V-COACTVWC-008, V-COCRDLIC-008, V-COCRDLIC-010,
/// V-COCRDSLC-007, V-COCRDUPC-008, P-COACTVWC-008, P-COCRDSLC-005,
/// P-COCRDUPC-023 [app/cbl/COACTUPC.cbl:L389-408].
#[must_use]
pub fn file_error_message(operation: &str, file: &str, resp: u32, resp2: u32) -> String {
    let text = [
        move_alnum(FILE_ERROR, 12),
        move_alnum(operation, 8),
        move_alnum(FILE_ERROR_ON, 4),
        move_alnum(file, 9),
        move_alnum(FILE_ERROR_RETURNED_RESP, 15),
        resp_field(resp),
        move_alnum(FILE_ERROR_RESP2, 7),
        resp_field(resp2),
    ]
    .concat();
    into_receiver(&text, RETURN_MSG_LEN)
}

/// COACTVWC and COACTUPC 9200-GETCARDXREF-BYACCT `NOTFND`: [`ACCOUNT_LABEL`],
/// `WS-CARD-RID-ACCT-ID-X X(11)`, [`NOT_FOUND_IN`], [`CROSS_REF_FILE_RESP`],
/// `ERROR-RESP`, [`REAS`] and `ERROR-RESP2`, `DELIMITED BY SIZE`, cut at 75
/// bytes — V-COACTUPC-004, V-COACTVWC-003, P-COACTUPC-043, P-COACTVWC-007
/// [app/cbl/COACTUPC.cbl:L3674-3684] [app/cbl/COACTVWC.cbl:L747-757].
///
/// Account `99999999999` with RESP 13 and RESP2 80 gives "Account:99999999999
/// not found in Cross ref file.  Resp:000000013  Reas:0000".
#[must_use]
pub fn xref_not_found_message(account_id: &str, resp: u32, resp2: u32) -> String {
    let text = [
        ACCOUNT_LABEL,
        &move_alnum(account_id, ACCOUNT_ID_LEN),
        NOT_FOUND_IN,
        CROSS_REF_FILE_RESP,
        &resp_field(resp),
        REAS,
        &resp_field(resp2),
    ]
    .concat();
    into_receiver(&text, RETURN_MSG_LEN)
}

/// COACTVWC and COACTUPC 9300-GETACCTDATA-BYACCT `NOTFND`: as
/// [`xref_not_found_message`] with [`ACCT_MASTER_FILE_RESP`] — V-COACTUPC-006,
/// V-COACTVWC-005, P-COACTUPC-044, P-COACTVWC-009
/// [app/cbl/COACTUPC.cbl:L3723-3733] [app/cbl/COACTVWC.cbl:L796-806].
#[must_use]
pub fn account_master_not_found_message(account_id: &str, resp: u32, resp2: u32) -> String {
    let text = [
        ACCOUNT_LABEL,
        &move_alnum(account_id, ACCOUNT_ID_LEN),
        NOT_FOUND_IN,
        ACCT_MASTER_FILE_RESP,
        &resp_field(resp),
        REAS,
        &resp_field(resp2),
    ]
    .concat();
    into_receiver(&text, RETURN_MSG_LEN)
}

/// COACTVWC and COACTUPC 9400-GETCUSTDATA-BYCUST `NOTFND`: [`CUST_ID_LABEL`],
/// `WS-CARD-RID-CUST-ID-X X(9)`, [`NOT_FOUND`], [`IN_CUSTOMER_MASTER_RESP`],
/// `ERROR-RESP`, [`REAS_UPPER`] and `ERROR-RESP2`, cut at 75 bytes —
/// V-COACTUPC-008, V-COACTVWC-007, P-COACTUPC-045, P-COACTVWC-010
/// [app/cbl/COACTUPC.cbl:L3773-3783] [app/cbl/COACTVWC.cbl:L846-856].
#[must_use]
pub fn customer_master_not_found_message(customer_id: &str, resp: u32, resp2: u32) -> String {
    let text = [
        CUST_ID_LABEL,
        &move_alnum(customer_id, CUSTOMER_ID_LEN),
        NOT_FOUND,
        IN_CUSTOMER_MASTER_RESP,
        &resp_field(resp),
        REAS_UPPER,
        &resp_field(resp2),
    ]
    .concat();
    into_receiver(&text, RETURN_MSG_LEN)
}

// --- Composers: authorizations (COPAUS0C, COPAUS1C, COPAUS2C)

/// The six COPAUS0C lookup failures of GETCARDXREF-BYACCT, GETACCTDATA-BYACCT
/// and GETCUSTDATA-BYCUST
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L812-965], each
/// composed by [`auth_lookup_message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthLookupFailure {
    /// `NOTFND` on the cross-reference read — V-COPAUS0C-004.
    XrefNotFound,
    /// Any other cross-reference read failure — V-COPAUS0C-005.
    XrefReadError,
    /// `NOTFND` on the account read — V-COPAUS0C-006.
    AccountNotFound,
    /// Any other account read failure — V-COPAUS0C-007.
    AccountReadError,
    /// `NOTFND` on the customer read — V-COPAUS0C-008.
    CustomerNotFound,
    /// Any other customer read failure — V-COPAUS0C-009.
    CustomerReadError,
}

impl AuthLookupFailure {
    /// The literal before the key, the key's `X(n)` width and the literal
    /// after it.
    const fn parts(self) -> (&'static str, usize, &'static str) {
        match self {
            Self::XrefNotFound => (ACCOUNT_LABEL, ACCOUNT_ID_LEN, NOT_FOUND_IN_XREF_FILE_RESP),
            Self::XrefReadError => (
                ACCOUNT_LABEL,
                ACCOUNT_ID_LEN,
                SYSTEM_ERROR_WHILE_READING_XREF_FILE_RESP,
            ),
            Self::AccountNotFound => (ACCOUNT_LABEL, ACCOUNT_ID_LEN, NOT_FOUND_IN_ACCT_FILE_RESP),
            Self::AccountReadError => (
                ACCOUNT_LABEL,
                ACCOUNT_ID_LEN,
                SYSTEM_ERROR_WHILE_READING_ACCT_FILE_RESP,
            ),
            Self::CustomerNotFound => {
                (CUSTOMER_LABEL, CUSTOMER_ID_LEN, NOT_FOUND_IN_CUST_FILE_RESP)
            }
            Self::CustomerReadError => (
                CUSTOMER_LABEL,
                CUSTOMER_ID_LEN,
                SYSTEM_ERROR_WHILE_READING_CUST_FILE_RESP,
            ),
        }
    }
}

/// A COPAUS0C lookup failure: [`ACCOUNT_LABEL`] or [`CUSTOMER_LABEL`], the key
/// in its `X(11)` or `X(9)` field, the failure text, `WS-RESP-CD-DIS 9(09)`,
/// [`REAS`] and `WS-REAS-CD-DIS 9(09)`, `DELIMITED BY SIZE` into the `X(80)`
/// `WS-MESSAGE` — V-COPAUS0C-004 to V-COPAUS0C-009, P-COPAUS0C-007,
/// P-COPAUS0C-008.
///
/// `resp` and `resp2` are the codes the slot carries (E-44).
#[must_use]
pub fn auth_lookup_message(failure: AuthLookupFailure, key: &str, resp: u32, resp2: u32) -> String {
    let (label, key_len, text) = failure.parts();
    let message = [
        label,
        &move_alnum(key, key_len),
        text,
        &resp_digits(resp),
        REAS,
        &resp_digits(resp2),
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// A COPAUS0C or COPAUS1C IMS failure: one of the `SYSTEM_ERROR_WHILE_*`
/// heads and `IMS-RETURN-CODE X(02)`, `DELIMITED BY SIZE` into the `X(80)`
/// `WS-MESSAGE` — V-COPAUS0C-010 to V-COPAUS0C-012, V-COPAUS0C-015,
/// V-COPAUS1C-002, V-COPAUS1C-003, V-COPAUS1C-006, V-COPAUS1C-011,
/// V-COPAUS1C-013.
///
/// `status` is the status the slot carries (E-44).
#[must_use]
pub fn ims_status_message(prefix: &str, status: &str) -> String {
    into_receiver(
        &[prefix, &move_alnum(status, IMS_RETURN_CODE_LEN)].concat(),
        WS_MESSAGE_LEN,
    )
}

/// A COPAUS2C fraud-table failure: [`SYSTEM_ERROR_DB2_CODE`] or
/// [`UPDT_ERROR_DB2_CODE`], `WS-SQLCODE PIC +9(06)`, [`DB2_STATE`] and
/// `WS-SQLSTATE PIC +9(09)`, `DELIMITED BY SIZE` into the `X(50)`
/// `WS-FRD-ACT-MSG`, which COPAUS1C shows — V-COPAUS1C-009, V-COPAUS1C-010,
/// P-COPAUS2C-005 [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L206-243].
///
/// `sqlstate` is the numeric value `MOVE SQLSTATE TO WS-SQLSTATE` stores; both
/// codes are those the slots carry (E-30).
#[must_use]
pub fn fraud_db2_error_message(prefix: &str, sqlcode: i32, sqlstate: u32) -> String {
    let message = [
        prefix,
        &edit(&PIC_FRAUD_SQLCODE, Decimal::from(sqlcode)),
        DB2_STATE,
        &edit(&PIC_FRAUD_SQLSTATE, Decimal::from(sqlstate)),
    ]
    .concat();
    into_receiver(&message, FRD_ACT_MSG_LEN)
}

// --- Composers: Db2 SQLCODE texts (COTRTUPC, COTRTLIC, COBTUPDT)

/// `prefix` followed by `sqlcode` edited as `PIC ----9`
/// ([`crate::format::edit_sqlcode`]), the shape every transaction-type SQL
/// text shares: `with_sqlcode(SQLCODE_LABEL, -532)` is " SQLCODE: -532".
#[must_use]
pub fn with_sqlcode(prefix: &str, sqlcode: i32) -> String {
    [prefix, &edit_sqlcode(sqlcode)].concat()
}

/// The five COTRTUPC SQL failures, each composed by [`tran_type_sql_message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranTypeSqlFailure {
    /// 9100-GET-TRANSACTION-TYPE, SQLCODE < 0 — V-COTRTUPC-005,
    /// V-COTRTUPC-016, V-COTRTUPC-026, P-COTRTUPC-021.
    Access,
    /// 9600-WRITE-PROCESSING update, SQLCODE < 0 — V-COTRTUPC-010,
    /// V-COTRTUPC-020, P-COTRTUPC-008.
    Update,
    /// 9700-INSERT-RECORD, any failure — V-COTRTUPC-011, V-COTRTUPC-021,
    /// P-COTRTUPC-009.
    Insert,
    /// 9800-DELETE-PROCESSING, SQLCODE -532 — V-COTRTUPC-027, P-COTRTUPC-005.
    DeleteChildRecords,
    /// 9800-DELETE-PROCESSING, any other failure — V-COTRTUPC-028,
    /// P-COTRTUPC-005.
    Delete,
}

impl TranTypeSqlFailure {
    /// The two literals before the edited SQLCODE and how many times
    /// `SQLERRM OF SQLCA` follows the separator.
    const fn parts(self) -> (&'static str, &'static str, usize) {
        match self {
            Self::Access => (ERROR_ACCESSING, TRANSACTION_TYPE_TABLE_SQLCODE, 1),
            Self::Update => (ERROR_UPDATING, TRANSACTION_TYPE_TABLE_SQLCODE_ON_WRITE, 1),
            Self::Insert => (
                ERROR_INSERTING_RECORD_INTO,
                TRANSACTION_TYPE_TABLE_SQLCODE_ON_WRITE,
                1,
            ),
            Self::DeleteChildRecords => (
                PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST,
                SQLCODE_LABEL_COTRTUPC,
                2,
            ),
            Self::Delete => (DELETE_FAILED_WITH_MESSAGE, SQLCODE_LABEL_COTRTUPC, 1),
        }
    }
}

/// A COTRTUPC SQL failure: the failure's two literals, `WS-DISP-SQLCODE PIC
/// ----9`, `':'` and `SQLERRM` (twice for SQLCODE -532), `DELIMITED BY SIZE`
/// into the `X(75)` `WS-RETURN-MSG`
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1495-1661].
///
/// `sqlerrm` is the text the `SQLERRM` slot carries (E-30).
#[must_use]
pub fn tran_type_sql_message(failure: TranTypeSqlFailure, sqlcode: i32, sqlerrm: &str) -> String {
    let (head, label, sqlerrm_count) = failure.parts();
    let mut message = [
        &with_sqlcode(&[head, label].concat(), sqlcode),
        SQLERRM_SEPARATOR,
    ]
    .concat();
    for _ in 0..sqlerrm_count {
        message.push_str(sqlerrm);
    }
    into_receiver(&message, RETURN_MSG_LEN)
}

/// COTRTLIC 9999-FORMAT-DB2-MESSAGE: `FUNCTION TRIM(WS-DB2-CURRENT-ACTION)`,
/// [`SQLCODE_LABEL`], `WS-DISP-SQLCODE PIC ----9`, a space and the DSNTIAC
/// text, `DELIMITED BY SIZE` into the `X(800)` `WS-LONG-MSG`, then moved to the
/// `X(75)` `WS-RETURN-MSG` — V-COTRTLIC-003, V-COTRTLIC-006, V-COTRTLIC-007,
/// V-COTRTLIC-010, V-COTRTLIC-016 to V-COTRTLIC-018, V-COTRTLIC-021 to
/// V-COTRTLIC-027, P-COTRTLIC-008, P-COTRTLIC-042
/// [app/app-transaction-type-db2/cpy/CSDB2RPY.cpy:L53-84].
///
/// `action` is one of the COTRTLIC action texts, moved to its `X(72)` field
/// and trimmed; `dsntiac_text` is the text the DSNTIAC slot carries (E-30).
/// The 800-byte intermediate is cut at 75 bytes, which equals one cut at 75.
#[must_use]
pub fn tran_type_list_db2_message(action: &str, sqlcode: i32, dsntiac_text: &str) -> String {
    let action = move_alnum(action, DB2_CURRENT_ACTION_LEN);
    let head = [action.trim_matches(' '), SQLCODE_LABEL].concat();
    let message = [
        &with_sqlcode(&head, sqlcode),
        DSNTIAC_SEPARATOR,
        dsntiac_text,
    ]
    .concat();
    into_receiver(&message, RETURN_MSG_LEN)
}

/// COBTUPDT 10031-INSERT-DB, 10032-UPDATE-DB and 10033-DELETE-DB for SQLCODE
/// < 0: [`ERROR_ACCESSING`], [`TRANSACTION_TYPE_TABLE_SQLCODE`] and
/// `WS-VAR-SQLCODE PIC ----9`, `DELIMITED BY SIZE` into the `X(80)`
/// `WS-RETURN-MSG` — P-COBTUPDT-007 to P-COBTUPDT-009
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L132-226].
#[must_use]
pub fn tran_type_batch_sql_message(sqlcode: i32) -> String {
    let message = with_sqlcode(
        &[ERROR_ACCESSING, TRANSACTION_TYPE_TABLE_SQLCODE].concat(),
        sqlcode,
    );
    into_receiver(&message, COBTUPDT_RETURN_MSG_LEN)
}

/// What COBTUPDT 9999-ABEND displays: the `X(80)` `WS-RETURN-MSG` after a
/// `STRING` wrote `written` over a receiver that held `previous`. The receiver
/// is never cleared, so the bytes past `written` keep the tail of the earlier
/// text — P-COBTUPDT-010
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L60-62] [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L230-233].
///
/// `previous` is the receiver's content before the `STRING`: spaces for the
/// first message of a run.
#[must_use]
pub fn cobtupdt_return_msg(previous: &str, written: &str) -> String {
    let receiver = move_alnum(previous, COBTUPDT_RETURN_MSG_LEN);
    let head = into_receiver(written, COBTUPDT_RETURN_MSG_LEN);
    let tail = receiver.get(head.len()..).unwrap_or_default();
    move_alnum(&[head.as_str(), tail].concat(), COBTUPDT_RETURN_MSG_LEN)
}

// --- Composers: success texts (COTRN02C, COBIL00C, COUSR01C to COUSR03C)

/// COTRN02C WRITE-TRANSACT-FILE success: [`TRANSACTION_ADDED_SUCCESSFULLY`],
/// [`YOUR_TRAN_ID_IS`], `TRAN-ID X(16)` `DELIMITED BY SPACE` and `'.'` into the
/// `X(80)` `WS-MESSAGE` — P-COTRN02C-024 [app/cbl/COTRN02C.cbl:L726-733].
#[must_use]
pub fn transaction_added_message(tran_id: &str) -> String {
    let message = [
        TRANSACTION_ADDED_SUCCESSFULLY,
        YOUR_TRAN_ID_IS,
        &delimited_by_space(tran_id, TRAN_ID_LEN),
        TRAN_ID_TERMINATOR,
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// COBIL00C WRITE-TRANSACT-FILE success: [`PAYMENT_SUCCESSFUL`],
/// [`YOUR_TRANSACTION_ID_IS`], `TRAN-ID X(16)` `DELIMITED BY SPACE` and `'.'`
/// into the `X(80)` `WS-MESSAGE` — P-COBIL00C-016 [app/cbl/COBIL00C.cbl:L525-531].
#[must_use]
pub fn payment_successful_message(tran_id: &str) -> String {
    let message = [
        PAYMENT_SUCCESSFUL,
        YOUR_TRANSACTION_ID_IS,
        &delimited_by_space(tran_id, TRAN_ID_LEN),
        TRAN_ID_TERMINATOR,
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// [`USER_LABEL`], `SEC-USR-ID X(08)` `DELIMITED BY SPACE` and `tail` into the
/// `X(80)` `WS-MESSAGE`.
fn user_message(user_id: &str, tail: &str) -> String {
    let message = [USER_LABEL, &delimited_by_space(user_id, USER_ID_LEN), tail].concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// COUSR01C success, `User <id> has been added ...` — P-COUSR01C-004,
/// P-COUSR01C-011 [app/cbl/COUSR01C.cbl:L253-258].
#[must_use]
pub fn user_added_message(user_id: &str) -> String {
    user_message(user_id, HAS_BEEN_ADDED)
}

/// COUSR02C success, `User <id> has been updated ...` — P-COUSR02C-006,
/// P-COUSR02C-013 [app/cbl/COUSR02C.cbl:L370-375].
#[must_use]
pub fn user_updated_message(user_id: &str) -> String {
    user_message(user_id, HAS_BEEN_UPDATED)
}

/// COUSR03C success, `User <id> has been deleted ...` — P-COUSR03C-005,
/// P-COUSR03C-011 [app/cbl/COUSR03C.cbl:L316-321].
#[must_use]
pub fn user_deleted_message(user_id: &str) -> String {
    user_message(user_id, HAS_BEEN_DELETED)
}

// --- Composers: report request (CORPT00C)

/// CORPT00C SUBMIT-JOB-TO-INTRDR with a blank confirmation:
/// [`PLEASE_CONFIRM_TO_PRINT_THE`], `WS-REPORT-NAME X(10)` `DELIMITED BY SPACE`
/// and [`REPORT_ELLIPSIS`] into the `X(80)` `WS-MESSAGE` — V-CORPT00C-016,
/// P-CORPT00C-010 [app/cbl/CORPT00C.cbl:L464-470].
///
/// `report_name` is [`REPORT_NAME_MONTHLY`], [`REPORT_NAME_YEARLY`] or
/// [`REPORT_NAME_CUSTOM`].
#[must_use]
pub fn confirm_print_report_message(report_name: &str) -> String {
    let message = [
        PLEASE_CONFIRM_TO_PRINT_THE,
        &delimited_by_space(report_name, REPORT_NAME_LEN),
        REPORT_ELLIPSIS,
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// CORPT00C SUBMIT-JOB-TO-INTRDR with a confirmation other than Y, N or blank:
/// `'"'`, `CONFIRMI X(1)` `DELIMITED BY SPACE` and
/// [`IS_NOT_A_VALID_VALUE_TO_CONFIRM`] into the `X(80)` `WS-MESSAGE` —
/// V-CORPT00C-018, P-CORPT00C-011 [app/cbl/CORPT00C.cbl:L484-490].
#[must_use]
pub fn invalid_confirm_value_message(confirm: &str) -> String {
    let message = [
        CONFIRM_QUOTE,
        &delimited_by_space(confirm, CONFIRM_LEN),
        IS_NOT_A_VALID_VALUE_TO_CONFIRM,
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

/// CORPT00C PROCESS-ENTER-KEY after the report request: `WS-REPORT-NAME X(10)`
/// `DELIMITED BY SPACE` and [`REPORT_SUBMITTED_FOR_PRINTING`] into the `X(80)`
/// `WS-MESSAGE` — P-CORPT00C-014 [app/cbl/CORPT00C.cbl:L445-452].
#[must_use]
pub fn report_submitted_message(report_name: &str) -> String {
    let message = [
        delimited_by_space(report_name, REPORT_NAME_LEN).as_str(),
        REPORT_SUBMITTED_FOR_PRINTING,
    ]
    .concat();
    into_receiver(&message, WS_MESSAGE_LEN)
}

// --- Composers: CDRA replies (COACCT01)

/// COACCT01 4000-PROCESS-REQUEST-REPLY on `NOTFND`:
/// [`INVALID_REQUEST_PARAMETERS`], [`ACCT_ID_LABEL`] and `WS-KEY`, `DELIMITED
/// BY SIZE` into `REPLY-MESSAGE` — P-COACCT01-014
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L428-435].
///
/// `key` is the request's 11 key bytes as the group move left them in the
/// `9(11)` `WS-KEY`, whatever they hold; the 1000-byte padding is
/// [`crate::format::pad_reply_1000`].
#[must_use]
pub fn cdra_account_not_found_reply(key: &str) -> String {
    [
        INVALID_REQUEST_PARAMETERS,
        ACCT_ID_LABEL,
        &move_alnum(key, ACCOUNT_ID_LEN),
    ]
    .concat()
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY when the function is not
/// [`CDRA_FUNCTION_INQA`] or the key is not above zero: the
/// [`cdra_account_not_found_reply`] text, [`FUNCTION_LABEL`] and `WS-FUNC
/// X(04)`, no space before the label, 63 bytes in all — P-COACCT01-016
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L448-457].
#[must_use]
pub fn cdra_invalid_request_reply(key: &str, function: &str) -> String {
    [
        cdra_account_not_found_reply(key).as_str(),
        FUNCTION_LABEL,
        &move_alnum(function, CDRA_FUNCTION_LEN),
    ]
    .concat()
}

// --- Composers: batch displays (CBTRN02C, CBACT04C, CBTRN03C, COBTUPDT)

/// 9910-DISPLAY-IO-STATUS: [`FILE_STATUS_IS`] and `IO-STATUS-04` —
/// P-CBTRN02C-021 [app/cbl/CBTRN02C.cbl:L714-727], the same paragraph in
/// CBACT04C [app/cbl/CBACT04C.cbl:L635-648] and CBTRN03C
/// [app/cbl/CBTRN03C.cbl:L633-646].
///
/// A numeric status whose first byte is not `9` shows as `00` and the two
/// digits; any other status shows its first byte and the binary value of its
/// second byte as three digits, so `9` with the byte 0x22 shows `9034`.
#[must_use]
pub fn file_status_message(status: [u8; 2]) -> String {
    let [stat1, stat2] = status;
    let io_status_04 = if stat1.is_ascii_digit() && stat2.is_ascii_digit() && stat1 != b'9' {
        format!("00{}{}", char::from(stat1), char::from(stat2))
    } else {
        format!("{}{stat2:03}", char::from(stat1))
    };
    [FILE_STATUS_IS, &io_status_04].concat()
}

/// CBTRN02C end-of-job count: [`TRANSACTIONS_PROCESSED`] and
/// `WS-TRANSACTION-COUNT PIC 9(09)` — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L227].
#[must_use]
pub fn transactions_processed_message(count: u64) -> String {
    [
        TRANSACTIONS_PROCESSED,
        &unsigned_display(count, COUNT_DIGITS),
    ]
    .concat()
}

/// CBTRN02C end-of-job count: [`TRANSACTIONS_REJECTED`] and `WS-REJECT-COUNT
/// PIC 9(09)` — P-CBTRN02C-020 [app/cbl/CBTRN02C.cbl:L228].
#[must_use]
pub fn transactions_rejected_message(count: u64) -> String {
    [
        TRANSACTIONS_REJECTED,
        &unsigned_display(count, COUNT_DIGITS),
    ]
    .concat()
}

/// COBTUPDT 1002-READ-RECORDS: [`PROCESSING`] and the 53-byte `WS-INPUT-REC`
/// the record was read into — P-COBTUPDT-005
/// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L100-107].
#[must_use]
pub fn processing_record_message(record: &str) -> String {
    [PROCESSING, &move_alnum(record, COBTUPDT_INPUT_REC_LEN)].concat()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// The validation audit whose message texts the catalog must hold.
    const VALIDATION_AUDIT: &str = include_str!("../../../docs/parity/validation-audit.md");

    /// This file, read to list the `pub const` texts it declares.
    const SOURCE: &str = include_str!("messages.rs");

    /// COACTUPC, read for the field labels it moves to `WS-EDIT-VARIABLE-NAME`.
    const COACTUPC: &str = include_str!("../../../app/cbl/COACTUPC.cbl");

    /// Every text [`field_edit_message`] appends to a field label.
    const FIELD_EDIT_SUFFIXES: [&str; 32] = [
        MUST_BE_SUPPLIED,
        MUST_BE_Y_OR_N,
        YEAR_MUST_BE_SUPPLIED,
        MUST_BE_4_DIGIT_NUMBER,
        CENTURY_NOT_VALID,
        MONTH_MUST_BE_SUPPLIED,
        MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12,
        DAY_MUST_BE_SUPPLIED,
        DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31,
        CANNOT_HAVE_31_DAYS_IN_THIS_MONTH,
        CANNOT_HAVE_30_DAYS_IN_THIS_MONTH,
        NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS,
        CANNOT_BE_IN_THE_FUTURE,
        IS_NOT_VALID,
        MUST_BE_ALL_NUMERIC,
        MUST_NOT_BE_ZERO,
        SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999,
        SHOULD_BE_BETWEEN_300_AND_850,
        CAN_HAVE_ALPHABETS_ONLY,
        IS_NOT_A_VALID_STATE_CODE,
        AREA_CODE_MUST_BE_SUPPLIED,
        AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER,
        AREA_CODE_CANNOT_BE_ZERO,
        NOT_VALID_NORTH_AMERICA_AREA_CODE,
        PREFIX_CODE_MUST_BE_SUPPLIED,
        PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER,
        PREFIX_CODE_CANNOT_BE_ZERO,
        LINE_NUMBER_CODE_MUST_BE_SUPPLIED,
        LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER,
        LINE_NUMBER_CODE_CANNOT_BE_ZERO,
        MUST_BE_NUMERIC,
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ];

    /// Every [`ims_status_message`] head.
    const IMS_PREFIXES: [&str; 8] = [
        SYSTEM_ERROR_WHILE_SCHEDULING_PSB,
        SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS0C,
        SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS0C,
        SYSTEM_ERROR_WHILE_REPOS_AUTH_DETAILS,
        SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS1C,
        SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS1C,
        SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK,
        SYSTEM_ERROR_WHILE_READING_NEXT_AUTH,
    ];

    /// Every [`tran_type_list_db2_message`] action the validation audit records;
    /// [`DB2_ACCESS_FAILURE`] ends the conversation instead (P-COTRTLIC-042).
    const TRAN_TYPE_LIST_ACTIONS: [&str; 12] = [
        ERROR_READING_TRANSACTION_TYPE_TABLE,
        C_TR_TYPE_FORWARD_FETCH,
        C_TR_TYPE_FORWARD_CLOSE,
        ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD,
        RECORD_NOT_FOUND_DELETED_BY_OTHERS,
        DEADLOCK_SOMEONE_ELSE_UPDATING,
        UPDATE_FAILED_WITH,
        PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST,
        DELETE_FAILED_WITH_MESSAGE,
        C_TR_TYPE_FORWARD_OPEN,
        C_TR_TYPE_BACKWARD_OPEN,
        C_TR_TYPE_BACKWARD_CLOSE,
    ];

    /// `text` with each backslash escape (`\"`, `\\`) replaced by the character
    /// it escapes: the audit's quoted texts and Rust string literals alike.
    fn unescape(text: &str) -> String {
        let mut plain = String::with_capacity(text.len());
        let mut chars = text.chars();
        while let Some(character) = chars.next() {
            if character == '\\' {
                plain.push(chars.next().expect("an escaped character"));
            } else {
                plain.push(character);
            }
        }
        plain
    }

    /// The cells of a markdown table row, where `\|` is a pipe inside a cell.
    fn cells(line: &str) -> Vec<String> {
        let line = line.trim().replace("\\|", "\u{0}");
        let inner = line
            .strip_prefix('|')
            .and_then(|rest| rest.strip_suffix('|'))
            .expect("a table row");
        inner
            .split('|')
            .map(|cell| cell.trim().replace('\u{0}', "|"))
            .collect()
    }

    /// The text of a "Message text" cell: its first code span, a quoted string.
    fn message_text(cell: &str) -> String {
        let span = cell.split('`').nth(1).expect("a code span");
        let quoted = span
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("a quoted text");
        unescape(quoted)
    }

    /// The message text of every validation-audit row the review log passes;
    /// `no-message` rows carry none.
    fn audited_texts() -> BTreeSet<String> {
        let mut passed = BTreeSet::new();
        let mut rows = Vec::new();
        for line in VALIDATION_AUDIT
            .lines()
            .filter(|line| line.starts_with("| V-"))
        {
            let row = cells(line);
            match row.as_slice() {
                [id, verdict, _, _] => {
                    if verdict == "pass" {
                        passed.insert(id.clone());
                    }
                }
                [id, _, _, _, text, _, _, _] => rows.push((id.clone(), text.clone())),
                _ => panic!("{} cells in {line}", row.len()),
            }
        }
        assert!(rows.len() > 300, "{} validation rows parsed", rows.len());
        rows.into_iter()
            .filter(|(id, text)| passed.contains(id) && !text.starts_with("no-message"))
            .map(|(_, text)| message_text(&text))
            .collect()
    }

    /// Every `pub const` text this file declares, with its name.
    fn catalog_constants() -> Vec<(String, String)> {
        let mut constants = Vec::new();
        let mut declaration = String::new();
        for line in SOURCE.lines().map(str::trim) {
            if declaration.is_empty() && !line.starts_with("pub const ") {
                continue;
            }
            declaration.push_str(line);
            if !line.ends_with(';') {
                declaration.push(' ');
                continue;
            }
            let (name, literal) = declaration
                .strip_prefix("pub const ")
                .and_then(|rest| rest.split_once(": &str ="))
                .expect("a text constant");
            let quoted = literal
                .trim()
                .strip_suffix(';')
                .and_then(|rest| rest.strip_prefix('"'))
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("a string literal");
            constants.push((name.to_owned(), unescape(quoted)));
            declaration.clear();
        }
        constants
    }

    /// The composed messages of the CICS read and lookup failures, as
    /// [`composed_templates`] describes.
    fn read_failure_templates() -> Vec<String> {
        let mut templates = vec![
            [
                FILE_ERROR,
                "{ERROR-OPNAME}",
                FILE_ERROR_ON,
                "{ERROR-FILE}",
                FILE_ERROR_RETURNED_RESP,
                "{ERROR-RESP}",
                FILE_ERROR_RESP2,
                "{ERROR-RESP2}",
            ]
            .concat(),
            [
                CUST_ID_LABEL,
                "{WS-CARD-RID-CUST-ID-X}",
                NOT_FOUND,
                IN_CUSTOMER_MASTER_RESP,
                "{ERROR-RESP}",
                REAS_UPPER,
                "{ERROR-RESP2}",
            ]
            .concat(),
        ];
        for file in [CROSS_REF_FILE_RESP, ACCT_MASTER_FILE_RESP] {
            let account = "{WS-CARD-RID-ACCT-ID-X}";
            let resp = "{ERROR-RESP}";
            let resp2 = "{ERROR-RESP2}";
            templates.push(
                [
                    ACCOUNT_LABEL,
                    account,
                    NOT_FOUND_IN,
                    file,
                    resp,
                    REAS,
                    resp2,
                ]
                .concat(),
            );
        }
        let account = "{WS-CARD-RID-ACCT-ID-X}";
        let customer = "{WS-CARD-RID-CUST-ID-X}";
        for (failure, key) in [
            (AuthLookupFailure::XrefNotFound, "{WS-ACCT-ID}"),
            (AuthLookupFailure::XrefReadError, account),
            (AuthLookupFailure::AccountNotFound, account),
            (AuthLookupFailure::AccountReadError, account),
            (AuthLookupFailure::CustomerNotFound, customer),
            (AuthLookupFailure::CustomerReadError, customer),
        ] {
            let (label, _, text) = failure.parts();
            let (resp, resp2) = ("{WS-RESP-CD-DIS}", "{WS-REAS-CD-DIS}");
            templates.push([label, key, text, resp, REAS, resp2].concat());
        }
        templates
    }

    /// The composed messages of the IMS and Db2 failures, as
    /// [`composed_templates`] describes.
    fn ims_and_db2_templates() -> Vec<String> {
        let mut templates: Vec<String> = IMS_PREFIXES
            .iter()
            .map(|prefix| [prefix, "{IMS-RETURN-CODE}"].concat())
            .collect();
        for prefix in [SYSTEM_ERROR_DB2_CODE, UPDT_ERROR_DB2_CODE] {
            templates.push([prefix, "{WS-SQLCODE}", DB2_STATE, "{WS-SQLSTATE}"].concat());
        }
        for failure in [
            TranTypeSqlFailure::Access,
            TranTypeSqlFailure::Update,
            TranTypeSqlFailure::Insert,
            TranTypeSqlFailure::DeleteChildRecords,
            TranTypeSqlFailure::Delete,
        ] {
            let (head, label, sqlerrm_count) = failure.parts();
            let sqlerrm = "{SQLERRM}".repeat(sqlerrm_count);
            let sqlcode = "{WS-DISP-SQLCODE}";
            templates.push([head, label, sqlcode, SQLERRM_SEPARATOR, &sqlerrm].concat());
        }
        for action in TRAN_TYPE_LIST_ACTIONS {
            let (sqlcode, text) = ("{WS-DISP-SQLCODE}", "{WS-DSNTIAC-FMTD-TEXT}");
            let action = action.trim_matches(' ');
            templates.push([action, SQLCODE_LABEL, sqlcode, DSNTIAC_SEPARATOR, text].concat());
        }
        templates
    }

    /// Each composed message as the validation audit writes it: the catalog
    /// literals in the composer's order, each variable operand replaced by the
    /// audit's `{FIELD}` slot.
    fn composed_templates() -> Vec<String> {
        let name = "{WS-EDIT-VARIABLE-NAME}";
        let mut templates: Vec<String> = FIELD_EDIT_SUFFIXES
            .iter()
            .map(|suffix| field_edit_message(name, suffix))
            .collect();
        let (severity, number) = ("{WS-SEVERITY}", "{WS-MSG-NO}");
        templates.push(
            [
                name,
                VALIDATION_ERROR_SEV_CODE,
                severity,
                MESSAGE_CODE,
                number,
            ]
            .concat(),
        );
        templates.extend(read_failure_templates());
        templates.extend(ims_and_db2_templates());
        let report = "{WS-REPORT-NAME}";
        templates.push([PLEASE_CONFIRM_TO_PRINT_THE, report, REPORT_ELLIPSIS].concat());
        templates.push([CONFIRM_QUOTE, "{CONFIRMI}", IS_NOT_A_VALID_VALUE_TO_CONFIRM].concat());
        templates
    }

    /// The literal of every `MOVE '<label>' TO WS-EDIT-VARIABLE-NAME` statement
    /// of the fixed-format COBOL `program`, in source order. Comment lines (`*`
    /// or `/` in column 7) are skipped and the code areas (columns 8 to 72) of
    /// the other lines are read as one stream, so a statement split over lines
    /// is found whole.
    fn edit_variable_name_literals(program: &str) -> Vec<String> {
        assert!(program.is_ascii(), "fixed-format columns are bytes");
        let code = program
            .lines()
            .filter(|line| !matches!(line.as_bytes().get(6), Some(b'*' | b'/')))
            .map(|line| line.get(7..line.len().clamp(7, 72)).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        code.split("MOVE ")
            .skip(1)
            .filter_map(|statement| {
                let (literal, rest) = statement
                    .trim_start()
                    .strip_prefix('\'')?
                    .split_once('\'')?;
                let mut words = rest.split_whitespace();
                (words.next() == Some("TO")
                    && words.next().map(|name| name.trim_end_matches('.'))
                        == Some("WS-EDIT-VARIABLE-NAME"))
                .then(|| literal.to_owned())
            })
            .collect()
    }

    #[test]
    fn every_audited_message_has_exactly_one_catalog_entry() {
        let constants = catalog_constants();
        let templates = composed_templates();
        let audited = audited_texts();
        for text in &audited {
            let entries = constants.iter().filter(|(_, value)| value == text).count()
                + templates
                    .iter()
                    .filter(|template| *template == text)
                    .count();
            assert_eq!(entries, 1, "catalog entries for {text:?}");
        }
        for template in &templates {
            assert!(
                audited.contains(template),
                "{template:?} is no audited message"
            );
        }
    }

    #[test]
    fn catalog_texts_are_distinct() {
        let constants = catalog_constants();
        for (name, value) in [
            ("CCDA_MSG_THANK_YOU", CCDA_MSG_THANK_YOU),
            (
                "IS_NOT_A_VALID_VALUE_TO_CONFIRM",
                IS_NOT_A_VALID_VALUE_TO_CONFIRM,
            ),
            (
                "NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS",
                NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS,
            ),
            ("UNEXPECTED_ERROR", UNEXPECTED_ERROR),
        ] {
            assert!(
                constants.contains(&(name.to_owned(), value.to_owned())),
                "{name} parsed"
            );
        }
        let mut names_by_text = BTreeMap::new();
        for (name, value) in &constants {
            if let Some(first) = names_by_text.insert(value, name) {
                panic!("{first} and {name} both hold {value:?}");
            }
        }
        assert_eq!(names_by_text.len(), SOURCE.matches("\npub const ").count());
    }

    #[test]
    fn csmsg01y_texts_are_the_literals_of_their_50_byte_fields() {
        // The VALUE literals are 49 bytes; the X(50) fields add one space.
        for text in [CCDA_MSG_THANK_YOU, CCDA_MSG_INVALID_KEY] {
            assert_eq!(text.len(), 49, "{text:?}");
            assert_eq!(move_alnum(text, 50), format!("{text} "));
        }
        assert_eq!(
            move_alnum(CCDA_MSG_THANK_YOU, 80),
            format!(
                "Thank you for using CardDemo application...{}",
                " ".repeat(37)
            ),
            "P-COSGN00C-016: the 50-byte field, then 30 spaces"
        );
    }

    #[test]
    fn field_and_date_edits_trim_the_label_cut_at_25_bytes() {
        for (label, suffix, expected) in [
            (
                FIELD_OPEN_DATE,
                CENTURY_NOT_VALID,
                "Open Date : Century is not valid.",
            ),
            (FIELD_ZIP, MUST_BE_SUPPLIED, "Zip must be supplied."),
            (
                FIELD_PHONE_NUMBER_1,
                AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER,
                "Phone Number 1: Area code must be A 3 digit number.",
            ),
            // E-45: the 26-byte label loses its last byte in WS-EDIT-VARIABLE-NAME.
            (
                FIELD_CURRENT_CYCLE_CREDIT_LIMIT,
                MUST_BE_SUPPLIED,
                "Current Cycle Credit Limi must be supplied.",
            ),
            (
                "  Tran Type code  ",
                MUST_NOT_BE_ZERO,
                "Tran Type code must not be zero.",
            ),
        ] {
            assert_eq!(field_edit_message(label, suffix), expected);
            assert_eq!(date_edit_message(label, suffix), expected);
        }
        let long = field_edit_message(FIELD_DATE_OF_BIRTH, &"x".repeat(80));
        assert_eq!(long.len(), 75, "WS-RETURN-MSG X(75)");
    }

    /// Each COACTUPC field label is, byte for byte and before the `X(25)` cut
    /// (E-45), the literal of one `MOVE … TO WS-EDIT-VARIABLE-NAME` in
    /// 1200-EDIT-MAP-INPUTS (P-COACTUPC-006) [app/cbl/COACTUPC.cbl:L1429-1680]
    /// or 1265-EDIT-US-SSN (P-COACTUPC-031) [app/cbl/COACTUPC.cbl:L2431-2488].
    /// The one literal without a label is 'SSN' [app/cbl/COACTUPC.cbl:L1529],
    /// which 1265-EDIT-US-SSN overwrites before any message reads it; the
    /// 'Address Line 2' move [app/cbl/COACTUPC.cbl:L1614] is a comment.
    #[test]
    fn coactupc_field_labels_are_the_literals_moved_to_ws_edit_variable_name() {
        let labels = [
            FIELD_ACCOUNT_STATUS,
            FIELD_OPEN_DATE,
            FIELD_CREDIT_LIMIT,
            FIELD_EXPIRY_DATE,
            FIELD_CASH_CREDIT_LIMIT,
            FIELD_REISSUE_DATE,
            FIELD_CURRENT_BALANCE,
            FIELD_CURRENT_CYCLE_CREDIT_LIMIT,
            FIELD_CURRENT_CYCLE_DEBIT_LIMIT,
            FIELD_SSN_FIRST_3_CHARS,
            FIELD_SSN_4TH_AND_5TH_CHARS,
            FIELD_SSN_LAST_4_CHARS,
            FIELD_DATE_OF_BIRTH,
            FIELD_FICO_SCORE,
            FIELD_FIRST_NAME,
            FIELD_MIDDLE_NAME,
            FIELD_LAST_NAME,
            FIELD_ADDRESS_LINE_1,
            FIELD_STATE,
            FIELD_ZIP,
            FIELD_CITY,
            FIELD_COUNTRY,
            FIELD_PHONE_NUMBER_1,
            FIELD_PHONE_NUMBER_2,
            FIELD_EFT_ACCOUNT_ID,
            FIELD_PRIMARY_CARD_HOLDER,
        ];
        let literals = edit_variable_name_literals(COACTUPC);
        for label in labels {
            let moves = literals.iter().filter(|literal| *literal == label).count();
            assert_eq!(moves, 1, "MOVEs of {label:?} to WS-EDIT-VARIABLE-NAME");
        }
        let unlabelled: Vec<&str> = literals
            .iter()
            .map(String::as_str)
            .filter(|literal| !labels.contains(literal))
            .collect();
        assert_eq!(unlabelled, ["SSN"], "{literals:?}");
    }

    #[test]
    fn date_validation_error_moves_severity_and_number_to_four_bytes() {
        assert_eq!(
            date_validation_error_message(FIELD_OPEN_DATE, "0003", "2508"),
            "Open Date validation error Sev code: 0003 Message code: 2508"
        );
        assert_eq!(
            date_validation_error_message(FIELD_DATE_OF_BIRTH, "12", "250812"),
            "Date of Birth validation error Sev code: 12   Message code: 2508"
        );
    }

    #[test]
    fn cics_read_failures_follow_their_fixed_fields() {
        let file_error = file_error_message("READ", "CXACAIX", 13, 80);
        assert_eq!(
            file_error,
            "File Error: READ     on CXACAIX   returned RESP 000000013 ,RESP2 000000080 "
        );
        assert_eq!(file_error.len(), 75);
        // V-COACTUPC-004 example: the 75-byte cut keeps four RESP2 digits.
        assert_eq!(
            xref_not_found_message("99999999999", 13, 80),
            "Account:99999999999 not found in Cross ref file.  Resp:000000013  Reas:0000"
        );
        assert_eq!(
            account_master_not_found_message("00000000001", 13, 80),
            "Account:00000000001 not found in Acct Master file.Resp:000000013  Reas:0000"
        );
        assert_eq!(
            customer_master_not_found_message("000000001", 13, 80),
            "CustId:000000001 not found in customer master.Resp: 000000013  REAS:0000000"
        );
    }

    #[test]
    fn authorization_texts_keep_their_receivers() {
        assert_eq!(
            auth_lookup_message(AuthLookupFailure::XrefNotFound, "00000000099", 13, 80),
            "Account:00000000099 not found in XREF file. Resp:000000013 Reas:000000080"
        );
        assert_eq!(
            auth_lookup_message(AuthLookupFailure::CustomerReadError, "000000001", 17, 0),
            "Customer:000000001 System error while reading CUST file. Resp:000000017 Reas:000"
        );
        assert_eq!(
            ims_status_message(SYSTEM_ERROR_WHILE_FRAUD_TAGGING_ROLLBACK, "AO"),
            " System error while FRAUD Tagging, ROLLBACK||AO"
        );
        assert_eq!(
            ims_status_message(SYSTEM_ERROR_WHILE_SCHEDULING_PSB, "A"),
            " System error while scheduling PSB: Code:A "
        );
        let fraud = fraud_db2_error_message(SYSTEM_ERROR_DB2_CODE, -803, 23505);
        assert_eq!(fraud, " SYSTEM ERROR DB2: CODE:-000803, STATE: +000023505");
        assert_eq!(fraud.len(), 50, "WS-FRD-ACT-MSG X(50)");
        assert_eq!(
            fraud_db2_error_message(UPDT_ERROR_DB2_CODE, -911, 40001),
            " UPDT ERROR DB2: CODE:-000911, STATE: +000040001"
        );
    }

    #[test]
    fn with_sqlcode_appends_the_edited_code() {
        for (sqlcode, expected) in [
            (-532, "SQLCODE: -532"),
            (-911, "SQLCODE: -911"),
            (100, "SQLCODE:  100"),
            (-803, "SQLCODE: -803"),
        ] {
            assert_eq!(with_sqlcode("SQLCODE:", sqlcode), expected);
        }
    }

    #[test]
    fn transaction_type_sql_texts() {
        assert_eq!(
            tran_type_sql_message(TranTypeSqlFailure::Access, -911, "lock timeout"),
            "Error accessing: TRANSACTION_TYPE table. SQLCODE: -911:lock timeout"
        );
        assert_eq!(
            tran_type_sql_message(TranTypeSqlFailure::DeleteChildRecords, -532, "fk"),
            "Please delete associated child records first:SQLCODE : -532:fkfk"
        );
        assert_eq!(
            tran_type_sql_message(TranTypeSqlFailure::Insert, -803, &"e".repeat(90)).len(),
            75
        );
        assert_eq!(
            tran_type_list_db2_message(ERROR_READING_TRANSACTION_TYPE_TABLE, -911, "deadlock"),
            "Error reading TRANSACTION_TYPE table SQLCODE: -911 deadlock"
        );
        assert_eq!(
            tran_type_list_db2_message(RECORD_NOT_FOUND_DELETED_BY_OTHERS, 100, &"d".repeat(800))
                .len(),
            75
        );
        assert_eq!(
            tran_type_batch_sql_message(-803),
            "Error accessing: TRANSACTION_TYPE table. SQLCODE: -803"
        );
    }

    #[test]
    fn cobtupdt_receiver_keeps_the_tail_of_a_longer_text() {
        let first = cobtupdt_return_msg("", &tran_type_batch_sql_message(-803));
        assert_eq!(first.len(), 80);
        let second = cobtupdt_return_msg(&first, NO_RECORDS_FOUND);
        assert_eq!(
            second.trim_end(),
            "No records found.TRANSACTION_TYPE table. SQLCODE: -803"
        );
        assert_eq!(second.len(), 80);
    }

    #[test]
    fn success_texts_delimit_the_key_by_space() {
        assert_eq!(
            transaction_added_message("0000000000000301"),
            "Transaction added successfully.  Your Tran ID is 0000000000000301."
        );
        assert_eq!(
            payment_successful_message("0000000000000302"),
            "Payment successful.  Your Transaction ID is 0000000000000302."
        );
        assert_eq!(
            user_added_message("USER0003"),
            "User USER0003 has been added ..."
        );
        assert_eq!(
            user_updated_message("AB CD"),
            "User AB has been updated ..."
        );
        assert_eq!(
            user_deleted_message("ADMIN001X"),
            "User ADMIN001 has been deleted ..."
        );
    }

    #[test]
    fn report_texts_use_the_report_name_and_confirmation() {
        assert_eq!(
            confirm_print_report_message(REPORT_NAME_MONTHLY),
            "Please confirm to print the Monthly report..."
        );
        assert_eq!(
            report_submitted_message(REPORT_NAME_CUSTOM),
            "Custom report submitted for printing ..."
        );
        assert_eq!(
            report_submitted_message(REPORT_NAME_YEARLY),
            "Yearly report submitted for printing ..."
        );
        assert_eq!(
            invalid_confirm_value_message("XY"),
            "\"X\" is not a valid value to confirm..."
        );
    }

    #[test]
    fn cdra_replies_carry_the_request_key_and_function() {
        let not_found = cdra_account_not_found_reply("99999999999");
        assert_eq!(
            not_found,
            "INVALID REQUEST PARAMETERS ACCT ID : 99999999999"
        );
        let invalid = cdra_invalid_request_reply("00000000001", "INQX");
        assert_eq!(
            invalid,
            "INVALID REQUEST PARAMETERS ACCT ID : 00000000001FUNCTION : INQX"
        );
        assert_eq!(invalid.len(), 63, "P-COACCT01-016");
    }

    #[test]
    fn program_literals_keep_their_cobol_bytes() {
        // No V-row of validation-audit.md holds these texts, so
        // `every_audited_message_has_exactly_one_catalog_entry` never checks
        // them; this table pins their COBOL bytes. P-CODATE01-015 and
        // P-COACCT01-017: the MQPUT error. P-COACCT01-015: the 28-byte literal
        // as declared, before its X(25) cut. P-CBACT04C-002 and P-CBACT04C-188:
        // the account and cross-reference read abends. P-CBTRN02C-221: the
        // DALYTRAN open abend; it and P-CBACT04C-188 end in the abend display.
        // P-CBTRN02C-008: reject reason 100.
        for (text, expected) in [
            (MQPUT_ERR, "MQPUT ERR"),
            (ERROR_WHILE_READING_ACCTFILE, "ERROR WHILE READING ACCTFILE"),
            (ERROR_READING_ACCOUNT_FILE, "ERROR READING ACCOUNT FILE"),
            (ERROR_READING_XREF_FILE, "ERROR READING XREF FILE"),
            (ABENDING_PROGRAM, "ABENDING PROGRAM"),
            (ERROR_OPENING_DALYTRAN, "ERROR OPENING DALYTRAN"),
            (INVALID_CARD_NUMBER_FOUND, "INVALID CARD NUMBER FOUND"),
        ] {
            assert_eq!(text, expected);
        }
    }

    #[test]
    fn batch_displays_edit_their_fields() {
        for (status, expected) in [
            (*b"23", "FILE STATUS IS: NNNN0023"),
            (*b"35", "FILE STATUS IS: NNNN0035"),
            ([b'9', 0x22], "FILE STATUS IS: NNNN9034"),
            (*b"9A", "FILE STATUS IS: NNNN9065"),
            (*b" 1", "FILE STATUS IS: NNNN 049"),
        ] {
            assert_eq!(file_status_message(status), expected);
        }
        assert_eq!(
            transactions_processed_message(300),
            "TRANSACTIONS PROCESSED :000000300"
        );
        assert_eq!(
            transactions_rejected_message(38),
            "TRANSACTIONS REJECTED  :000000038"
        );
        let processing = processing_record_message("A01Purchase");
        assert_eq!(processing.trim_end(), "PROCESSING   A01Purchase");
        assert_eq!(processing.len(), 13 + 53);
    }

    #[test]
    fn request_written_texts() {
        for (text, expected) in [
            (AUTHENTICATION_REQUIRED, "Authentication required"),
            (ADMIN_ACCESS_REQUIRED, "Admin access required"),
            (REWARD_RATE_NEGATIVE, "reward_rate must not be negative"),
            (
                REWARD_MIN_EXCEEDS_MAX,
                "min_transaction_amount must not exceed max_transaction_amount",
            ),
            (
                REWARD_ACTIVE_TO_BEFORE_FROM,
                "active_to must not precede active_from",
            ),
            (REWARD_RULE_NOT_FOUND, "Reward rule not found"),
            (ACCOUNT_NOT_FOUND, "Account not found"),
            (UNEXPECTED_ERROR, "Unexpected error"),
        ] {
            assert_eq!(text, expected);
        }
    }
}
