//! The wire types of CTLI and CTTU over the Db2 transaction type master
//! `CARDDEMO.TRANSACTION_TYPE` [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4],
//! table `transaction_types`:
//!
//! | Operation | Program and map | Request | Error fields |
//! |---|---|---|---|
//! | `GET /admin/transaction-types` | `COTRTLIC`, `COTRTLI` | [`TranTypeListQuery`] and [`TranTypeListPageQuery`] | [`TranTypeListField`] |
//! | `POST /admin/transaction-types` | `COTRTUPC`, `COTRTUP` | [`TranTypeCreateRequest`] | [`TranTypeUpsertField`] |
//! | `PUT /admin/transaction-types/{id}` | `COTRTUPC`, `COTRTUP` | [`TranTypeUpdateRequest`] | [`TranTypeUpsertField`] |
//! | `PUT /admin/transaction-types/{id}?origin=list` | `COTRTLIC`, `COTRTLI` | [`TranTypeUpdateRequest`] | none |
//! | `DELETE /admin/transaction-types/{id}` | `COTRTLIC`, `COTRTLI` | the path only | none |
//! | `DELETE /admin/transaction-types/{id}?origin=form` | `COTRTUPC`, `COTRTUP` | the path and [`TranTypeDeleteQuery`] | [`TranTypeUpsertField`] |
//!
//! Every write reaches the Db2 master only. The reports read the processing
//! copy `trantype_file`, which shows an edit made here only after
//! `batch seed --only tranextr`.
//!
//! # One update, two programs
//!
//! CTTU and CTLI share `PUT /admin/transaction-types/{id}`.
//! [`TranTypeUpdateQuery::origin`] selects the program: absent, the
//! `COTRTUPC` edits and texts apply (V-COTRTUPC-001 to V-COTRTUPC-011), with
//! the cursor fields `TRTYPCD` and `TRTYDSC` of `3300-SETUP-SCREEN-ATTRS`
//! [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1303-1325]; `list`, the
//! `COTRTLIC` inline description update of `1211-EDIT-ARRAY-DESC` and
//! `9200-UPDATE-RECORD` applies (V-COTRTLIC-012 to V-COTRTLIC-018). Those
//! rows name no BMS field: the description being edited is the row field
//! `TRTYPD1` to `TRTYPD7` of whichever row was selected
//! [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1336-1370], so no single
//! field exists for an error to name, and their errors carry none.
//!
//! # One delete, two programs
//!
//! CTLI and CTTU share `DELETE /admin/transaction-types/{id}`, the plan's
//! CTLI resource. [`TranTypeDeleteQuery::origin`] selects the program, as
//! the update's `origin` does: absent, it is `COTRTLIC`'s
//! `9300-DELETE-RECORD` (V-COTRTLIC-021 to V-COTRTLIC-023), whose rows name
//! no field either; `form`, the `COTRTUP` F4 delete of `COTRTUPC`, with its
//! key edits, its read, `Delete successful.` and its own Db2 texts
//! (P-COTRTUPC-004, P-COTRTUPC-005, V-COTRTUPC-022 to V-COTRTUPC-028), on
//! `TRTYPCD` (E-23, P-COTRTUPC-028). [`TranTypeCreateRequest`], the `POST`
//! body, is the `COTRTUPC` add: F5 on a key not found. The map's `F6=Add`
//! legend has no executable path, as it stays dark and F6 is never accepted
//! (P-COTRTUPC-001, P-COTRTUPC-026).
//!
//! # Lengths
//!
//! A request field keeps the length of the BMS field it replaces: `TRTYPE`
//! and `TRTYPCD` are `X(2)`, `TRDESC`, `TRTYDSC` and `TRTYPD1` to `TRTYPD7`
//! are `X(50)` [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L66-90]
//! [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L60-66]. The 3270 field
//! holds no more, so a longer value has no COBOL path and no audited text: it
//! is rejected 400 with an empty message, naming the field where the
//! program's rows name one. Every value within the length reaches the rule
//! exactly as entered, so blanks, zeros and non-digits get the audited COBOL
//! messages. [`check_type_code_key`] applies the same length to the `{id}`
//! path segment, and [`TranTypeListPageQuery::check`] to the list's
//! `start_key`.
//!
//! # Encoding
//!
//! Both programs `MOVE` the UTF-8 bytes of a value into its `X(n)` field, so
//! a character above `U+007F` takes two to four bytes there. Where an audited
//! edit reads every byte that `MOVE` keeps, or the rule searches the value as
//! entered, the rule answers such a value with its own text, and nothing here
//! precedes it; where neither holds, the value is held to ASCII
//! ([`crate::dto::ascii_text`]) and answered as an over-length one:
//!
//! - `COTRTUPC` (`POST`, `PUT` without `origin`, and `DELETE` with
//!   `origin=form`): the type code, from the body or the `{id}` segment, goes
//!   to `1245-EDIT-NUM-REQD`, and the description to
//!   `1230-EDIT-ALPHANUM-REQD`. A non-ASCII character leaves a byte of `0x80`
//!   or more within the two or fifty bytes kept, which is no `NUMVAL`
//!   argument and no letter, digit or space, so the key is refused `must be
//!   numeric.` (V-COTRTUPC-002, V-COTRTUPC-013, V-COTRTUPC-023) and the
//!   description `can have numbers or alphabets only.` (V-COTRTUPC-008,
//!   V-COTRTUPC-018) before anything is read or written. No ASCII check is
//!   added.
//! - `COTRTLIC` (`PUT` with `origin=list`, and `DELETE` without `origin`):
//!   the `{id}` segment is the listed row's `TRTTYPn` and has no edit, so a
//!   non-ASCII segment would address the type its first two bytes spell; it
//!   is held to ASCII, on no field, as the rows name none. The row
//!   description goes to `1240-EDIT-ALPHANUM-REQD`. The rule receives it
//!   into `X(50)` (`1100-RECEIVE-SCREEN`) keeping whole a character that byte
//!   50 splits, which stands for the lead bytes the COBOL field keeps, so the
//!   edit reads every non-ASCII character of a description within the length
//!   and refuses it `can have numbers or alphabets only.` (V-COTRTLIC-014)
//!   before anything is written. No ASCII check is added.
//! - `COTRTLIC` list (`GET`): the type code filter goes to
//!   `1220-EDIT-TYPECD`, whose `IS NUMERIC` refuses a byte of `0x80` or more
//!   with its own text (V-COTRTLIC-001), so no ASCII check is added. The
//!   description filter is moved into `X(50)` by `1230-EDIT-DESC` with no
//!   content edit (P-COTRTLIC-020), and the rule keeps whole a character
//!   that byte 50 splits, so a non-ASCII filter is searched as entered: the
//!   count finds only descriptions holding its characters, and a filter
//!   matching none is answered `No Records found for these filter
//!   conditions` (V-COTRTLIC-002). No ASCII check is added. The
//!   `start_key` is the `X(2)` COMMAREA key `WS-CA-FIRST-TR-CODE` or
//!   `WS-CA-LAST-TR-CODE` that the program sets from the rows it showed
//!   (P-COTRTLIC-030, P-COTRTLIC-031, P-COTRTLIC-033) and no edit reads, so a
//!   non-ASCII key would browse from the code its first two bytes spell; it
//!   is held to ASCII on no field, as no BMS field holds it.

use carddemo_domain::numeric::TranTypeCode;
use carddemo_domain::rules::Direction;
use carddemo_domain::rules::tran_type_list::{
    TranTypeDeleted, TranTypeListInput, TranTypeMaster, TranTypePage, TranTypeUpdated,
};
use carddemo_domain::rules::tran_type_update::{
    self, FIELD_DESCRIPTION, FIELD_TYPE_CODE, TranTypeSaved, TranTypeUpsertInput,
    TranTypeUpsertOutcome,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationErrors};

use crate::dto::{BmsField, id_string, reject_invalid};
use crate::error::ApiError;
use crate::pagination::{Page, PageDirection, PageQuery, PageSize};

/// The length of the type code fields: `TRTYPEI PIC X(2)` of `COTRTLI`, and
/// `TRTYPCDI PIC X(2)` of `COTRTUP`
/// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L66]
/// [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L60], the width of
/// `TR_TYPE CHAR(2)` [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L2].
// `u64`, the type `validator` takes its length bounds in.
const TYPE_CODE_MAX_LEN: u64 = 2;

/// The length of the description fields: `TRDESCI` and `TRTYPD1I` to
/// `TRTYPD7I PIC X(50)` of `COTRTLI`, and `TRTYDSCI PIC X(50)` of `COTRTUP`
/// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L72]
/// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L90]
/// [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L66], the width of
/// `TR_DESCRIPTION VARCHAR(50)` [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L3].
const DESCRIPTION_MAX_LEN: u64 = 50;

