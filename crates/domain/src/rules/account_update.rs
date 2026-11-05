//! `COACTUPC` (CAUP), the account update: `PUT /accounts/{id}`.
//!
//! The 3270 dialogue takes three turns: the key and the fetch
//! (`1210-EDIT-ACCOUNT`, `9000-READ-ACCT`), the ENTER turn that compares and
//! edits the changes (`1205-COMPARE-OLD-NEW`, `1200-EDIT-MAP-INPUTS`), and the
//! PF5 turn that writes them (`9600-WRITE-PROCESSING`). One request performs
//! all three (E-56), in the order the validation audit ranks their messages
//! (V-COACTUPC-002 to V-COACTUPC-128): [`update`] runs [`edit_account_id`],
//! [`read_snapshot`], the receiving and edits of [`validate`] and [`apply`] in
//! that order, editing the key once, and each step is public for an adapter
//! that sequences them itself. The copybook procedures `CSUTLDPY` (the date
//! edits, through [`super::date_edit`]) and `CSLKPCDY` (the lookup tables
//! below) are part of the program.
//!
//! Between turns the program keeps the fetched values, `ACUP-OLD-DETAILS`, in
//! its COMMAREA. The stateless API keeps a [`VersionToken`] instead (E-24):
//! [`record_version`] digests the fields `9700-CHECK-CHANGE-IN-REC` compares,
//! keyed with the server's [`VersionKey`], `GET /accounts/{id}` returns it, and
//! [`apply`] recomputes it under the same key from the rows it locks. A request
//! without a token is the API's 400 (V-COACTUPC-001) and never reaches this
//! module.
//!
//! # Outcomes
//!
//! | Outcome | Kind | BMS field | Rows |
//! |---|---|---|---|
//! | Key blank, not 11 digits or zero | `Empty`, `Invalid` | `ACCTSID` | V-COACTUPC-002, V-COACTUPC-003 |
//! | Cross-reference, account or customer not found or not read | `NotFound`, `Unexpected` | `ACCTSID` | V-COACTUPC-004 to V-COACTUPC-009 |
//! | No change detected | none: [`AccountEdit::NoChange`], 200 | — | V-COACTUPC-010 |
//! | A field edit failed | `Empty`, `Invalid` | the flagged fields | V-COACTUPC-011 to V-COACTUPC-123 |
//! | Lock failed; rewrite failed | `Unexpected` | `ACCTSID` | V-COACTUPC-124, V-COACTUPC-125, V-COACTUPC-127, V-COACTUPC-128 |
//! | Record changed since it was fetched | `Conflict` | `ACCTSID` | V-COACTUPC-126 |
//! | Rewritten | none: [`AccountUpdated`], 200 | — | P-COACTUPC-015 |
//! | Input class held at `halt` | `Unexpected`, [`UNEXPECTED_ERROR`] | — | E-26, E-40, E-47; E-74 answers as V-COACTUPC-128 |
//!
//! [`validate`] returns its refusals as an [`EditRefusal`]: a key or field
//! edit's [`DomainError`], or, for an input class it holds at `halt`,
//! [`EditRefusal::Halted`] with the escalation that holds it (E-26, E-40,
//! E-47). The escalation is the cause an adapter logs beside the answer and
//! is never answered: the refusal converts into the [`DomainError`] the
//! request is answered with, for a halt the unexpected-error outcome with no
//! fields. [`update`] answers that conversion.
//!
//! Every edit of `1200-EDIT-MAP-INPUTS` runs, and each writes its text only
//! `IF WS-RETURN-MSG-OFF` [app/cbl/COACTUPC.cbl:L479-480], so the first failing
//! edit in COBOL order supplies the message and its kind; `fields` lists every
//! field an edit flagged, in that order, each with the text of the edit that
//! flagged it, so the first entry is the field the cursor goes to
//! (P-COACTUPC-039, validation-audit COACTUPC Precedence).
//!
//! # Input classes held at `halt`
//!
//! No code guesses an outcome the audit does not pin. These answer the
//! unexpected-error outcome, and [`validate`] refuses each of the first two
//! with [`EditRefusal::Halted`] and the escalation named here:
//!
//! - A date whose outcome `super::date_edit` reports unproven: every date that
//!   reaches `CSUTLDTC` (E-26), a month holding a non-digit (E-47) and a day of
//!   exactly one half (E-40). The `CSUTLDTC` outcome is pinned once an earlier
//!   edit has set the message: a rejection would set no text and, through
//!   `EDIT-DATE-LE-EXIT`, leave the date flags valid [app/cpy/CSUTLDPY.cpy:L298-331],
//!   which is what an acceptance leaves, so the edits go on.
//! - A signed amount whose `NUMVAL-C` value is a midpoint beyond the cent or
//!   needs more than the 10 integer digits of `S9(10)V99` (E-40,
//!   P-COACTUPC-047, P-COACTUPC-049), and one `TEST-NUMVAL-C` accepts only
//!   under a reading the audit does not record: grouping commas or a lower-case
//!   `cr` or `db`.
//! - A received customer ID holding a non-digit (E-74): nothing is written and
//!   the request answers as V-COACTUPC-128, after the locks and the change
//!   check that come before that write.
//!
//! # Stored form of blank fields
//!
//! `1100-RECEIVE-MAP` turns a field holding `*` or spaces into `LOW-VALUES`
//! (P-COACTUPC-019), and `9600-WRITE-PROCESSING` rewrites the received values,
//! so a blank group ID, middle name, address line 2, government ID or phone
//! reaches the records as `X'00'` bytes. `PostgreSQL` text holds no NUL (the
//! storage-form check of `migrations/0002_master_data.sql`; E-07 meets the
//! same limit), so every `X'00'` byte of a rewritten text field is written as
//! a space, the form a blank `X(n)` item takes in the tables, and
//! `1205-COMPARE-OLD-NEW` reads the received values in that stored form: a
//! received blank equals a stored blank, as it does in the COBOL once the
//! program has rewritten the field from `LOW-VALUES`. This is the reading the
//! example of V-COACTUPC-010 records (account 00000000001, whose group ID is
//! blank, repeating its fetched values changes nothing).
//!
//! # The account update record
//!
//! `ACCT-UPDATE-RECORD` [app/cbl/COACTUPC.cbl:L418-433] has no
//! `ACCT-ADDR-ZIP`: its `ACCT-UPDATE-GROUP-ID` occupies offsets 102-111, which
//! `ACCOUNT-RECORD` [app/cpy/CVACT01Y.cpy:L4-17] reads as `ACCT-ADDR-ZIP`, and
//! the offsets `ACCOUNT-RECORD` reads as `ACCT-GROUP-ID` hold the record's
//! `FILLER`, which `INITIALIZE` does not set [app/cbl/COACTUPC.cbl:L3956]. The
//! rewritten account therefore carries the entered group ID in
//! `acct_addr_zip` and, in `acct_group_id`, the spaces E-25 writes for bytes
//! the COBOL never sets. `CUST-UPDATE-RECORD` [app/cbl/COACTUPC.cbl:L434-456]
//! matches `CUSTOMER-RECORD` field for field.

use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;

use super::date_edit::{
    DateBmsFields, DateEditFlags, DateParts, FlagState, edit_date_ccyymmdd, edit_date_of_birth,
};
use super::date_validation::DateValidationError;
use super::{ReturnMsg, VersionKey, VersionToken, version_token};
use crate::entities::{Account, Customer};
use crate::error::{DomainError, ErrorKind, FieldError};
use crate::messages::{
    ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO, AREA_CODE_CANNOT_BE_ZERO,
    AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER, AREA_CODE_MUST_BE_SUPPLIED, CAN_HAVE_ALPHABETS_ONLY,
    CHANGES_COMMITTED_TO_DATABASE, COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE,
    COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE, FIELD_ACCOUNT_STATUS, FIELD_ADDRESS_LINE_1,
    FIELD_CASH_CREDIT_LIMIT, FIELD_CITY, FIELD_COUNTRY, FIELD_CREDIT_LIMIT, FIELD_CURRENT_BALANCE,
    FIELD_CURRENT_CYCLE_CREDIT_LIMIT, FIELD_CURRENT_CYCLE_DEBIT_LIMIT, FIELD_DATE_OF_BIRTH,
    FIELD_EFT_ACCOUNT_ID, FIELD_EXPIRY_DATE, FIELD_FICO_SCORE, FIELD_FIRST_NAME, FIELD_LAST_NAME,
    FIELD_MIDDLE_NAME, FIELD_OPEN_DATE, FIELD_PHONE_NUMBER_1, FIELD_PHONE_NUMBER_2,
    FIELD_PRIMARY_CARD_HOLDER, FIELD_REISSUE_DATE, FIELD_SSN_4TH_AND_5TH_CHARS,
    FIELD_SSN_FIRST_3_CHARS, FIELD_SSN_LAST_4_CHARS, FIELD_STATE, FIELD_ZIP,
    INVALID_ZIP_CODE_FOR_STATE, IS_NOT_A_VALID_STATE_CODE, IS_NOT_VALID,
    LINE_NUMBER_CODE_CANNOT_BE_ZERO, LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER,
    LINE_NUMBER_CODE_MUST_BE_SUPPLIED, MUST_BE_ALL_NUMERIC, MUST_BE_SUPPLIED, MUST_BE_Y_OR_N,
    MUST_NOT_BE_ZERO, NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED, NO_INPUT_RECEIVED,
    NOT_VALID_NORTH_AMERICA_AREA_CODE, PREFIX_CODE_CANNOT_BE_ZERO,
    PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER, PREFIX_CODE_MUST_BE_SUPPLIED,
    RECORD_CHANGED_BY_SOME_ONE_ELSE, SHOULD_BE_BETWEEN_300_AND_850,
    SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999, UNEXPECTED_ERROR, UPDATE_OF_RECORD_FAILED,
    account_master_not_found_message, customer_master_not_found_message, field_edit_message,
    file_error_message, xref_not_found_message,
};
use crate::numeric::{AccountGroupId, AccountId, Alnum, CustSsn, CustomerId, FicoScore, Money};
use crate::ports::{
    AccountReader, AccountWriter, CustomerReader, CustomerWriter, PortError, XrefReader,
};

/// `ACCTSID`, the account number of the `COACTUP` map: the key edit, the read
/// and write failures and the `CSUTLDTC` rejection place the cursor here.
const FIELD_ACCTSID: &str = "ACCTSID";
/// `ACSTTUS`, the account status.
const FIELD_ACSTTUS: &str = "ACSTTUS";
/// `ACRDLIM`, the credit limit.
const FIELD_ACRDLIM: &str = "ACRDLIM";
/// `ACSHLIM`, the cash credit limit.
const FIELD_ACSHLIM: &str = "ACSHLIM";
/// `ACURBAL`, the current balance.
const FIELD_ACURBAL: &str = "ACURBAL";
/// `ACRCYCR`, the current cycle credit.
const FIELD_ACRCYCR: &str = "ACRCYCR";
/// `ACRCYDB`, the current cycle debit.
const FIELD_ACRCYDB: &str = "ACRCYDB";
/// `ACTSSN1`, the first three SSN digits.
const FIELD_ACTSSN1: &str = "ACTSSN1";
/// `ACTSSN2`, the fourth and fifth SSN digits.
const FIELD_ACTSSN2: &str = "ACTSSN2";
/// `ACTSSN3`, the last four SSN digits.
const FIELD_ACTSSN3: &str = "ACTSSN3";
/// `ACSTFCO`, the FICO score.
const FIELD_ACSTFCO: &str = "ACSTFCO";
/// `ACSFNAM`, the first name.
const FIELD_ACSFNAM: &str = "ACSFNAM";
/// `ACSMNAM`, the middle name.
const FIELD_ACSMNAM: &str = "ACSMNAM";
/// `ACSLNAM`, the last name.
const FIELD_ACSLNAM: &str = "ACSLNAM";
/// `ACSADL1`, address line 1.
const FIELD_ACSADL1: &str = "ACSADL1";
/// `ACSSTTE`, the state code.
const FIELD_ACSSTTE: &str = "ACSSTTE";
/// `ACSZIPC`, the zip code.
const FIELD_ACSZIPC: &str = "ACSZIPC";
/// `ACSCITY`, the city: address line 3.
const FIELD_ACSCITY: &str = "ACSCITY";
/// `ACSCTRY`, the country code.
const FIELD_ACSCTRY: &str = "ACSCTRY";
/// `ACSEFTC`, the EFT account ID.
const FIELD_ACSEFTC: &str = "ACSEFTC";
/// `ACSPFLG`, the primary card holder indicator.
const FIELD_ACSPFLG: &str = "ACSPFLG";

/// The BMS fields of phone number 1: `ACSPH1A`, `ACSPH1B`, `ACSPH1C`.
const PHONE_1_FIELDS: PhoneFields = PhoneFields {
    area: "ACSPH1A",
    prefix: "ACSPH1B",
    line: "ACSPH1C",
};

/// The BMS fields of phone number 2: `ACSPH2A`, `ACSPH2B`, `ACSPH2C`.
const PHONE_2_FIELDS: PhoneFields = PhoneFields {
    area: "ACSPH2A",
    prefix: "ACSPH2B",
    line: "ACSPH2C",
};

/// The COBOL figurative constant `LOW-VALUE`.
const LOW_VALUE: u8 = 0x00;

/// The COBOL figurative constant `SPACE`.
const SPACE: u8 = b' ';

/// `ERROR-OPNAME` of a failed read [app/cbl/COACTUPC.cbl:L3689].
const ERROR_OPNAME_READ: &str = "READ";

/// `LIT-CARDXREFNAME-ACCT-PATH` [app/cbl/COACTUPC.cbl:L581-582].
const LIT_CARDXREFNAME_ACCT_PATH: &str = "CXACAIX ";

/// `LIT-ACCTFILENAME` [app/cbl/COACTUPC.cbl:L573-574].
const LIT_ACCTFILENAME: &str = "ACCTDAT ";

/// `LIT-CUSTFILENAME` [app/cbl/COACTUPC.cbl:L575-576].
const LIT_CUSTFILENAME: &str = "CUSTDAT ";

/// The CICS `RESP` of `NOTFND` on `READ`, as E-44 records it.
const RESP_NOTFND: u32 = 13;

/// The `RESP2` of `NOTFND` on `READ` (E-44).
const RESP2_NOTFND: u32 = 80;

/// The CICS `RESP` E-44 proposes for a database failure, which has no CICS
/// code: `IOERR`.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 proposes alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// The default currency sign `NUMVAL-C` reads: `COACTUPC` declares no
/// `CURRENCY SIGN` clause.
const CURRENCY_SIGN: u8 = b'$';

/// The escalation that holds every `CSUTLDTC` call of `CSUTLDPY` at `halt`.
const E_CSUTLDPY_LINKAGE: &str = "E-26";

/// The escalation that holds at `halt` the `NUMVAL-C` arguments and values
/// the audit does not pin (P-COACTUPC-049).
const E_NUMVAL_C: &str = "E-40";

/// The integer digits of the `S9(10)V99` receivers of `NUMVAL-C`: a value of
/// `10^10` or more does not fit (E-40).
const AMOUNT_INTEGER_LIMIT: i64 = 10_000_000_000;

// --- CSLKPCDY: the lookup tables, transcribed in copybook order

/// `CSLKPCDY` `88 VALID-PHONE-AREA-CODE` of `WS-US-PHONE-AREA-CODE-TO-EDIT`
/// [app/cpy/CSLKPCDY.cpy:L24-520] (P-CSLKPCDY-001): every North American
/// area code the copybook lists, the general-purpose codes followed by the
/// easily recognisable ones. `COACTUPC` tests only
/// [`VALID_GENERAL_PURP_CODE`]; the table is kept with the others it declares.
pub const VALID_PHONE_AREA_CODE: [&str; 490] = [
    "201", "202", "203", "204", "205", "206", "207", "208", "209", "210", "212", "213", "214",
    "215", "216", "217", "218", "219", "220", "223", "224", "225", "226", "228", "229", "231",
    "234", "236", "239", "240", "242", "246", "248", "249", "250", "251", "252", "253", "254",
    "256", "260", "262", "264", "267", "268", "269", "270", "272", "276", "279", "281", "284",
    "289", "301", "302", "303", "304", "305", "306", "307", "308", "309", "310", "312", "313",
    "314", "315", "316", "317", "318", "319", "320", "321", "323", "325", "326", "330", "331",
    "332", "334", "336", "337", "339", "340", "341", "343", "345", "346", "347", "351", "352",
    "360", "361", "364", "365", "367", "368", "380", "385", "386", "401", "402", "403", "404",
    "405", "406", "407", "408", "409", "410", "412", "413", "414", "415", "416", "417", "418",
    "419", "423", "424", "425", "430", "431", "432", "434", "435", "437", "438", "440", "441",
    "442", "443", "445", "447", "448", "450", "458", "463", "464", "469", "470", "473", "474",
    "475", "478", "479", "480", "484", "501", "502", "503", "504", "505", "506", "507", "508",
    "509", "510", "512", "513", "514", "515", "516", "517", "518", "519", "520", "530", "531",
    "534", "539", "540", "541", "548", "551", "559", "561", "562", "563", "564", "567", "570",
    "571", "572", "573", "574", "575", "579", "580", "581", "582", "585", "586", "587", "601",
    "602", "603", "604", "605", "606", "607", "608", "609", "610", "612", "613", "614", "615",
    "616", "617", "618", "619", "620", "623", "626", "628", "629", "630", "631", "636", "639",
    "640", "641", "646", "647", "649", "650", "651", "656", "657", "658", "659", "660", "661",
    "662", "664", "667", "669", "670", "671", "672", "678", "680", "681", "682", "683", "684",
    "689", "701", "702", "703", "704", "705", "706", "707", "708", "709", "712", "713", "714",
    "715", "716", "717", "718", "719", "720", "721", "724", "725", "726", "727", "731", "732",
    "734", "737", "740", "742", "743", "747", "753", "754", "757", "758", "760", "762", "763",
    "765", "767", "769", "770", "771", "772", "773", "774", "775", "778", "779", "780", "781",
    "782", "784", "785", "786", "787", "801", "802", "803", "804", "805", "806", "807", "808",
    "809", "810", "812", "813", "814", "815", "816", "817", "818", "819", "820", "825", "826",
    "828", "829", "830", "831", "832", "838", "839", "840", "843", "845", "847", "848", "849",
    "850", "854", "856", "857", "858", "859", "860", "862", "863", "864", "865", "867", "868",
    "869", "870", "872", "873", "876", "878", "901", "902", "903", "904", "905", "906", "907",
    "908", "909", "910", "912", "913", "914", "915", "916", "917", "918", "919", "920", "925",
    "928", "929", "930", "931", "934", "936", "937", "938", "939", "940", "941", "943", "945",
    "947", "948", "949", "951", "952", "954", "956", "959", "970", "971", "972", "973", "978",
    "979", "980", "983", "984", "985", "986", "989", "200", "211", "222", "233", "244", "255",
    "266", "277", "288", "299", "300", "311", "322", "333", "344", "355", "366", "377", "388",
    "399", "400", "411", "422", "433", "444", "455", "466", "477", "488", "499", "500", "511",
    "522", "533", "544", "555", "566", "577", "588", "599", "600", "611", "622", "633", "644",
    "655", "666", "677", "688", "699", "700", "711", "722", "733", "744", "755", "766", "777",
    "788", "799", "800", "811", "822", "833", "844", "855", "866", "877", "888", "899", "900",
    "911", "922", "933", "944", "955", "966", "977", "988", "999",
];

/// `CSLKPCDY` `88 VALID-GENERAL-PURP-CODE` [app/cpy/CSLKPCDY.cpy:L521-930]
/// (P-CSLKPCDY-002): the area codes `EDIT-AREA-CODE` accepts
/// (V-COACTUPC-101, V-COACTUPC-111).
pub const VALID_GENERAL_PURP_CODE: [&str; 410] = [
    "201", "202", "203", "204", "205", "206", "207", "208", "209", "210", "212", "213", "214",
    "215", "216", "217", "218", "219", "220", "223", "224", "225", "226", "228", "229", "231",
    "234", "236", "239", "240", "242", "246", "248", "249", "250", "251", "252", "253", "254",
    "256", "260", "262", "264", "267", "268", "269", "270", "272", "276", "279", "281", "284",
    "289", "301", "302", "303", "304", "305", "306", "307", "308", "309", "310", "312", "313",
    "314", "315", "316", "317", "318", "319", "320", "321", "323", "325", "326", "330", "331",
    "332", "334", "336", "337", "339", "340", "341", "343", "345", "346", "347", "351", "352",
    "360", "361", "364", "365", "367", "368", "380", "385", "386", "401", "402", "403", "404",
    "405", "406", "407", "408", "409", "410", "412", "413", "414", "415", "416", "417", "418",
    "419", "423", "424", "425", "430", "431", "432", "434", "435", "437", "438", "440", "441",
    "442", "443", "445", "447", "448", "450", "458", "463", "464", "469", "470", "473", "474",
    "475", "478", "479", "480", "484", "501", "502", "503", "504", "505", "506", "507", "508",
    "509", "510", "512", "513", "514", "515", "516", "517", "518", "519", "520", "530", "531",
    "534", "539", "540", "541", "548", "551", "559", "561", "562", "563", "564", "567", "570",
    "571", "572", "573", "574", "575", "579", "580", "581", "582", "585", "586", "587", "601",
    "602", "603", "604", "605", "606", "607", "608", "609", "610", "612", "613", "614", "615",
    "616", "617", "618", "619", "620", "623", "626", "628", "629", "630", "631", "636", "639",
    "640", "641", "646", "647", "649", "650", "651", "656", "657", "658", "659", "660", "661",
    "662", "664", "667", "669", "670", "671", "672", "678", "680", "681", "682", "683", "684",
    "689", "701", "702", "703", "704", "705", "706", "707", "708", "709", "712", "713", "714",
    "715", "716", "717", "718", "719", "720", "721", "724", "725", "726", "727", "731", "732",
    "734", "737", "740", "742", "743", "747", "753", "754", "757", "758", "760", "762", "763",
    "765", "767", "769", "770", "771", "772", "773", "774", "775", "778", "779", "780", "781",
    "782", "784", "785", "786", "787", "801", "802", "803", "804", "805", "806", "807", "808",
    "809", "810", "812", "813", "814", "815", "816", "817", "818", "819", "820", "825", "826",
    "828", "829", "830", "831", "832", "838", "839", "840", "843", "845", "847", "848", "849",
    "850", "854", "856", "857", "858", "859", "860", "862", "863", "864", "865", "867", "868",
    "869", "870", "872", "873", "876", "878", "901", "902", "903", "904", "905", "906", "907",
    "908", "909", "910", "912", "913", "914", "915", "916", "917", "918", "919", "920", "925",
    "928", "929", "930", "931", "934", "936", "937", "938", "939", "940", "941", "943", "945",
    "947", "948", "949", "951", "952", "954", "956", "959", "970", "971", "972", "973", "978",
    "979", "980", "983", "984", "985", "986", "989",
];

