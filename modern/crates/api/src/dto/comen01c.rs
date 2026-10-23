//! The menu contract: `MenuResponse` and `MenuOptionDto`, the body of endpoint 2,
//! `GET /api/v1/menus/main` (transaction `CM00`, COMEN01C), and of endpoint 3,
//! `GET /api/v1/menus/admin` (transaction `CA00`, COADM01C; AAP 0.3.4.1).
//!
//! Both programs build their content as a `domain::online::comen01c::Menu`, the main menu through
//! `comen01c::main_menu` and the admin menu through `coadm01c::admin_menu`, so this one
//! conversion maps both and `dto::coadm01c` declares no schema of its own. What differs between
//! the two responses is data the domain decides: 11 or 6 options, the admin-only flags, the
//! unavailable texts, whether each option states its user type, and whether an admin-only text
//! and the caller's user type are stated (COMEN01C-020, COADM01C-005, COADM01C-012).
//!
//! The ENTER option checks run on the screen from this response, without a further request, so
//! the response carries every text those checks show: the invalid-option and admin-only texts and
//! each option's unavailable text, all as the program moves them, trailing spaces included
//! (COMEN01C-011, COMEN01C-015, COADM01C-014). A menu is a single resource, a fixed table with no
//! stored rows, so neither endpoint takes a `page_size` (Bounded Pagination; COMEN01C-004,
//! COADM01C-005). The option number travels as its two digits of text, so no integer is
//! converted here (Checked Integer Narrowing).

use domain::online::comen01c as rules;
use serde::de::{self, Deserializer, MapAccess};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    Member, ObjectMembers, digits, field_if_present, full_text, read_object, skip_member, text,
};

/// One menu option: one row `BUILD-MENU-OPTIONS` fills, with what the screen needs to decide
/// ENTER on it (COMEN01C-013, COMEN01C-020, COMEN01C-021, COADM01C-006 to COADM01C-012).
// The schema name `MenuOptionDto` is the frontend's `components["schemas"]["MenuOptionDto"]`
// (Generated API Types Only); `MenuResponse` references it, which registers it.
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
#[schema(example = json!({
    "number": "01",
    "name": "Account View                       ",
    "text": "01. Account View                        ",
    "program": "COACTVWC",
    "user_type": "U",
    "admin_only": false,
    "installed": true
}))]
pub struct MenuOptionDto {
    /// `CDEMO-MENU-OPT-NUM PIC 9(02)`: the option's two digits with the leading zero, `01` to
    /// `11` on the main menu and `01` to `06` on the admin menu, the value the screen's
    /// right-justified, zero-filled option input is matched against (COMEN01C-020).
    pub number: String,
    /// `CDEMO-MENU-OPT-NAME PIC X(35)` on the main menu and `CDEMO-ADMIN-OPT-NAME PIC X(35)` on
    /// the admin menu: the option's name as declared, all 35 bytes, trailing spaces included;
    /// for example `Account View` followed by 23 spaces (COMEN01C-020, COADM01C-006 to
    /// COADM01C-011).
    pub name: String,
    /// The row text as `BUILD-MENU-OPTIONS` strings it into the 40-byte `OPTNnnn` field
    /// cleared to spaces: the two digits, `. `, the 35-byte name and one space, all 40 bytes,
    /// trailing spaces included; for example `01. Account View` followed by 24 spaces
    /// (COMEN01C-021, COADM01C-006 to COADM01C-011).
    pub text: String,
    /// `CDEMO-MENU-OPT-PGMNAME PIC X(08)` without trailing spaces, such as `COACTVWC`: the
    /// program the option transfers to, whose route the client's program table gives
    /// (COMEN01C-020).
    pub program: String,
    /// `CDEMO-MENU-OPT-USRTYPE PIC X(01)`: the option's user-type byte as declared, verbatim,
    /// `U` for all 11 main-menu options (COMEN01C-020). Absent on the admin menu, whose
    /// `COADM02Y` table has no user-type column (COADM01C-012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    /// Whether the option is refused to a `U` user: the main menu's `'A'` user-type column,
    /// which no shipped option has, and every admin-menu option, whose table has no user-type
    /// column and which only an `A` token reaches (COMEN01C-020, COADM01C-012).
    pub admin_only: bool,
    /// Whether the option's program can be taken, the outcome of the program's installation
    /// check: `true` for every shipped option (COMEN01C-013, COADM01C-013).
    pub installed: bool,
    /// The text the screen shows when the option cannot be taken, as the program composes it,
    /// or absent when the program composes none for the option. On the main menu only option
    /// 11 has one, its not-installed text; every admin-menu option carries
    /// `This option is not installed ...`, which the admin screen also shows when a transfer
    /// finds no screen (COMEN01C-015, COMEN01C-017, COADM01C-014).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_message: Option<String>,
}