/// The message of a field longer than its BMS field. The 3270 field cannot
/// hold the longer value, so neither program has a path or a text for it, and
/// the validation audit records none.
const OVER_LENGTH_MESSAGE: &str = "";

/// A BMS field of the `COTRTLI` map that a `GET /admin/transaction-types`
/// error names: the two filters, on which `2400-SETUP-SCREEN-ATTRS` places
/// the cursor when their edits fail
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1477-1485].
///
/// The `COTRTLIC` row actions name no field (V-COTRTLIC-011 to
/// V-COTRTLIC-023), so this enum holds the filters only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TranTypeListField {
    /// The type code filter `TRTYPE`, `X(2)`: V-COTRTLIC-001 to
    /// V-COTRTLIC-010 and V-COTRTLIC-024 to V-COTRTLIC-028.
    #[serde(rename = "TRTYPE")]
    TrType,
    /// The description filter `TRDESC`, `X(50)`, which `1290-CROSS-EDITS`
    /// marks with the type filter when the filters match no type
    /// (V-COTRTLIC-002, V-COTRTLIC-028).
    #[serde(rename = "TRDESC")]
    TrDesc,
}

impl TranTypeListField {
    /// The BMS field name, the `fields[].field` value of an error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrType => "TRTYPE",
            Self::TrDesc => "TRDESC",
        }
    }

    /// The field of a [`TranTypeListQuery`] parameter, by its JSON name.
    fn of_filter(name: &str) -> Option<Self> {
        match name {
            "type_code" => Some(Self::TrType),
            "description" => Some(Self::TrDesc),
            _ => None,
        }
    }
}

impl BmsField for TranTypeListField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// `TRTYPE` ranks first, its earliest row being Order 1 (V-COTRTLIC-001).
    /// `TRDESC` heads no row of its own; it ranks second, the Order of the
    /// cross-edit that marks it (V-COTRTLIC-002), after `TRTYPE` as the
    /// cursor placement tests them.
    fn rank(self) -> u16 {
        match self {
            Self::TrType => 1,
            Self::TrDesc => 2,
        }
    }
}

/// A BMS field of the `COTRTUP` map that a `POST /admin/transaction-types`,
/// `PUT /admin/transaction-types/{id}` or
/// `DELETE /admin/transaction-types/{id}?origin=form` error names, where
/// `3300-SETUP-SCREEN-ATTRS` places the cursor
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1303-1325].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum TranTypeUpsertField {
    /// The type code `TRTYPCD`, `X(2)`: the key edits, the read, every write
    /// failure but the lock error (V-COTRTUPC-001 to V-COTRTUPC-005,
    /// V-COTRTUPC-010 to V-COTRTUPC-016, V-COTRTUPC-020, V-COTRTUPC-021), and
    /// every failure of the delete (V-COTRTUPC-022 to V-COTRTUPC-028).
    #[serde(rename = "TRTYPCD")]
    TrTypCd,
    /// The description `TRTYDSC`, `X(50)`: the no-change compare, the
    /// description edits and the lock error, whose changes-made state puts
    /// the cursor there (V-COTRTUPC-006 to V-COTRTUPC-009, V-COTRTUPC-017 to
    /// V-COTRTUPC-019; P-COTRTUPC-025).
    #[serde(rename = "TRTYDSC")]
    TrTyDsc,
}

impl TranTypeUpsertField {
    /// The BMS field name, the `fields[].field` value of an error: the same
    /// text the `COTRTUPC` rules put on their errors.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrTypCd => FIELD_TYPE_CODE,
            Self::TrTyDsc => FIELD_DESCRIPTION,
        }
    }

    /// The field of a [`TranTypeCreateRequest`] or [`TranTypeUpdateRequest`]
    /// member, by its JSON name.
    fn of_member(name: &str) -> Option<Self> {
        match name {
            "type_code" => Some(Self::TrTypCd),
            "description" => Some(Self::TrTyDsc),
            _ => None,
        }
    }
}

impl BmsField for TranTypeUpsertField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The lowest audit Order of the field's rows on either operation:
    /// `TRTYPCD` 1 (V-COTRTUPC-001, V-COTRTUPC-012), `TRTYDSC` 5
    /// (V-COTRTUPC-006, V-COTRTUPC-017).
    fn rank(self) -> u16 {
        match self {
            Self::TrTypCd => 1,
            Self::TrTyDsc => 5,
        }
    }
}

/// The filters of `GET /admin/transaction-types`: the `TRTYPE` and `TRDESC`
/// fields of the `COTRTLI` map, as entered.
///
/// The browse position travels separately, as [`TranTypeListPageQuery`], so
/// the handler extracts two query structs. The filter edits are the rule's
/// (`1230-EDIT-DESC` and `1220-EDIT-TYPECD`, through
/// [`carddemo_domain::rules::tran_type_list::edit_filters`]): blanks or `00`
/// set no type condition, anything but two digits is rejected with the
/// audited text (V-COTRTLIC-001), and a description becomes a `LIKE` pattern.
#[derive(Debug, Clone, Default, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct TranTypeListQuery {
    /// The type code filter, `TRTYPE` `X(2)`: two digits, or blank or `00` for
    /// every type.
    // COTRTLIC 1220-EDIT-TYPECD — V-COTRTLIC-001: the pattern admits exactly
    // the text `rules::tran_type_list::edit_filters` passes once it is moved
    // into `X(2)` and padded with spaces: all spaces (empty included), both
    // bytes low-values, or two digits (`00` among them). One digit, or a mix
    // of a space and low-values, is not blank and not numeric.
    #[serde(default)]
    #[param(max_length = 2, pattern = r"^(?: {0,2}|\x00{2}|[0-9]{2})$")]
    #[validate(length(max = TYPE_CODE_MAX_LEN))]
    pub type_code: Option<String>,
    /// The description filter, `TRDESC` `X(50)`: the types whose description
    /// contains this text, its leading and trailing spaces dropped.
    // COTRTLIC 1230-EDIT-DESC — P-COTRTLIC-020: the filter has no content
    // edit, and the rule searches it as entered, a character outside ASCII
    // included, so only its length is checked here (see the module's notes
    // on the encoding).
    #[serde(default)]
    #[param(max_length = 50)]
    #[validate(length(max = DESCRIPTION_MAX_LEN))]
    pub description: Option<String>,
}

impl TranTypeListQuery {
    /// Rejects a filter longer than its BMS field: 400 with an empty message
    /// on `TRTYPE` or `TRDESC`.
    ///
    /// COTRTLIC 1220-EDIT-TYPECD, 1230-EDIT-DESC — P-COTRTLIC-020,
    /// V-COTRTLIC-001, V-COTRTLIC-002: within the length each filter reaches
    /// the rule as entered. `1220-EDIT-TYPECD` refuses a non-ASCII type code
    /// filter with its own text, and a non-ASCII description filter is
    /// searched as entered, so one that matches no type is answered by the
    /// count. Blank and low-value filters pass, for the rule's no-condition
    /// reading.
    ///
    /// # Errors
    ///
    /// The 400 [`ApiError`] when a filter exceeds its length.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|errors| {
            reject_invalid(&errors, TranTypeListField::of_filter, OVER_LENGTH_MESSAGE)
        })
    }

    /// The rule input of one list page: the filters as entered and the
    /// browse position [`PageQuery::resolve`] gives for the
    /// [`TranTypeListPageQuery`].
    ///
    /// The start key is moved into the `X(2)` code as the COMMAREA keys
    /// `WS-CA-FIRST-TR-CODE` and `WS-CA-LAST-TR-CODE` hold it, so call
    /// [`TranTypeListPageQuery::check`] first: a longer or non-ASCII key
    /// would be cut to the bytes of another code. A blank key is no key: it
    /// is the value those fields hold before any page was shown, so the
    /// request reads the first page.
    #[must_use]
    pub fn into_input(
        self,
        start_key: Option<String>,
        direction: Direction,
        page_size: u16,
    ) -> TranTypeListInput {
        let start_key = start_key
            .map(|key| TranTypeCode::move_from(&key))
            .filter(|code| !code.as_alnum().is_blank());
        TranTypeListInput {
            type_filter: self.type_code,
            desc_filter: self.description,
            start_key,
            direction,
            page_size,
        }
    }
}