/// `CSLKPCDY` `88 VALID-EASY-RECOG-AREA-CODE` [app/cpy/CSLKPCDY.cpy:L931-1010]
/// (P-CSLKPCDY-003): the easily recognisable area codes, which no `COACTUPC`
/// edit tests.
pub const VALID_EASY_RECOG_AREA_CODE: [&str; 80] = [
    "200", "211", "222", "233", "244", "255", "266", "277", "288", "299", "300", "311", "322",
    "333", "344", "355", "366", "377", "388", "399", "400", "411", "422", "433", "444", "455",
    "466", "477", "488", "499", "500", "511", "522", "533", "544", "555", "566", "577", "588",
    "599", "600", "611", "622", "633", "644", "655", "666", "677", "688", "699", "700", "711",
    "722", "733", "744", "755", "766", "777", "788", "799", "800", "811", "822", "833", "844",
    "855", "866", "877", "888", "899", "900", "911", "922", "933", "944", "955", "966", "977",
    "988", "999",
];

/// `CSLKPCDY` `88 VALID-US-STATE-CODE` of `US-STATE-CODE-TO-EDIT PIC X(2)`
/// [app/cpy/CSLKPCDY.cpy:L1012-1069] (P-CSLKPCDY-004): the upper-case codes
/// `1270-EDIT-US-STATE-CD` accepts (V-COACTUPC-090).
pub const VALID_US_STATE_CODE: [&str; 56] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "DC", "AS", "GU", "MP", "PR", "VI",
];

/// `CSLKPCDY` `88 VALID-US-STATE-ZIP-CD2-COMBO` of
/// `US-STATE-AND-FIRST-ZIP2 PIC X(4)` [app/cpy/CSLKPCDY.cpy:L1071-1313]
/// (P-CSLKPCDY-005): a state code followed by the first two digits of a zip
/// code, the pairs `1280-EDIT-US-STATE-ZIP-CD` accepts (V-COACTUPC-123).
pub const VALID_US_STATE_ZIP_CD2_COMBO: [&str; 240] = [
    "AA34", "AE90", "AE91", "AE92", "AE93", "AE94", "AE95", "AE96", "AE97", "AE98", "AK99", "AL35",
    "AL36", "AP96", "AR71", "AR72", "AS96", "AZ85", "AZ86", "CA90", "CA91", "CA92", "CA93", "CA94",
    "CA95", "CA96", "CO80", "CO81", "CT60", "CT61", "CT62", "CT63", "CT64", "CT65", "CT66", "CT67",
    "CT68", "CT69", "DC20", "DC56", "DC88", "DE19", "FL32", "FL33", "FL34", "FM96", "GA30", "GA31",
    "GA39", "GU96", "HI96", "IA50", "IA51", "IA52", "ID83", "IL60", "IL61", "IL62", "IN46", "IN47",
    "KS66", "KS67", "KY40", "KY41", "KY42", "LA70", "LA71", "MA10", "MA11", "MA12", "MA13", "MA14",
    "MA15", "MA16", "MA17", "MA18", "MA19", "MA20", "MA21", "MA22", "MA23", "MA24", "MA25", "MA26",
    "MA27", "MA55", "MD20", "MD21", "ME39", "ME40", "ME41", "ME42", "ME43", "ME44", "ME45", "ME46",
    "ME47", "ME48", "ME49", "MH96", "MI48", "MI49", "MN55", "MN56", "MO63", "MO64", "MO65", "MO72",
    "MP96", "MS38", "MS39", "MT59", "NC27", "NC28", "ND58", "NE68", "NE69", "NH30", "NH31", "NH32",
    "NH33", "NH34", "NH35", "NH36", "NH37", "NH38", "NJ70", "NJ71", "NJ72", "NJ73", "NJ74", "NJ75",
    "NJ76", "NJ77", "NJ78", "NJ79", "NJ80", "NJ81", "NJ82", "NJ83", "NJ84", "NJ85", "NJ86", "NJ87",
    "NJ88", "NJ89", "NM87", "NM88", "NV88", "NV89", "NY50", "NY54", "NY63", "NY10", "NY11", "NY12",
    "NY13", "NY14", "OH43", "OH44", "OH45", "OK73", "OK74", "OR97", "PA15", "PA16", "PA17", "PA18",
    "PA19", "PR60", "PR61", "PR62", "PR63", "PR64", "PR65", "PR66", "PR67", "PR68", "PR69", "PR70",
    "PR71", "PR72", "PR73", "PR74", "PR75", "PR76", "PR77", "PR78", "PR79", "PR90", "PR91", "PR92",
    "PR93", "PR94", "PR95", "PR96", "PR97", "PR98", "PW96", "RI28", "RI29", "SC29", "SD57", "TN37",
    "TN38", "TX73", "TX75", "TX76", "TX77", "TX78", "TX79", "TX88", "UT84", "VA20", "VA22", "VA23",
    "VA24", "VI80", "VI82", "VI83", "VI84", "VI85", "VT50", "VT51", "VT52", "VT53", "VT54", "VT56",
    "VT57", "VT58", "VT59", "WA98", "WA99", "WI53", "WI54", "WV24", "WV25", "WV26", "WY82", "WY83",
];

// --- Types

/// One `PUT /accounts/{id}` request: the `COACTUP` map input as entered.
///
/// `account_id` is the `{id}` path segment (`ACCTSIDI`); every other text is
/// the body field of one editable map field [app/cpy-bms/COACTUP.CPY:L60-312].
/// An absent value is passed as empty text, which `1100-RECEIVE-MAP` reads as
/// a field not entered. Each value is moved into its `X(n)` map field as
/// typed, cut at the field's width as the map's field length cuts it; the API
/// request DTO bounds the lengths before this. `customer_id` and `country` are
/// protected on the 3270 screen and sent back as fetched; the body may carry
/// others, which the rule edits, compares and writes as the paragraphs do
/// (E-74).
///
/// `Debug` withholds the social security number parts, as
/// [`CustSsn`] does.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountUpdateInput {
    /// `ACCTSIDI X(11)`: the account number.
    pub account_id: String,
    /// `ACSTTUSI X(1)`: the active status.
    pub active_status: String,
    /// `OPNYEARI X(4)`: the open date year.
    pub open_year: String,
    /// `OPNMONI X(2)`: the open date month.
    pub open_month: String,
    /// `OPNDAYI X(2)`: the open date day.
    pub open_day: String,
    /// `ACRDLIMI X(15)`: the credit limit, as `NUMVAL-C` reads it.
    pub credit_limit: String,
    /// `EXPYEARI X(4)`: the expiry date year.
    pub expiry_year: String,
    /// `EXPMONI X(2)`: the expiry date month.
    pub expiry_month: String,
    /// `EXPDAYI X(2)`: the expiry date day.
    pub expiry_day: String,
    /// `ACSHLIMI X(15)`: the cash credit limit.
    pub cash_credit_limit: String,
    /// `RISYEARI X(4)`: the reissue date year.
    pub reissue_year: String,
    /// `RISMONI X(2)`: the reissue date month.
    pub reissue_month: String,
    /// `RISDAYI X(2)`: the reissue date day.
    pub reissue_day: String,
    /// `ACURBALI X(15)`: the current balance.
    pub current_balance: String,
    /// `ACRCYCRI X(15)`: the current cycle credit.
    pub current_cycle_credit: String,
    /// `AADDGRPI X(10)`: the account group ID, which no edit checks.
    pub group_id: String,
    /// `ACRCYDBI X(15)`: the current cycle debit.
    pub current_cycle_debit: String,
    /// `ACSTNUMI X(9)`: the customer number, protected on the 3270 screen.
    pub customer_id: String,
    /// `ACTSSN1I X(3)`: the first three SSN digits.
    pub ssn_part_1: String,
    /// `ACTSSN2I X(2)`: the fourth and fifth SSN digits.
    pub ssn_part_2: String,
    /// `ACTSSN3I X(4)`: the last four SSN digits.
    pub ssn_part_3: String,
    /// `DOBYEARI X(4)`: the date of birth year.
    pub dob_year: String,
    /// `DOBMONI X(2)`: the date of birth month.
    pub dob_month: String,
    /// `DOBDAYI X(2)`: the date of birth day.
    pub dob_day: String,
    /// `ACSTFCOI X(3)`: the FICO score.
    pub fico_score: String,
    /// `ACSFNAMI X(25)`: the first name.
    pub first_name: String,
    /// `ACSMNAMI X(25)`: the middle name.
    pub middle_name: String,
    /// `ACSLNAMI X(25)`: the last name.
    pub last_name: String,
    /// `ACSADL1I X(50)`: address line 1.
    pub address_line_1: String,
    /// `ACSSTTEI X(2)`: the state code.
    pub state: String,
    /// `ACSADL2I X(50)`: address line 2, which no edit checks.
    pub address_line_2: String,
    /// `ACSZIPCI X(5)`: the zip code.
    pub zip: String,
    /// `ACSCITYI X(50)`: the city, stored as address line 3.
    pub city: String,
    /// `ACSCTRYI X(3)`: the country code, protected on the 3270 screen.
    pub country: String,
    /// `ACSPH1AI X(3)`: the area code of phone number 1.
    pub phone_1_area: String,
    /// `ACSPH1BI X(3)`: the prefix of phone number 1.
    pub phone_1_prefix: String,
    /// `ACSPH1CI X(4)`: the line number of phone number 1.
    pub phone_1_line: String,
    /// `ACSGOVTI X(20)`: the government-issued ID, which no edit checks.
    pub govt_issued_id: String,
    /// `ACSPH2AI X(3)`: the area code of phone number 2.
    pub phone_2_area: String,
    /// `ACSPH2BI X(3)`: the prefix of phone number 2.
    pub phone_2_prefix: String,
    /// `ACSPH2CI X(4)`: the line number of phone number 2.
    pub phone_2_line: String,
    /// `ACSEFTCI X(10)`: the EFT account ID.
    pub eft_account_id: String,
    /// `ACSPFLGI X(1)`: the primary card holder indicator.
    pub primary_card_holder: String,
    /// The token of the account and customer as the client fetched them:
    /// `ACUP-OLD-DETAILS`.
    pub version: VersionToken,
}

/// The SSN parts withheld.
impl fmt::Debug for AccountUpdateInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = format_args!("<redacted>");
        formatter
            .debug_struct("AccountUpdateInput")
            .field("account_id", &self.account_id)
            .field("active_status", &self.active_status)
            .field("open_year", &self.open_year)
            .field("open_month", &self.open_month)
            .field("open_day", &self.open_day)
            .field("credit_limit", &self.credit_limit)
            .field("expiry_year", &self.expiry_year)
            .field("expiry_month", &self.expiry_month)
            .field("expiry_day", &self.expiry_day)
            .field("cash_credit_limit", &self.cash_credit_limit)
            .field("reissue_year", &self.reissue_year)
            .field("reissue_month", &self.reissue_month)
            .field("reissue_day", &self.reissue_day)
            .field("current_balance", &self.current_balance)
            .field("current_cycle_credit", &self.current_cycle_credit)
            .field("group_id", &self.group_id)
            .field("current_cycle_debit", &self.current_cycle_debit)
            .field("customer_id", &self.customer_id)
            .field("ssn_part_1", &redacted)
            .field("ssn_part_2", &redacted)
            .field("ssn_part_3", &redacted)
            .field("dob_year", &self.dob_year)
            .field("dob_month", &self.dob_month)
            .field("dob_day", &self.dob_day)
            .field("fico_score", &self.fico_score)
            .field("first_name", &self.first_name)
            .field("middle_name", &self.middle_name)
            .field("last_name", &self.last_name)
            .field("address_line_1", &self.address_line_1)
            .field("state", &self.state)
            .field("address_line_2", &self.address_line_2)
            .field("zip", &self.zip)
            .field("city", &self.city)
            .field("country", &self.country)
            .field("phone_1_area", &self.phone_1_area)
            .field("phone_1_prefix", &self.phone_1_prefix)
            .field("phone_1_line", &self.phone_1_line)
            .field("govt_issued_id", &self.govt_issued_id)
            .field("phone_2_area", &self.phone_2_area)
            .field("phone_2_prefix", &self.phone_2_prefix)
            .field("phone_2_line", &self.phone_2_line)
            .field("eft_account_id", &self.eft_account_id)
            .field("primary_card_holder", &self.primary_card_holder)
            .field("version", &self.version)
            .finish()
    }
}

/// The account and customer as `9000-READ-ACCT` fetched them: the source of
/// `ACUP-OLD-DETAILS` (`9500-STORE-FETCHED-DATA`) that [`validate`] compares
/// the input with, and the state `GET /accounts/{id}` shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSnapshot {
    /// The `ACCTDAT` record.
    pub account: Account,
    /// The `CUSTDAT` record of the cross-reference's customer.
    pub customer: Customer,
}

/// The records `9600-WRITE-PROCESSING` builds from input that passed every
/// edit, for [`apply`] to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedAccountChanges {
    /// `ACCT-UPDATE-RECORD` as `ACCOUNT-RECORD` reads it (see the module notes
    /// on the account update record).
    pub account: Account,
    /// `CUST-UPDATE-RECORD`. When the received customer ID holds a non-digit
    /// its `cust_id` is the fetched one, and [`apply`] writes nothing (E-74).
    pub customer: Customer,
    /// Whether `CUST-UPDATE-ID` is the nine digits received.
    customer_key: CustomerKey,
}

/// What `CUST-UPDATE-ID` holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomerKey {
    /// The nine digits received, in `customer.cust_id`.
    Received,
    /// A received value holding a non-digit, which the `MOVE` to the `9(09)`
    /// field turns into a value the audit does not pin (E-74).
    Unpinned,
}

/// The outcome of [`validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEdit {
    /// Every edit passed: the records to write.
    Changed(Box<ValidatedAccountChanges>),
    /// The input equals the fetched values as `1205-COMPARE-OLD-NEW` compares
    /// them: nothing is edited or written, and the request succeeds with
    /// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`]
    /// (V-COACTUPC-010, 200).
    NoChange,
}

/// Why [`validate`] refused a request: an error to answer as it is, or an
/// input class held at `halt` with the escalation that holds it.
///
/// The escalation is the cause of a halt's answer, for the adapter's log; it
/// is never answered. Every refusal converts into the [`DomainError`] the
/// request is answered with, a halt into the unexpected-error outcome every
/// `halt` answers: [`ErrorKind::Unexpected`], [`UNEXPECTED_ERROR`] and no
/// fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditRefusal {
    /// A rule's error, answered unchanged: a failed key or field edit, or the
    /// unexpected-error outcome of a value the edits that passed should have
    /// proved, which no escalation holds.
    Error(DomainError),
    /// An input class held at `halt` (see the module notes): no code guesses
    /// its outcome, and the request is answered with the unexpected-error
    /// outcome.
    Halted {
        /// The ID of the `docs/parity/escalations.md` entry that holds the
        /// input class at `halt`, for example `E-26`: logged by the adapter,
        /// never part of the answer.
        escalation_id: &'static str,
    },
}

impl EditRefusal {
    /// The escalation ID of a halted input class, `None` for an error.
    #[must_use]
    pub const fn escalation_id(&self) -> Option<&'static str> {
        match self {
            Self::Error(_) => None,
            Self::Halted { escalation_id } => Some(*escalation_id),
        }
    }
}

impl From<DomainError> for EditRefusal {
    /// `error` answered unchanged.
    fn from(error: DomainError) -> Self {
        Self::Error(error)
    }
}

impl From<EditRefusal> for DomainError {
    /// The answer to `refusal`: its error, or for a halt the unexpected-error
    /// outcome, which carries nothing of the escalation.
    fn from(refusal: EditRefusal) -> Self {
        match refusal {
            EditRefusal::Error(error) => error,
            EditRefusal::Halted { .. } => halt(),
        }
    }
}

/// A request that succeeded: the records as they now stand and the
/// information text the program shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountUpdated {
    /// The account rewritten, or the fetched one when nothing changed.
    pub account: Account,
    /// The customer rewritten, or the fetched one when nothing changed.
    pub customer: Customer,
    /// [`CHANGES_COMMITTED_TO_DATABASE`] after both rewrites (P-COACTUPC-015),
    /// or [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`] when nothing
    /// changed (V-COACTUPC-010).
    pub message: &'static str,
}

/// The BMS fields of one phone number's three parts.
#[derive(Clone, Copy)]
struct PhoneFields {
    /// The area code field.
    area: &'static str,
    /// The prefix field.
    prefix: &'static str,
    /// The line number field.
    line: &'static str,
}

/// A phone number as `1100-RECEIVE-MAP` leaves its parts in
/// `ACUP-NEW-CUST-PHONE-NUM-1` or `-2`: `(2:3)`, `(6:3)` and `(10:4)`.
#[derive(Clone, Copy)]
struct PhoneParts {
    /// `ACUP-NEW-CUST-PHONE-NUM-nA X(3)`.
    area: Alnum<3>,
    /// `ACUP-NEW-CUST-PHONE-NUM-nB X(3)`.
    prefix: Alnum<3>,
    /// `ACUP-NEW-CUST-PHONE-NUM-nC X(4)`.
    line: Alnum<4>,
}

/// A signed amount as `1100-RECEIVE-MAP` leaves it: the entered text in
/// `ACUP-NEW-*-X X(15)` and, when `TEST-NUMVAL-C` accepted it, the
/// `NUMVAL-C` value in the `S9(10)V99` redefine (P-COACTUPC-019,
/// P-COACTUPC-047).
#[derive(Clone, Copy)]
struct SignedAmount {
    /// `ACUP-NEW-*-X X(15)`.
    text: Alnum<15>,
    /// `ACUP-NEW-*-N`, or `None` when the text was blank or `TEST-NUMVAL-C`
    /// rejected it and the field kept the spaces `INITIALIZE` gave it.
    value: Option<Money<10>>,
}

/// `ACUP-NEW-DETAILS` [app/cbl/COACTUPC.cbl:L757-849] as `1100-RECEIVE-MAP`
/// leaves it.
struct NewDetails {
    /// `ACUP-NEW-ACCT-ID-X X(11)`.
    acct_id: Alnum<11>,
    /// `ACUP-NEW-ACTIVE-STATUS X(1)`.
    active_status: Alnum<1>,
    /// `ACUP-NEW-CREDIT-LIMIT`.
    credit_limit: SignedAmount,
    /// `ACUP-NEW-CASH-CREDIT-LIMIT`.
    cash_credit_limit: SignedAmount,
    /// `ACUP-NEW-CURR-BAL`.
    curr_bal: SignedAmount,
    /// `ACUP-NEW-CURR-CYC-CREDIT`.
    curr_cyc_credit: SignedAmount,
    /// `ACUP-NEW-CURR-CYC-DEBIT`.
    curr_cyc_debit: SignedAmount,
    /// `ACUP-NEW-OPEN-DATE X(8)`.
    open_date: DateParts,
    /// `ACUP-NEW-EXPIRAION-DATE X(8)`.
    expiry_date: DateParts,
    /// `ACUP-NEW-REISSUE-DATE X(8)`.
    reissue_date: DateParts,
    /// `ACUP-NEW-GROUP-ID X(10)`.
    group_id: Alnum<10>,
    /// `ACUP-NEW-CUST-ID-X X(9)`.
    cust_id: Alnum<9>,
    /// `ACUP-NEW-CUST-SSN-1 X(3)`.
    ssn_1: Alnum<3>,
    /// `ACUP-NEW-CUST-SSN-2 X(2)`.
    ssn_2: Alnum<2>,
    /// `ACUP-NEW-CUST-SSN-3 X(4)`.
    ssn_3: Alnum<4>,
    /// `ACUP-NEW-CUST-DOB-YYYY-MM-DD X(8)`.
    dob: DateParts,
    /// `ACUP-NEW-CUST-FICO-SCORE-X X(3)`.
    fico: Alnum<3>,
    /// `ACUP-NEW-CUST-FIRST-NAME X(25)`.
    first_name: Alnum<25>,
    /// `ACUP-NEW-CUST-MIDDLE-NAME X(25)`.
    middle_name: Alnum<25>,
    /// `ACUP-NEW-CUST-LAST-NAME X(25)`.
    last_name: Alnum<25>,
    /// `ACUP-NEW-CUST-ADDR-LINE-1 X(50)`.
    addr_line_1: Alnum<50>,
    /// `ACUP-NEW-CUST-ADDR-LINE-2 X(50)`.
    addr_line_2: Alnum<50>,
    /// `ACUP-NEW-CUST-ADDR-LINE-3 X(50)`, from `ACSCITYI`.
    addr_line_3: Alnum<50>,
    /// `ACUP-NEW-CUST-ADDR-STATE-CD X(2)`.
    state: Alnum<2>,
    /// `ACUP-NEW-CUST-ADDR-COUNTRY-CD X(3)`.
    country: Alnum<3>,
    /// `ACUP-NEW-CUST-ADDR-ZIP X(10)`, from the `X(5)` `ACSZIPCI`.
    zip: Alnum<10>,
    /// `ACUP-NEW-CUST-PHONE-NUM-1`.
    phone_1: PhoneParts,
    /// `ACUP-NEW-CUST-PHONE-NUM-2`.
    phone_2: PhoneParts,
    /// `ACUP-NEW-CUST-GOVT-ISSUED-ID X(20)`.
    govt_issued_id: Alnum<20>,
    /// `ACUP-NEW-CUST-EFT-ACCOUNT-ID X(10)`.
    eft_account_id: Alnum<10>,
    /// `ACUP-NEW-CUST-PRI-HOLDER-IND X(1)`.
    pri_holder: Alnum<1>,
}