/// Every member in declaration order, an absent `user_type` or `unavailable_message` left out.
impl Serialize for MenuOptionDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 6
            + usize::from(self.user_type.is_some())
            + usize::from(self.unavailable_message.is_some());
        let mut state = serializer.serialize_struct("MenuOptionDto", len)?;
        state.serialize_field("number", &self.number)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("text", &self.text)?;
        state.serialize_field("program", &self.program)?;
        field_if_present(&mut state, "user_type", self.user_type.as_ref())?;
        state.serialize_field("admin_only", &self.admin_only)?;
        state.serialize_field("installed", &self.installed)?;
        field_if_present(
            &mut state,
            "unavailable_message",
            self.unavailable_message.as_ref(),
        )?;
        state.end()
    }
}

/// A JSON object whose `user_type` and `unavailable_message` may be absent or `null` and whose
/// other members are required. An unknown member is skipped; a repeated one is an error.
impl<'de> Deserialize<'de> for MenuOptionDto {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        read_object::<D, MenuOptionDtoMembers>(deserializer)
    }
}

/// The members of a [`MenuOptionDto`] being read.
#[derive(Default)]
struct MenuOptionDtoMembers {
    number: Member<String>,
    name: Member<String>,
    text: Member<String>,
    program: Member<String>,
    user_type: Member<Option<String>>,
    admin_only: Member<bool>,
    installed: Member<bool>,
    unavailable_message: Member<Option<String>>,
}

impl ObjectMembers for MenuOptionDtoMembers {
    type Object = MenuOptionDto;
    const NAME: &'static str = "MenuOptionDto";
    const FIELDS: &'static [&'static str] = &[
        "number",
        "name",
        "text",
        "program",
        "user_type",
        "admin_only",
        "installed",
        "unavailable_message",
    ];

    fn read<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<(), A::Error> {
        match key {
            "number" => self.number.read(map, "number"),
            "name" => self.name.read(map, "name"),
            "text" => self.text.read(map, "text"),
            "program" => self.program.read(map, "program"),
            "user_type" => self.user_type.read(map, "user_type"),
            "admin_only" => self.admin_only.read(map, "admin_only"),
            "installed" => self.installed.read(map, "installed"),
            "unavailable_message" => self.unavailable_message.read(map, "unavailable_message"),
            _ => skip_member(map),
        }
    }

    fn finish<E: de::Error>(self) -> Result<MenuOptionDto, E> {
        Ok(MenuOptionDto {
            number: self.number.required("number")?,
            name: self.name.required("name")?,
            text: self.text.required("text")?,
            program: self.program.required("program")?,
            user_type: self.user_type.optional(),
            admin_only: self.admin_only.required("admin_only")?,
            installed: self.installed.required("installed")?,
            unavailable_message: self.unavailable_message.optional(),
        })
    }
}