/// The page position of `GET /admin/transaction-types`: `start_key`,
/// `direction` and `page_size`.
///
/// COTRTLIC 8000-READ-FORWARD, 8100-READ-BACKWARDS, 2500-SETUP-MESSAGE —
/// P-COTRTLIC-004, P-COTRTLIC-030, P-COTRTLIC-031, P-COTRTLIC-033: the
/// parameters are those of [`PageQuery`], declared here so that the
/// published `start_key` carries the `X(2)` width of the COMMAREA keys
/// `WS-CA-FIRST-TR-CODE` and `WS-CA-LAST-TR-CODE` and the ASCII pattern
/// that [`TranTypeListPageQuery::check`] holds it to. `direction` and
/// `page_size` are published and read exactly as `PageQuery` declares them,
/// and the query converts into one, so [`PageQuery::resolve`] gives the
/// browse its direction and its seven-row default (`WS-MAX-SCREEN-LINES`).
/// An unknown `direction` or a `page_size` outside 1 to 100 fails to
/// deserialize, so [`ApiQuery`](crate::error::ApiQuery) answers it 400 with
/// code `INVALID`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct TranTypeListPageQuery {
    /// The type code the page starts from, `X(2)` in ASCII: a previous
    /// page's `next_key` or `prev_key`. Forward (F8), the page starts at that
    /// code, inclusive; backward (F7), it holds the types before it. Absent,
    /// empty, all spaces or all low-values is no key: both directions read
    /// the first page, backward with `No previous pages to display`.
    #[param(max_length = 2, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(
        length(max = TYPE_CODE_MAX_LEN),
        custom(function = "crate::dto::ascii_text")
    )]
    pub start_key: Option<String>,
    /// `forward` (ENTER and F8, the default) or `backward` (F7).
    // Inlined, as `PageQuery` inlines it: query parameters register no
    // component schema, so a reference to `PageDirection` would dangle.
    #[param(inline)]
    pub direction: Option<PageDirection>,
    /// Rows on the page, from 1 to 100. Defaults to 7, the rows of the
    /// `COTRTLI` screen.
    #[param(value_type = Option<u16>, minimum = 1, maximum = 100)]
    pub page_size: Option<PageSize>,
}

impl TranTypeListPageQuery {
    /// Rejects a `start_key` longer than the type code `X(2)` or holding a
    /// character above `U+007F`: 400 with an empty message and no field.
    ///
    /// COTRTLIC 8000-READ-FORWARD, 8100-READ-BACKWARDS — P-COTRTLIC-030,
    /// P-COTRTLIC-031, P-COTRTLIC-033: the key is the COMMAREA code the
    /// program set from the rows it showed, which no operator types and no
    /// edit reads. Moved into `X(2)`, a longer key, or the UTF-8 bytes of a
    /// non-ASCII one, would start the browse at another code. No BMS field
    /// holds the key, so the error names none, as the `COTRTLIC` row keys'
    /// errors name none ([`check_type_code_key`]). Blank, space and
    /// low-value keys pass: they read the first page.
    ///
    /// # Errors
    ///
    /// The 400 [`ApiError`] when `start_key` has more than two characters or
    /// is not ASCII.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|errors| reject_list_row(&errors))
    }
}

/// The shared pagination query of the type list, each parameter unchanged.
impl From<TranTypeListPageQuery> for PageQuery {
    fn from(query: TranTypeListPageQuery) -> Self {
        let TranTypeListPageQuery {
            start_key,
            direction,
            page_size,
        } = query;
        Self {
            start_key,
            direction,
            page_size,
        }
    }
}

/// One transaction type: a row of the `COTRTLI` list (`TRTTYPn`,
/// `TRTYPDn`) and the details of the `COTRTUP` screen (`TRTYPCD`, `TRTYDSC`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeResponse {
    /// `TR_TYPE`, the two-character type code.
    pub type_code: String,
    /// `TR_DESCRIPTION` as stored, at its own length.
    pub description: String,
}

impl From<&TranTypeMaster> for TranTypeResponse {
    fn from(master: &TranTypeMaster) -> Self {
        Self {
            type_code: id_string(&master.tr_type),
            // A VARCHAR keeps its own length: the text is copied, never padded.
            description: master.tr_description.clone(),
        }
    }
}

/// The page of `GET /admin/transaction-types`: the types shown, the codes
/// the neighbouring pages start at, and the page text of the rule
/// (V-COTRTLIC-004, V-COTRTLIC-005, V-COTRTLIC-008, V-COTRTLIC-009).
impl From<TranTypePage> for Page<TranTypeResponse> {
    fn from(page: TranTypePage) -> Self {
        Self::new(
            page.items.iter().map(TranTypeResponse::from).collect(),
            page.next_key.as_ref().map(id_string),
            page.prev_key.as_ref().map(id_string),
            page.message.map(str::to_owned),
        )
    }
}

/// The body of `POST /admin/transaction-types`, the `COTRTUP` add: the type
/// code `TRTYPCD` and the description `TRTYDSC`, as entered.
///
/// An absent member is the empty field, which the `COTRTUPC` edits answer
/// with their must-be-supplied texts (V-COTRTUPC-012, V-COTRTUPC-017).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema, Validate)]
pub struct TranTypeCreateRequest {
    /// The new type code, `TRTYPCD` `X(2)`: a number from 1 to 99, stored as
    /// two digits.
    #[serde(default)]
    #[schema(max_length = 2)]
    #[validate(length(max = TYPE_CODE_MAX_LEN))]
    pub type_code: Option<String>,
    /// The description, `TRTYDSC` `X(50)`: letters, digits and spaces.
    #[serde(default)]
    #[schema(max_length = 50)]
    #[validate(length(max = DESCRIPTION_MAX_LEN))]
    pub description: Option<String>,
}

impl TranTypeCreateRequest {
    /// Rejects a member longer than its BMS field: 400 with an empty message
    /// on `TRTYPCD` or `TRTYDSC`.
    ///
    /// # Errors
    ///
    /// The 400 [`ApiError`] when a member exceeds its length.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|errors| reject_upsert(&errors))
    }

    /// The `COTRTUPC` input of the add, each member as entered and an absent
    /// one empty.
    #[must_use]
    pub fn into_input(self) -> TranTypeUpsertInput {
        TranTypeUpsertInput {
            type_code: self.type_code.unwrap_or_default(),
            description: self.description.unwrap_or_default(),
        }
    }
}

/// The program whose edits and texts `PUT /admin/transaction-types/{id}`
/// applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranTypeOrigin {
    /// The inline update of a listed row, `COTRTLIC` `U` and F10.
    List,
}

/// The query of `PUT /admin/transaction-types/{id}`.
///
/// A value of `origin` other than `list` does not deserialize, so the query
/// extractor answers it 400.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TranTypeUpdateQuery {
    /// `list` for the inline update of the type list (`COTRTLIC`); absent for
    /// the maintenance screen (`COTRTUPC`).
    // Inlined: query parameters register no component schema, so a reference
    // to `TranTypeOrigin` would dangle in the OpenAPI document.
    #[serde(default)]
    #[param(inline)]
    pub origin: Option<TranTypeOrigin>,
}

/// The program whose edits and texts `DELETE /admin/transaction-types/{id}`
/// applies when the request names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranTypeDeleteOrigin {
    /// The F4 delete of the maintenance screen, `COTRTUPC` F4 confirmed.
    Form,
}

/// The query of `DELETE /admin/transaction-types/{id}`.
///
/// A value of `origin` other than `form` does not deserialize, so the query
/// extractor answers it 400.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TranTypeDeleteQuery {
    /// `form` for the F4 delete of the maintenance screen (`COTRTUPC`);
    /// absent for the row delete of the type list (`COTRTLIC`).
    // Inlined, as `TranTypeUpdateQuery::origin` is.
    #[serde(default)]
    #[param(inline)]
    pub origin: Option<TranTypeDeleteOrigin>,
}

/// The body of `PUT /admin/transaction-types/{id}`: the new description, as
/// entered. The type code is the path segment.
///
/// Without `origin` it is the `COTRTUP` `TRTYDSC`; with `origin=list` it is
/// the `COTRTLI` row description `TRTYPDn`. Both are `X(50)`. An absent
/// description is the empty field, which both programs answer with their
/// must-be-supplied text (V-COTRTUPC-007, V-COTRTLIC-013).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema, Validate)]
pub struct TranTypeUpdateRequest {
    /// The new description, `X(50)`: letters, digits and spaces.
    // No pattern: the alphanumeric edit of either origin refuses every
    // non-ASCII description with its audited text (see the module's notes on
    // the encoding).
    #[serde(default)]
    #[schema(max_length = 50)]
    #[validate(length(max = DESCRIPTION_MAX_LEN))]
    pub description: Option<String>,
}

/// The input of the `COTRTLIC` inline update: the arguments of
/// [`carddemo_domain::rules::tran_type_list::update_description`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeListUpdate {
    /// The code of the listed row, moved into the `X(2)` code.
    pub code: TranTypeCode,
    /// The row description as entered; empty when absent.
    pub description: String,
}