impl NewDetails {
    /// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-018, P-COACTUPC-019,
    /// P-COACTUPC-047, P-COACTUPC-049
    ///
    /// Every field of `input` received in map order [app/cbl/COACTUPC.cbl:L1047-1424]:
    /// moved into its `X(n)` map field, then `LOW-VALUES` when it holds `*` or
    /// spaces, else moved into its `ACUP-NEW-*` item; the five signed amounts
    /// are converted by `NUMVAL-C` when `TEST-NUMVAL-C` accepts them.
    ///
    /// # Errors
    ///
    /// [`EditRefusal::Halted`] with E-40 for an amount whose `TEST-NUMVAL-C`
    /// or `NUMVAL-C` result the audit does not pin.
    fn receive(input: &AccountUpdateInput) -> Result<Self, EditRefusal> {
        Ok(Self {
            acct_id: receive(&input.account_id),
            active_status: receive(&input.active_status),
            credit_limit: receive_amount(&input.credit_limit)?,
            cash_credit_limit: receive_amount(&input.cash_credit_limit)?,
            curr_bal: receive_amount(&input.current_balance)?,
            curr_cyc_credit: receive_amount(&input.current_cycle_credit)?,
            curr_cyc_debit: receive_amount(&input.current_cycle_debit)?,
            open_date: receive_date(&input.open_year, &input.open_month, &input.open_day),
            expiry_date: receive_date(&input.expiry_year, &input.expiry_month, &input.expiry_day),
            reissue_date: receive_date(
                &input.reissue_year,
                &input.reissue_month,
                &input.reissue_day,
            ),
            group_id: receive(&input.group_id),
            cust_id: receive(&input.customer_id),
            ssn_1: receive(&input.ssn_part_1),
            ssn_2: receive(&input.ssn_part_2),
            ssn_3: receive(&input.ssn_part_3),
            dob: receive_date(&input.dob_year, &input.dob_month, &input.dob_day),
            fico: receive(&input.fico_score),
            first_name: receive(&input.first_name),
            middle_name: receive(&input.middle_name),
            last_name: receive(&input.last_name),
            addr_line_1: receive(&input.address_line_1),
            addr_line_2: receive(&input.address_line_2),
            addr_line_3: receive(&input.city),
            state: receive(&input.state),
            country: receive(&input.country),
            zip: receive_into::<ZIP_MAP_LEN, ZIP_LEN>(&input.zip),
            phone_1: receive_phone(
                &input.phone_1_area,
                &input.phone_1_prefix,
                &input.phone_1_line,
            ),
            phone_2: receive_phone(
                &input.phone_2_area,
                &input.phone_2_prefix,
                &input.phone_2_line,
            ),
            govt_issued_id: receive(&input.govt_issued_id),
            eft_account_id: receive(&input.eft_account_id),
            pri_holder: receive(&input.primary_card_holder),
        })
    }
}

/// `ACSZIPCI PIC X(5)` [app/cpy-bms/COACTUP.CPY:L246].
const ZIP_MAP_LEN: usize = 5;

/// `ACUP-NEW-CUST-ADDR-ZIP PIC X(10)` [app/cbl/COACTUPC.cbl:L809].
const ZIP_LEN: usize = 10;

/// The bytes `1245-EDIT-NUM-REQD` edits of the zip code: `MOVE 5 TO
/// WS-EDIT-ALPHANUM-LENGTH` [app/cbl/COACTUPC.cbl:L1605-1611].
const ZIP_EDIT_LEN: usize = 5;

/// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-019
///
/// The map field `text` moved into its `X(N)` receiver, or `LOW-VALUES` when
/// it holds `'*'` or `SPACES`: empty text, spaces and a lone `*` are all a
/// value not entered.
fn receive<const N: usize>(text: &str) -> Alnum<N> {
    receive_into::<N, N>(text)
}

/// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-019
///
/// The `X(M)` map field `text` moved into an `X(N)` item: `MOVE LOW-VALUES`
/// fills all `N` bytes, and an entered value is moved left-justified and
/// padded with spaces, as `ACSZIPCI X(5)` reaches `ACUP-NEW-CUST-ADDR-ZIP
/// X(10)`.
fn receive_into<const M: usize, const N: usize>(text: &str) -> Alnum<N> {
    let field = Alnum::<M>::move_from(text);
    if field.cobol_cmp(b"").is_eq() || field.cobol_cmp(b"*").is_eq() {
        Alnum::from_array([LOW_VALUE; N])
    } else {
        Alnum::move_from_bytes(field.as_bytes())
    }
}

/// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-019
///
/// A date's year, month and day parts, each received as [`receive`] does.
fn receive_date(year: &str, month: &str, day: &str) -> DateParts {
    DateParts {
        ccyy: *receive::<4>(year).as_bytes(),
        mm: *receive::<2>(month).as_bytes(),
        dd: *receive::<2>(day).as_bytes(),
    }
}

/// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-019
///
/// A phone number's three parts, each received as [`receive`] does.
fn receive_phone(area: &str, prefix: &str, line: &str) -> PhoneParts {
    PhoneParts {
        area: receive(area),
        prefix: receive(prefix),
        line: receive(line),
    }
}

/// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-019, P-COACTUPC-047,
/// P-COACTUPC-049
///
/// A signed amount: the text received into `ACUP-NEW-*-X X(15)`, and the
/// `COMPUTE ACUP-NEW-*-N = FUNCTION NUMVAL-C(…)` value when
/// `FUNCTION TEST-NUMVAL-C(…) = 0` [app/cbl/COACTUPC.cbl:L1072-1136].
///
/// # Errors
///
/// [`EditRefusal::Halted`] with E-40 when the text is a `NUMVAL-C` argument
/// only under a reading the audit does not record, or its value is a midpoint
/// beyond the cent or exceeds the 10 integer digits of `S9(10)V99`.
fn receive_amount(text: &str) -> Result<SignedAmount, EditRefusal> {
    let unpinned = EditRefusal::Halted {
        escalation_id: E_NUMVAL_C,
    };
    let text = receive::<15>(text);
    let value = if text.is_blank() {
        None
    } else {
        match read_numval_c(text.as_bytes()) {
            NumvalC::Invalid => None,
            NumvalC::Valid(number) => Some(number.store().ok_or(unpinned)?),
            NumvalC::Unpinned => return Err(unpinned),
        }
    };
    Ok(SignedAmount { text, value })
}

/// The unexpected-error outcome of an input class held at `halt`: no code
/// guesses its result.
fn halt() -> DomainError {
    DomainError::unexpected(UNEXPECTED_ERROR)
}

// --- FUNCTION TEST-NUMVAL-C and NUMVAL-C

/// How `FUNCTION TEST-NUMVAL-C` reads an argument.
enum NumvalC {
    /// Not an argument under any reading of the IBM formats: `TEST-NUMVAL-C`
    /// is not 0.
    Invalid,
    /// An argument of the IBM formats with no grouping comma and with an
    /// upper-case `CR` or `DB`.
    Valid(NumvalNumber),
    /// An argument only under a reading the audit does not record: a grouping
    /// comma in the integer part, or a lower-case `cr` or `db`.
    Unpinned,
}

/// How far the `NUMVAL-C` formats are read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// The formats as the audit records them.
    Strict,
    /// The formats with every extension IBM's rules may allow: commas
    /// anywhere in the integer part, `CR` and `DB` in any case.
    Lenient,
}

/// The sign and digits of a `NUMVAL-C` argument.
struct NumvalNumber {
    /// A `-`, `CR` or `DB` sign.
    negative: bool,
    /// The digits before the decimal point, commas left out.
    integer: Vec<u8>,
    /// The digits after the decimal point.
    fraction: Vec<u8>,
}

/// COACTUPC 1100-RECEIVE-MAP, 1250-EDIT-SIGNED-9V2 — P-COACTUPC-019,
/// P-COACTUPC-026, V-COACTUPC-025, V-COACTUPC-038, V-COACTUPC-051,
/// V-COACTUPC-053, V-COACTUPC-055
///
/// `FUNCTION TEST-NUMVAL-C(text)` with the default currency sign `$`. The two
/// IBM formats are read, with `sp` one or more spaces, `cs` the currency sign
/// and `d` one or more digits: `[sp][+|-][sp][cs][sp]{d[.[d]] | .d}[sp]` and
/// `[sp][cs][sp]{d[.[d]] | .d}[sp][+|-|CR|DB][sp]`. Letters, two signs, two
/// decimal points and embedded spaces are rejected.
fn read_numval_c(text: &[u8]) -> NumvalC {
    if let Some(number) = scan_numval_c(text, Reading::Strict) {
        NumvalC::Valid(number)
    } else if scan_numval_c(text, Reading::Lenient).is_some() {
        NumvalC::Unpinned
    } else {
        NumvalC::Invalid
    }
}

/// `text` read as a `NUMVAL-C` argument under `reading`, `None` when it is
/// not one.
fn scan_numval_c(text: &[u8], reading: Reading) -> Option<NumvalNumber> {
    let mut at = skip_spaces(text, 0);
    let leading = match text.get(at) {
        Some(b'+') => Some(false),
        Some(b'-') => Some(true),
        _ => None,
    };
    if leading.is_some() {
        at = skip_spaces(text, at + 1);
    }
    if text.get(at) == Some(&CURRENCY_SIGN) {
        at = skip_spaces(text, at + 1);
    }
    let (integer, after_integer) = integer_digits(text, at, reading);
    at = after_integer;
    let mut fraction = Vec::new();
    if text.get(at) == Some(&b'.') {
        let digits = text
            .get(at + 1..)
            .unwrap_or_default()
            .iter()
            .take_while(|byte| byte.is_ascii_digit());
        fraction.extend(digits);
        at += 1 + fraction.len();
    }
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    at = skip_spaces(text, at);
    let mut trailing = None;
    if leading.is_none() {
        let (negative, len) = trailing_sign(text.get(at..).unwrap_or_default(), reading);
        trailing = negative;
        at = skip_spaces(text, at + len);
    }
    (at == text.len()).then(|| NumvalNumber {
        negative: leading.or(trailing).unwrap_or(false),
        integer,
        fraction,
    })
}

/// The offset of the first byte from `at` on that is not a space.
fn skip_spaces(text: &[u8], at: usize) -> usize {
    at + text
        .get(at..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&byte| byte == SPACE)
        .count()
}

/// The digits of the integer part from `at` on, and the offset after it. The
/// lenient reading also passes over commas, which it leaves out.
fn integer_digits(text: &[u8], at: usize, reading: Reading) -> (Vec<u8>, usize) {
    let run = text
        .get(at..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&byte| {
            byte.is_ascii_digit() || (reading == Reading::Lenient && byte == b',')
        });
    let mut digits = Vec::new();
    let mut len = 0;
    for &byte in run {
        len += 1;
        if byte.is_ascii_digit() {
            digits.push(byte);
        }
    }
    (digits, at + len)
}

/// The trailing sign at the start of `rest`, whether it is negative, and its
/// length; `(None, 0)` when there is none.
fn trailing_sign(rest: &[u8], reading: Reading) -> (Option<bool>, usize) {
    match rest {
        [b'+', ..] => (Some(false), 1),
        [b'-', ..] => (Some(true), 1),
        [b'C', b'R', ..] | [b'D', b'B', ..] => (Some(true), 2),
        [first, second, ..]
            if reading == Reading::Lenient
                && ((first.eq_ignore_ascii_case(&b'C') && second.eq_ignore_ascii_case(&b'R'))
                    || (first.eq_ignore_ascii_case(&b'D')
                        && second.eq_ignore_ascii_case(&b'B'))) =>
        {
            (Some(true), 2)
        }
        _ => (None, 0),
    }
}

impl NumvalNumber {
    /// COACTUPC 1100-RECEIVE-MAP — P-COACTUPC-047, P-COACTUPC-049
    ///
    /// The value `COMPUTE ACUP-NEW-*-N = FUNCTION NUMVAL-C(…)` stores in
    /// `S9(10)V99`: `NUMVAL-C` is evaluated in floating point and the store
    /// rounds at the cent (E-40), so text with at most two decimals converts
    /// exactly and text with more converts to the nearest cent (`"10.006"` is
    /// 10.01). A negative zero is stored as positive zero, the store rule of
    /// every money field (E-42).
    ///
    /// `None` for the classes E-40 holds at `halt`: digits after the second
    /// decimal that are a `5` followed only by zeros, and a value whose integer
    /// part needs more than 10 digits.
    fn store(&self) -> Option<Money<10>> {
        let digit = |total: i64, byte: &u8| -> Option<i64> {
            total.checked_mul(10)?.checked_add(i64::from(byte - b'0'))
        };
        let units = self.integer.iter().try_fold(0, digit)?;
        if units >= AMOUNT_INTEGER_LIMIT {
            return None;
        }
        let kept = self
            .fraction
            .iter()
            .chain(std::iter::repeat(&b'0'))
            .take(2)
            .try_fold(0, digit)?;
        let round_up = match self.fraction.get(2..).unwrap_or_default() {
            [] => false,
            [b'5', rest @ ..] if rest.iter().all(|&byte| byte == b'0') => return None,
            [first, ..] => *first >= b'5',
        };
        let cents = units
            .checked_mul(100)?
            .checked_add(kept)?
            .checked_add(i64::from(round_up))?;
        Money::from_cents(if self.negative { -cents } else { cents }).ok()
    }
}

// --- 1210-EDIT-ACCOUNT and 9000-READ-ACCT: the key and the fetch

/// COACTUPC 1100-RECEIVE-MAP, 1210-EDIT-ACCOUNT, 1200-EDIT-MAP-INPUTS —
/// P-COACTUPC-003, P-COACTUPC-018, V-COACTUPC-002, V-COACTUPC-003
///
/// The account number as entered, received into `CC-ACCT-ID X(11)`
/// [app/cbl/COACTUPC.cbl:L1051-1058] and edited [app/cbl/COACTUPC.cbl:L1783-1820].
///
/// # Errors
///
/// - [`ErrorKind::Empty`] with [`NO_INPUT_RECEIVED`] at `ACCTSID` when the
///   number is blank or `*`: `1200-EDIT-MAP-INPUTS` replaces the prompt
///   `1210` sets (E-45).
/// - [`ErrorKind::Invalid`] with
///   [`ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO`] at `ACCTSID`
///   when it is not 11 digits or is all zeros.
pub fn edit_account_id(account_id: &str) -> Result<AccountId, DomainError> {
    let key = receive::<11>(account_id);
    if key.is_blank() {
        return Err(DomainError::empty(NO_INPUT_RECEIVED).with_field(FIELD_ACCTSID));
    }
    if !key.is_numeric() || is_all_zeros(key.as_bytes()) {
        return Err(
            DomainError::invalid(ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO)
                .with_field(FIELD_ACCTSID),
        );
    }
    AccountId::from_str(&key.as_text()).map_err(|_| halt())
}

/// COACTUPC 9000-READ-ACCT, 9200-GETCARDXREF-BYACCT, 9300-GETACCTDATA-BYACCT,
/// 9400-GETCUSTDATA-BYCUST, 9500-STORE-FETCHED-DATA — P-COACTUPC-043 to
/// P-COACTUPC-045, P-COACTUPC-050, V-COACTUPC-004 to V-COACTUPC-009
///
/// The fetch [app/cbl/COACTUPC.cbl:L3608-3796]: the card cross-reference of
/// `account_id` through the `CXACAIX` path, then the account, then the
/// customer of the cross-reference, each a CICS `READ` without a lock. A
/// cross-reference failure ends the fetch; the customer is read even when the
/// account read failed, because the `DID-NOT-FIND-ACCT-IN-ACCTDAT` test never
/// matches (E-45). The snapshot is the source of `ACUP-OLD-DETAILS`.
///
/// # Errors
///
/// All at `ACCTSID`, with the text the last failing read leaves in
/// `WS-RETURN-MSG`:
///
/// - [`ErrorKind::NotFound`] with [`xref_not_found_message`] (`RESP` 13,
///   `RESP2` 80) when the account has no cross-reference, and
///   [`ErrorKind::Unexpected`] with the [`file_error_message`] of `CXACAIX`
///   on any other failure (`RESP` 17, `RESP2` 0, E-44).
/// - [`ErrorKind::NotFound`] with [`account_master_not_found_message`] when
///   the account is not on file, and [`ErrorKind::Unexpected`] with the
///   [`file_error_message`] of `ACCTDAT` on any other failure.
/// - [`ErrorKind::NotFound`] with [`customer_master_not_found_message`] when
///   the account was read and the customer is not on file.
/// - [`ErrorKind::Unexpected`] with the [`file_error_message`] of `CUSTDAT`
///   when the customer read fails otherwise: `WHEN OTHER` moves its text
///   unconditionally [app/cbl/COACTUPC.cbl:L3785-3792], replacing an account
///   text.
pub async fn read_snapshot<S: XrefReader + AccountReader + CustomerReader>(
    store: &mut S,
    account_id: AccountId,
) -> Result<AccountSnapshot, DomainError> {
    let account_text = account_id.to_string();
    let xref = store
        .read_xref_by_account(account_id)
        .await
        .map_err(|error| {
            let failure = if error == PortError::NotFound {
                DomainError::not_found(xref_not_found_message(
                    &account_text,
                    RESP_NOTFND,
                    RESP2_NOTFND,
                ))
            } else {
                read_failure(LIT_CARDXREFNAME_ACCT_PATH)
            };
            failure.with_field(FIELD_ACCTSID)
        })?;
    let account = store.read_account(account_id).await;
    let customer_id = xref.xref_cust_id;
    let customer = store.read_customer(customer_id).await;
    let failure =
        match (account, customer) {
            (Ok(account), Ok(customer)) => return Ok(AccountSnapshot { account, customer }),
            (_, Err(error)) if error != PortError::NotFound => read_failure(LIT_CUSTFILENAME),
            (Err(PortError::NotFound), _) => DomainError::not_found(
                account_master_not_found_message(&account_text, RESP_NOTFND, RESP2_NOTFND),
            ),
            (Err(_), _) => read_failure(LIT_ACCTFILENAME),
            (Ok(_), Err(_)) => DomainError::not_found(customer_master_not_found_message(
                &customer_id.to_string(),
                RESP_NOTFND,
                RESP2_NOTFND,
            )),
        };
    Err(failure.with_field(FIELD_ACCTSID))
}

/// COACTUPC 9200-GETCARDXREF-BYACCT, 9300-GETACCTDATA-BYACCT,
/// 9400-GETCUSTDATA-BYCUST `WHEN OTHER` — V-COACTUPC-005, V-COACTUPC-007,
/// V-COACTUPC-009
///
/// `WS-FILE-ERROR-MESSAGE` of a failed `READ` of `file`, with the `RESP` and
/// `RESP2` E-44 gives a database failure.
fn read_failure(file: &str) -> DomainError {
    DomainError::unexpected(file_error_message(
        ERROR_OPNAME_READ,
        file,
        RESP_IOERR,
        RESP2_IOERR,
    ))
}

// --- Byte helpers of the COBOL conditions

/// `= SPACES OR = LOW-VALUES` of a reference-modified range: every byte a
/// space, or every byte `X'00'`. A mix of the two is neither.
fn is_blank_bytes(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == SPACE) || bytes.iter().all(|&byte| byte == LOW_VALUE)
}

/// `= SPACES` of a range.
fn is_spaces(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == SPACE)
}

/// `= LOW-VALUES` of a range.
fn is_low_values(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == LOW_VALUE)
}

/// `= ZEROS` of an alphanumeric range: every byte the digit `0`.
fn is_all_zeros(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == b'0')
}

/// The class test `IS NUMERIC` of a range: every byte a digit.
fn is_digits(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_digit)
}

/// `FUNCTION UPPER-CASE`: the letters `a` to `z` upper-cased, every other
/// byte unchanged.
fn upper_cased(bytes: &[u8]) -> Vec<u8> {
    bytes.to_ascii_uppercase()
}

/// `FUNCTION LOWER-CASE`: the letters `A` to `Z` lower-cased, every other
/// byte unchanged.
fn lower_cased(bytes: &[u8]) -> Vec<u8> {
    bytes.to_ascii_lowercase()
}

/// `FUNCTION UPPER-CASE (FUNCTION TRIM (…))` as an alphanumeric comparison
/// operand: leading and trailing spaces removed, letters upper-cased. Two
/// such operands are equal under the space-padded comparison exactly when
/// these bytes are equal.
fn trim_upper(bytes: &[u8]) -> Vec<u8> {
    let start = bytes.iter().position(|&byte| byte != SPACE);
    let end = bytes.iter().rposition(|&byte| byte != SPACE);
    match (start, end) {
        (Some(start), Some(end)) => upper_cased(bytes.get(start..=end).unwrap_or_default()),
        _ => Vec::new(),
    }
}

/// A received item in its stored form: each `X'00'` byte a space (see the
/// module notes on the stored form of blank fields).
fn stored<const N: usize>(field: &Alnum<N>) -> Alnum<N> {
    Alnum::from_array(stored_bytes(field.as_bytes()))
}

/// `bytes` with each `X'00'` byte a space.
fn stored_bytes<const N: usize>(bytes: &[u8; N]) -> [u8; N] {
    bytes.map(|byte| if byte == LOW_VALUE { SPACE } else { byte })
}

/// The reference modification `(start:len)` of a stored field, empty where
/// it would run past the field; every call names a range inside the field.
fn part<const N: usize>(field: &Alnum<N>, start: usize, len: usize) -> &[u8] {
    field.ref_mod(start, len).unwrap_or_default()
}

/// The eight bytes of a received date, in stored form: `CCYY`, `MM`, `DD`.
fn date_bytes(parts: DateParts) -> Vec<u8> {
    [parts.ccyy.as_slice(), &parts.mm, &parts.dd]
        .concat()
        .into_iter()
        .map(|byte| if byte == LOW_VALUE { SPACE } else { byte })
        .collect()
}