/// The body of endpoints 2 and 3: the option table in table order and the program's rejection
/// texts (COMEN01C-004, COMEN01C-011, COMEN01C-020, COADM01C-005, COADM01C-014).
// The schema name `MenuResponse` is fixed by AAP 0.3.4.1 and 0.6.2 and is the only menu payload
// type of the frontend's `schema.d.ts` (Generated API Types Only); `COMEN01.tsx` and `COADM01.tsx`
// both use it.
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
#[schema(example = json!({
    "options": [{
        "number": "01",
        "name": "Account View                       ",
        "text": "01. Account View                        ",
        "program": "COACTVWC",
        "user_type": "U",
        "admin_only": false,
        "installed": true
    }],
    "user_type": "U",
    "invalid_option_message": "Please enter a valid option number...",
    "admin_only_message": "No access - Admin Only option... "
}))]
pub struct MenuResponse {
    /// The options in table order: 11 on the main menu, 6 on the admin menu (COMEN01C-020,
    /// COADM01C-005).
    pub options: Vec<MenuOptionDto>,
    /// The caller's user type exactly as the verified token's `usr_type` claim holds it, the
    /// value the main screen's admin-only check compares with `admin_only`; never taken from a
    /// request field (AAP 0.5.1; COMEN01C-004). Absent on the admin menu, whose program compares
    /// no user type (COADM01C-012).
    // The domain menu states the user type for COMEN01C-004, a `server` row of the audit; the
    // field carries it so that endpoint 2 can be proved to state it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    /// The text of an option the range edit refuses, `Please enter a valid option number...`,
    /// as the program moves it (COMEN01C-011, COADM01C-014).
    pub invalid_option_message: String,
    /// The text of an admin-only option refused to a `U` user,
    /// `No access - Admin Only option... ` with its trailing space; absent on the admin menu,
    /// which refuses no option by user type (COMEN01C-011, COADM01C-012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_only_message: Option<String>,
}

/// Every member in declaration order, an absent `user_type` or `admin_only_message` left out.
impl Serialize for MenuResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = 2
            + usize::from(self.user_type.is_some())
            + usize::from(self.admin_only_message.is_some());
        let mut state = serializer.serialize_struct("MenuResponse", len)?;
        state.serialize_field("options", &self.options)?;
        field_if_present(&mut state, "user_type", self.user_type.as_ref())?;
        state.serialize_field("invalid_option_message", &self.invalid_option_message)?;
        field_if_present(
            &mut state,
            "admin_only_message",
            self.admin_only_message.as_ref(),
        )?;
        state.end()
    }
}

/// A JSON object whose `user_type` and `admin_only_message` may be absent or `null` and whose
/// other members are required, each option an object of its own. An unknown member is skipped;
/// a repeated one is an error.
impl<'de> Deserialize<'de> for MenuResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        read_object::<D, MenuResponseMembers>(deserializer)
    }
}

/// The members of a [`MenuResponse`] being read.
#[derive(Default)]
struct MenuResponseMembers {
    options: Member<Vec<MenuOptionDto>>,
    user_type: Member<Option<String>>,
    invalid_option_message: Member<String>,
    admin_only_message: Member<Option<String>>,
}

impl ObjectMembers for MenuResponseMembers {
    type Object = MenuResponse;
    const NAME: &'static str = "MenuResponse";
    const FIELDS: &'static [&'static str] = &[
        "options",
        "user_type",
        "invalid_option_message",
        "admin_only_message",
    ];

    fn read<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<(), A::Error> {
        match key {
            "options" => self.options.read(map, "options"),
            "user_type" => self.user_type.read(map, "user_type"),
            "invalid_option_message" => self
                .invalid_option_message
                .read(map, "invalid_option_message"),
            "admin_only_message" => self.admin_only_message.read(map, "admin_only_message"),
            _ => skip_member(map),
        }
    }

    fn finish<E: de::Error>(self) -> Result<MenuResponse, E> {
        Ok(MenuResponse {
            options: self.options.required("options")?,
            user_type: self.user_type.optional(),
            invalid_option_message: self
                .invalid_option_message
                .required("invalid_option_message")?,
            admin_only_message: self.admin_only_message.optional(),
        })
    }
}

/// One option row: the domain option and the unavailable text its program gives it.
impl From<&rules::MenuEntry> for MenuOptionDto {
    fn from(entry: &rules::MenuEntry) -> Self {
        let option = entry.option();
        Self {
            number: digits(option.number()),
            name: full_text(option.name()),
            text: full_text(&option.display_text()),
            program: text(option.program()),
            // The declared byte verbatim, as `full_text` renders a `PIC X` field, rather than
            // a trimmed form, so a space would travel as a space.
            user_type: option
                .user_type()
                .map(|code| String::from_utf8_lossy(&[code]).into_owned()),
            admin_only: option.access().is_admin_only(),
            installed: option.installation().is_installed(),
            unavailable_message: entry.unavailable_message().map(str::to_owned),
        }
    }
}