impl TranTypeUpdateRequest {
    /// Rejects a description longer than its BMS field: 400 with an empty
    /// message, on `TRTYDSC` without `origin` and on no field with
    /// `origin=list`, whose rows name none.
    ///
    /// Within the length the description reaches the rule as entered, so a
    /// character above `U+007F` gets the alphanumeric text of the program:
    /// `COTRTUPC` `1230-EDIT-ALPHANUM-REQD` without `origin`
    /// (V-COTRTUPC-008), and with `origin=list` `COTRTLIC`
    /// `1240-EDIT-ALPHANUM-REQD` (V-COTRTLIC-014), whose 50-byte receive
    /// keeps whole a character the cut splits (P-COTRTLIC-013). Blank
    /// descriptions pass either way, for the must-be-supplied edits
    /// (V-COTRTUPC-007, V-COTRTLIC-013).
    ///
    /// # Errors
    ///
    /// The 400 [`ApiError`] when the description exceeds its length.
    pub fn check(&self, origin: Option<TranTypeOrigin>) -> Result<(), ApiError> {
        match origin {
            None => self.validate().map_err(|errors| reject_upsert(&errors)),
            Some(TranTypeOrigin::List) => ListRowDescription {
                description: self.description.as_deref().unwrap_or_default(),
            }
            .validate()
            .map_err(|errors| reject_list_row(&errors)),
        }
    }

    /// The `COTRTUPC` input of the update without `origin`: the path's type
    /// code and the description as entered, empty when absent.
    #[must_use]
    pub fn into_upsert_input(self, type_code: String) -> TranTypeUpsertInput {
        TranTypeUpsertInput {
            type_code,
            description: self.description.unwrap_or_default(),
        }
    }

    /// The `COTRTLIC` input of the update with `origin=list`: the path's
    /// type code, the listed row's `TRTTYPn`, and the description as
    /// entered, empty when absent.
    ///
    /// The code is moved into the `X(2)` code, so call
    /// [`check_type_code_key`] first: a longer segment would be cut to
    /// another type's code.
    // `&str`, not the owned `String` of the other conversions: the code is
    // copied out of the text, and clippy's `needless_pass_by_value` (denied)
    // rejects an owned argument that is only borrowed.
    #[must_use]
    pub fn into_list_update(self, type_code: &str) -> TranTypeListUpdate {
        TranTypeListUpdate {
            code: TranTypeCode::move_from(type_code),
            description: self.description.unwrap_or_default(),
        }
    }
}

/// Rejects a `{id}` path segment longer than the type code `X(2)`: 400 with
/// an empty message, on `TRTYPCD` without `origin`, and on no field with
/// `origin=list`, the `COTRTLIC` row actions whose rows name none.
/// `DELETE /admin/transaction-types/{id}` without `origin` is the `COTRTLIC`
/// delete, so it passes `Some(TranTypeOrigin::List)`; with `origin=form` it
/// is the `COTRTUPC` delete, which passes `None`.
///
/// The segment is the code a `COTRTUPC` key field or a listed row holds,
/// never more than two characters. Moved into `X(2)`, a longer one would be
/// cut to the code of another type, which the update or the delete would
/// then change. Within the length, the segment goes to the rule as entered.
///
/// With `origin=list` the segment must also be ASCII: `COTRTLIC`
/// `9200-UPDATE-RECORD` and `9300-DELETE-RECORD` address the listed row's
/// `TRTTYPn` with no edit (P-COTRTLIC-007, P-COTRTLIC-009), so the first two
/// UTF-8 bytes of a non-ASCII segment would spell another code. Without
/// `origin`, `COTRTUPC` `1245-EDIT-NUM-REQD` refuses such a segment `Tran
/// Type code must be numeric.` (V-COTRTUPC-002), which is left to answer it.
/// Blank, space and low-value segments pass, for the rules' own edits.
///
/// # Errors
///
/// The 400 [`ApiError`] when the segment has more than two characters, or,
/// with `origin=list`, holds a character above `U+007F`.
pub fn check_type_code_key(
    type_code: &str,
    origin: Option<TranTypeOrigin>,
) -> Result<(), ApiError> {
    match origin {
        None => TypeCodeKey { type_code }
            .validate()
            .map_err(|errors| reject_upsert(&errors)),
        Some(TranTypeOrigin::List) => ListedTypeCodeKey { type_code }
            .validate()
            .map_err(|errors| reject_list_row(&errors)),
    }
}

/// The `{id}` path segment under the length check of the type code members,
/// so that it counts characters as they do and maps to the same field.
#[derive(Validate)]
struct TypeCodeKey<'a> {
    #[validate(length(max = TYPE_CODE_MAX_LEN))]
    type_code: &'a str,
}

/// The `{id}` path segment of a `COTRTLIC` row action, under the length
/// check of [`TypeCodeKey`] and the ASCII check of its unedited code.
#[derive(Validate)]
struct ListedTypeCodeKey<'a> {
    #[validate(
        length(max = TYPE_CODE_MAX_LEN),
        custom(function = "crate::dto::ascii_text")
    )]
    type_code: &'a str,
}

/// The row description of a `COTRTLIC` inline update, an absent one empty,
/// under the length check of [`TranTypeUpdateRequest`]. Its content is left
/// to `1240-EDIT-ALPHANUM-REQD` (V-COTRTLIC-013, V-COTRTLIC-014).
#[derive(Validate)]
struct ListRowDescription<'a> {
    #[validate(length(max = DESCRIPTION_MAX_LEN))]
    description: &'a str,
}

/// The 400 of an over-length `COTRTUP` member, on its field.
fn reject_upsert(errors: &ValidationErrors) -> ApiError {
    reject_invalid(errors, TranTypeUpsertField::of_member, OVER_LENGTH_MESSAGE)
}

/// The 400 of a `COTRTLI` row key or row description, or of the list's
/// `start_key`, that fails its length check, or a key its ASCII check, on no
/// field: the rows of the row actions name none (V-COTRTLIC-012 to
/// V-COTRTLIC-023), and no BMS field holds the COMMAREA start key.
fn reject_list_row(errors: &ValidationErrors) -> ApiError {
    reject_invalid(
        errors,
        |_: &str| None::<TranTypeListField>,
        OVER_LENGTH_MESSAGE,
    )
}

/// The answer of a successful write: the type as stored, the text the
/// program shows, and the `COTRTLIC` info line of an inline update.
///
/// The texts are the rules': `COTRTUPC` `Changes committed to database` or
/// `No change detected with respect to values fetched.` (V-COTRTUPC-006),
/// `COTRTLIC` `HIGHLIGHTED row was updated` or `No change detected with
/// respect to database values.` (V-COTRTLIC-012), on the `COTRTLIC` delete
/// `HIGHLIGHTED row deleted.Hit Enter to continue`, with no type, and on the
/// `COTRTUPC` delete `Delete successful.`, with the type as it was read
/// (P-COTRTUPC-004, P-COTRTUPC-023).
///
/// `info` is present only on `PUT` with `origin=list`. It is left out of every
/// other answer rather than sent as `null`, so the schema declares it an
/// optional, non-nullable string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeMessageResponse {
    /// The type as stored after the write; `null` after the `COTRTLIC`
    /// delete, and the deleted type as read after the `COTRTUPC` delete.
    // Required although nullable: the member is always serialized, as `null`
    // after the `COTRTLIC` delete, so the published schema says so.
    #[schema(required = true)]
    pub transaction_type: Option<TranTypeResponse>,
    /// The program's text, unchanged.
    pub message: String,
    /// The `COTRTLI` info line `INFOMSG` `X(45)`
    /// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L222], uncentred:
    /// `HIGHLIGHTED row was updated`, which `COTRTLIC` `9200-UPDATE-RECORD`
    /// sets once the inline update is stored (P-COTRTLIC-009). It comes with
    /// every `origin=list` update, beside the no-change `message` of a change
    /// of case alone (V-COTRTLIC-012). Absent on the `COTRTUPC` writes and on
    /// both deletes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub info: Option<String>,
}

/// The `COTRTUPC` F5 write, on `POST` and on `PUT` without `origin`
/// (P-COTRTUPC-007, P-COTRTUPC-009).
impl From<TranTypeSaved> for TranTypeMessageResponse {
    fn from(saved: TranTypeSaved) -> Self {
        Self {
            transaction_type: Some(TranTypeResponse::from(&saved.master)),
            message: saved.message,
            info: None,
        }
    }
}

/// `PUT` without `origin`: the write, or the type as fetched with the
/// no-change text and nothing written (V-COTRTUPC-006).
impl From<TranTypeUpsertOutcome> for TranTypeMessageResponse {
    fn from(outcome: TranTypeUpsertOutcome) -> Self {
        match outcome {
            TranTypeUpsertOutcome::Saved(saved) => Self::from(saved),
            TranTypeUpsertOutcome::Unchanged { master, message } => Self {
                transaction_type: Some(TranTypeResponse::from(&master)),
                message,
                info: None,
            },
        }
    }
}