/// COACTUPC 9500-STORE-FETCHED-DATA, 1205-COMPARE-OLD-NEW — P-COACTUPC-004,
/// P-COACTUPC-005
///
/// The year, month and day of a stored `X(10)` date as `9500-STORE-FETCHED-DATA`
/// moves them: `(1:4)`, `(6:2)` and `(9:2)`.
fn fetched_date_bytes(date: &Alnum<10>) -> Vec<u8> {
    [part(date, 1, 4), part(date, 6, 2), part(date, 9, 2)].concat()
}

/// `bytes` with each `X'00'` byte a space.
fn stored_vec(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&byte| if byte == LOW_VALUE { SPACE } else { byte })
        .collect()
}

// --- 1205-COMPARE-OLD-NEW

/// COACTUPC 1205-COMPARE-OLD-NEW — P-COACTUPC-005, V-COACTUPC-010
///
/// Whether every received value equals the fetched value as the paragraph
/// compares them [app/cbl/COACTUPC.cbl:L1681-1782]: the account data first,
/// then the customer data. A received value is read in its stored form (see
/// the module notes).
fn no_changes_found(new: &NewDetails, old: &AccountSnapshot) -> bool {
    account_unchanged(new, &old.account) && customer_unchanged(new, &old.customer)
}

/// COACTUPC 1205-COMPARE-OLD-NEW — P-COACTUPC-005
///
/// The account half [app/cbl/COACTUPC.cbl:L1684-1701]: the account number
/// exactly; the status upper-cased; each signed amount by its `S9(10)V99`
/// value, so an amount `TEST-NUMVAL-C` rejected, whose redefine kept the
/// spaces of `INITIALIZE`, always differs; each date by its eight `CCYYMMDD`
/// bytes; the group ID trimmed and upper-cased.
fn account_unchanged(new: &NewDetails, old: &Account) -> bool {
    let amounts = [
        (&new.curr_bal, old.acct_curr_bal),
        (&new.credit_limit, old.acct_credit_limit),
        (&new.cash_credit_limit, old.acct_cash_credit_limit),
        (&new.curr_cyc_credit, old.acct_curr_cyc_credit),
        (&new.curr_cyc_debit, old.acct_curr_cyc_debit),
    ];
    let dates = [
        (&new.open_date, &old.acct_open_date),
        (&new.expiry_date, &old.acct_expiraion_date),
        (&new.reissue_date, &old.acct_reissue_date),
    ];
    stored_vec(new.acct_id.as_bytes()) == old.acct_id.to_string().as_bytes()
        && upper_cased(&stored_vec(new.active_status.as_bytes()))
            == upper_cased(old.acct_active_status.as_bytes())
        && amounts
            .iter()
            .all(|(amount, fetched)| amount.value == Some(*fetched))
        && dates
            .iter()
            .all(|(date, fetched)| date_bytes(**date) == fetched_date_bytes(fetched))
        && trim_upper(&stored_vec(new.group_id.as_bytes()))
            == trim_upper(old.acct_group_id.as_bytes())
}

/// COACTUPC 1205-COMPARE-OLD-NEW — P-COACTUPC-005
///
/// The customer half [app/cbl/COACTUPC.cbl:L1708-1772]: the customer ID,
/// names, address lines, state, country, zip, government ID and primary
/// holder indicator trimmed and upper-cased; the phone parts, the nine SSN
/// digits, the eight date-of-birth bytes, the EFT account ID and the FICO
/// score exactly.
fn customer_unchanged(new: &NewDetails, old: &Customer) -> bool {
    let same_trimmed =
        |received: &[u8], fetched: &[u8]| trim_upper(&stored_vec(received)) == trim_upper(fetched);
    let same = |received: &[u8], fetched: &[u8]| stored_vec(received) == fetched;
    let ssn = [
        new.ssn_1.as_bytes().as_slice(),
        new.ssn_2.as_bytes().as_slice(),
        new.ssn_3.as_bytes().as_slice(),
    ]
    .concat();
    same_trimmed(new.cust_id.as_bytes(), old.cust_id.to_string().as_bytes())
        && same_trimmed(new.first_name.as_bytes(), old.cust_first_name.as_bytes())
        && same_trimmed(new.middle_name.as_bytes(), old.cust_middle_name.as_bytes())
        && same_trimmed(new.last_name.as_bytes(), old.cust_last_name.as_bytes())
        && same_trimmed(new.addr_line_1.as_bytes(), old.cust_addr_line_1.as_bytes())
        && same_trimmed(new.addr_line_2.as_bytes(), old.cust_addr_line_2.as_bytes())
        && same_trimmed(new.addr_line_3.as_bytes(), old.cust_addr_line_3.as_bytes())
        && same_trimmed(new.state.as_bytes(), old.cust_addr_state_cd.as_bytes())
        && same_trimmed(new.country.as_bytes(), old.cust_addr_country_cd.as_bytes())
        && same_trimmed(new.zip.as_bytes(), old.cust_addr_zip.as_bytes())
        && phone_unchanged(&new.phone_1, &old.cust_phone_num_1)
        && phone_unchanged(&new.phone_2, &old.cust_phone_num_2)
        && same(&ssn, old.cust_ssn.to_string().as_bytes())
        && same_trimmed(
            new.govt_issued_id.as_bytes(),
            old.cust_govt_issued_id.as_bytes(),
        )
        && date_bytes(new.dob) == fetched_date_bytes(&old.cust_dob_yyyy_mm_dd)
        && same(
            new.eft_account_id.as_bytes(),
            old.cust_eft_account_id.as_bytes(),
        )
        && same_trimmed(
            new.pri_holder.as_bytes(),
            old.cust_pri_card_holder_ind.as_bytes(),
        )
        && same(
            new.fico.as_bytes(),
            old.cust_fico_credit_score.to_string().as_bytes(),
        )
}

/// COACTUPC 1205-COMPARE-OLD-NEW — P-COACTUPC-005, P-COACTUPC-027
///
/// The three received parts of a phone number against `(2:3)`, `(6:3)` and
/// `(10:4)` of the fetched `X(15)` number, which `ACUP-OLD-CUST-PHONE-NUM-nA`
/// to `-nC` redefine [app/cbl/COACTUPC.cbl:L722-741].
fn phone_unchanged(new: &PhoneParts, old: &Alnum<15>) -> bool {
    stored_vec(new.area.as_bytes()) == part(old, 2, 3)
        && stored_vec(new.prefix.as_bytes()) == part(old, 6, 3)
        && stored_vec(new.line.as_bytes()) == part(old, 10, 4)
}

// --- 1200-EDIT-MAP-INPUTS: the field edits

/// `WS-RETURN-MSG`, the kind of the text it holds, and the field flags the
/// edits of `1200-EDIT-MAP-INPUTS` set.
///
/// Each failing edit flags its field and writes its text only while no text
/// is set, so the first failing edit supplies the message and its kind, and
/// `fields` lists every flagged field in edit order, each with the text of
/// the edit that flagged it (validation-audit COACTUPC Precedence).
#[derive(Default)]
struct Edits {
    /// `WS-RETURN-MSG`.
    msg: ReturnMsg,
    /// The kind of the text `msg` holds.
    kind: Option<ErrorKind>,
    /// The flagged fields in edit order.
    fields: Vec<FieldError>,
}

impl Edits {
    /// A failing edit of `kind` flags `field` with `text`.
    fn flag(&mut self, kind: ErrorKind, field: &'static str, text: String) {
        if self.msg.is_off() {
            self.kind = Some(kind);
            self.msg.set_if_off(text.clone(), field);
        }
        self.fields.push(FieldError {
            field,
            message: text,
        });
    }

    /// A field edit failure: `STRING FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)`
    /// and `suffix` into `WS-RETURN-MSG X(75)`.
    fn flag_edit(&mut self, kind: ErrorKind, field: &'static str, label: &str, suffix: &str) {
        self.flag(kind, field, field_edit_message(label, suffix));
    }

    /// The outcome of the edits: `INPUT-ERROR` is the first text with its
    /// kind and every flagged field.
    fn finish(self) -> Result<(), DomainError> {
        let Self { msg, kind, fields } = self;
        match (msg.into_parts(), kind) {
            (Some((message, _)), Some(kind)) => {
                let mut error = DomainError::new(kind, message);
                error.fields = fields;
                Err(error)
            }
            _ => Ok(()),
        }
    }

    /// COACTUPC 1215-EDIT-MANDATORY — P-COACTUPC-020, V-COACTUPC-087
    ///
    /// `value` blank or low-values is not supplied.
    fn mandatory(&mut self, label: &str, value: &[u8], field: &'static str) {
        if is_blank_bytes(value) {
            self.flag_edit(ErrorKind::Empty, field, label, MUST_BE_SUPPLIED);
        }
    }

    /// COACTUPC 1220-EDIT-YESNO — P-COACTUPC-021, V-COACTUPC-011,
    /// V-COACTUPC-012, V-COACTUPC-121, V-COACTUPC-122
    ///
    /// Low-values, a space or `0` is not supplied; anything but an upper-case
    /// `Y` or `N` (88 `FLG-YES-NO-ISVALID`) is rejected.
    fn yes_no(&mut self, label: &str, value: Alnum<1>, field: &'static str) {
        match value.as_bytes() {
            [LOW_VALUE | SPACE | b'0'] => {
                self.flag_edit(ErrorKind::Empty, field, label, MUST_BE_SUPPLIED);
            }
            [b'Y' | b'N'] => {}
            _ => self.flag_edit(ErrorKind::Invalid, field, label, MUST_BE_Y_OR_N),
        }
    }

    /// COACTUPC 1225-EDIT-ALPHA-REQD — P-COACTUPC-022, V-COACTUPC-082,
    /// V-COACTUPC-083, V-COACTUPC-085, V-COACTUPC-086, V-COACTUPC-088,
    /// V-COACTUPC-089, V-COACTUPC-094 to V-COACTUPC-097
    ///
    /// `value` blank or low-values is not supplied; a byte other than a
    /// letter or a space is rejected: `INSPECT … CONVERTING` turns the 52
    /// letters into spaces and the trimmed rest must be empty. Whether the
    /// edit passed (`FLG-ALPHA-ISVALID`).
    fn alpha_required(&mut self, label: &str, value: &[u8], field: &'static str) -> bool {
        if is_blank_bytes(value) {
            self.flag_edit(ErrorKind::Empty, field, label, MUST_BE_SUPPLIED);
            false
        } else {
            self.alpha_only(label, value, field)
        }
    }

    /// COACTUPC 1235-EDIT-ALPHA-OPT — P-COACTUPC-024, V-COACTUPC-084
    ///
    /// `value` blank or low-values passes; otherwise the alphabetic test of
    /// [`Edits::alpha_required`].
    fn alpha_optional(&mut self, label: &str, value: &[u8], field: &'static str) {
        if !is_blank_bytes(value) {
            self.alpha_only(label, value, field);
        }
    }

    /// The alphabetic test of 1225 and 1235: every byte a letter or a space.
    fn alpha_only(&mut self, label: &str, value: &[u8], field: &'static str) -> bool {
        let alphabetic = value
            .iter()
            .all(|&byte| byte.is_ascii_alphabetic() || byte == SPACE);
        if !alphabetic {
            self.flag_edit(ErrorKind::Invalid, field, label, CAN_HAVE_ALPHABETS_ONLY);
        }
        alphabetic
    }

    /// COACTUPC 1245-EDIT-NUM-REQD — P-COACTUPC-025, V-COACTUPC-056 to
    /// V-COACTUPC-058, V-COACTUPC-060 to V-COACTUPC-065, V-COACTUPC-078 to
    /// V-COACTUPC-080, V-COACTUPC-091 to V-COACTUPC-093, V-COACTUPC-118 to
    /// V-COACTUPC-120
    ///
    /// `value`, the first `WS-EDIT-ALPHANUM-LENGTH` bytes, blank or low-values
    /// is not supplied; one that is not all digits is rejected; one whose
    /// `NUMVAL` is zero, all zeros once it is all digits, is rejected. Whether
    /// the edit passed (`FLG-ALPHNANUM-ISVALID`).
    fn num_required(&mut self, label: &str, value: &[u8], field: &'static str) -> bool {
        let failure = if is_blank_bytes(value) {
            Some((ErrorKind::Empty, MUST_BE_SUPPLIED))
        } else if !is_digits(value) {
            Some((ErrorKind::Invalid, MUST_BE_ALL_NUMERIC))
        } else if is_all_zeros(value) {
            Some((ErrorKind::Invalid, MUST_NOT_BE_ZERO))
        } else {
            None
        };
        if let Some((kind, suffix)) = failure {
            self.flag_edit(kind, field, label, suffix);
        }
        failure.is_none()
    }

    /// COACTUPC 1250-EDIT-SIGNED-9V2 — P-COACTUPC-026, V-COACTUPC-024,
    /// V-COACTUPC-025, V-COACTUPC-037, V-COACTUPC-038, V-COACTUPC-050 to
    /// V-COACTUPC-055
    ///
    /// `WS-EDIT-SIGNED-NUMBER-9V2-X` low-values or spaces is not supplied;
    /// text `TEST-NUMVAL-C` rejects is not valid. `1100-RECEIVE-MAP` already
    /// applied the same test and kept the value only when it passed.
    fn signed_9v2(&mut self, label: &str, amount: &SignedAmount, field: &'static str) {
        if amount.text.is_blank() {
            self.flag_edit(ErrorKind::Empty, field, label, MUST_BE_SUPPLIED);
        } else if amount.value.is_none() {
            self.flag_edit(ErrorKind::Invalid, field, label, IS_NOT_VALID);
        }
    }
}

/// The texts of one phone part's three failures: not supplied, not a number
/// of its width, zero.
struct PhonePartTexts {
    /// `': … code must be supplied.'`.
    supplied: &'static str,
    /// `': … code must be A n digit number.'`.
    not_numeric: &'static str,
    /// `': … code cannot be zero'`.
    zero: &'static str,
}

/// The texts of `EDIT-US-PHONE-PREFIX`.
const PREFIX_TEXTS: PhonePartTexts = PhonePartTexts {
    supplied: PREFIX_CODE_MUST_BE_SUPPLIED,
    not_numeric: PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER,
    zero: PREFIX_CODE_CANNOT_BE_ZERO,
};

/// The texts of `EDIT-US-PHONE-LINENUM`.
const LINE_NUMBER_TEXTS: PhonePartTexts = PhonePartTexts {
    supplied: LINE_NUMBER_CODE_MUST_BE_SUPPLIED,
    not_numeric: LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER,
    zero: LINE_NUMBER_CODE_CANNOT_BE_ZERO,
};

impl Edits {
    /// COACTUPC 1260-EDIT-US-PHONE-NUM — P-COACTUPC-027 to P-COACTUPC-030,
    /// V-COACTUPC-098 to V-COACTUPC-117
    ///
    /// A phone number whose three parts are blank passes: the test compares
    /// the area code twice [app/cbl/COACTUPC.cbl:L2234-2241], which no input
    /// can observe because `1100-RECEIVE-MAP` turns blank parts into
    /// low-values (E-45). Otherwise the area code, the prefix and the line
    /// number are each edited, every `GO TO` leading to the next part's
    /// paragraph.
    fn phone(&mut self, label: &str, phone: &PhoneParts, fields: PhoneFields) {
        let (area, prefix, line) = (
            phone.area.as_bytes().as_slice(),
            phone.prefix.as_bytes().as_slice(),
            phone.line.as_bytes().as_slice(),
        );
        if is_blank_bytes(area)
            && is_blank_bytes(prefix)
            && (is_spaces(area) || is_low_values(line))
        {
            return;
        }
        self.area_code(label, area, fields.area);
        self.phone_part(label, prefix, fields.prefix, &PREFIX_TEXTS);
        self.phone_part(label, line, fields.line, &LINE_NUMBER_TEXTS);
    }

    /// COACTUPC EDIT-AREA-CODE — P-COACTUPC-028, P-CSLKPCDY-002,
    /// V-COACTUPC-098 to V-COACTUPC-101, V-COACTUPC-108 to V-COACTUPC-111
    ///
    /// The area code blank or low-values is not supplied; not three digits,
    /// zero, or not one of the general-purpose codes of `CSLKPCDY`
    /// (88 `VALID-GENERAL-PURP-CODE`) is rejected.
    fn area_code(&mut self, label: &str, area: &[u8], field: &'static str) {
        let failure = if is_blank_bytes(area) {
            Some((ErrorKind::Empty, AREA_CODE_MUST_BE_SUPPLIED))
        } else if !is_digits(area) {
            Some((ErrorKind::Invalid, AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER))
        } else if is_all_zeros(area) {
            Some((ErrorKind::Invalid, AREA_CODE_CANNOT_BE_ZERO))
        } else if !VALID_GENERAL_PURP_CODE
            .iter()
            .any(|code| code.as_bytes() == area)
        {
            Some((ErrorKind::Invalid, NOT_VALID_NORTH_AMERICA_AREA_CODE))
        } else {
            None
        };
        if let Some((kind, suffix)) = failure {
            self.flag_edit(kind, field, label, suffix);
        }
    }

    /// COACTUPC EDIT-US-PHONE-PREFIX, EDIT-US-PHONE-LINENUM — P-COACTUPC-029,
    /// P-COACTUPC-030, V-COACTUPC-102 to V-COACTUPC-107, V-COACTUPC-112 to
    /// V-COACTUPC-117
    ///
    /// The prefix or line number blank or low-values is not supplied; not all
    /// digits or zero is rejected.
    fn phone_part(
        &mut self,
        label: &str,
        value: &[u8],
        field: &'static str,
        texts: &PhonePartTexts,
    ) {
        let failure = if is_blank_bytes(value) {
            Some((ErrorKind::Empty, texts.supplied))
        } else if !is_digits(value) {
            Some((ErrorKind::Invalid, texts.not_numeric))
        } else if is_all_zeros(value) {
            Some((ErrorKind::Invalid, texts.zero))
        } else {
            None
        };
        if let Some((kind, suffix)) = failure {
            self.flag_edit(kind, field, label, suffix);
        }
    }

    /// COACTUPC 1265-EDIT-US-SSN — P-COACTUPC-031, V-COACTUPC-056 to
    /// V-COACTUPC-065
    ///
    /// Part 1 is edited as three required digits and must not be 666 or 900
    /// to 999 (88 `INVALID-SSN-PART1`; zero has already failed). The `IF` that
    /// tests part 1 is closed only by the paragraph's period
    /// [app/cbl/COACTUPC.cbl:L2448-2488], so parts 2 and 3 are edited only
    /// when part 1 passed its digit edit (E-45).
    fn ssn(&mut self, new: &NewDetails) {
        let first = new.ssn_1.as_bytes();
        if !self.num_required(FIELD_SSN_FIRST_3_CHARS, first, FIELD_ACTSSN1) {
            return;
        }
        let invalid_area = matches!(first, [b'6', b'6', b'6'] | [b'9', _, _]);
        if invalid_area {
            self.flag_edit(
                ErrorKind::Invalid,
                FIELD_ACTSSN1,
                FIELD_SSN_FIRST_3_CHARS,
                SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999,
            );
        }
        self.num_required(
            FIELD_SSN_4TH_AND_5TH_CHARS,
            new.ssn_2.as_bytes(),
            FIELD_ACTSSN2,
        );
        self.num_required(FIELD_SSN_LAST_4_CHARS, new.ssn_3.as_bytes(), FIELD_ACTSSN3);
    }

    /// COACTUPC 1225-EDIT-ALPHA-REQD, 1270-EDIT-US-STATE-CD — P-COACTUPC-032,
    /// P-CSLKPCDY-004, V-COACTUPC-088 to V-COACTUPC-090
    ///
    /// The state code edited as two required letters, then, when that passed,
    /// looked up in `CSLKPCDY` (88 `VALID-US-STATE-CODE`, upper case only).
    /// Whether the state is valid (`FLG-STATE-ISVALID`).
    fn state(&mut self, state: Alnum<2>) -> bool {
        if !self.alpha_required(FIELD_STATE, state.as_bytes(), FIELD_ACSSTTE) {
            return false;
        }
        let listed = VALID_US_STATE_CODE
            .iter()
            .any(|code| code.as_bytes() == state.as_bytes());
        if !listed {
            self.flag_edit(
                ErrorKind::Invalid,
                FIELD_ACSSTTE,
                FIELD_STATE,
                IS_NOT_A_VALID_STATE_CODE,
            );
        }
        listed
    }

    /// COACTUPC 1245-EDIT-NUM-REQD, 1275-EDIT-FICO-SCORE — P-COACTUPC-033,
    /// V-COACTUPC-078 to V-COACTUPC-081
    ///
    /// The FICO score edited as three required digits, then, when that
    /// passed, checked against 300 to 850 (88 `FICO-RANGE-IS-VALID`).
    fn fico_score(&mut self, fico: Alnum<3>) {
        if !self.num_required(FIELD_FICO_SCORE, fico.as_bytes(), FIELD_ACSTFCO) {
            return;
        }
        let in_range = FicoScore::from_str(&fico.as_text())
            .is_ok_and(|score| (FICO_MIN..=FICO_MAX).contains(&score.get()));
        if !in_range {
            self.flag_edit(
                ErrorKind::Invalid,
                FIELD_ACSTFCO,
                FIELD_FICO_SCORE,
                SHOULD_BE_BETWEEN_300_AND_850,
            );
        }
    }

    /// COACTUPC 1280-EDIT-US-STATE-ZIP-CD — P-COACTUPC-007, P-CSLKPCDY-005,
    /// V-COACTUPC-123
    ///
    /// The state code followed by the first two zip digits must be one of the
    /// `CSLKPCDY` pairs (88 `VALID-US-STATE-ZIP-CD2-COMBO`); a failure flags
    /// the state and the zip code.
    fn state_zip(&mut self, state: Alnum<2>, zip: &Alnum<10>) {
        let pair = [state.as_bytes().as_slice(), zip.prefix(2)].concat();
        let listed = VALID_US_STATE_ZIP_CD2_COMBO
            .iter()
            .any(|combo| combo.as_bytes() == pair.as_slice());
        if !listed {
            self.flag(
                ErrorKind::Invalid,
                FIELD_ACSSTTE,
                INVALID_ZIP_CODE_FOR_STATE.to_owned(),
            );
            self.fields.push(FieldError {
                field: FIELD_ACSZIPC,
                message: INVALID_ZIP_CODE_FOR_STATE.to_owned(),
            });
        }
    }