/// The menu as either endpoint returns it, one option per domain entry in the same order and
/// every literal unchanged.
impl From<rules::Menu> for MenuResponse {
    fn from(menu: rules::Menu) -> Self {
        Self {
            options: menu.entries().iter().map(MenuOptionDto::from).collect(),
            // The claim's single byte verbatim, a space included, rather than the trimmed
            // `text` rendering, so the field equals the `usr_type` claim the token carries.
            user_type: menu
                .user_type()
                .map(|user_type| String::from_utf8_lossy(user_type.code().as_bytes()).into_owned()),
            invalid_option_message: menu.invalid_option().to_owned(),
            admin_only_message: menu.admin_only().map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::copybook::{PicU, PicX};
    use domain::online::coadm01c::admin_menu;
    use domain::online::comen01c::{
        ADMIN_ONLY, Access, INVALID_OPTION, Installation, Menu, MenuEntry, MenuOption, main_menu,
    };
    use domain::online::cosgn00c::UserType;
    use serde_json::json;
    use utoipa::ToSchema;

    use super::{MenuOptionDto, MenuResponse};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The main menu's row texts in table order, before the padding of the 40-byte field
    /// (COMEN01C-021).
    const MAIN_TEXTS: [&str; 11] = [
        "01. Account View",
        "02. Account Update",
        "03. Credit Card List",
        "04. Credit Card View",
        "05. Credit Card Update",
        "06. Transaction List",
        "07. Transaction View",
        "08. Transaction Add",
        "09. Transaction Reports",
        "10. Bill Payment",
        "11. Pending Authorization View",
    ];

    /// The main menu's option names in table order, before the padding of the 35-byte field
    /// (COMEN01C-020).
    const MAIN_NAMES: [&str; 11] = [
        "Account View",
        "Account Update",
        "Credit Card List",
        "Credit Card View",
        "Credit Card Update",
        "Transaction List",
        "Transaction View",
        "Transaction Add",
        "Transaction Reports",
        "Bill Payment",
        "Pending Authorization View",
    ];

    /// The main menu's programs in table order (COMEN01C-020).
    const MAIN_PROGRAMS: [&str; 11] = [
        "COACTVWC", "COACTUPC", "COCRDLIC", "COCRDSLC", "COCRDUPC", "COTRN00C", "COTRN01C",
        "COTRN02C", "CORPT00C", "COBIL00C", "COPAUS0C",
    ];

    /// The admin menu's row texts in table order, before the padding of the 40-byte field
    /// (COADM01C-006 to COADM01C-011).
    const ADMIN_TEXTS: [&str; 6] = [
        "01. User List (Security)",
        "02. User Add (Security)",
        "03. User Update (Security)",
        "04. User Delete (Security)",
        "05. Transaction Type List/Update (Db2)",
        "06. Transaction Type Maintenance (Db2)",
    ];

    /// The admin menu's option names in table order, before the padding of the 35-byte field
    /// (COADM01C-006 to COADM01C-011).
    const ADMIN_NAMES: [&str; 6] = [
        "User List (Security)",
        "User Add (Security)",
        "User Update (Security)",
        "User Delete (Security)",
        "Transaction Type List/Update (Db2)",
        "Transaction Type Maintenance (Db2)",
    ];

    /// The admin menu's programs in table order (COADM01C-006 to COADM01C-011).
    const ADMIN_PROGRAMS: [&str; 6] = [
        "COUSR00C", "COUSR01C", "COUSR02C", "COUSR03C", "COTRTLIC", "COTRTUPC",
    ];

    /// The option numbers `01` to `count`, two digits each.
    fn numbers(count: u8) -> Vec<String> {
        (1..=count).map(|number| format!("{number:02}")).collect()
    }

    /// `text` as the 40-byte option field holds it, space-padded on the right (COMEN01C-021,
    /// COADM01C-006 to COADM01C-011).
    fn padded(text: &str) -> String {
        format!("{text:<40}")
    }

    /// `name` as the 35-byte name field holds it, space-padded on the right (COMEN01C-020,
    /// COADM01C-006 to COADM01C-011).
    fn name_field(name: &str) -> String {
        format!("{name:<35}")
    }

    /// One text member of every option, in table order.
    fn field(options: &[MenuOptionDto], get: fn(&MenuOptionDto) -> &str) -> Vec<&str> {
        options.iter().map(get).collect()
    }

    // audit: COMEN01C-004
    // audit: COMEN01C-011
    // audit: COMEN01C-013
    // audit: COMEN01C-015
    // audit: COMEN01C-020
    // audit: COMEN01C-021
    #[test]
    fn main_menu_maps_to_eleven_options_in_table_order() {
        let response = MenuResponse::from(main_menu(&UserType::USER));
        assert_eq!(response.options.len(), 11);
        assert_eq!(
            response
                .options
                .iter()
                .map(|o| o.number.clone())
                .collect::<Vec<_>>(),
            numbers(11)
        );
        assert_eq!(
            field(&response.options, |o| o.name.as_str()),
            MAIN_NAMES.map(name_field)
        );
        assert!(response.options.iter().all(|o| o.name.len() == 35));
        assert_eq!(
            field(&response.options, |o| o.text.as_str()),
            MAIN_TEXTS.map(padded)
        );
        assert_eq!(
            field(&response.options, |o| o.program.as_str()),
            MAIN_PROGRAMS
        );
        assert!(
            response
                .options
                .iter()
                .all(|o| o.user_type.as_deref() == Some("U") && !o.admin_only && o.installed)
        );
        assert!(response.options.iter().all(|o| o.text.len() == 40));
        assert_eq!(
            response.options.last().map(|o| o.text.as_str()),
            Some("11. Pending Authorization View          ")
        );
        let (last, rest) = response.options.split_last().unzip();
        assert_eq!(
            last.and_then(|o| o.unavailable_message.as_deref()),
            Some("This option Pending Authorization View is not installed...")
        );
        assert!(rest.is_some_and(|rest| rest.iter().all(|o| o.unavailable_message.is_none())));
        assert_eq!(response.user_type.as_deref(), Some("U"));
        assert_eq!(response.invalid_option_message, INVALID_OPTION);
        assert_eq!(
            response.admin_only_message.as_deref(),
            Some("No access - Admin Only option... ")
        );
    }

    // audit: COMEN01C-004
    #[test]
    fn main_menu_states_the_user_type_as_the_claim_holds_it() {
        for (user_type, claim) in [
            (UserType::USER, "U"),
            (UserType::ADMIN, "A"),
            (UserType::from_code(PicX::from_array([b' '])), " "),
        ] {
            let response = MenuResponse::from(main_menu(&user_type));
            assert_eq!(response.user_type.as_deref(), Some(claim));
            let user = MenuResponse::from(main_menu(&UserType::USER));
            assert_eq!(response.options, user.options);
            assert_eq!(response.admin_only_message, user.admin_only_message);
        }
    }

    // audit: COADM01C-005
    // audit: COADM01C-006
    // audit: COADM01C-007
    // audit: COADM01C-008
    // audit: COADM01C-009
    // audit: COADM01C-010
    // audit: COADM01C-011
    // audit: COADM01C-012
    // audit: COADM01C-013
    // audit: COADM01C-014
    #[test]
    fn admin_menu_maps_to_six_admin_only_options() {
        let response = MenuResponse::from(admin_menu());
        assert_eq!(response.options.len(), 6);
        assert_eq!(
            response
                .options
                .iter()
                .map(|o| o.number.clone())
                .collect::<Vec<_>>(),
            numbers(6)
        );
        assert_eq!(
            field(&response.options, |o| o.name.as_str()),
            ADMIN_NAMES.map(name_field)
        );
        assert!(response.options.iter().all(|o| o.name.len() == 35));
        assert_eq!(
            field(&response.options, |o| o.text.as_str()),
            ADMIN_TEXTS.map(padded)
        );
        assert!(response.options.iter().all(|o| o.text.len() == 40));
        assert_eq!(
            field(&response.options, |o| o.program.as_str()),
            ADMIN_PROGRAMS
        );
        assert!(
            response
                .options
                .iter()
                .all(|o| o.user_type.is_none() && o.admin_only && o.installed)
        );
        assert!(response.options.iter().all(|o| {
            o.unavailable_message.as_deref() == Some("This option is not installed ...")
        }));
        assert_eq!(response.user_type, None);
        assert_eq!(response.invalid_option_message, INVALID_OPTION);
        assert_eq!(response.admin_only_message, None);
    }

    // audit: COMEN01C-013
    // audit: COMEN01C-017
    #[test]
    fn access_and_installation_map_to_the_two_flags() -> TestResult {
        let cases = [
            (Access::AnyUser, Installation::Installed, false, true),
            (Access::AdminOnly, Installation::Installed, true, true),
            (Access::AnyUser, Installation::NotInstalled, false, false),
            (Access::AdminOnly, Installation::ComingSoon, true, false),
        ];
        for (access, installation, admin_only, installed) in cases {
            let option = MenuOption::new(
                PicU::new(3)?,
                PicX::from_array(*b"Credit Card List                   "),
                PicX::from_array(*b"DUMMY001"),
                None,
                access,
                installation,
            );
            let message = Some("This option Creditis coming soon ...".to_owned());
            let menu = Menu::new(
                vec![MenuEntry::new(option, message)],
                None,
                INVALID_OPTION,
                None,
            );
            let response = MenuResponse::from(menu);
            let expected = MenuOptionDto {
                number: "03".to_owned(),
                name: name_field("Credit Card List"),
                text: padded("03. Credit Card List"),
                program: "DUMMY001".to_owned(),
                user_type: None,
                admin_only,
                installed,
                unavailable_message: Some("This option Creditis coming soon ...".to_owned()),
            };
            assert_eq!(response.options, [expected]);
        }
        Ok(())
    }

    // audit: COMEN01C-011
    // audit: COMEN01C-020
    // audit: COMEN01C-021
    // audit: COADM01C-006
    // audit: COADM01C-012
    #[test]
    fn wire_form_keeps_the_literals_and_omits_absent_texts() -> TestResult {
        let main = MenuResponse::from(main_menu(&UserType::USER));
        let value = serde_json::to_value(&main)?;
        assert_eq!(
            value.get("options").and_then(|options| options.get(0)),
            Some(&json!({
                "number": "01",
                "name": "Account View                       ",
                "text": "01. Account View                        ",
                "program": "COACTVWC",
                "user_type": "U",
                "admin_only": false,
                "installed": true
            }))
        );
        assert_eq!(value.get("user_type"), Some(&json!("U")));
        assert_eq!(
            value.get("invalid_option_message"),
            Some(&json!("Please enter a valid option number..."))
        );
        assert_eq!(value.get("admin_only_message"), Some(&json!(ADMIN_ONLY)));
        assert_eq!(serde_json::from_value::<MenuResponse>(value)?, main);

        let admin = MenuResponse::from(admin_menu());
        let value = serde_json::to_value(&admin)?;
        assert_eq!(
            value.get("options").and_then(|options| options.get(0)),
            Some(&json!({
                "number": "01",
                "name": "User List (Security)               ",
                "text": "01. User List (Security)                ",
                "program": "COUSR00C",
                "admin_only": true,
                "installed": true,
                "unavailable_message": "This option is not installed ..."
            }))
        );
        assert_eq!(value.get("user_type"), None);
        assert_eq!(value.get("admin_only_message"), None);
        assert_eq!(serde_json::from_value::<MenuResponse>(value)?, admin);
        Ok(())
    }

    #[test]
    fn schema_names_are_the_contract_names() -> TestResult {
        assert_eq!(MenuResponse::name(), "MenuResponse");
        assert_eq!(MenuOptionDto::name(), "MenuOptionDto");
        let schema = serde_json::to_value(<MenuResponse as utoipa::PartialSchema>::schema())?;
        let required = schema.get("required").cloned().unwrap_or_default();
        assert_eq!(required, json!(["options", "invalid_option_message"]));
        assert!(schema.pointer("/properties/page_size").is_none());
        let option = serde_json::to_value(<MenuOptionDto as utoipa::PartialSchema>::schema())?;
        assert_eq!(
            option.get("required"),
            Some(&json!([
                "number",
                "name",
                "text",
                "program",
                "admin_only",
                "installed"
            ]))
        );
        assert!(option.pointer("/properties/user_type").is_some());
        Ok(())
    }
}