/// `PUT` with `origin=list`: the type as stored, the rule's text, the
/// no-change text included when only the case changed, and the info line the
/// stored update sets (P-COTRTLIC-009, V-COTRTLIC-012).
impl From<TranTypeUpdated> for TranTypeMessageResponse {
    fn from(updated: TranTypeUpdated) -> Self {
        Self {
            transaction_type: Some(TranTypeResponse::from(&updated.transaction_type)),
            message: updated.message.to_owned(),
            info: Some(updated.info.to_owned()),
        }
    }
}

/// The `COTRTLIC` delete (P-COTRTLIC-007): no type, and the deleted-row text.
impl From<TranTypeDeleted> for TranTypeMessageResponse {
    fn from(deleted: TranTypeDeleted) -> Self {
        Self {
            transaction_type: None,
            message: deleted.message.to_owned(),
            info: None,
        }
    }
}

/// The `COTRTUPC` F4 delete, `DELETE` with `origin=form` (P-COTRTUPC-004):
/// the type as read before the delete, which the delete-done screen still
/// shows (P-COTRTUPC-023), and `Delete successful.`.
impl From<tran_type_update::TranTypeDeleted> for TranTypeMessageResponse {
    fn from(deleted: tran_type_update::TranTypeDeleted) -> Self {
        Self {
            transaction_type: Some(TranTypeResponse::from(&deleted.master)),
            message: deleted.message,
            info: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::TranTypeMaster;
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages::{
        self, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY, FIELD_TRAN_TYPE_CODE, FIELD_TRANSACTION_DESC,
        MUST_BE_NUMERIC, field_edit_message,
    };
    use carddemo_domain::numeric::TranTypeCode;
    use carddemo_domain::ports::Direction;
    use carddemo_domain::rules::ReturnMsg;
    use carddemo_domain::rules::tran_type_list::{
        self, TranTypeDeleted, TranTypeListInput, TranTypePage, TranTypeUpdated,
    };
    use carddemo_domain::rules::tran_type_update::{
        self, TranTypeSaved, TranTypeUpsertInput, TranTypeUpsertOutcome,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema};

    use super::{
        DESCRIPTION_MAX_LEN, TYPE_CODE_MAX_LEN, TranTypeCreateRequest, TranTypeDeleteOrigin,
        TranTypeDeleteQuery, TranTypeListField, TranTypeListPageQuery, TranTypeListQuery,
        TranTypeListUpdate, TranTypeMessageResponse, TranTypeOrigin, TranTypeResponse,
        TranTypeUpdateQuery, TranTypeUpdateRequest, TranTypeUpsertField, check_type_code_key,
    };
    use crate::dto::BmsField;
    use crate::error::ApiError;
    use crate::pagination::{Page, PageQuery};

    /// A value of `count` characters.
    fn chars(text: &str, count: u64) -> String {
        text.repeat(usize::try_from(count).expect("a small count"))
    }

    /// A value one character longer than `max`.
    fn over(max: u64) -> String {
        chars("9", max + 1)
    }

    /// The query struct a query string deserializes to, or the 400 the
    /// handler's query extractor answers.
    fn query<T: DeserializeOwned>(text: &str) -> Result<T, ApiError> {
        let uri: Uri = format!("/admin/transaction-types/01?{text}")
            .parse()
            .expect("a valid URI");
        Query::<T>::try_from_uri(&uri)
            .map(|Query(value)| value)
            .map_err(ApiError::from)
    }

    /// The error body of a failed check as JSON.
    fn body(result: Result<(), ApiError>) -> Value {
        let error = result.expect_err("the check fails");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        serde_json::to_value(error.body()).expect("serializes")
    }

    /// The over-length 400: kind `Invalid`, an empty message, and `fields`
    /// naming each BMS field (none when the rows name none).
    fn over_length(fields: &[&str]) -> Value {
        let mut expected = json!({"code": ErrorKind::Invalid.code(), "message": ""});
        if !fields.is_empty() {
            expected["fields"] = fields
                .iter()
                .map(|field| json!({"field": field, "message": ""}))
                .collect();
        }
        expected
    }

    fn master(code: &str, description: &str) -> TranTypeMaster {
        TranTypeMaster {
            tr_type: TranTypeCode::move_from(code),
            tr_description: description.to_owned(),
        }
    }

    #[test]
    fn origin_deserializes_from_the_query_string() {
        // (query text, origin)
        let cases = [
            ("origin=list", Some(TranTypeOrigin::List)),
            ("", None),
            ("other=1", None),
        ];
        for (text, origin) in cases {
            let parsed: TranTypeUpdateQuery = query(text).expect("deserializes");
            assert_eq!(parsed.origin, origin, "{text}");
        }
        for text in ["origin=LIST", "origin=", "origin=cttu"] {
            let error = query::<TranTypeUpdateQuery>(text).expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{text}");
        }
    }

    // E-23: `form` selects the COTRTUPC delete; absent, the COTRTLIC one.
    #[test]
    fn delete_origin_deserializes_from_the_query_string() {
        // (query text, origin)
        let cases = [
            ("origin=form", Some(TranTypeDeleteOrigin::Form)),
            ("", None),
            ("other=1", None),
        ];
        for (text, origin) in cases {
            let parsed: TranTypeDeleteQuery = query(text).expect("deserializes");
            assert_eq!(parsed.origin, origin, "{text}");
        }
        for text in ["origin=FORM", "origin=", "origin=list"] {
            let error = query::<TranTypeDeleteQuery>(text).expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{text}");
        }
    }

    #[test]
    fn field_enums_carry_the_bms_names_on_the_wire_and_in_rank_order() {
        let list = [
            (TranTypeListField::TrType, "TRTYPE", 1),
            (TranTypeListField::TrDesc, "TRDESC", 2),
        ];
        for (field, name, rank) in list {
            assert_eq!(field.as_str(), name);
            assert_eq!(BmsField::as_str(field), name);
            assert_eq!(BmsField::rank(field), rank);
            assert_eq!(
                serde_json::to_value(field).expect("serializes"),
                json!(name)
            );
            let back: TranTypeListField = serde_json::from_value(json!(name)).expect("parses");
            assert_eq!(back, field);
        }
        let upsert = [
            (TranTypeUpsertField::TrTypCd, "TRTYPCD", 1),
            (TranTypeUpsertField::TrTyDsc, "TRTYDSC", 5),
        ];
        for (field, name, rank) in upsert {
            assert_eq!(field.as_str(), name);
            assert_eq!(BmsField::as_str(field), name);
            assert_eq!(BmsField::rank(field), rank);
            assert_eq!(
                serde_json::to_value(field).expect("serializes"),
                json!(name)
            );
            let back: TranTypeUpsertField = serde_json::from_value(json!(name)).expect("parses");
            assert_eq!(back, field);
        }
        let list_schema = serde_json::to_value(TranTypeListField::schema()).expect("serializes");
        assert_eq!(list_schema["enum"], json!(["TRTYPE", "TRDESC"]));
        let upsert_schema =
            serde_json::to_value(TranTypeUpsertField::schema()).expect("serializes");
        assert_eq!(upsert_schema["enum"], json!(["TRTYPCD", "TRTYDSC"]));
    }

    #[test]
    fn list_field_names_the_field_the_rule_marks() {
        let mut msg = ReturnMsg::default();
        let filter = tran_type_list::edit_filters(Some("0A"), None, &mut msg);
        assert_eq!(filter.type_code, None);
        let (message, field) = msg.into_parts().expect("the type filter is rejected");
        assert_eq!(
            message,
            messages::TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT
        );
        assert_eq!(field, TranTypeListField::TrType.as_str());
    }

    #[test]
    fn values_within_their_bms_lengths_pass_every_check() {
        let code = chars("0", TYPE_CODE_MAX_LEN);
        // Lengths count characters, as the X(n) field holds one per byte.
        let descriptions = [
            chars("A", DESCRIPTION_MAX_LEN),
            chars("é", DESCRIPTION_MAX_LEN),
            String::new(),
        ];
        for description in descriptions {
            let filters = TranTypeListQuery {
                type_code: Some(code.clone()),
                description: Some(description.clone()),
            };
            assert!(filters.check().is_ok(), "{description:?}");
            let create = TranTypeCreateRequest {
                type_code: Some(code.clone()),
                description: Some(description.clone()),
            };
            assert!(create.check().is_ok(), "{description:?}");
            let update = TranTypeUpdateRequest {
                description: Some(description.clone()),
            };
            for origin in [None, Some(TranTypeOrigin::List)] {
                assert!(update.check(origin).is_ok(), "{description:?} {origin:?}");
            }
        }
        assert!(TranTypeListQuery::default().check().is_ok());
        assert!(TranTypeCreateRequest::default().check().is_ok());
        assert!(TranTypeUpdateRequest::default().check(None).is_ok());
    }

    #[test]
    fn an_over_length_type_code_names_the_field_of_its_map() {
        let filters = TranTypeListQuery {
            type_code: Some(over(TYPE_CODE_MAX_LEN)),
            description: None,
        };
        assert_eq!(body(filters.check()), over_length(&["TRTYPE"]));
        let create = TranTypeCreateRequest {
            type_code: Some(over(TYPE_CODE_MAX_LEN)),
            description: Some("PURCHASE".to_owned()),
        };
        assert_eq!(body(create.check()), over_length(&["TRTYPCD"]));
    }

    #[test]
    fn an_over_length_description_names_the_field_of_its_origin() {
        let long = Some(over(DESCRIPTION_MAX_LEN));
        let filters = TranTypeListQuery {
            type_code: None,
            description: long.clone(),
        };
        assert_eq!(body(filters.check()), over_length(&["TRDESC"]));
        let create = TranTypeCreateRequest {
            type_code: Some("01".to_owned()),
            description: long.clone(),
        };
        assert_eq!(body(create.check()), over_length(&["TRTYDSC"]));
        let update = TranTypeUpdateRequest { description: long };
        assert_eq!(body(update.check(None)), over_length(&["TRTYDSC"]));
        // The inline update's rows name no field (V-COTRTLIC-012 to -018).
        assert_eq!(
            body(update.check(Some(TranTypeOrigin::List))),
            over_length(&[])
        );
    }

    #[test]
    fn both_over_length_members_are_named_in_rank_order() {
        let filters = TranTypeListQuery {
            type_code: Some(over(TYPE_CODE_MAX_LEN)),
            description: Some(over(DESCRIPTION_MAX_LEN)),
        };
        let create = TranTypeCreateRequest {
            type_code: Some(over(TYPE_CODE_MAX_LEN)),
            description: Some(over(DESCRIPTION_MAX_LEN)),
        };
        // The validator's error map iterates in random order; the body must not.
        for _ in 0..16 {
            assert_eq!(body(filters.check()), over_length(&["TRTYPE", "TRDESC"]));
            assert_eq!(body(create.check()), over_length(&["TRTYPCD", "TRTYDSC"]));
        }
    }

    #[test]
    fn the_path_type_code_is_held_to_two_characters() {
        for code in ["01", "1", "", "0A"] {
            for origin in [None, Some(TranTypeOrigin::List)] {
                assert!(
                    check_type_code_key(code, origin).is_ok(),
                    "{code:?} {origin:?}"
                );
            }
        }
        assert_eq!(
            body(check_type_code_key("012", None)),
            over_length(&["TRTYPCD"])
        );
        assert_eq!(
            body(check_type_code_key("012", Some(TranTypeOrigin::List))),
            over_length(&[])
        );
    }

    /// The rule error of a `COTRTUPC` edit: kind, text and field.
    fn edit_error(error: &DomainError) -> (ErrorKind, &str, Vec<&str>) {
        (
            error.kind,
            error.message.as_str(),
            error.fields.iter().map(|field| field.field).collect(),
        )
    }

    #[test]
    fn non_ascii_text_is_refused_where_no_edit_reads_its_bytes() {
        let list = Some(TranTypeOrigin::List);
        // Two-, three- and four-byte characters, alone and beside ASCII.
        let codes = [
            "\u{e9}9",
            "9\u{e9}",
            "\u{e9}",
            "\u{80}",
            "\u{20ac}",
            "\u{1f600}",
        ];
        for code in codes {
            // COTRTLIC row actions (PUT origin=list, DELETE): no edit, so the
            // key is refused on no field.
            assert_eq!(
                body(check_type_code_key(code, list)),
                over_length(&[]),
                "{code:?}"
            );
            // COTRTUPC: the key reaches 1245-EDIT-NUM-REQD, which refuses it
            // with its audited text (V-COTRTUPC-002, V-COTRTUPC-013).
            assert!(check_type_code_key(code, None).is_ok(), "{code:?}");
            let create = TranTypeCreateRequest {
                type_code: Some(code.to_owned()),
                description: Some("PURCHASE".to_owned()),
            };
            assert!(create.check().is_ok(), "{code:?}");
            let error = tran_type_update::edit(&create.into_input(), None)
                .expect_err("the key edit refuses it");
            let numeric = field_edit_message(FIELD_TRAN_TYPE_CODE, MUST_BE_NUMERIC);
            assert_eq!(
                edit_error(&error),
                (ErrorKind::Invalid, numeric.as_str(), vec!["TRTYPCD"]),
                "{code:?}"
            );
        }
        // Blank, space, low-value and NUMVAL keys still reach the rules.
        for code in ["", "  ", "\0\0", "01", "+7", ".7", "\u{7f}"] {
            for origin in [None, list] {
                assert!(
                    check_type_code_key(code, origin).is_ok(),
                    "{code:?} {origin:?}"
                );
            }
        }
    }

    #[test]
    fn non_ascii_descriptions_reach_the_alphanumeric_edit_of_either_origin() {
        let list = Some(TranTypeOrigin::List);
        let descriptions = [
            "Caf\u{e9}".to_owned(),
            "Espa\u{f1}a".to_owned(),
            // 50 characters, 51 bytes: the 50-byte receive of COTRTLIC splits
            // the last one, which its rule keeps whole.
            format!("{}\u{e9}", chars("A", DESCRIPTION_MAX_LEN - 1)),
            chars("\u{e9}", DESCRIPTION_MAX_LEN),
            "\u{20ac}".to_owned(),
            "\u{1f600}".to_owned(),
        ];
        let alphanumeric =
            field_edit_message(FIELD_TRANSACTION_DESC, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY);
        for description in descriptions {
            let update = TranTypeUpdateRequest {
                description: Some(description.clone()),
            };
            // COTRTLIC: the row description reaches 1240-EDIT-ALPHANUM-REQD as
            // entered, which refuses it on no field (V-COTRTLIC-014, the
            // rule's own tests).
            assert!(update.check(list).is_ok(), "{description:?}");
            let listed = update.clone().into_list_update("01");
            assert_eq!(listed.description, description);
            // COTRTUPC: 1230-EDIT-ALPHANUM-REQD refuses it with its audited
            // text (V-COTRTUPC-008, V-COTRTUPC-018).
            assert!(update.check(None).is_ok(), "{description:?}");
            let input = update.into_upsert_input("01".to_owned());
            let error = tran_type_update::edit(&input, None).expect_err("the edit refuses it");
            assert_eq!(
                edit_error(&error),
                (ErrorKind::Invalid, alphanumeric.as_str(), vec!["TRTYDSC"]),
                "{description:?}"
            );
        }
        // Over the length, a non-ASCII description is refused as any other.
        let long = TranTypeUpdateRequest {
            description: Some(chars("\u{e9}", DESCRIPTION_MAX_LEN + 1)),
        };
        assert_eq!(body(long.check(list)), over_length(&[]));
        assert_eq!(body(long.check(None)), over_length(&["TRTYDSC"]));
        // Blank descriptions reach the must-be-supplied edits of both origins.
        for description in ["", "   ", "*", "\0", "PURCHASE", "\u{7f}"] {
            let update = TranTypeUpdateRequest {
                description: Some(description.to_owned()),
            };
            for origin in [None, list] {
                assert!(update.check(origin).is_ok(), "{description:?} {origin:?}");
            }
        }
        assert!(TranTypeUpdateRequest::default().check(list).is_ok());
    }

    #[test]
    fn the_list_filters_leave_the_description_to_the_search_and_the_code_to_its_edit() {
        // COTRTLIC 1230-EDIT-DESC moves the description filter into X(50) with
        // no content edit (P-COTRTLIC-020), and the rule keeps whole a
        // character the 50-byte cut splits, so every filter within the length
        // is searched as entered: (filter, the LIKE pattern of the count).
        let split = format!("{}\u{e9}", chars("A", DESCRIPTION_MAX_LEN - 1));
        let descriptions = [
            ("\u{e9}".to_owned(), Some("%\u{e9}%".to_owned())),
            ("\u{80}".to_owned(), Some("%\u{80}%".to_owned())),
            ("\u{20ac}".to_owned(), Some("%\u{20ac}%".to_owned())),
            ("\u{1f600}".to_owned(), Some("%\u{1f600}%".to_owned())),
            (" Caf\u{e9} ".to_owned(), Some("%Caf\u{e9}%".to_owned())),
            // 50 characters, 51 bytes: the last one is searched too.
            (split.clone(), Some(format!("%{split}%"))),
            (
                chars("A", DESCRIPTION_MAX_LEN),
                Some(format!("%{}%", chars("A", DESCRIPTION_MAX_LEN))),
            ),
            (String::new(), None),
            ("   ".to_owned(), None),
            ("%pur_".to_owned(), Some("%%pur_%".to_owned())),
            ("\u{7f}".to_owned(), Some("%\u{7f}%".to_owned())),
        ];
        for (description, pattern) in descriptions {
            let filters = TranTypeListQuery {
                type_code: None,
                description: Some(description.clone()),
            };
            assert!(filters.check().is_ok(), "{description:?}");
            let mut msg = ReturnMsg::default();
            let edited = tran_type_list::edit_filters(None, Some(&description), &mut msg);
            assert_eq!(edited.description, pattern, "{description:?}");
            assert!(msg.is_off(), "{description:?}");
        }
        // Over the length, a non-ASCII filter is refused as any other.
        let long = TranTypeListQuery {
            type_code: None,
            description: Some(chars("\u{e9}", DESCRIPTION_MAX_LEN + 1)),
        };
        assert_eq!(body(long.check()), over_length(&["TRDESC"]));
        // 1220-EDIT-TYPECD refuses every non-ASCII type code filter with its
        // audited text (V-COTRTLIC-001): a byte of 0x80 or more is not NUMERIC.
        for code in ["\u{e9}", "9\u{e9}", "\u{80}", "\u{20ac}", "\u{1f600}"] {
            let filters = TranTypeListQuery {
                type_code: Some(code.to_owned()),
                description: None,
            };
            assert!(filters.check().is_ok(), "{code:?}");
            let mut msg = ReturnMsg::default();
            let edited = tran_type_list::edit_filters(Some(code), None, &mut msg);
            assert_eq!(edited.type_code, None, "{code:?}");
            assert_eq!(
                msg.message(),
                Some(messages::TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT),
                "{code:?}"
            );
        }
    }

    #[test]
    fn list_input_keeps_the_filters_as_entered() {
        let filters: TranTypeListQuery =
            query("type_code=0A&description=%20purch%20").expect("deserializes");
        let input = filters.into_input(Some("03".to_owned()), Direction::Backward, 7);
        assert_eq!(
            input,
            TranTypeListInput {
                type_filter: Some("0A".to_owned()),
                desc_filter: Some(" purch ".to_owned()),
                start_key: Some(TranTypeCode::move_from("03")),
                direction: Direction::Backward,
                page_size: 7,
            }
        );
        let empty: TranTypeListQuery = query("type_code=&description=").expect("deserializes");
        assert_eq!(empty.type_code.as_deref(), Some(""));
        assert_eq!(empty.description.as_deref(), Some(""));
        let absent: TranTypeListQuery = query("").expect("deserializes");
        assert_eq!(absent.type_code, None);
        assert_eq!(absent.description, None);
    }

    #[test]
    fn the_type_code_filter_pattern_admits_exactly_what_edit_filters_passes() {
        // COTRTLIC 1220-EDIT-TYPECD, V-COTRTLIC-001: (filter, admitted by the
        // published pattern, the type code it selects).
        let cases = [
            ("", true, None),
            (" ", true, None),
            ("  ", true, None),
            ("\0\0", true, None),
            ("00", true, None),
            ("01", true, Some("01")),
            ("99", true, Some("99")),
            ("1", false, None),
            ("0", false, None),
            (" 1", false, None),
            ("1 ", false, None),
            ("\0", false, None),
            ("\0 ", false, None),
            (" \0", false, None),
            ("0A", false, None),
            ("é9", false, None),
        ];
        for (filter, admitted, selected) in cases {
            let filters = TranTypeListQuery {
                type_code: Some(filter.to_owned()),
                description: None,
            };
            assert!(filters.check().is_ok(), "{filter:?}");
            let mut msg = ReturnMsg::default();
            let edited = tran_type_list::edit_filters(Some(filter), None, &mut msg);
            assert_eq!(
                edited.type_code,
                selected.map(TranTypeCode::move_from),
                "{filter:?}"
            );
            let expected =
                (!admitted).then_some(messages::TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT);
            assert_eq!(msg.message(), expected, "{filter:?}");
        }
        let params =
            serde_json::to_value(TranTypeListQuery::into_params(|| None)).expect("serializes");
        assert_eq!(
            params[0]["schema"]["pattern"],
            json!(r"^(?: {0,2}|\x00{2}|[0-9]{2})$")
        );
    }

    #[test]
    fn list_input_reads_a_blank_start_key_as_none() {
        // (start key, code the browse starts at)
        let cases = [
            (None, None),
            (Some(""), None),
            (Some("  "), None),
            (Some("\0\0"), None),
            (Some("07"), Some("07")),
            (Some("7"), Some("7 ")),
        ];
        for (start_key, expected) in cases {
            let input = TranTypeListQuery::default().into_input(
                start_key.map(str::to_owned),
                Direction::Forward,
                tran_type_list::SCREEN_ROWS,
            );
            assert_eq!(
                input.start_key,
                expected.map(TranTypeCode::move_from),
                "{start_key:?}"
            );
            assert_eq!(input.page_size, tran_type_list::SCREEN_ROWS);
        }
    }

    #[test]
    fn the_list_start_key_is_held_to_two_ascii_characters() {
        // The COMMAREA key WS-CA-FIRST-TR-CODE or WS-CA-LAST-TR-CODE X(2): a
        // longer or non-ASCII key would start the browse at another code, and
        // no BMS field holds it, so the 400 names none. Percent-encoded keys
        // arrive decoded.
        let refused = [
            "start_key=%C3%A9",
            "start_key=%C3%A99",
            "start_key=012",
            "start_key=9%E2%82%AC",
            "start_key=%F0%9F%98%80",
            "start_key=%C3%A9%C3%A9%C3%A9&direction=backward",
        ];
        for text in refused {
            let page: TranTypeListPageQuery = query(text).expect("deserializes");
            assert_eq!(body(page.check()), over_length(&[]), "{text}");
        }
        // (query text, the resolved start key, direction and rows, the code
        // the browse starts at): unchanged from the shared query.
        let rows = tran_type_list::SCREEN_ROWS;
        let cases = [
            ("", None, Direction::Forward, rows, None),
            (
                "start_key=01",
                Some("01"),
                Direction::Forward,
                rows,
                Some("01"),
            ),
            ("start_key=", Some(""), Direction::Forward, rows, None),
            (
                "start_key=%20%20&direction=backward",
                Some("  "),
                Direction::Backward,
                rows,
                None,
            ),
            (
                "start_key=%00%00&page_size=100",
                Some("\0\0"),
                Direction::Forward,
                100,
                None,
            ),
            (
                "start_key=7&direction=forward&page_size=1",
                Some("7"),
                Direction::Forward,
                1,
                Some("7 "),
            ),
        ];
        for (text, start_key, direction, page_size, code) in cases {
            let page: TranTypeListPageQuery = query(text).expect("deserializes");
            assert!(page.check().is_ok(), "{text}");
            let shared: PageQuery = query(text).expect("deserializes");
            assert_eq!(PageQuery::from(page.clone()), shared, "{text}");
            let resolved = PageQuery::from(page).resolve(rows);
            assert_eq!(
                resolved,
                (start_key.map(str::to_owned), direction, page_size),
                "{text}"
            );
            let (start_key, direction, page_size) = resolved;
            let input = TranTypeListQuery::default().into_input(start_key, direction, page_size);
            assert_eq!(input.start_key, code.map(TranTypeCode::move_from), "{text}");
        }
        for text in [
            "direction=up",
            "page_size=0",
            "page_size=101",
            "page_size=x",
        ] {
            let error = query::<TranTypeListPageQuery>(text).expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{text}");
        }
    }

    #[test]
    fn write_inputs_carry_the_members_as_entered_and_absent_ones_empty() {
        let create: TranTypeCreateRequest =
            serde_json::from_value(json!({"type_code": " 5", "description": "gas station"}))
                .expect("parses");
        assert_eq!(
            create.into_input(),
            TranTypeUpsertInput {
                type_code: " 5".to_owned(),
                description: "gas station".to_owned(),
            }
        );
        let bare: TranTypeCreateRequest = serde_json::from_value(json!({})).expect("parses");
        assert_eq!(bare.into_input(), TranTypeUpsertInput::default());

        let update: TranTypeUpdateRequest =
            serde_json::from_value(json!({"description": "  purchase "})).expect("parses");
        assert_eq!(
            update.clone().into_upsert_input("01".to_owned()),
            TranTypeUpsertInput {
                type_code: "01".to_owned(),
                description: "  purchase ".to_owned(),
            }
        );
        assert_eq!(
            update.into_list_update("01"),
            TranTypeListUpdate {
                code: TranTypeCode::move_from("01"),
                description: "  purchase ".to_owned(),
            }
        );
        let empty: TranTypeUpdateRequest =
            serde_json::from_value(json!({"description": null})).expect("parses");
        assert_eq!(empty.description, None);
        assert_eq!(
            empty.into_list_update("02").description,
            String::new(),
            "an absent description is the empty field"
        );
    }

    #[test]
    fn response_keeps_the_stored_description_unchanged() {
        let cases = [
            ("01", "PURCHASE"),
            ("06", "REVERAL"),
            ("07", "  Adjustment  "),
            ("05", ""),
            ("04", "é"),
        ];
        for (code, description) in cases {
            let response = TranTypeResponse::from(&master(code, description));
            assert_eq!(
                response,
                TranTypeResponse {
                    type_code: code.to_owned(),
                    description: description.to_owned(),
                }
            );
        }
        let text = serde_json::to_value(TranTypeResponse::from(&master("1", "CREDIT")))
            .expect("serializes");
        assert_eq!(text, json!({"type_code": "1", "description": "CREDIT"}));
    }

    #[test]
    fn page_carries_trimmed_neighbour_keys_and_the_rule_text() {
        let page = TranTypePage {
            items: vec![master("01", "PURCHASE"), master("02", "PAYMENT")],
            next_key: Some(TranTypeCode::move_from("03")),
            prev_key: None,
            message: Some(messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC),
        };
        let wire = Page::<TranTypeResponse>::from(page);
        assert_eq!(
            serde_json::to_value(&wire).expect("serializes"),
            json!({
                "items": [
                    {"type_code": "01", "description": "PURCHASE"},
                    {"type_code": "02", "description": "PAYMENT"},
                ],
                "next_key": "03",
                "prev_key": null,
                "message": messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC,
            })
        );
        let empty = Page::<TranTypeResponse>::from(TranTypePage::default());
        assert_eq!(
            serde_json::to_value(&empty).expect("serializes"),
            json!({"items": [], "next_key": null, "prev_key": null})
        );
    }

    #[test]
    fn message_responses_carry_the_rule_texts() {
        let stored = master("01", "Purchase");
        let typed = Some(TranTypeResponse::from(&stored));
        let saved = TranTypeSaved {
            master: stored.clone(),
            inserted: false,
            message: messages::CHANGES_COMMITTED_TO_DATABASE.to_owned(),
        };
        // (response, type, text, info line)
        let cases = [
            (
                TranTypeMessageResponse::from(saved.clone()),
                typed.clone(),
                messages::CHANGES_COMMITTED_TO_DATABASE,
                None,
            ),
            (
                TranTypeMessageResponse::from(TranTypeUpsertOutcome::Saved(saved)),
                typed.clone(),
                messages::CHANGES_COMMITTED_TO_DATABASE,
                None,
            ),
            (
                TranTypeMessageResponse::from(TranTypeUpsertOutcome::Unchanged {
                    master: stored.clone(),
                    message: messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED.to_owned(),
                }),
                typed.clone(),
                messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
                None,
            ),
            // The inline update of a changed description (P-COTRTLIC-009).
            (
                TranTypeMessageResponse::from(TranTypeUpdated {
                    transaction_type: stored.clone(),
                    message: messages::HIGHLIGHTED_ROW_WAS_UPDATED,
                    info: messages::HIGHLIGHTED_ROW_WAS_UPDATED,
                }),
                typed.clone(),
                messages::HIGHLIGHTED_ROW_WAS_UPDATED,
                Some(messages::HIGHLIGHTED_ROW_WAS_UPDATED),
            ),
            // A change of case alone: stored, with the no-change text and the
            // info line (V-COTRTLIC-012).
            (
                TranTypeMessageResponse::from(TranTypeUpdated {
                    transaction_type: stored.clone(),
                    message: messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES,
                    info: messages::HIGHLIGHTED_ROW_WAS_UPDATED,
                }),
                typed.clone(),
                messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES,
                Some(messages::HIGHLIGHTED_ROW_WAS_UPDATED),
            ),
            (
                TranTypeMessageResponse::from(TranTypeDeleted {
                    type_code: stored.tr_type,
                    message: messages::HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
                }),
                None,
                messages::HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
                None,
            ),
            // The COTRTUPC F4 delete: the type as read, which the delete-done
            // screen shows (P-COTRTUPC-004, P-COTRTUPC-023).
            (
                TranTypeMessageResponse::from(tran_type_update::TranTypeDeleted {
                    master: stored,
                    message: messages::DELETE_SUCCESSFUL.to_owned(),
                }),
                typed,
                messages::DELETE_SUCCESSFUL,
                None,
            ),
        ];
        for (response, transaction_type, message, info) in cases {
            assert_eq!(response.transaction_type, transaction_type, "{message}");
            assert_eq!(response.message, message);
            assert_eq!(response.info.as_deref(), info, "{message}");
            // `info` is serialized only when present, never as `null`.
            let mut expected = json!({
                "transaction_type": transaction_type,
                "message": message,
            });
            if let Some(info) = info {
                expected["info"] = json!(info);
            }
            let wire = serde_json::to_value(&response).expect("serializes");
            assert_eq!(wire, expected, "{message}");
            let back: TranTypeMessageResponse = serde_json::from_value(wire).expect("deserializes");
            assert_eq!(back, response, "{message}");
        }
    }

    #[test]
    fn openapi_declares_the_bms_lengths_and_an_inline_origin() {
        let filters =
            serde_json::to_value(TranTypeListQuery::into_params(|| None)).expect("serializes");
        let names: Vec<&str> = filters
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["type_code", "description"]);
        assert_eq!(filters[0]["in"], json!("query"));
        assert_eq!(filters[0]["required"], json!(false));
        assert_eq!(filters[0]["schema"]["maxLength"], json!(TYPE_CODE_MAX_LEN));
        assert_eq!(
            filters[1]["schema"]["maxLength"],
            json!(DESCRIPTION_MAX_LEN)
        );
        // The description filter is searched as entered: no pattern.
        assert!(filters[1]["schema"].get("pattern").is_none(), "{filters}");

        // The page position: the two-character ASCII start key, then the
        // shared query's parameters (their schemas: the handler's tests).
        let page =
            serde_json::to_value(TranTypeListPageQuery::into_params(|| None)).expect("serializes");
        let names: Vec<&str> = page
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["start_key", "direction", "page_size"]);
        assert_eq!(page[0]["in"], json!("query"));
        assert_eq!(page[0]["required"], json!(false));
        assert_eq!(page[0]["schema"]["type"], json!("string"));
        assert_eq!(page[0]["schema"]["maxLength"], json!(TYPE_CODE_MAX_LEN));
        assert_eq!(page[0]["schema"]["pattern"], json!("^[\\x00-\\x7F]*$"));
        assert!(!page.to_string().contains("$ref"), "{page}");

        let origin =
            serde_json::to_value(TranTypeUpdateQuery::into_params(|| None)).expect("serializes");
        assert_eq!(origin[0]["name"], json!("origin"));
        assert_eq!(origin[0]["in"], json!("query"));
        assert_eq!(origin[0]["required"], json!(false));
        assert_eq!(origin[0]["schema"]["enum"], json!(["list"]));
        assert!(!origin.to_string().contains("$ref"), "{origin}");
        let delete_origin =
            serde_json::to_value(TranTypeDeleteQuery::into_params(|| None)).expect("serializes");
        assert_eq!(delete_origin[0]["name"], json!("origin"));
        assert_eq!(delete_origin[0]["in"], json!("query"));
        assert_eq!(delete_origin[0]["required"], json!(false));
        assert_eq!(delete_origin[0]["schema"]["enum"], json!(["form"]));
        assert!(
            !delete_origin.to_string().contains("$ref"),
            "{delete_origin}"
        );

        let create = serde_json::to_value(TranTypeCreateRequest::schema()).expect("serializes");
        assert_eq!(
            create["properties"]["type_code"]["maxLength"],
            json!(TYPE_CODE_MAX_LEN)
        );
        assert_eq!(
            create["properties"]["description"]["maxLength"],
            json!(DESCRIPTION_MAX_LEN)
        );
        let update = serde_json::to_value(TranTypeUpdateRequest::schema()).expect("serializes");
        assert_eq!(
            update["properties"]["description"]["maxLength"],
            json!(DESCRIPTION_MAX_LEN)
        );
        // Either origin's alphanumeric edit answers the content: no pattern.
        assert!(
            update["properties"]["description"].get("pattern").is_none(),
            "{update}"
        );
        assert!(update["properties"].get("type_code").is_none(), "{update}");

        let message = serde_json::to_value(TranTypeMessageResponse::schema()).expect("serializes");
        assert_eq!(message["required"], json!(["transaction_type", "message"]));
        // The `origin=list` info line: an optional, non-nullable string.
        assert_eq!(message["properties"]["info"]["type"], json!("string"));
        assert!(
            message["properties"]["info"].get("oneOf").is_none(),
            "{message}"
        );
        let response = serde_json::to_value(TranTypeResponse::schema()).expect("serializes");
        assert_eq!(response["required"], json!(["type_code", "description"]));
    }
}