    /// CSUTLDPY EDIT-DATE-CCYYMMDD THRU EDIT-DATE-CCYYMMDD-EXIT, as
    /// `1200-EDIT-MAP-INPUTS` performs it — P-CSUTLDPY-001 to P-CSUTLDPY-012,
    /// P-CSUTLDPY-014, V-COACTUPC-013 to V-COACTUPC-023, V-COACTUPC-026 to
    /// V-COACTUPC-036, V-COACTUPC-039 to V-COACTUPC-049, V-COACTUPC-066 to
    /// V-COACTUPC-076
    ///
    /// The date edits of `parts`, whose first text stands as every other
    /// edit's does. Whether the date is valid (`WS-EDIT-DATE-IS-VALID`).
    ///
    /// A date that passes every `CSUTLDPY` edit reaches `CSUTLDTC`, whose
    /// outcome E-26 holds at `halt`. Once an earlier edit has set the message
    /// that outcome is pinned: a rejection would write no text and its exit
    /// would leave the flags valid [app/cpy/CSUTLDPY.cpy:L315-331], as an
    /// acceptance does, so the date is valid.
    ///
    /// # Errors
    ///
    /// [`EditRefusal::Halted`] with the escalation `super::date_edit` reports:
    /// for a date `CSUTLDTC` would decide while no message is set (E-26), and
    /// for a month holding a non-digit (E-47) or a day of exactly one half
    /// (E-40).
    fn date(
        &mut self,
        label: &'static str,
        parts: DateParts,
        fields: DateBmsFields,
    ) -> Result<bool, EditRefusal> {
        let mut own = ReturnMsg::default();
        match edit_date_ccyymmdd(label, &parts, &mut own, fields) {
            Ok(flags) => {
                self.date_flags(parts, flags, fields, own);
                Ok(flags.is_valid())
            }
            Err(DateValidationError::Unproven { escalation_id })
                if escalation_id == E_CSUTLDPY_LINKAGE && !self.msg.is_off() =>
            {
                Ok(true)
            }
            Err(DateValidationError::Unproven { escalation_id }) => {
                Err(EditRefusal::Halted { escalation_id })
            }
        }
    }

    /// CSUTLDPY EDIT-DATE-OF-BIRTH — P-CSUTLDPY-013, V-COACTUPC-077
    ///
    /// The date of birth, valid after [`Edits::date`], must be before
    /// `today`; a failure flags its three parts.
    fn date_of_birth(&mut self, parts: DateParts, today: NaiveDate) {
        let fields = DateBmsFields::DATE_OF_BIRTH;
        let mut own = ReturnMsg::default();
        let flags = edit_date_of_birth(FIELD_DATE_OF_BIRTH, &parts, today, &mut own, fields.year);
        self.date_flags(parts, flags, fields, own);
    }

    /// The `entered` parts of one date, the flags its edit left and the text
    /// it wrote into `own`: the text stands if no earlier text is set, and
    /// every part the flags leave not valid is listed with it. A text that
    /// names no part, the `CSUTLDTC` rejection at `ACCTSID`, is listed at its
    /// own field.
    ///
    /// The text's kind is that of the rejection that wrote it: `Empty` when
    /// the part whose field it names is blank as entered (`= SPACES OR =
    /// LOW-VALUES`, the test of `EDIT-YEAR-CCYY`, `EDIT-MONTH` and
    /// `EDIT-DAY`), else `Invalid`. A part's "must be supplied" text is
    /// written before any later step can name that part's field, so this is
    /// exact; the text naming no part, and the birth date's `cannot be in the
    /// future` on parts every edit passed, stay `Invalid`.
    ///
    /// The final flags cannot decide the kind, because a later step
    /// overwrites a `'B'` flag without writing a text: a blank month is set
    /// `FLG-MONTH-BLANK` with its text by `EDIT-MONTH` (P-CSUTLDPY-005)
    /// [app/cpy/CSUTLDPY.cpy:L94-105], then with day 31 it is not a
    /// `WS-31-DAY-MONTH`, so `EDIT-DAY-MONTH-YEAR` sets `FLG-MONTH-NOT-OK`
    /// and `FLG-DAY-NOT-OK` (P-CSUTLDPY-009) [app/cpy/CSUTLDPY.cpy:L213-225];
    /// the month's text still stands with kind `Empty` (V-COACTUPC-016).
    fn date_flags(
        &mut self,
        entered: DateParts,
        flags: DateEditFlags,
        fields: DateBmsFields,
        own: ReturnMsg,
    ) {
        let Some((text, field)) = own.into_parts() else {
            return;
        };
        let blank = [
            (entered.ccyy.as_slice(), fields.year),
            (entered.mm.as_slice(), fields.month),
            (entered.dd.as_slice(), fields.day),
        ]
        .iter()
        .any(|&(bytes, name)| name == field && is_blank_bytes(bytes));
        let parts = [
            (flags.year, fields.year),
            (flags.month, fields.month),
            (flags.day, fields.day),
        ];
        let kind = if blank {
            ErrorKind::Empty
        } else {
            ErrorKind::Invalid
        };
        if self.msg.is_off() {
            self.kind = Some(kind);
            self.msg.set_if_off(text.clone(), field);
        }
        let flagged: Vec<&'static str> = parts
            .iter()
            .filter(|&&(state, _)| state != FlagState::Valid)
            .map(|&(_, name)| name)
            .collect();
        let listed = if flagged.is_empty() {
            vec![field]
        } else {
            flagged
        };
        self.fields
            .extend(listed.into_iter().map(|name| FieldError {
                field: name,
                message: text.clone(),
            }));
    }
}

/// The lowest score of 88 `FICO-RANGE-IS-VALID` [app/cbl/COACTUPC.cbl:L848-849].
const FICO_MIN: u16 = 300;

/// The highest score of 88 `FICO-RANGE-IS-VALID`.
const FICO_MAX: u16 = 850;

/// COACTUPC 1200-EDIT-MAP-INPUTS — P-COACTUPC-006, V-COACTUPC-011 to
/// V-COACTUPC-123
///
/// Every edit in the order of the paragraph [app/cbl/COACTUPC.cbl:L1472-1669]:
/// the account fields, then the customer fields, then the state and zip
/// cross edit.
///
/// # Errors
///
/// [`EditRefusal::Error`] with the first failing edit's text and kind and
/// every flagged field, or [`EditRefusal::Halted`] for a date held at `halt`.
fn edit_map_inputs(new: &NewDetails, today: NaiveDate) -> Result<(), EditRefusal> {
    let mut edits = Edits::default();
    edit_account_fields(&mut edits, new)?;
    edit_customer_fields(&mut edits, new, today)?;
    edits.finish().map_err(EditRefusal::from)
}

/// COACTUPC 1200-EDIT-MAP-INPUTS — P-COACTUPC-006
///
/// The account fields [app/cbl/COACTUPC.cbl:L1472-1527]: status, open date,
/// credit limit, expiry date, cash credit limit, reissue date, current
/// balance, current cycle credit, current cycle debit.
///
/// # Errors
///
/// [`EditRefusal::Halted`] for a date held at `halt`.
fn edit_account_fields(edits: &mut Edits, new: &NewDetails) -> Result<(), EditRefusal> {
    edits.yes_no(FIELD_ACCOUNT_STATUS, new.active_status, FIELD_ACSTTUS);
    edits.date(FIELD_OPEN_DATE, new.open_date, DateBmsFields::OPEN_DATE)?;
    edits.signed_9v2(FIELD_CREDIT_LIMIT, &new.credit_limit, FIELD_ACRDLIM);
    edits.date(
        FIELD_EXPIRY_DATE,
        new.expiry_date,
        DateBmsFields::EXPIRY_DATE,
    )?;
    edits.signed_9v2(
        FIELD_CASH_CREDIT_LIMIT,
        &new.cash_credit_limit,
        FIELD_ACSHLIM,
    );
    edits.date(
        FIELD_REISSUE_DATE,
        new.reissue_date,
        DateBmsFields::REISSUE_DATE,
    )?;
    edits.signed_9v2(FIELD_CURRENT_BALANCE, &new.curr_bal, FIELD_ACURBAL);
    edits.signed_9v2(
        FIELD_CURRENT_CYCLE_CREDIT_LIMIT,
        &new.curr_cyc_credit,
        FIELD_ACRCYCR,
    );
    edits.signed_9v2(
        FIELD_CURRENT_CYCLE_DEBIT_LIMIT,
        &new.curr_cyc_debit,
        FIELD_ACRCYDB,
    );
    Ok(())
}

/// COACTUPC 1200-EDIT-MAP-INPUTS — P-COACTUPC-006, P-COACTUPC-007
///
/// The customer fields [app/cbl/COACTUPC.cbl:L1529-1669]: SSN, date of birth,
/// FICO score, first, middle and last name, address line 1, state, zip, city
/// (address line 3), country, both phone numbers, EFT account ID, primary
/// card holder, then the state and zip cross edit when both passed. Address
/// line 2 and the government ID have no edit.
///
/// # Errors
///
/// [`EditRefusal::Halted`] for a date held at `halt`.
fn edit_customer_fields(
    edits: &mut Edits,
    new: &NewDetails,
    today: NaiveDate,
) -> Result<(), EditRefusal> {
    edits.ssn(new);
    if edits.date(FIELD_DATE_OF_BIRTH, new.dob, DateBmsFields::DATE_OF_BIRTH)? {
        edits.date_of_birth(new.dob, today);
    }
    edits.fico_score(new.fico);
    edits.alpha_required(FIELD_FIRST_NAME, new.first_name.as_bytes(), FIELD_ACSFNAM);
    edits.alpha_optional(FIELD_MIDDLE_NAME, new.middle_name.as_bytes(), FIELD_ACSMNAM);
    edits.alpha_required(FIELD_LAST_NAME, new.last_name.as_bytes(), FIELD_ACSLNAM);
    edits.mandatory(
        FIELD_ADDRESS_LINE_1,
        new.addr_line_1.as_bytes(),
        FIELD_ACSADL1,
    );
    let state_valid = edits.state(new.state);
    let zip_valid = edits.num_required(FIELD_ZIP, new.zip.prefix(ZIP_EDIT_LEN), FIELD_ACSZIPC);
    edits.alpha_required(FIELD_CITY, new.addr_line_3.as_bytes(), FIELD_ACSCITY);
    edits.alpha_required(FIELD_COUNTRY, new.country.as_bytes(), FIELD_ACSCTRY);
    edits.phone(FIELD_PHONE_NUMBER_1, &new.phone_1, PHONE_1_FIELDS);
    edits.phone(FIELD_PHONE_NUMBER_2, &new.phone_2, PHONE_2_FIELDS);
    edits.num_required(
        FIELD_EFT_ACCOUNT_ID,
        new.eft_account_id.as_bytes(),
        FIELD_ACSEFTC,
    );
    edits.yes_no(FIELD_PRIMARY_CARD_HOLDER, new.pri_holder, FIELD_ACSPFLG);
    if state_valid && zip_valid {
        edits.state_zip(new.state, &new.zip);
    }
    Ok(())
}

// --- 9600-WRITE-PROCESSING: the update records

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-013, P-COACTUPC-014
///
/// `ACCT-UPDATE-RECORD` and `CUST-UPDATE-RECORD` as the paragraph builds them
/// from the received values [app/cbl/COACTUPC.cbl:L3956-4059]. `fetched` is
/// the customer the cross-reference names, whose ID stands in the record
/// when the received one holds a non-digit (E-74).
///
/// # Errors
///
/// The unexpected-error outcome for a value the edits that passed should
/// have proved: a missing amount, or an SSN or FICO score that is not all
/// digits.
fn update_records(
    account_id: AccountId,
    new: &NewDetails,
    fetched: &Customer,
) -> Result<ValidatedAccountChanges, DomainError> {
    let account = account_update_record(account_id, new)?;
    let (customer, customer_key) = customer_update_record(new, fetched)?;
    Ok(ValidatedAccountChanges {
        account,
        customer,
        customer_key,
    })
}

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-013
///
/// `ACCT-UPDATE-RECORD` [app/cbl/COACTUPC.cbl:L3956-4002]: the account number,
/// status and the five `-N` amounts; each date strung `YYYY-MM-DD` from its
/// received parts as entered; the received group ID, which the record places
/// where `ACCOUNT-RECORD` reads `ACCT-ADDR-ZIP`, and spaces where it reads
/// `ACCT-GROUP-ID` (see the module notes on the account update record).
///
/// # Errors
///
/// The unexpected-error outcome when an amount has no `-N` value, which the
/// passed `1250-EDIT-SIGNED-9V2` edits exclude.
fn account_update_record(account_id: AccountId, new: &NewDetails) -> Result<Account, DomainError> {
    let (
        Some(acct_curr_bal),
        Some(acct_credit_limit),
        Some(acct_cash_credit_limit),
        Some(acct_curr_cyc_credit),
        Some(acct_curr_cyc_debit),
    ) = (
        new.curr_bal.value,
        new.credit_limit.value,
        new.cash_credit_limit.value,
        new.curr_cyc_credit.value,
        new.curr_cyc_debit.value,
    )
    else {
        return Err(halt());
    };
    Ok(Account {
        acct_id: account_id,
        acct_active_status: stored(&new.active_status),
        acct_curr_bal,
        acct_credit_limit,
        acct_cash_credit_limit,
        acct_open_date: dashed_date(new.open_date),
        acct_expiraion_date: dashed_date(new.expiry_date),
        acct_reissue_date: dashed_date(new.reissue_date),
        acct_curr_cyc_credit,
        acct_curr_cyc_debit,
        acct_addr_zip: stored(&new.group_id),
        acct_group_id: AccountGroupId::from_alnum(Alnum::spaces()),
    })
}

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-014
///
/// `CUST-UPDATE-RECORD` [app/cbl/COACTUPC.cbl:L4007-4059]: every received
/// customer field; each phone number strung `(AAA)BBB-CCCC` into the
/// initialized `X(15)`, which keeps its last two spaces; the SSN from its
/// three parts; the date of birth strung `YYYY-MM-DD`; and the customer ID
/// received, with whether it holds nine digits.
///
/// # Errors
///
/// The unexpected-error outcome when the SSN or FICO score is not all
/// digits, which the passed `1245-EDIT-NUM-REQD` edits exclude.
fn customer_update_record(
    new: &NewDetails,
    fetched: &Customer,
) -> Result<(Customer, CustomerKey), DomainError> {
    let (cust_id, customer_key) = if new.cust_id.is_numeric() {
        let id = CustomerId::from_str(&new.cust_id.as_text()).map_err(|_| halt())?;
        (id, CustomerKey::Received)
    } else {
        (fetched.cust_id, CustomerKey::Unpinned)
    };
    let ssn = [
        new.ssn_1.as_bytes().as_slice(),
        new.ssn_2.as_bytes().as_slice(),
        new.ssn_3.as_bytes().as_slice(),
    ]
    .concat();
    let cust_ssn =
        CustSsn::from_str(&Alnum::<9>::move_from_bytes(&ssn).as_text()).map_err(|_| halt())?;
    let cust_fico_credit_score = FicoScore::from_str(&new.fico.as_text()).map_err(|_| halt())?;
    let customer = Customer {
        cust_id,
        cust_first_name: stored(&new.first_name),
        cust_middle_name: stored(&new.middle_name),
        cust_last_name: stored(&new.last_name),
        cust_addr_line_1: stored(&new.addr_line_1),
        cust_addr_line_2: stored(&new.addr_line_2),
        cust_addr_line_3: stored(&new.addr_line_3),
        cust_addr_state_cd: stored(&new.state),
        cust_addr_country_cd: stored(&new.country),
        cust_addr_zip: stored(&new.zip),
        cust_phone_num_1: phone_number(&new.phone_1),
        cust_phone_num_2: phone_number(&new.phone_2),
        cust_ssn,
        cust_govt_issued_id: stored(&new.govt_issued_id),
        cust_dob_yyyy_mm_dd: dashed_date(new.dob),
        cust_eft_account_id: stored(&new.eft_account_id),
        cust_pri_card_holder_ind: stored(&new.pri_holder),
        cust_fico_credit_score,
    };
    Ok((customer, customer_key))
}

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-013, P-COACTUPC-014
///
/// `STRING year '-' month '-' day DELIMITED BY SIZE` into an `X(10)` date,
/// in stored form.
fn dashed_date(parts: DateParts) -> Alnum<10> {
    let text = [
        parts.ccyy.as_slice(),
        b"-",
        parts.mm.as_slice(),
        b"-",
        parts.dd.as_slice(),
    ]
    .concat();
    Alnum::move_from_bytes(&stored_vec(&text))
}

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-014
///
/// `STRING '(' A ')' B '-' C DELIMITED BY SIZE` into the `X(15)` phone number
/// [app/cbl/COACTUPC.cbl:L4027-4041], in stored form.
fn phone_number(phone: &PhoneParts) -> Alnum<15> {
    let text = [
        b"(".as_slice(),
        phone.area.as_bytes(),
        b")",
        phone.prefix.as_bytes(),
        b"-",
        phone.line.as_bytes(),
    ]
    .concat();
    Alnum::move_from_bytes(&stored_vec(&text))
}

// --- The version token: 9700-CHECK-CHANGE-IN-REC

/// COACTUPC 9700-CHECK-CHANGE-IN-REC, 9500-STORE-FETCHED-DATA —
/// P-COACTUPC-012, P-COACTUPC-048
///
/// The [`VersionToken`] of `account` and `customer` under `key`: the keyed
/// digest of exactly the fields the paragraph compares, in its order and with
/// its normalization [app/cbl/COACTUPC.cbl:L4115-4190], so under one key two
/// snapshots give the same token exactly when the paragraph finds no change
/// between them. The
/// account status, the five amounts, each date's `(1:4)`, `(6:2)` and
/// `(9:2)`, the group ID lower-cased; the names, address lines, state and
/// country upper-cased; the zip, both phone numbers and the SSN exactly; the
/// government ID upper-cased; the date of birth's three parts; the EFT
/// account ID, the primary holder indicator and the FICO score exactly. The
/// account and customer IDs are the keys the records were read by and are
/// not compared. `account_view` returns this token with `GET /accounts/{id}`;
/// the key keeps it from disclosing the digested fields.
///
/// # Errors
///
/// [`ErrorKind::Unexpected`] with [`UNEXPECTED_ERROR`] at `ACCTSID` when the
/// digest fails; as [`version_token`] documents, no record's fields reach the
/// input size that makes it fail.
pub fn record_version(
    key: &VersionKey,
    account: &Account,
    customer: &Customer,
) -> Result<VersionToken, DomainError> {
    let mut parts: Vec<Vec<u8>> = vec![account.acct_active_status.as_bytes().to_vec()];
    parts.extend(
        [
            account.acct_curr_bal,
            account.acct_credit_limit,
            account.acct_cash_credit_limit,
            account.acct_curr_cyc_credit,
            account.acct_curr_cyc_debit,
        ]
        .iter()
        .map(|amount| amount.cents().to_string().into_bytes()),
    );
    for date in [
        &account.acct_open_date,
        &account.acct_expiraion_date,
        &account.acct_reissue_date,
    ] {
        parts.extend(date_parts(date));
    }
    parts.push(lower_cased(account.acct_group_id.as_bytes()));
    for name in [
        &customer.cust_first_name,
        &customer.cust_middle_name,
        &customer.cust_last_name,
    ] {
        parts.push(upper_cased(name.as_bytes()));
    }
    for line in [
        &customer.cust_addr_line_1,
        &customer.cust_addr_line_2,
        &customer.cust_addr_line_3,
    ] {
        parts.push(upper_cased(line.as_bytes()));
    }
    parts.push(upper_cased(customer.cust_addr_state_cd.as_bytes()));
    parts.push(upper_cased(customer.cust_addr_country_cd.as_bytes()));
    parts.push(customer.cust_addr_zip.as_bytes().to_vec());
    parts.push(customer.cust_phone_num_1.as_bytes().to_vec());
    parts.push(customer.cust_phone_num_2.as_bytes().to_vec());
    parts.push(customer.cust_ssn.to_string().into_bytes());
    parts.push(upper_cased(customer.cust_govt_issued_id.as_bytes()));
    parts.extend(date_parts(&customer.cust_dob_yyyy_mm_dd));
    parts.push(customer.cust_eft_account_id.as_bytes().to_vec());
    parts.push(customer.cust_pri_card_holder_ind.as_bytes().to_vec());
    parts.push(customer.cust_fico_credit_score.to_string().into_bytes());
    let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    version_token(key, &slices)
        .map_err(|_| DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_ACCTSID))
}

/// COACTUPC 9700-CHECK-CHANGE-IN-REC — P-COACTUPC-012
///
/// The `(1:4)`, `(6:2)` and `(9:2)` of a stored `X(10)` date, the three
/// operands `9700-CHECK-CHANGE-IN-REC` compares.
fn date_parts(date: &Alnum<10>) -> [Vec<u8>; 3] {
    [
        part(date, 1, 4).to_vec(),
        part(date, 6, 2).to_vec(),
        part(date, 9, 2).to_vec(),
    ]
}

// --- The request: validate, apply, update

/// COACTUPC 1100-RECEIVE-MAP, 1205-COMPARE-OLD-NEW, 1200-EDIT-MAP-INPUTS,
/// 9600-WRITE-PROCESSING — P-COACTUPC-003 to P-COACTUPC-008, P-COACTUPC-018 to
/// P-COACTUPC-033, P-COACTUPC-047, P-COACTUPC-049, V-COACTUPC-002,
/// V-COACTUPC-003, V-COACTUPC-010 to V-COACTUPC-123
///
/// The ENTER turn against `snapshot`, the records [`read_snapshot`] fetched:
/// the key of `input` is edited as [`edit_account_id`] edits it; every field
/// is received; when the received values equal the fetched ones nothing more
/// happens; otherwise every edit runs, and when all pass the result holds the
/// records `9600-WRITE-PROCESSING` builds. `today` is the processing date the
/// birth-date check compares with (`FUNCTION CURRENT-DATE`).
///
/// # Errors
///
/// - [`EditRefusal::Error`] with the key edit's error, as
///   [`edit_account_id`] returns it.
/// - [`EditRefusal::Error`] with the first failing field edit in
///   `1200-EDIT-MAP-INPUTS` order, with its kind ([`ErrorKind::Empty`] for a
///   field not supplied, [`ErrorKind::Invalid`] otherwise) and every flagged
///   field.
/// - [`EditRefusal::Error`] with [`ErrorKind::Unexpected`] and
///   [`UNEXPECTED_ERROR`] for a received value the edits that passed should
///   have proved.
/// - [`EditRefusal::Halted`] with its escalation ID (E-26, E-40 or E-47) for
///   an input class held at `halt` (see the module notes); it converts into
///   [`ErrorKind::Unexpected`] with [`UNEXPECTED_ERROR`] and no fields.
pub fn validate(
    snapshot: &AccountSnapshot,
    input: &AccountUpdateInput,
    today: NaiveDate,
) -> Result<AccountEdit, EditRefusal> {
    let account_id = edit_account_id(&input.account_id)?;
    edit_changes(account_id, snapshot, input, today)
}

/// COACTUPC 1100-RECEIVE-MAP, 1205-COMPARE-OLD-NEW, 1200-EDIT-MAP-INPUTS,
/// 9600-WRITE-PROCESSING — P-COACTUPC-005 to P-COACTUPC-008, P-COACTUPC-018
/// to P-COACTUPC-033, P-COACTUPC-047, P-COACTUPC-049, V-COACTUPC-010 to
/// V-COACTUPC-123
///
/// The ENTER turn after the key edit, against `snapshot`, the records
/// [`read_snapshot`] fetched, with `account_id`, the key of `input`
/// [`edit_account_id`] passed: every field is received; when the received
/// values equal the fetched ones nothing more happens; otherwise every edit
/// runs, and when all pass the result holds the records
/// `9600-WRITE-PROCESSING` builds for `account_id`. `today` is the processing
/// date the birth-date check compares with (`FUNCTION CURRENT-DATE`).
///
/// # Errors
///
/// - [`EditRefusal::Error`] with the first failing field edit in
///   `1200-EDIT-MAP-INPUTS` order, with its kind ([`ErrorKind::Empty`] for a
///   field not supplied, [`ErrorKind::Invalid`] otherwise) and every flagged
///   field.
/// - [`EditRefusal::Error`] with [`ErrorKind::Unexpected`] and
///   [`UNEXPECTED_ERROR`] for a received value the edits that passed should
///   have proved ([`update_records`]).
/// - [`EditRefusal::Halted`] with its escalation ID for an input class held
///   at `halt` (see the module notes).
fn edit_changes(
    account_id: AccountId,
    snapshot: &AccountSnapshot,
    input: &AccountUpdateInput,
    today: NaiveDate,
) -> Result<AccountEdit, EditRefusal> {
    let new = NewDetails::receive(input)?;
    if no_changes_found(&new, snapshot) {
        return Ok(AccountEdit::NoChange);
    }
    edit_map_inputs(&new, today)?;
    update_records(account_id, &new, &snapshot.customer)
        .map(|changes| AccountEdit::Changed(Box::new(changes)))
        .map_err(EditRefusal::from)
}

/// COACTUPC 9600-WRITE-PROCESSING, 9700-CHECK-CHANGE-IN-REC,
/// 3250-SETUP-INFOMSG — P-COACTUPC-009 to P-COACTUPC-015, P-COACTUPC-048,
/// V-COACTUPC-124 to V-COACTUPC-128
///
/// The PF5 write [app/cbl/COACTUPC.cbl:L3888-4104]: `READ … UPDATE` of the
/// account `account_id` and of the customer `customer_id`, the
/// cross-reference's; the change check of the locked records against
/// `expected`, their token computed under `key`, the key `expected` was issued
/// under; then the account `REWRITE` and the customer `REWRITE` of
/// `changes`, in that order. Each write is one legacy write: the account
/// rewrite stands when the customer rewrite fails, because every `CardDemo`
/// file is `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96] and `SYNCPOINT
/// ROLLBACK` does not undo it (P-COACTUPC-014); the adapter runs each write
/// under its own savepoint and commits whatever the outcome.
///
/// A `REWRITE` that changes the key it was locked by is rejected by VSAM
/// (E-74), so a record whose ID differs from its lock key is not written and
/// answers as that rewrite's failure; a received customer ID holding a
/// non-digit writes nothing (E-74, `halt`).
///
/// # Errors
///
/// All at `ACCTSID`:
///
/// - [`ErrorKind::Unexpected`] with [`COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE`]
///   or [`COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE`] when a locked read
///   fails for any reason; nothing is written.
/// - [`ErrorKind::Unexpected`] with [`UNEXPECTED_ERROR`] when the locked
///   records' token cannot be computed ([`record_version`]); nothing is
///   written.
/// - [`ErrorKind::Conflict`] with [`RECORD_CHANGED_BY_SOME_ONE_ELSE`] when the
///   locked records' token differs from `expected`; nothing is written.
/// - [`ErrorKind::Unexpected`] with [`UPDATE_OF_RECORD_FAILED`] when the
///   account rewrite fails (nothing is written), when the customer rewrite
///   fails (the account rewrite stands), or for a received customer ID
///   holding a non-digit (nothing is written).
pub async fn apply<S>(
    store: &mut S,
    key: &VersionKey,
    account_id: AccountId,
    customer_id: CustomerId,
    expected: &VersionToken,
    changes: ValidatedAccountChanges,
) -> Result<AccountUpdated, DomainError>
where
    S: AccountReader + AccountWriter + CustomerReader + CustomerWriter,
{
    let locked_account = store
        .read_account_for_update(account_id)
        .await
        .map_err(|_| write_failure(COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE))?;
    let locked_customer = store
        .read_customer_for_update(customer_id)
        .await
        .map_err(|_| write_failure(COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE))?;
    if record_version(key, &locked_account, &locked_customer)? != *expected {
        return Err(
            DomainError::conflict(RECORD_CHANGED_BY_SOME_ONE_ELSE).with_field(FIELD_ACCTSID)
        );
    }
    let ValidatedAccountChanges {
        account,
        customer,
        customer_key,
    } = changes;
    if customer_key == CustomerKey::Unpinned || account.acct_id != account_id {
        return Err(write_failure(UPDATE_OF_RECORD_FAILED));
    }
    store
        .rewrite_account(account.clone())
        .await
        .map_err(|_| write_failure(UPDATE_OF_RECORD_FAILED))?;
    if customer.cust_id != customer_id {
        return Err(write_failure(UPDATE_OF_RECORD_FAILED));
    }
    store
        .rewrite_customer(customer.clone())
        .await
        .map_err(|_| write_failure(UPDATE_OF_RECORD_FAILED))?;
    Ok(AccountUpdated {
        account,
        customer,
        message: CHANGES_COMMITTED_TO_DATABASE,
    })
}

/// COACTUPC 9600-WRITE-PROCESSING — P-COACTUPC-010, P-COACTUPC-011,
/// P-COACTUPC-013, P-COACTUPC-014, V-COACTUPC-124, V-COACTUPC-125,
/// V-COACTUPC-127, V-COACTUPC-128
///
/// A failed lock or rewrite of `9600-WRITE-PROCESSING`: `message` at
/// `ACCTSID`.
fn write_failure(message: &'static str) -> DomainError {
    DomainError::unexpected(message).with_field(FIELD_ACCTSID)
}

/// COACTUPC 0000-MAIN, 2000-DECIDE-ACTION — P-COACTUPC-004, P-COACTUPC-009,
/// P-COACTUPC-015, V-COACTUPC-002 to V-COACTUPC-128
///
/// One `PUT /accounts/{id}` from key to write (E-56): [`edit_account_id`],
/// [`read_snapshot`], the receiving, comparison and field edits of
/// [`validate`] for the key already edited and, for a change, [`apply`] with
/// `key`, the cross-reference's customer and the token of `input`. No change
/// succeeds with the fetched records and
/// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`] (V-COACTUPC-010).
///
/// # Errors
///
/// The first error of those steps, as each documents it; a refusal of the
/// edits as the [`DomainError`] its [`EditRefusal`] converts into.
pub async fn update<S>(
    store: &mut S,
    key: &VersionKey,
    input: &AccountUpdateInput,
    today: NaiveDate,
) -> Result<AccountUpdated, DomainError>
where
    S: XrefReader + AccountReader + AccountWriter + CustomerReader + CustomerWriter,
{
    let account_id = edit_account_id(&input.account_id)?;
    let snapshot = read_snapshot(store, account_id).await?;
    match edit_changes(account_id, &snapshot, input, today).map_err(DomainError::from)? {
        AccountEdit::NoChange => Ok(AccountUpdated {
            account: snapshot.account,
            customer: snapshot.customer,
            message: NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
        }),
        AccountEdit::Changed(changes) => {
            let customer_id = snapshot.customer.cust_id;
            apply(
                store,
                key,
                account_id,
                customer_id,
                &input.version,
                *changes,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use chrono::NaiveDate;

    use super::{
        AccountEdit, AccountSnapshot, AccountUpdateInput, CustomerKey, EditRefusal, NewDetails,
        VALID_EASY_RECOG_AREA_CODE, VALID_GENERAL_PURP_CODE, VALID_PHONE_AREA_CODE,
        VALID_US_STATE_CODE, VALID_US_STATE_ZIP_CD2_COMBO, ValidatedAccountChanges, apply,
        edit_account_id, read_snapshot, receive_amount, record_version, update, update_records,
        validate,
    };
    use crate::entities::{Account, CardXref, Customer};
    use crate::error::{DomainError, ErrorKind, FieldError};
    use crate::messages;
    use crate::messages::{
        AREA_CODE_CANNOT_BE_ZERO, AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER, AREA_CODE_MUST_BE_SUPPLIED,
        CAN_HAVE_ALPHABETS_ONLY, CANNOT_BE_IN_THE_FUTURE, CANNOT_HAVE_30_DAYS_IN_THIS_MONTH,
        CANNOT_HAVE_31_DAYS_IN_THIS_MONTH, CENTURY_NOT_VALID,
        DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, DAY_MUST_BE_SUPPLIED, FIELD_ACCOUNT_STATUS,
        FIELD_ADDRESS_LINE_1, FIELD_CASH_CREDIT_LIMIT, FIELD_CITY, FIELD_COUNTRY,
        FIELD_CREDIT_LIMIT, FIELD_CURRENT_BALANCE, FIELD_CURRENT_CYCLE_CREDIT_LIMIT,
        FIELD_CURRENT_CYCLE_DEBIT_LIMIT, FIELD_DATE_OF_BIRTH, FIELD_EFT_ACCOUNT_ID,
        FIELD_EXPIRY_DATE, FIELD_FICO_SCORE, FIELD_FIRST_NAME, FIELD_LAST_NAME, FIELD_MIDDLE_NAME,
        FIELD_OPEN_DATE, FIELD_PHONE_NUMBER_1, FIELD_PHONE_NUMBER_2, FIELD_PRIMARY_CARD_HOLDER,
        FIELD_REISSUE_DATE, FIELD_SSN_4TH_AND_5TH_CHARS, FIELD_SSN_FIRST_3_CHARS,
        FIELD_SSN_LAST_4_CHARS, FIELD_STATE, FIELD_ZIP, INVALID_ZIP_CODE_FOR_STATE,
        IS_NOT_A_VALID_STATE_CODE, IS_NOT_VALID, LINE_NUMBER_CODE_CANNOT_BE_ZERO,
        LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER, LINE_NUMBER_CODE_MUST_BE_SUPPLIED,
        MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12, MONTH_MUST_BE_SUPPLIED, MUST_BE_4_DIGIT_NUMBER,
        MUST_BE_ALL_NUMERIC, MUST_BE_SUPPLIED, MUST_BE_Y_OR_N, MUST_NOT_BE_ZERO,
        NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS, NOT_VALID_NORTH_AMERICA_AREA_CODE,
        PREFIX_CODE_CANNOT_BE_ZERO, PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER,
        PREFIX_CODE_MUST_BE_SUPPLIED, SHOULD_BE_BETWEEN_300_AND_850,
        SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999, YEAR_MUST_BE_SUPPLIED, date_edit_message,
        field_edit_message, xref_not_found_message,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustSsn, CustomerId, FicoScore, Money,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on, version_key};

    /// The copybook the five lookup tables are transcribed from.
    const CSLKPCDY: &str = include_str!("../../../../app/cpy/CSLKPCDY.cpy");

    /// The card of account 00000000001 and customer 000000001
    /// [app/data/ASCII/cardxref.txt:L49].
    const CARD: &str = "9680294154603697";

    fn account_id() -> AccountId {
        AccountId::new(1).expect("an account number")
    }

    fn customer_id() -> CustomerId {
        CustomerId::new(1).expect("a customer number")
    }

    fn cents(value: i64) -> Money<10> {
        Money::from_cents(value).expect("an amount of at most 10 integer digits")
    }

    /// The processing date the birth-date check compares with.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 15).expect("a calendar date")
    }

    /// Account 00000000001 as `acctdata.txt` L1 decodes (P-CVACT01Y-015).
    fn fixture_account() -> Account {
        Account {
            acct_id: account_id(),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(19_400),
            acct_credit_limit: cents(202_000),
            acct_cash_credit_limit: cents(102_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    /// Customer 000000001 as `custdata.txt` L1 decodes (P-CVCUS01Y-021).
    fn fixture_customer() -> Customer {
        Customer {
            cust_id: customer_id(),
            cust_first_name: Alnum::move_from("Immanuel"),
            cust_middle_name: Alnum::move_from("Madeline"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    fn fixture_xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(CARD),
            xref_cust_id: customer_id(),
            xref_acct_id: account_id(),
        }
    }

    fn fixture_store() -> FakeStore {
        FakeStore::default()
            .with_xref(fixture_xref())
            .with_account(fixture_account())
            .with_customer(fixture_customer())
    }

    fn snapshot() -> AccountSnapshot {
        AccountSnapshot {
            account: fixture_account(),
            customer: fixture_customer(),
        }
    }

    /// The `GET /accounts/00000000001` values sent back unchanged.
    fn fetched_input() -> AccountUpdateInput {
        let text = |value: &str| value.to_owned();
        AccountUpdateInput {
            account_id: text("00000000001"),
            active_status: text("Y"),
            open_year: text("2014"),
            open_month: text("11"),
            open_day: text("20"),
            credit_limit: text("2020.00"),
            expiry_year: text("2025"),
            expiry_month: text("05"),
            expiry_day: text("20"),
            cash_credit_limit: text("1020.00"),
            reissue_year: text("2025"),
            reissue_month: text("05"),
            reissue_day: text("20"),
            current_balance: text("194.00"),
            current_cycle_credit: text("0.00"),
            group_id: text(""),
            current_cycle_debit: text("0.00"),
            customer_id: text("000000001"),
            ssn_part_1: text("020"),
            ssn_part_2: text("97"),
            ssn_part_3: text("3888"),
            dob_year: text("1961"),
            dob_month: text("06"),
            dob_day: text("08"),
            fico_score: text("274"),
            first_name: text("Immanuel"),
            middle_name: text("Madeline"),
            last_name: text("Kessler"),
            address_line_1: text("618 Deshaun Route"),
            state: text("NC"),
            address_line_2: text("Apt. 802"),
            zip: text("12546"),
            city: text("Altenwerthshire"),
            country: text("USA"),
            phone_1_area: text("908"),
            phone_1_prefix: text("119"),
            phone_1_line: text("8310"),
            govt_issued_id: text("00000000000049368437"),
            phone_2_area: text("373"),
            phone_2_prefix: text("693"),
            phone_2_line: text("8684"),
            eft_account_id: text("0053581756"),
            primary_card_holder: text("Y"),
            version: token(),
        }
    }

    /// The fetched values with the three that fail their edits on the
    /// fixture corrected (FICO 274, phone 2 area 373, NC with zip 12) and
    /// the status rejected, so the status's [`MUST_BE_Y_OR_N`] text is the
    /// message and every later edit's text shows only in `fields`.
    fn clean_input() -> AccountUpdateInput {
        AccountUpdateInput {
            active_status: "X".to_owned(),
            fico_score: "700".to_owned(),
            phone_2_area: "908".to_owned(),
            zip: "27546".to_owned(),
            ..fetched_input()
        }
    }

    /// The `fields` entries `entries` names, in order.
    fn entries(entries: &[(&'static str, &str)]) -> Vec<FieldError> {
        entries
            .iter()
            .map(|&(field, message)| FieldError {
                field,
                message: message.to_owned(),
            })
            .collect()
    }

    /// The rejection [`validate`] gives `input` against the fixture, as the
    /// error it is answered with.
    fn rejection(input: &AccountUpdateInput) -> DomainError {
        match validate(&snapshot(), input, today()) {
            Err(refusal) => DomainError::from(refusal),
            Ok(edit) => panic!("expected a rejection, got {edit:?}"),
        }
    }

    /// Asserts that `result` is the rejection `kind`, `message` at `field`,
    /// as the error it is answered with.
    fn assert_rejected<T: Debug>(
        result: Result<T, impl Into<DomainError>>,
        (kind, message, field): (ErrorKind, &str, Option<&str>),
        case: &str,
    ) {
        match result.map_err(Into::<DomainError>::into) {
            Err(error) => {
                assert_eq!(error.kind, kind, "{case}");
                assert_eq!(error.code, kind.code(), "{case}");
                assert_eq!(error.message, message, "{case}");
                assert_eq!(error.first_field(), field, "{case}");
            }
            Ok(value) => panic!("{case}: expected a rejection, got {value:?}"),
        }
    }

    /// The records [`update_records`] builds from `input`.
    fn changes_for(input: &AccountUpdateInput) -> ValidatedAccountChanges {
        let new = NewDetails::receive(input).expect("amounts NUMVAL-C pins");
        update_records(account_id(), &new, &fixture_customer()).expect("the update records")
    }

    /// The 88-level value lists of `CSLKPCDY`, each with its name, in
    /// copybook order: every quoted value from the `88` line to the line
    /// that ends with the clause's period, comment lines skipped.
    fn copybook_tables() -> Vec<(String, Vec<String>)> {
        let mut tables: Vec<(String, Vec<String>)> = Vec::new();
        let mut open = false;
        for line in CSLKPCDY.lines() {
            let code = line.trim();
            if code.starts_with('*') {
                continue;
            }
            let words: Vec<&str> = code.split_whitespace().collect();
            if let Some(at) = words.iter().position(|&word| word == "88") {
                let name = words.get(at + 1).copied().unwrap_or_default();
                tables.push((name.to_owned(), Vec::new()));
                open = true;
            }
            if open {
                if let Some((_, values)) = tables.last_mut() {
                    values.extend(code.split('\'').skip(1).step_by(2).map(str::to_owned));
                }
                open = !code.ends_with('.');
            }
        }
        tables
    }

    #[test]
    fn the_lookup_tables_equal_the_copybook_lists_value_for_value() {
        let transcribed: [(&str, &[&str]); 5] = [
            ("VALID-PHONE-AREA-CODE", &VALID_PHONE_AREA_CODE),
            ("VALID-GENERAL-PURP-CODE", &VALID_GENERAL_PURP_CODE),
            ("VALID-EASY-RECOG-AREA-CODE", &VALID_EASY_RECOG_AREA_CODE),
            ("VALID-US-STATE-CODE", &VALID_US_STATE_CODE),
            (
                "VALID-US-STATE-ZIP-CD2-COMBO",
                &VALID_US_STATE_ZIP_CD2_COMBO,
            ),
        ];
        let parsed = copybook_tables();
        assert_eq!(parsed.len(), transcribed.len());
        for ((name, values), (expected_name, expected)) in parsed.iter().zip(transcribed) {
            assert_eq!(name, expected_name);
            assert_eq!(values, expected, "{name}");
        }
        assert_eq!(
            [490, 410, 80, 56, 240],
            transcribed.map(|(_, values)| values.len())
        );
    }

    /// The `-N` value 1100-RECEIVE-MAP keeps for `text` in cents, `None` when
    /// `TEST-NUMVAL-C` rejects it, or the halt refusal.
    fn received_cents(text: &str) -> Result<Option<i64>, EditRefusal> {
        receive_amount(text).map(|amount| amount.value.map(Money::cents))
    }

    #[test]
    fn numval_c_converts_the_arguments_test_numval_c_accepts() {
        // P-COACTUPC-047, P-COACTUPC-049, V-COACTUPC-025, E-40.
        let cases: [(&str, Option<i64>); 30] = [
            ("2020.00", Some(202_000)),
            ("2020", Some(202_000)),
            ("  12.5   ", Some(1_250)),
            ("-12.34", Some(-1_234)),
            ("- 12", Some(-1_200)),
            ("+12.34", Some(1_234)),
            ("12.34-", Some(-1_234)),
            ("12 -", Some(-1_200)),
            ("12.34CR", Some(-1_234)),
            ("12.34 DB", Some(-1_234)),
            ("$12.00", Some(1_200)),
            ("- $ 5", Some(-500)),
            (".5", Some(50)),
            ("5.", Some(500)),
            ("10.006", Some(1_001)),
            ("10.004", Some(1_000)),
            ("-10.0051", Some(-1_001)),
            ("9999999999.99", Some(999_999_999_999)),
            ("-0.00", Some(0)),
            ("12A4", None),
            ("1.2.3", None),
            ("--1", None),
            ("+1-", None),
            ("1 2", None),
            (".", None),
            ("-", None),
            ("$", None),
            ("CR12", None),
            ("12$", None),
            ("1e5", None),
        ];
        for (text, expected) in cases {
            assert_eq!(received_cents(text), Ok(expected), "{text:?}");
        }
        assert_eq!(received_cents(""), Ok(None));
        assert_eq!(received_cents("*"), Ok(None));
    }

    #[test]
    fn numval_c_inputs_the_audit_does_not_pin_halt() {
        // E-40: midpoints and integer overflow; readings the audit does not
        // record: grouping commas, lower-case cr and db.
        for text in [
            "10.005",
            "10.00500",
            "-0.005",
            "10000000000",
            "9999999999.995",
            "1,000.00",
            "12cr",
            "12.00db",
        ] {
            assert_eq!(
                received_cents(text).map_err(|refusal| refusal.escalation_id()),
                Err(Some("E-40")),
                "{text:?}"
            );
            assert_rejected(
                received_cents(text),
                (ErrorKind::Unexpected, messages::UNEXPECTED_ERROR, None),
                text,
            );
        }
    }

    #[test]
    fn the_account_key_is_edited_before_any_read() {
        // P-COACTUPC-003, V-COACTUPC-002, V-COACTUPC-003.
        let blank = (
            ErrorKind::Empty,
            messages::NO_INPUT_RECEIVED,
            Some("ACCTSID"),
        );
        let invalid = (
            ErrorKind::Invalid,
            messages::ACCOUNT_NUMBER_IF_SUPPLIED_MUST_BE_11_DIGIT_NON_ZERO,
            Some("ACCTSID"),
        );
        for (text, expected) in [
            ("", blank),
            ("   ", blank),
            ("*", blank),
            ("0000000000A", invalid),
            ("00000000000", invalid),
            ("123", invalid),
        ] {
            assert_rejected(edit_account_id(text), expected, text);
        }
        assert_eq!(edit_account_id("00000000001"), Ok(account_id()));
    }

    #[test]
    fn the_fetch_reads_the_cross_reference_account_and_customer() {
        // P-COACTUPC-043 to P-COACTUPC-045.
        let mut store = fixture_store();
        let snapshot = block_on(read_snapshot(&mut store, account_id())).expect("the snapshot");
        assert_eq!(snapshot, self::snapshot());
        assert_eq!(
            store.calls,
            ["read_xref_by_account", "read_account", "read_customer"]
        );
    }

    #[test]
    fn a_missing_cross_reference_ends_the_fetch() {
        // V-COACTUPC-004, V-COACTUPC-005, E-44.
        let missing = AccountId::new(99_999_999_999).expect("an account number");
        let mut store = fixture_store();
        assert_rejected(
            block_on(read_snapshot(&mut store, missing)),
            (
                ErrorKind::NotFound,
                &xref_not_found_message("99999999999", 13, 80),
                Some("ACCTSID"),
            ),
            "V-COACTUPC-004",
        );
        assert_eq!(store.calls, ["read_xref_by_account"]);
        let mut store = fixture_store();
        store.fail_next("read_xref_by_account", PortError::Contention);
        assert_rejected(
            block_on(read_snapshot(&mut store, account_id())),
            (
                ErrorKind::Unexpected,
                &messages::file_error_message("READ", "CXACAIX ", 17, 0),
                Some("ACCTSID"),
            ),
            "V-COACTUPC-005",
        );
        assert_eq!(store.calls, ["read_xref_by_account"]);
    }

    #[test]
    fn the_last_failing_read_decides_the_fetch_outcome() {
        // V-COACTUPC-006 to V-COACTUPC-009, E-45.
        let account_text = messages::account_master_not_found_message("00000000001", 13, 80);
        let customer_text = messages::customer_master_not_found_message("000000001", 13, 80);
        let account_error = messages::file_error_message("READ", "ACCTDAT ", 17, 0);
        let customer_error = messages::file_error_message("READ", "CUSTDAT ", 17, 0);
        let unavailable = || PortError::Unavailable("database".to_owned());
        let cases = [
            (
                Some(PortError::NotFound),
                None,
                ErrorKind::NotFound,
                &account_text,
            ),
            (
                Some(PortError::NotFound),
                Some(PortError::NotFound),
                ErrorKind::NotFound,
                &account_text,
            ),
            (
                Some(unavailable()),
                Some(PortError::NotFound),
                ErrorKind::Unexpected,
                &account_error,
            ),
            (
                None,
                Some(PortError::NotFound),
                ErrorKind::NotFound,
                &customer_text,
            ),
            (
                Some(PortError::NotFound),
                Some(unavailable()),
                ErrorKind::Unexpected,
                &customer_error,
            ),
            (
                Some(unavailable()),
                Some(unavailable()),
                ErrorKind::Unexpected,
                &customer_error,
            ),
            (
                None,
                Some(unavailable()),
                ErrorKind::Unexpected,
                &customer_error,
            ),
        ];
        for (index, (account, customer, kind, message)) in cases.into_iter().enumerate() {
            let mut store = fixture_store();
            if let Some(error) = account {
                store.fail_next("read_account", error);
            }
            if let Some(error) = customer {
                store.fail_next("read_customer", error);
            }
            let case = format!("case {index}");
            assert_rejected(
                block_on(read_snapshot(&mut store, account_id())),
                (kind, message, Some("ACCTSID")),
                &case,
            );
            assert_eq!(
                store.calls,
                ["read_xref_by_account", "read_account", "read_customer"],
                "{case}"
            );
        }
    }

    #[test]
    fn repeating_the_fetched_values_changes_nothing() {
        // P-COACTUPC-005, V-COACTUPC-010: the status, names and other
        // trimmed fields are compared upper-cased, and a blank group ID
        // equals the stored blank.
        let same_upper_cased = AccountUpdateInput {
            active_status: "y".to_owned(),
            first_name: " immanuel".to_owned(),
            state: "nc".to_owned(),
            ..fetched_input()
        };
        for input in [fetched_input(), same_upper_cased] {
            assert_eq!(
                validate(&snapshot(), &input, today()),
                Ok(AccountEdit::NoChange)
            );
        }
    }

    #[test]
    fn a_changed_exact_field_is_a_change() {
        // P-COACTUPC-005: phone parts, SSN, EFT ID and FICO score are
        // compared exactly, and an amount by its value.
        let cases: [fn(&mut AccountUpdateInput); 6] = [
            |input| input.phone_1_line = "8311".to_owned(),
            |input| input.ssn_part_3 = "3889".to_owned(),
            |input| input.eft_account_id = "0053581757".to_owned(),
            |input| input.fico_score = "275".to_owned(),
            |input| input.current_balance = "194.01".to_owned(),
            |input| input.group_id = "G1".to_owned(),
        ];
        for (index, change) in cases.into_iter().enumerate() {
            let mut input = fetched_input();
            change(&mut input);
            assert_ne!(
                validate(&snapshot(), &input, today()),
                Ok(AccountEdit::NoChange),
                "case {index}"
            );
        }
        let same_value = AccountUpdateInput {
            current_balance: "$194".to_owned(),
            ..fetched_input()
        };
        assert_eq!(
            validate(&snapshot(), &same_value, today()),
            Ok(AccountEdit::NoChange)
        );
    }

    #[test]
    fn the_status_edit_supplies_the_first_message() {
        // V-COACTUPC-011, V-COACTUPC-012: the status is edited first.
        let supplied = (ErrorKind::Empty, MUST_BE_SUPPLIED);
        let y_or_n = (ErrorKind::Invalid, MUST_BE_Y_OR_N);
        for (status, (kind, suffix)) in [
            ("", supplied),
            (" ", supplied),
            ("0", supplied),
            ("*", supplied),
            ("X", y_or_n),
            ("n", y_or_n),
        ] {
            let input = AccountUpdateInput {
                active_status: status.to_owned(),
                ..clean_input()
            };
            assert_rejected(
                validate(&snapshot(), &input, today()),
                (
                    kind,
                    &field_edit_message(FIELD_ACCOUNT_STATUS, suffix),
                    Some("ACSTTUS"),
                ),
                status,
            );
        }
    }

    /// Asserts the rejection the open date `year`-`month`-`day` gives with a
    /// valid status: the date is the first edit after the status, and its
    /// text is the open date's `suffix` at `field`.
    fn assert_open_date(
        (year, month, day): (&str, &str, &str),
        kind: ErrorKind,
        (suffix, field): (&str, &str),
    ) {
        let input = AccountUpdateInput {
            active_status: "Y".to_owned(),
            open_year: year.to_owned(),
            open_month: month.to_owned(),
            open_day: day.to_owned(),
            ..clean_input()
        };
        let message = date_edit_message(FIELD_OPEN_DATE, suffix);
        assert_rejected(
            validate(&snapshot(), &input, today()),
            (kind, &message, Some(field)),
            &message,
        );
    }

    #[test]
    fn the_open_date_edits_supply_the_first_message() {
        // V-COACTUPC-013 to V-COACTUPC-022.
        for (date, text) in [
            (("", "11", "20"), (YEAR_MUST_BE_SUPPLIED, "OPNYEAR")),
            (("2014", "", "20"), (MONTH_MUST_BE_SUPPLIED, "OPNMON")),
            (("2014", "11", ""), (DAY_MUST_BE_SUPPLIED, "OPNDAY")),
        ] {
            assert_open_date(date, ErrorKind::Empty, text);
        }
        for (date, text) in [
            (("20A2", "11", "20"), (MUST_BE_4_DIGIT_NUMBER, "OPNYEAR")),
            (("1899", "11", "20"), (CENTURY_NOT_VALID, "OPNYEAR")),
            (
                ("2014", "13", "20"),
                (MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12, "OPNMON"),
            ),
            (
                ("2014", "11", "32"),
                (DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, "OPNDAY"),
            ),
            (
                ("2014", "11", "ab"),
                (DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, "OPNDAY"),
            ),
            (
                ("2024", "04", "31"),
                (CANNOT_HAVE_31_DAYS_IN_THIS_MONTH, "OPNMON"),
            ),
            (
                ("2024", "02", "30"),
                (CANNOT_HAVE_30_DAYS_IN_THIS_MONTH, "OPNMON"),
            ),
            (
                ("2023", "02", "29"),
                (NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS, "OPNYEAR"),
            ),
        ] {
            assert_open_date(date, ErrorKind::Invalid, text);
        }
    }

    #[test]
    fn a_date_failure_lists_every_flagged_part_with_its_text() {
        // V-COACTUPC-020, V-COACTUPC-022: the 31-day edit flags the month and
        // the day, the leap-year edit all three parts.
        let input = AccountUpdateInput {
            active_status: "Y".to_owned(),
            open_year: "2023".to_owned(),
            open_month: "02".to_owned(),
            open_day: "29".to_owned(),
            ..clean_input()
        };
        let leap = date_edit_message(FIELD_OPEN_DATE, NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS);
        assert_eq!(
            rejection(&input).fields,
            entries(&[("OPNYEAR", &leap), ("OPNMON", &leap), ("OPNDAY", &leap)])
        );
    }

    #[test]
    fn a_blank_month_later_flagged_by_the_31_day_check_stays_empty() {
        // V-COACTUPC-016, V-COACTUPC-029, V-COACTUPC-042, V-COACTUPC-069: the
        // blank month's text keeps its kind Empty after the 31-day edit
        // (P-CSUTLDPY-009) flags the month and the day not OK.
        use super::{DateBmsFields, DateParts, Edits};

        type Change = fn(&mut AccountUpdateInput);
        let open: Change = |i| (i.open_month, i.open_day) = (String::new(), "31".to_owned());
        let expiry: Change = |i| (i.expiry_month, i.expiry_day) = (String::new(), "31".to_owned());
        let reissue: Change =
            |i| (i.reissue_month, i.reissue_day) = (String::new(), "31".to_owned());
        let birth: Change = |i| (i.dob_month, i.dob_day) = (String::new(), "31".to_owned());
        for (label, fields, change) in [
            (FIELD_OPEN_DATE, DateBmsFields::OPEN_DATE, open),
            (FIELD_EXPIRY_DATE, DateBmsFields::EXPIRY_DATE, expiry),
            (FIELD_REISSUE_DATE, DateBmsFields::REISSUE_DATE, reissue),
            (FIELD_DATE_OF_BIRTH, DateBmsFields::DATE_OF_BIRTH, birth),
        ] {
            let text = messages::date_edit_message(label, messages::MONTH_MUST_BE_SUPPLIED);
            let flagged = [(fields.month, text.as_str()), (fields.day, text.as_str())];
            // The date's edit supplies the first message.
            let mut edits = Edits::default();
            let entered = DateParts::from_entered("2024", "", "31");
            assert_eq!(edits.date(label, entered, fields), Ok(false), "{label}");
            let error = edits.finish().expect_err(label);
            assert_eq!(error.fields, entries(&flagged), "{label}");
            assert_rejected::<()>(
                Err(error),
                (ErrorKind::Empty, text.as_str(), Some(fields.month)),
                label,
            );
            // After the rejected status, the date's parts are listed in order.
            assert_flagged(change, &flagged);
        }
        // The open date is the first edit after a valid status.
        let input = AccountUpdateInput {
            active_status: "Y".to_owned(),
            open_month: String::new(),
            open_day: "31".to_owned(),
            ..clean_input()
        };
        let text = messages::date_edit_message(FIELD_OPEN_DATE, messages::MONTH_MUST_BE_SUPPLIED);
        assert_rejected(
            validate(&snapshot(), &input, today()),
            (ErrorKind::Empty, text.as_str(), Some("OPNMON")),
            "open date",
        );
        assert_eq!(
            rejection(&input).fields,
            entries(&[("OPNMON", text.as_str()), ("OPNDAY", text.as_str())])
        );
    }

    #[test]
    fn a_date_that_reaches_csutldtc_with_no_message_set_halts() {
        // E-26: every request whose status and open date pass reaches
        // CSUTLDTC with no message set; E-47: a month holding a non-digit.
        let valid = AccountUpdateInput {
            active_status: "Y".to_owned(),
            credit_limit: "12A4".to_owned(),
            ..clean_input()
        };
        let month = AccountUpdateInput {
            open_month: "1A".to_owned(),
            ..clean_input()
        };
        for input in [valid, month] {
            assert_rejected(
                validate(&snapshot(), &input, today()),
                (ErrorKind::Unexpected, messages::UNEXPECTED_ERROR, None),
                "halt",
            );
        }
    }

    #[test]
    fn a_halted_input_class_reports_its_escalation_and_answers_the_halt() {
        // E-26: the status and open date pass, so the open date reaches
        // CSUTLDTC with no message set; E-47: a month holding a non-digit;
        // E-40: an amount NUMVAL-C converts to a midpoint beyond the cent.
        let cases = [
            (
                AccountUpdateInput {
                    active_status: "Y".to_owned(),
                    ..clean_input()
                },
                "E-26",
            ),
            (
                AccountUpdateInput {
                    open_month: "1A".to_owned(),
                    ..clean_input()
                },
                "E-47",
            ),
            (
                AccountUpdateInput {
                    credit_limit: "10.005".to_owned(),
                    ..clean_input()
                },
                "E-40",
            ),
        ];
        for (input, escalation_id) in cases {
            let refusal = validate(&snapshot(), &input, today()).expect_err(escalation_id);
            assert_eq!(refusal, EditRefusal::Halted { escalation_id });
            assert_eq!(refusal.escalation_id(), Some(escalation_id));
            assert_eq!(
                DomainError::from(refusal),
                DomainError::unexpected(messages::UNEXPECTED_ERROR),
                "{escalation_id}"
            );
        }
        // An edit's error names no escalation and is answered unchanged.
        let refusal = validate(&snapshot(), &clean_input(), today()).expect_err("the status");
        let EditRefusal::Error(error) = refusal.clone() else {
            panic!("the status edit halted: {refusal:?}");
        };
        assert_eq!(refusal.escalation_id(), None);
        assert_eq!(DomainError::from(refusal), error);
    }

    /// Asserts that `change` to [`clean_input`] adds `expected` to the
    /// `fields` after the status entry, the message staying the status's.
    fn assert_flagged(change: fn(&mut AccountUpdateInput), expected: &[(&'static str, &str)]) {
        let mut input = clean_input();
        change(&mut input);
        let error = rejection(&input);
        let status = field_edit_message(FIELD_ACCOUNT_STATUS, MUST_BE_Y_OR_N);
        assert_eq!(error.message, status);
        assert_eq!(error.kind, ErrorKind::Invalid);
        let mut all = vec![("ACSTTUS", status.as_str())];
        all.extend_from_slice(expected);
        assert_eq!(error.fields, entries(&all), "{expected:?}");
    }

    #[test]
    fn the_clean_input_fails_only_its_status() {
        assert_flagged(|_| {}, &[]);
    }

    #[test]
    fn the_account_field_edits_flag_their_fields_in_order() {
        // V-COACTUPC-024 to V-COACTUPC-055 (E-45: the 26-byte name is cut).
        assert_flagged(
            |i| i.credit_limit = String::new(),
            &[(
                "ACRDLIM",
                &field_edit_message(FIELD_CREDIT_LIMIT, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.credit_limit = "12A4".to_owned(),
            &[(
                "ACRDLIM",
                &field_edit_message(FIELD_CREDIT_LIMIT, IS_NOT_VALID),
            )],
        );
        assert_flagged(
            |i| i.expiry_year = String::new(),
            &[(
                "EXPYEAR",
                &date_edit_message(FIELD_EXPIRY_DATE, YEAR_MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.cash_credit_limit = "1.2.3".to_owned(),
            &[(
                "ACSHLIM",
                &field_edit_message(FIELD_CASH_CREDIT_LIMIT, IS_NOT_VALID),
            )],
        );
        assert_flagged(
            |i| i.reissue_day = "32".to_owned(),
            &[(
                "RISDAY",
                &date_edit_message(FIELD_REISSUE_DATE, DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31),
            )],
        );
        let days_31 = date_edit_message(FIELD_REISSUE_DATE, CANNOT_HAVE_31_DAYS_IN_THIS_MONTH);
        assert_flagged(
            |i| (i.reissue_month, i.reissue_day) = ("04".to_owned(), "31".to_owned()),
            &[("RISMON", &days_31), ("RISDAY", &days_31)],
        );
        assert_flagged(
            |i| i.current_balance = String::new(),
            &[(
                "ACURBAL",
                &field_edit_message(FIELD_CURRENT_BALANCE, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.current_cycle_credit = "--1".to_owned(),
            &[(
                "ACRCYCR",
                &field_edit_message(FIELD_CURRENT_CYCLE_CREDIT_LIMIT, IS_NOT_VALID),
            )],
        );
        assert_flagged(
            |i| i.current_cycle_debit = "*".to_owned(),
            &[(
                "ACRCYDB",
                &field_edit_message(FIELD_CURRENT_CYCLE_DEBIT_LIMIT, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| (i.credit_limit, i.city) = (String::new(), String::new()),
            &[
                (
                    "ACRDLIM",
                    &field_edit_message(FIELD_CREDIT_LIMIT, MUST_BE_SUPPLIED),
                ),
                ("ACSCITY", &field_edit_message(FIELD_CITY, MUST_BE_SUPPLIED)),
            ],
        );
    }

    #[test]
    fn the_ssn_parts_after_the_first_are_edited_only_when_it_is_digits() {
        // V-COACTUPC-056 to V-COACTUPC-065, E-45.
        assert_flagged(
            |i| (i.ssn_part_1, i.ssn_part_2) = (String::new(), String::new()),
            &[(
                "ACTSSN1",
                &field_edit_message(FIELD_SSN_FIRST_3_CHARS, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.ssn_part_1 = "12A".to_owned(),
            &[(
                "ACTSSN1",
                &field_edit_message(FIELD_SSN_FIRST_3_CHARS, MUST_BE_ALL_NUMERIC),
            )],
        );
        assert_flagged(
            |i| i.ssn_part_1 = "000".to_owned(),
            &[(
                "ACTSSN1",
                &field_edit_message(FIELD_SSN_FIRST_3_CHARS, MUST_NOT_BE_ZERO),
            )],
        );
        let area = field_edit_message(
            FIELD_SSN_FIRST_3_CHARS,
            SHOULD_NOT_BE_000_666_OR_BETWEEN_900_AND_999,
        );
        assert_flagged(
            |i| (i.ssn_part_1, i.ssn_part_2) = ("666".to_owned(), "AB".to_owned()),
            &[
                ("ACTSSN1", &area),
                (
                    "ACTSSN2",
                    &field_edit_message(FIELD_SSN_4TH_AND_5TH_CHARS, MUST_BE_ALL_NUMERIC),
                ),
            ],
        );
        assert_flagged(|i| i.ssn_part_1 = "901".to_owned(), &[("ACTSSN1", &area)]);
        assert_flagged(
            |i| i.ssn_part_2 = "00".to_owned(),
            &[(
                "ACTSSN2",
                &field_edit_message(FIELD_SSN_4TH_AND_5TH_CHARS, MUST_NOT_BE_ZERO),
            )],
        );
        assert_flagged(
            |i| i.ssn_part_3 = String::new(),
            &[(
                "ACTSSN3",
                &field_edit_message(FIELD_SSN_LAST_4_CHARS, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(|i| i.ssn_part_1 = "899".to_owned(), &[]);
    }

    #[test]
    fn the_birth_date_and_fico_score_edits_flag_their_fields() {
        // V-COACTUPC-066 to V-COACTUPC-081.
        let future = date_edit_message(FIELD_DATE_OF_BIRTH, CANNOT_BE_IN_THE_FUTURE);
        let leap = date_edit_message(FIELD_DATE_OF_BIRTH, CANNOT_HAVE_31_DAYS_IN_THIS_MONTH);
        assert_flagged(
            |i| i.dob_year = String::new(),
            &[(
                "DOBYEAR",
                &date_edit_message(FIELD_DATE_OF_BIRTH, YEAR_MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| (i.dob_month, i.dob_day) = ("04".to_owned(), "31".to_owned()),
            &[("DOBMON", &leap), ("DOBDAY", &leap)],
        );
        assert_flagged(
            |i| i.dob_year = "2099".to_owned(),
            &[
                ("DOBYEAR", &future),
                ("DOBMON", &future),
                ("DOBDAY", &future),
            ],
        );
        assert_flagged(
            |i| i.fico_score = String::new(),
            &[(
                "ACSTFCO",
                &field_edit_message(FIELD_FICO_SCORE, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.fico_score = "7O0".to_owned(),
            &[(
                "ACSTFCO",
                &field_edit_message(FIELD_FICO_SCORE, MUST_BE_ALL_NUMERIC),
            )],
        );
        assert_flagged(
            |i| i.fico_score = "000".to_owned(),
            &[(
                "ACSTFCO",
                &field_edit_message(FIELD_FICO_SCORE, MUST_NOT_BE_ZERO),
            )],
        );
        for score in ["274", "299", "851"] {
            let mut input = clean_input();
            input.fico_score = score.to_owned();
            let error = rejection(&input);
            assert_eq!(
                error.fields.get(1),
                Some(&FieldError {
                    field: "ACSTFCO",
                    message: field_edit_message(FIELD_FICO_SCORE, SHOULD_BE_BETWEEN_300_AND_850),
                }),
                "{score}"
            );
        }
        assert_flagged(|i| i.fico_score = "300".to_owned(), &[]);
        assert_flagged(|i| i.fico_score = "850".to_owned(), &[]);
    }

    #[test]
    fn the_name_address_and_state_edits_flag_their_fields() {
        // V-COACTUPC-082 to V-COACTUPC-097.
        assert_flagged(
            |i| i.first_name = String::new(),
            &[(
                "ACSFNAM",
                &field_edit_message(FIELD_FIRST_NAME, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.first_name = "J0hn".to_owned(),
            &[(
                "ACSFNAM",
                &field_edit_message(FIELD_FIRST_NAME, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
        assert_flagged(|i| i.middle_name = String::new(), &[]);
        assert_flagged(
            |i| i.middle_name = "M.".to_owned(),
            &[(
                "ACSMNAM",
                &field_edit_message(FIELD_MIDDLE_NAME, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
        assert_flagged(
            |i| i.last_name = "  ".to_owned(),
            &[(
                "ACSLNAM",
                &field_edit_message(FIELD_LAST_NAME, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.last_name = "O'Neil".to_owned(),
            &[(
                "ACSLNAM",
                &field_edit_message(FIELD_LAST_NAME, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
        assert_flagged(
            |i| i.address_line_1 = String::new(),
            &[(
                "ACSADL1",
                &field_edit_message(FIELD_ADDRESS_LINE_1, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(|i| i.address_line_1 = "#12, Lane 4!".to_owned(), &[]);
        assert_flagged(
            |i| i.state = String::new(),
            &[(
                "ACSSTTE",
                &field_edit_message(FIELD_STATE, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.state = "N1".to_owned(),
            &[(
                "ACSSTTE",
                &field_edit_message(FIELD_STATE, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
        let not_listed = field_edit_message(FIELD_STATE, IS_NOT_A_VALID_STATE_CODE);
        assert_flagged(|i| i.state = "nc".to_owned(), &[("ACSSTTE", &not_listed)]);
        assert_flagged(|i| i.state = "XX".to_owned(), &[("ACSSTTE", &not_listed)]);
        assert_flagged(
            |i| i.city = "Area 51".to_owned(),
            &[(
                "ACSCITY",
                &field_edit_message(FIELD_CITY, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
        assert_flagged(
            |i| i.country = String::new(),
            &[(
                "ACSCTRY",
                &field_edit_message(FIELD_COUNTRY, MUST_BE_SUPPLIED),
            )],
        );
        assert_flagged(
            |i| i.country = "U5A".to_owned(),
            &[(
                "ACSCTRY",
                &field_edit_message(FIELD_COUNTRY, CAN_HAVE_ALPHABETS_ONLY),
            )],
        );
    }

    #[test]
    fn the_zip_code_is_edited_alone_then_with_the_state() {
        // V-COACTUPC-091 to V-COACTUPC-093, V-COACTUPC-123, P-CSLKPCDY-005.
        let combo = INVALID_ZIP_CODE_FOR_STATE;
        assert_flagged(
            |i| i.zip = String::new(),
            &[("ACSZIPC", &field_edit_message(FIELD_ZIP, MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.zip = "12A45".to_owned(),
            &[(
                "ACSZIPC",
                &field_edit_message(FIELD_ZIP, MUST_BE_ALL_NUMERIC),
            )],
        );
        assert_flagged(
            |i| i.zip = "00000".to_owned(),
            &[("ACSZIPC", &field_edit_message(FIELD_ZIP, MUST_NOT_BE_ZERO))],
        );
        assert_flagged(
            |i| i.zip = "12546".to_owned(),
            &[("ACSSTTE", combo), ("ACSZIPC", combo)],
        );
        assert_flagged(
            |i| (i.zip, i.city) = ("12546".to_owned(), String::new()),
            &[
                ("ACSCITY", &field_edit_message(FIELD_CITY, MUST_BE_SUPPLIED)),
                ("ACSSTTE", combo),
                ("ACSZIPC", combo),
            ],
        );
        assert_flagged(
            |i| (i.state, i.zip) = ("TX".to_owned(), "75001".to_owned()),
            &[],
        );
        assert_flagged(
            |i| (i.state, i.zip) = ("XX".to_owned(), "12546".to_owned()),
            &[(
                "ACSSTTE",
                &field_edit_message(FIELD_STATE, IS_NOT_A_VALID_STATE_CODE),
            )],
        );
    }

    #[test]
    fn the_phone_number_parts_are_each_edited() {
        // V-COACTUPC-098 to V-COACTUPC-117, P-CSLKPCDY-002.
        let blank = |i: &mut AccountUpdateInput| {
            (i.phone_1_area, i.phone_1_prefix, i.phone_1_line) =
                (String::new(), String::new(), String::new());
        };
        assert_flagged(blank, &[]);
        let phone_1 = |suffix: &str| field_edit_message(FIELD_PHONE_NUMBER_1, suffix);
        let phone_2 = |suffix: &str| field_edit_message(FIELD_PHONE_NUMBER_2, suffix);
        assert_flagged(
            |i| i.phone_1_area = String::new(),
            &[("ACSPH1A", &phone_1(AREA_CODE_MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.phone_1_area = "9A8".to_owned(),
            &[("ACSPH1A", &phone_1(AREA_CODE_MUST_BE_A_3_DIGIT_NUMBER))],
        );
        assert_flagged(
            |i| i.phone_1_area = "000".to_owned(),
            &[("ACSPH1A", &phone_1(AREA_CODE_CANNOT_BE_ZERO))],
        );
        assert_flagged(
            |i| i.phone_1_area = "373".to_owned(),
            &[("ACSPH1A", &phone_1(NOT_VALID_NORTH_AMERICA_AREA_CODE))],
        );
        assert_flagged(
            |i| i.phone_1_prefix = String::new(),
            &[("ACSPH1B", &phone_1(PREFIX_CODE_MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.phone_1_prefix = "1A9".to_owned(),
            &[("ACSPH1B", &phone_1(PREFIX_CODE_MUST_BE_A_3_DIGIT_NUMBER))],
        );
        assert_flagged(
            |i| i.phone_1_prefix = "000".to_owned(),
            &[("ACSPH1B", &phone_1(PREFIX_CODE_CANNOT_BE_ZERO))],
        );
        assert_flagged(
            |i| i.phone_1_line = String::new(),
            &[("ACSPH1C", &phone_1(LINE_NUMBER_CODE_MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.phone_1_line = "83A0".to_owned(),
            &[(
                "ACSPH1C",
                &phone_1(LINE_NUMBER_CODE_MUST_BE_A_4_DIGIT_NUMBER),
            )],
        );
        assert_flagged(
            |i| i.phone_1_line = "0000".to_owned(),
            &[("ACSPH1C", &phone_1(LINE_NUMBER_CODE_CANNOT_BE_ZERO))],
        );
        assert_flagged(
            |i| (i.phone_2_area, i.phone_2_prefix) = (String::new(), String::new()),
            &[
                ("ACSPH2A", &phone_2(AREA_CODE_MUST_BE_SUPPLIED)),
                ("ACSPH2B", &phone_2(PREFIX_CODE_MUST_BE_SUPPLIED)),
            ],
        );
        assert_flagged(
            |i| i.phone_2_area = "373".to_owned(),
            &[("ACSPH2A", &phone_2(NOT_VALID_NORTH_AMERICA_AREA_CODE))],
        );
    }

    #[test]
    fn the_eft_account_and_primary_holder_edits_flag_their_fields() {
        // V-COACTUPC-118 to V-COACTUPC-122.
        let eft = |suffix: &str| field_edit_message(FIELD_EFT_ACCOUNT_ID, suffix);
        let holder = |suffix: &str| field_edit_message(FIELD_PRIMARY_CARD_HOLDER, suffix);
        assert_flagged(
            |i| i.eft_account_id = String::new(),
            &[("ACSEFTC", &eft(MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.eft_account_id = "12345678AB".to_owned(),
            &[("ACSEFTC", &eft(MUST_BE_ALL_NUMERIC))],
        );
        assert_flagged(
            |i| i.eft_account_id = "0000000000".to_owned(),
            &[("ACSEFTC", &eft(MUST_NOT_BE_ZERO))],
        );
        assert_flagged(
            |i| i.primary_card_holder = String::new(),
            &[("ACSPFLG", &holder(MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.primary_card_holder = "0".to_owned(),
            &[("ACSPFLG", &holder(MUST_BE_SUPPLIED))],
        );
        assert_flagged(
            |i| i.primary_card_holder = "y".to_owned(),
            &[("ACSPFLG", &holder(MUST_BE_Y_OR_N))],
        );
    }

    #[test]
    fn the_fixture_customer_fails_its_fico_phone_and_zip_edits() {
        // V-COACTUPC-081, V-COACTUPC-111, V-COACTUPC-123 on account 1 itself.
        let input = AccountUpdateInput {
            active_status: "X".to_owned(),
            ..fetched_input()
        };
        let fields: Vec<&str> = rejection(&input)
            .fields
            .iter()
            .map(|entry| entry.field)
            .collect();
        assert_eq!(
            fields,
            ["ACSTTUS", "ACSTFCO", "ACSPH2A", "ACSSTTE", "ACSZIPC"]
        );
    }

    #[test]
    fn the_update_records_are_strung_from_the_received_values() {
        // P-COACTUPC-013, P-COACTUPC-014: dates and phones strung from their
        // parts as received, the group ID where ACCOUNT-RECORD reads the
        // zip, blank fields stored as spaces.
        let input = AccountUpdateInput {
            active_status: "N".to_owned(),
            open_day: "5".to_owned(),
            credit_limit: "-5.10".to_owned(),
            group_id: "grp1".to_owned(),
            middle_name: String::new(),
            phone_2_area: String::new(),
            phone_2_prefix: String::new(),
            phone_2_line: String::new(),
            fico_score: "700".to_owned(),
            ..fetched_input()
        };
        let changes = changes_for(&input);
        assert_eq!(
            changes.account,
            Account {
                acct_active_status: Alnum::move_from("N"),
                acct_credit_limit: cents(-510),
                acct_open_date: Alnum::move_from("2014-11-5 "),
                acct_addr_zip: Alnum::move_from("grp1"),
                ..fixture_account()
            }
        );
        assert_eq!(
            changes.customer,
            Customer {
                cust_middle_name: Alnum::spaces(),
                cust_phone_num_2: Alnum::move_from("(   )   -"),
                cust_fico_credit_score: FicoScore::new(700).expect("a three-digit score"),
                ..fixture_customer()
            }
        );
        assert_eq!(changes.customer_key, CustomerKey::Received);
    }

    #[test]
    fn a_customer_id_holding_a_non_digit_is_unpinned() {
        // E-74: the record keeps the cross-reference customer's ID.
        for text in ["00000000A", "1", ""] {
            let input = AccountUpdateInput {
                customer_id: text.to_owned(),
                ..fetched_input()
            };
            let changes = changes_for(&input);
            assert_eq!(changes.customer.cust_id, customer_id(), "{text:?}");
            assert_eq!(changes.customer_key, CustomerKey::Unpinned, "{text:?}");
        }
    }

    #[test]
    fn record_version_changes_exactly_when_9700_finds_a_change() {
        // P-COACTUPC-012, P-COACTUPC-048.
        let token = |change: fn(&mut Account, &mut Customer)| {
            let (mut account, mut customer) = (fixture_account(), fixture_customer());
            change(&mut account, &mut customer);
            record_version(&version_key(), &account, &customer).expect("a snapshot's token")
        };
        let base = token(|_, _| {});
        let same: [fn(&mut Account, &mut Customer); 7] = [
            |a, _| a.acct_id = AccountId::new(2).expect("an account number"),
            |a, _| a.acct_expiraion_date = Alnum::move_from("2025/05/20"),
            |a, _| a.acct_addr_zip = Alnum::move_from("B000000000"),
            |_, c| c.cust_id = CustomerId::new(2).expect("a customer number"),
            |_, c| c.cust_first_name = Alnum::move_from("IMMANUEL"),
            |_, c| c.cust_addr_state_cd = Alnum::move_from("nc"),
            |_, c| c.cust_last_name = Alnum::move_from("KESSLER"),
        ];
        for (index, change) in same.into_iter().enumerate() {
            assert_eq!(token(change), base, "same {index}");
        }
        let changed: [fn(&mut Account, &mut Customer); 14] = [
            |a, _| a.acct_active_status = Alnum::move_from("N"),
            |a, _| a.acct_curr_bal = cents(19_401),
            |a, _| a.acct_curr_cyc_debit = cents(1),
            |a, _| a.acct_open_date = Alnum::move_from("2014-11-21"),
            |a, _| a.acct_expiraion_date = Alnum::move_from("2025-06-20"),
            |a, _| a.acct_reissue_date = Alnum::move_from("2026-05-20"),
            |a, _| a.acct_group_id = AccountGroupId::move_from("G"),
            |_, c| c.cust_addr_line_2 = Alnum::move_from("Apt. 803"),
            |_, c| c.cust_addr_zip = Alnum::move_from("12547"),
            |_, c| c.cust_phone_num_2 = Alnum::move_from("(373)693-8685"),
            |_, c| c.cust_ssn = CustSsn::new(20_973_889).expect("a nine-digit number"),
            |_, c| c.cust_dob_yyyy_mm_dd = Alnum::move_from("1961-06-09"),
            |_, c| c.cust_pri_card_holder_ind = Alnum::move_from("N"),
            |_, c| c.cust_fico_credit_score = FicoScore::new(275).expect("a score"),
        ];
        for (index, change) in changed.into_iter().enumerate() {
            assert_ne!(token(change), base, "changed {index}");
        }
        let group = |text: &'static str| {
            let account = Account {
                acct_group_id: AccountGroupId::move_from(text),
                ..fixture_account()
            };
            record_version(&version_key(), &account, &fixture_customer())
                .expect("a snapshot's token")
        };
        assert_eq!(group("Gold"), group("GOLD"));
    }

    /// The records a change of the first name gives, built as `validate`
    /// builds them once every edit passed.
    fn first_name_changes() -> ValidatedAccountChanges {
        changes_for(&AccountUpdateInput {
            first_name: "Jane".to_owned(),
            fico_score: "700".to_owned(),
            ..fetched_input()
        })
    }

    /// The token of the fixture records under the tests' server key, as
    /// `GET /accounts/{id}` issues it.
    fn token() -> super::VersionToken {
        record_version(&version_key(), &fixture_account(), &fixture_customer())
            .expect("the fixture records' token")
    }

    #[test]
    fn both_records_are_rewritten_and_committed() {
        // P-COACTUPC-013 to P-COACTUPC-015.
        let changes = first_name_changes();
        let mut store = fixture_store();
        let updated = block_on(apply(
            &mut store,
            &version_key(),
            account_id(),
            customer_id(),
            &token(),
            changes.clone(),
        ))
        .expect("the update");
        assert_eq!(updated.message, messages::CHANGES_COMMITTED_TO_DATABASE);
        assert_eq!(updated.account, changes.account);
        assert_eq!(updated.customer, changes.customer);
        assert_eq!(store.accounts.get(&account_id()), Some(&changes.account));
        assert_eq!(store.customers.get(&customer_id()), Some(&changes.customer));
        assert_eq!(
            store.calls,
            [
                "read_account_for_update",
                "read_customer_for_update",
                "rewrite_account",
                "rewrite_customer"
            ]
        );
    }

    /// Asserts that applying `changes` to `store` fails with `message` at
    /// `ACCTSID` of `kind` after exactly `calls`, leaving `account` and
    /// `customer` stored.
    fn assert_apply_fails(
        mut store: FakeStore,
        changes: ValidatedAccountChanges,
        (kind, message): (ErrorKind, &str),
        calls: &[&str],
        (account, customer): (&Account, &Customer),
    ) {
        assert_rejected(
            block_on(apply(
                &mut store,
                &version_key(),
                account_id(),
                customer_id(),
                &token(),
                changes,
            )),
            (kind, message, Some("ACCTSID")),
            message,
        );
        assert_eq!(store.calls, calls, "{message}");
        assert_eq!(
            store.accounts.get(&account_id()),
            Some(account),
            "{message}"
        );
        assert_eq!(
            store.customers.get(&customer_id()),
            Some(customer),
            "{message}"
        );
    }

    #[test]
    fn a_failed_lock_writes_nothing() {
        // V-COACTUPC-124, V-COACTUPC-125.
        let fixture = (&fixture_account(), &fixture_customer());
        let mut store = fixture_store();
        store.fail_next("read_account_for_update", PortError::Contention);
        assert_apply_fails(
            store,
            first_name_changes(),
            (
                ErrorKind::Unexpected,
                messages::COULD_NOT_LOCK_ACCOUNT_RECORD_FOR_UPDATE,
            ),
            &["read_account_for_update"],
            fixture,
        );
        let mut store = fixture_store();
        store.fail_next("read_customer_for_update", PortError::NotFound);
        assert_apply_fails(
            store,
            first_name_changes(),
            (
                ErrorKind::Unexpected,
                messages::COULD_NOT_LOCK_CUSTOMER_RECORD_FOR_UPDATE,
            ),
            &["read_account_for_update", "read_customer_for_update"],
            fixture,
        );
    }

    #[test]
    fn a_record_changed_since_the_fetch_is_a_conflict() {
        // V-COACTUPC-126: another update moved the balance.
        let moved = Account {
            acct_curr_bal: cents(20_000),
            ..fixture_account()
        };
        let store = FakeStore::default()
            .with_xref(fixture_xref())
            .with_account(moved.clone())
            .with_customer(fixture_customer());
        assert_apply_fails(
            store,
            first_name_changes(),
            (
                ErrorKind::Conflict,
                messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
            ),
            &["read_account_for_update", "read_customer_for_update"],
            (&moved, &fixture_customer()),
        );
    }

    #[test]
    fn a_failed_account_rewrite_stops_before_the_customer() {
        // V-COACTUPC-127, P-COACTUPC-013.
        let mut store = fixture_store();
        store.fail_next(
            "rewrite_account",
            PortError::Unavailable("database".to_owned()),
        );
        assert_apply_fails(
            store,
            first_name_changes(),
            (ErrorKind::Unexpected, messages::UPDATE_OF_RECORD_FAILED),
            &[
                "read_account_for_update",
                "read_customer_for_update",
                "rewrite_account",
            ],
            (&fixture_account(), &fixture_customer()),
        );
    }

    #[test]
    fn a_failed_customer_rewrite_leaves_the_account_rewrite_applied() {
        // V-COACTUPC-128, P-COACTUPC-014: RECOVERY(NONE).
        let changes = AccountUpdateInput {
            active_status: "N".to_owned(),
            ..fetched_input()
        };
        let changes = changes_for(&changes);
        let mut store = fixture_store();
        store.fail_next(
            "rewrite_customer",
            PortError::Unavailable("database".to_owned()),
        );
        assert_apply_fails(
            store,
            changes.clone(),
            (ErrorKind::Unexpected, messages::UPDATE_OF_RECORD_FAILED),
            &[
                "read_account_for_update",
                "read_customer_for_update",
                "rewrite_account",
                "rewrite_customer",
            ],
            (&changes.account, &fixture_customer()),
        );
    }

    #[test]
    fn a_sent_customer_id_never_rewrites_another_customer() {
        // E-74: nine digits that differ change the VSAM key, so the customer
        // rewrite fails after the account rewrite; a non-digit writes nothing.
        let moved = changes_for(&AccountUpdateInput {
            customer_id: "000000002".to_owned(),
            ..fetched_input()
        });
        assert_eq!(moved.customer_key, CustomerKey::Received);
        assert_apply_fails(
            fixture_store(),
            moved.clone(),
            (ErrorKind::Unexpected, messages::UPDATE_OF_RECORD_FAILED),
            &[
                "read_account_for_update",
                "read_customer_for_update",
                "rewrite_account",
            ],
            (&moved.account, &fixture_customer()),
        );
        let unpinned = changes_for(&AccountUpdateInput {
            customer_id: "00000000A".to_owned(),
            ..fetched_input()
        });
        assert_apply_fails(
            fixture_store(),
            unpinned,
            (ErrorKind::Unexpected, messages::UPDATE_OF_RECORD_FAILED),
            &["read_account_for_update", "read_customer_for_update"],
            (&fixture_account(), &fixture_customer()),
        );
    }

    #[test]
    fn an_update_without_a_change_reads_and_writes_nothing_more() {
        // V-COACTUPC-010.
        let mut store = fixture_store();
        let updated = block_on(update(
            &mut store,
            &version_key(),
            &fetched_input(),
            today(),
        ))
        .expect("no change");
        assert_eq!(
            updated.message,
            messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED
        );
        assert_eq!(updated.account, fixture_account());
        assert_eq!(updated.customer, fixture_customer());
        assert_eq!(
            store.calls,
            ["read_xref_by_account", "read_account", "read_customer"]
        );
    }

    #[test]
    fn an_update_stops_at_its_first_failing_step() {
        // V-COACTUPC-002, V-COACTUPC-012, E-26.
        let status = field_edit_message(FIELD_ACCOUNT_STATUS, MUST_BE_Y_OR_N);
        let cases = [
            (
                AccountUpdateInput {
                    account_id: String::new(),
                    ..fetched_input()
                },
                (
                    ErrorKind::Empty,
                    messages::NO_INPUT_RECEIVED,
                    Some("ACCTSID"),
                ),
                0,
            ),
            (
                clean_input(),
                (ErrorKind::Invalid, status.as_str(), Some("ACSTTUS")),
                3,
            ),
            (
                AccountUpdateInput {
                    active_status: "N".to_owned(),
                    ..fetched_input()
                },
                (ErrorKind::Unexpected, messages::UNEXPECTED_ERROR, None),
                3,
            ),
        ];
        for (index, (input, expected, reads)) in cases.into_iter().enumerate() {
            let mut store = fixture_store();
            assert_rejected(
                block_on(update(&mut store, &version_key(), &input, today())),
                expected,
                &format!("case {index}"),
            );
            assert_eq!(store.calls.len(), reads, "case {index}");
            assert_eq!(store.accounts.get(&account_id()), Some(&fixture_account()));
        }
    }
}
