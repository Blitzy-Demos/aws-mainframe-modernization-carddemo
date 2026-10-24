//! COMEN01C, the main menu: the content of endpoint 2, `GET /api/v1/menus/main` (transaction
//! `CM00`, map `COMEN1A`; AAP 0.3.4.1, 0.3.5).
//!
//! This module ports the `server` rows of `modern/audit/online/COMEN01C.md`, the values the
//! endpoint's `MenuResponse` carries:
//!
//! | Rows | Paragraph | Implemented by |
//! |---|---|---|
//! | COMEN01C-004 | `MAIN-PARA` | [`main_menu`]: one menu for every user type, which it states as [`Menu::user_type`] |
//! | COMEN01C-011 | `PROCESS-ENTER-KEY` | [`INVALID_OPTION`] and [`ADMIN_ONLY`], carried by every [`Menu`] it builds |
//! | COMEN01C-013 | `PROCESS-ENTER-KEY` | [`Installation`]: the outcome of the `INQUIRE PROGRAM` check, installed for all 11 |
//! | COMEN01C-015, COMEN01C-017 | `PROCESS-ENTER-KEY` | [`unavailable_message`], over [`not_installed_message`] and [`coming_soon_message`] |
//! | COMEN01C-020 | `BUILD-MENU-OPTIONS` | [`MAIN_MENU_OPTIONS`], [`OPTION_COUNT`], [`MenuOption::name`], [`MenuOption::user_type`] and [`Access`] |
//! | COMEN01C-021 | `BUILD-MENU-OPTIONS` | [`MenuOption::display_text`] |
//!
//! COMEN01C-002, the 401 for a missing, malformed, wrongly signed or expired token, is the API's
//! `SignedOnUser` extractor (n5): nothing here sees a token. The `client` rows send no request
//! and belong to the `COMEN01` screen, which applies them with this content: the cleared message
//! line, the sign-on redirect, the first entry, F3, the invalid key, the ENTER option edit, the
//! range, admin-only and not-installed checks, every transfer, the header and the option rows
//! (COMEN01C-001, COMEN01C-003, COMEN01C-005 to COMEN01C-010, COMEN01C-012, COMEN01C-014,
//! COMEN01C-016, COMEN01C-018, COMEN01C-019, COMEN01C-022). The two halt candidates of the audit,
//! the out-of-range option subscript and the transfer without a COMMAREA, lie on those client
//! paths, so no path of this module is halted.
//!
//! # Flow
//!
//! `api::services::comen01c` calls [`main_menu`] with the user type of the verified token
//! (AAP 0.5.1) and the handler maps the [`Menu`] it returns; nothing is read or written. The menu
//! is a single resource, the fixed table of `COMEN02Y`, not a collection, so it takes no page size
//! (Bounded Pagination; COMEN01C-004). No option is refused on the server: whether a `U` user may
//! take an option is the screen's comparison of [`Menu::user_type`] with [`MenuOption::access`]
//! (COMEN01C-012).
//!
//! # Shared shape
//!
//! [`Menu`], [`MenuEntry`], [`MenuOption`], [`Access`] and [`Installation`] are also the admin
//! menu's (`coadm01c`, endpoint 3), so that one `MenuResponse` maps both menus. What differs
//! between the two programs is data: the option table, the messages a menu carries, the text an
//! option shows when it cannot be taken, and whether the menu states a user type.
//!
//! # Literals
//!
//! Every text is the program's literal byte for byte, trailing spaces included, and the two
//! composed texts follow their `STRING` statements exactly: the not-installed text keeps the name
//! up to its first two consecutive spaces, the coming-soon text only up to its first space, with
//! no space before `is` (Behaviour constraint). Nothing here parses request input.

use copybook::{PicU, PicX};

use super::cosgn00c::UserType;

/// `CDEMO-MENU-OPT-COUNT PIC 9(02) VALUE 11` (`COMEN02Y.cpy:21`): the number of options
/// `BUILD-MENU-OPTIONS` shows and the highest option the range edit accepts (COMEN01C-020).
pub const OPTION_COUNT: u8 = 11;

/// Width of an option's display text: `WS-MENU-OPT-TXT PIC X(40)` and the map's `OPTN001O` to
/// `OPTN012O PIC X(40)` (COMEN01C-021).
pub const OPTION_TEXT_WIDTH: usize = 40;

/// The invalid-option text: an option of `00`, above [`OPTION_COUNT`] or not numeric
/// (`COMEN01C.cbl:131`). 37 characters, carried by the main menu for the screen's range edit
/// (COMEN01C-011).
pub const INVALID_OPTION: &str = "Please enter a valid option number...";

/// The admin-only text: a `U` user takes an option whose user type is `'A'`
/// (`COMEN01C.cbl:140`). 33 characters, the last a space as the literal has it, carried by the
/// main menu for the screen's admin-only check (COMEN01C-011, COMEN01C-012).
pub const ADMIN_ONLY: &str = "No access - Admin Only option... ";

/// The first operand of both option `STRING`s, `'This option '` with its trailing space
/// (`COMEN01C.cbl:163,172`; COMEN01C-015, COMEN01C-017).
const THIS_OPTION: &[u8] = b"This option ";

/// The last operand of the not-installed `STRING`, with its leading space
/// (`COMEN01C.cbl:166`; COMEN01C-015).
const IS_NOT_INSTALLED: &[u8] = b" is not installed...";

/// The last operand of the coming-soon `STRING`, with no leading space
/// (`COMEN01C.cbl:175`; COMEN01C-017).
const IS_COMING_SOON: &[u8] = b"is coming soon ...";

/// The one program COMEN01C inquires before transferring to it (`COMEN01C.cbl:147`;
/// COMEN01C-013).
const INQUIRED_PROGRAM: [u8; 8] = *b"COPAUS0C";

/// The `CDEMO-MENU-OPT-USRTYPE` value the admin-only check compares (`COMEN01C.cbl:137`).
const ADMIN_USER_TYPE: u8 = b'A';

/// Who may take an option: the option's user-type byte, `CDEMO-MENU-OPT-USRTYPE PIC X(01)`, as
/// the screen's admin-only check reads it (COMEN01C-012, COMEN01C-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    /// Any signed-on user: a user-type byte other than `'A'`. All 11 main-menu options have
    /// `'U'` (COMEN01C-020).
    AnyUser,
    /// Refused to a `U` user with [`ADMIN_ONLY`]: the byte `'A'`, which no main-menu option has
    /// (COMEN01C-020). Every admin-menu option is this, its table having no user-type column
    /// (COADM01C-012).
    AdminOnly,
}

impl Access {
    /// The access a `CDEMO-MENU-OPT-USRTYPE` byte gives: [`Access::AdminOnly`] exactly for `'A'`,
    /// the only value `PROCESS-ENTER-KEY` tests, and [`Access::AnyUser`] for every other byte
    /// (COMEN01C-020).
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        if code == ADMIN_USER_TYPE {
            Self::AdminOnly
        } else {
            Self::AnyUser
        }
    }

    /// Whether the option is [`Access::AdminOnly`]: the response's `admin_only` flag
    /// (COMEN01C-020).
    #[must_use]
    pub const fn is_admin_only(self) -> bool {
        matches!(self, Self::AdminOnly)
    }
}

/// Whether an option's program can be taken, as `PROCESS-ENTER-KEY` decides before it transfers
/// (COMEN01C-013, COMEN01C-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Installation {
    /// The program is defined: the screen transfers to it. `COPAUS0C` answers the
    /// `INQUIRE PROGRAM` with `NORMAL`, because this build includes the authorization screens,
    /// and the other ten programs are transferred to without a test; so every main-menu option
    /// is this (COMEN01C-013).
    Installed,
    /// The inquired program is not defined: the screen shows the option's not-installed text
    /// and transfers nowhere (COMEN01C-016). No option of this build is.
    NotInstalled,
    /// The program name begins with `DUMMY`: the screen shows the option's coming-soon text as
    /// information and transfers nowhere. No program of the shipped table does (COMEN01C-017).
    ComingSoon,
}

impl Installation {
    /// Whether the option is [`Installation::Installed`]: the response's `installed` flag
    /// (COMEN01C-013).
    #[must_use]
    pub const fn is_installed(self) -> bool {
        matches!(self, Self::Installed)
    }
}

/// One menu option, an occurrence of `CDEMO-MENU-OPT` (`COMEN02Y.cpy:94-98`): its number, name,
/// program and user-type byte as declared, with the access that byte gives and the installation
/// its program has (COMEN01C-013, COMEN01C-020). An admin-menu option has no user-type byte,
/// its table having no such column (COADM01C-012).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenuOption {
    number: PicU<2>,
    name: PicX<35>,
    program: PicX<8>,
    user_type: Option<u8>,
    access: Access,
    installation: Installation,
}

impl MenuOption {
    /// The option with these values, for a menu's option table. `user_type` is the option's
    /// `CDEMO-MENU-OPT-USRTYPE` byte, `None` for a table without that column (COADM01C-012).
    #[must_use]
    pub const fn new(
        number: PicU<2>,
        name: PicX<35>,
        program: PicX<8>,
        user_type: Option<u8>,
        access: Access,
        installation: Installation,
    ) -> Self {
        Self {
            number,
            name,
            program,
            user_type,
            access,
            installation,
        }
    }

    /// `CDEMO-MENU-OPT-NUM PIC 9(02)`: the number the option input selects (COMEN01C-020).
    #[must_use]
    pub const fn number(&self) -> PicU<2> {
        self.number
    }

    /// `CDEMO-MENU-OPT-NAME PIC X(35)`: the 35-byte name, trailing spaces kept (COMEN01C-020).
    #[must_use]
    pub const fn name(&self) -> &PicX<35> {
        &self.name
    }

    /// `CDEMO-MENU-OPT-PGMNAME PIC X(08)`: the program the option transfers to, whose screen
    /// route the client maps (COMEN01C-020).
    #[must_use]
    pub const fn program(&self) -> &PicX<8> {
        &self.program
    }

    /// `CDEMO-MENU-OPT-USRTYPE PIC X(01)`: the option's user-type byte as declared, `'U'` for
    /// all 11 main-menu options (COMEN01C-020); `None` on the admin menu, whose `COADM02Y` table
    /// has no user-type column (COADM01C-012).
    #[must_use]
    pub const fn user_type(&self) -> Option<u8> {
        self.user_type
    }

    /// Who may take the option (COMEN01C-020).
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }

    /// Whether the option's program can be taken (COMEN01C-013).
    #[must_use]
    pub const fn installation(&self) -> Installation {
        self.installation
    }

    /// The option's row text, as `BUILD-MENU-OPTIONS` strings it into `WS-MENU-OPT-TXT` cleared
    /// to spaces: the two digits of the number, `'. '` and the full 35-byte name, all
    /// `DELIMITED BY SIZE`, then the one space left of the 40 bytes; for example
    /// `'01. Account View                        '` (`COMEN01C.cbl:267-272`; COMEN01C-021,
    /// COADM01C-006).
    #[must_use]
    pub fn display_text(&self) -> PicX<OPTION_TEXT_WIDTH> {
        let mut text = Vec::with_capacity(OPTION_TEXT_WIDTH);
        self.number.encode_into(&mut text);
        text.extend_from_slice(b". ");
        self.name.encode_into(&mut text);
        PicX::moved_from(&text)
    }
}

/// One row of a [`Menu`]: the option and the text the screen shows when the option cannot be
/// taken, which its program composes (COMEN01C-015, COMEN01C-017).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenuEntry {
    option: MenuOption,
    unavailable_message: Option<String>,
}

impl MenuEntry {
    /// The row of `option`, with the text its menu's program gives it when it cannot be taken.
    #[must_use]
    pub fn new(option: MenuOption, unavailable_message: Option<String>) -> Self {
        Self {
            option,
            unavailable_message,
        }
    }

    /// The option.
    #[must_use]
    pub const fn option(&self) -> &MenuOption {
        &self.option
    }

    /// The text the screen shows when the option is not [`Installation::Installed`], or `None`
    /// when the program composes none for it. On the main menu only option 11 has one, its
    /// not-installed text, which the screen never shows because the option is installed
    /// (COMEN01C-015, COMEN01C-017).
    #[must_use]
    pub fn unavailable_message(&self) -> Option<&str> {
        self.unavailable_message.as_deref()
    }
}

/// A menu as its endpoint returns it: the option rows in table order and the program's
/// rejection texts, which the screen shows on ENTER without a further request
/// (COMEN01C-004, COMEN01C-011, COMEN01C-020).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Menu {
    entries: Vec<MenuEntry>,
    user_type: Option<UserType>,
    invalid_option: &'static str,
    admin_only: Option<&'static str>,
}

impl Menu {
    /// The menu of these rows. `user_type` is the caller's verified user type when the menu's
    /// checks compare one, `None` otherwise; `admin_only` is the text the program shows when it
    /// refuses an admin-only option, `None` when it refuses none.
    #[must_use]
    pub fn new(
        entries: Vec<MenuEntry>,
        user_type: Option<UserType>,
        invalid_option: &'static str,
        admin_only: Option<&'static str>,
    ) -> Self {
        Self {
            entries,
            user_type,
            invalid_option,
            admin_only,
        }
    }

    /// The option rows, in table order (COMEN01C-020).
    #[must_use]
    pub fn entries(&self) -> &[MenuEntry] {
        &self.entries
    }

    /// The caller's user type, the verified token's `usr_type` claim that the screen's
    /// admin-only check compares; never taken from a request field (AAP 0.5.1; COMEN01C-004).
    /// The main menu always states it.
    #[must_use]
    pub const fn user_type(&self) -> Option<&UserType> {
        self.user_type.as_ref()
    }

    /// The text of an option the range edit refuses: [`INVALID_OPTION`] on the main menu
    /// (COMEN01C-011).
    #[must_use]
    pub const fn invalid_option(&self) -> &'static str {
        self.invalid_option
    }

    /// The text of an admin-only option refused to a `U` user: [`ADMIN_ONLY`] on the main menu
    /// (COMEN01C-011).
    #[must_use]
    pub const fn admin_only(&self) -> Option<&'static str> {
        self.admin_only
    }
}

/// The 11 options of `CDEMO-MENU-OPTIONS-DATA` in table order, transcribed byte for byte from
/// `COMEN02Y.cpy:25-90`: number, 35-byte name with its trailing spaces, program and user type
/// `'U'` for every option (COMEN01C-013, COMEN01C-020). Option 8 has the name of line 70: line
/// 69's `'Transaction Add (Admin Only)       '` is a comment. The table's 12th occurrence
/// (`OCCURS 12`, line 94) holds no values and is not an option.
pub const MAIN_MENU_OPTIONS: [MenuOption; 11] = [
    table_option(
        1,
        b"Account View                       ",
        *b"COACTVWC",
        b'U',
    ),
    table_option(
        2,
        b"Account Update                     ",
        *b"COACTUPC",
        b'U',
    ),
    table_option(
        3,
        b"Credit Card List                   ",
        *b"COCRDLIC",
        b'U',
    ),
    table_option(
        4,
        b"Credit Card View                   ",
        *b"COCRDSLC",
        b'U',
    ),
    table_option(
        5,
        b"Credit Card Update                 ",
        *b"COCRDUPC",
        b'U',
    ),
    table_option(
        6,
        b"Transaction List                   ",
        *b"COTRN00C",
        b'U',
    ),
    table_option(
        7,
        b"Transaction View                   ",
        *b"COTRN01C",
        b'U',
    ),
    table_option(
        8,
        b"Transaction Add                    ",
        *b"COTRN02C",
        b'U',
    ),
    table_option(
        9,
        b"Transaction Reports                ",
        *b"CORPT00C",
        b'U',
    ),
    table_option(
        10,
        b"Bill Payment                       ",
        *b"COBIL00C",
        b'U',
    ),
    table_option(
        11,
        b"Pending Authorization View         ",
        *b"COPAUS0C",
        b'U',
    ),
];

/// One `COMEN02Y` occurrence from its four `VALUE` clauses. Only constant items call it, so a
/// number that did not fit `PIC 9(02)` would stop the build, never a running program.
const fn table_option(number: u64, name: &[u8; 35], program: [u8; 8], user_type: u8) -> MenuOption {
    let Ok(number) = PicU::new(number) else {
        panic!("a COMEN02Y option number fits PIC 9(02)")
    };
    MenuOption::new(
        number,
        PicX::from_array(*name),
        PicX::from_array(program),
        Some(user_type),
        Access::from_code(user_type),
        installation_of(program),
    )
}

/// The installation `PROCESS-ENTER-KEY` finds for a program: coming soon for a name beginning
/// with `DUMMY` (COMEN01C-017), installed otherwise, `COPAUS0C` included, whose inquiry answers
/// `NORMAL` in this build (COMEN01C-013).
const fn installation_of(program: [u8; 8]) -> Installation {
    if is_dummy(program) {
        Installation::ComingSoon
    } else {
        Installation::Installed
    }
}

/// `CDEMO-MENU-OPT-PGMNAME(WS-OPTION)(1:5) = 'DUMMY'` (`COMEN01C.cbl:169`).
const fn is_dummy(program: [u8; 8]) -> bool {
    matches!(program, [b'D', b'U', b'M', b'M', b'Y', ..])
}

/// The text the main menu gives `option` for when it cannot be taken, by the branch of
/// `PROCESS-ENTER-KEY`'s `EVALUATE` its program takes: the [`not_installed_message`] of the one
/// inquired program, `COPAUS0C` (COMEN01C-015); the [`coming_soon_message`] of a program whose
/// name begins with `DUMMY` (COMEN01C-017); no text for any other program, to which the option
/// transfers without a test. Of the shipped table only option 11 has a text.
#[must_use]
pub fn unavailable_message(option: &MenuOption) -> Option<String> {
    let program = *option.program().as_bytes();
    if program == INQUIRED_PROGRAM {
        Some(not_installed_message(option))
    } else if is_dummy(program) {
        Some(coming_soon_message(option))
    } else {
        None
    }
}

/// The not-installed text, as `COMEN01C.cbl:163-167` strings it into the cleared
/// `WS-MESSAGE PIC X(80)`: `'This option '`, the name up to its first two consecutive spaces
/// (`DELIMITED BY '  '`, the whole 35 bytes when it has none) and `' is not installed...'`.
/// Option 11 gives `'This option Pending Authorization View is not installed...'`, 58
/// characters (COMEN01C-015). The text is at most 67 characters, so it never fills the 80-byte
/// field or the 78-byte message row.
#[must_use]
pub fn not_installed_message(option: &MenuOption) -> String {
    let name = option.name().as_bytes();
    let kept = name
        .windows(2)
        .position(|pair| pair == b"  ")
        .and_then(|end| name.get(..end))
        .unwrap_or(name);
    composed(kept, IS_NOT_INSTALLED)
}

/// The coming-soon text, as `COMEN01C.cbl:172-176` strings it into the cleared
/// `WS-MESSAGE PIC X(80)`: `'This option '`, the name up to its first space
/// (`DELIMITED BY SPACE`) and `'is coming soon ...'` with no space before `is`; a name
/// `'Credit Card List'` gives `'This option Creditis coming soon ...'` (COMEN01C-017). The text
/// is at most 65 characters.
#[must_use]
pub fn coming_soon_message(option: &MenuOption) -> String {
    let name = option.name().as_bytes();
    let kept = name
        .iter()
        .position(|&byte| byte == b' ')
        .and_then(|end| name.get(..end))
        .unwrap_or(name);
    composed(kept, IS_COMING_SOON)
}

/// `'This option '`, `name` and `tail`, in that order, as the receiving field holds them.
fn composed(name: &[u8], tail: &[u8]) -> String {
    let mut text = Vec::with_capacity(THIS_OPTION.len() + name.len() + tail.len());
    text.extend_from_slice(THIS_OPTION);
    text.extend_from_slice(name);
    text.extend_from_slice(tail);
    String::from_utf8_lossy(&text).into_owned()
}

/// Endpoint 2's content for a caller of `user_type`: the 11 [`MAIN_MENU_OPTIONS`] in table order,
/// each with its [`unavailable_message`], the texts [`INVALID_OPTION`] and [`ADMIN_ONLY`], and
/// the caller's user type. COMEN01C tests no user type on entry, so a `U`, an `A` or any other
/// user type gets the same options, flags and texts; only the stated user type differs
/// (COMEN01C-004, COMEN01C-011, COMEN01C-013, COMEN01C-015, COMEN01C-020).
#[must_use]
pub fn main_menu(user_type: &UserType) -> Menu {
    let entries = MAIN_MENU_OPTIONS
        .into_iter()
        .map(|option| {
            let message = unavailable_message(&option);
            MenuEntry::new(option, message)
        })
        .collect();
    Menu::new(
        entries,
        Some(user_type.clone()),
        INVALID_OPTION,
        Some(ADMIN_ONLY),
    )
}

#[cfg(test)]
mod tests {
    use copybook::PicX;

    use super::{
        ADMIN_ONLY, Access, INVALID_OPTION, Installation, MAIN_MENU_OPTIONS, Menu, MenuEntry,
        MenuOption, OPTION_COUNT, OPTION_TEXT_WIDTH, coming_soon_message, main_menu,
        not_installed_message, table_option, unavailable_message,
    };
    use crate::online::cosgn00c::UserType;

    /// `app/cpy/COMEN02Y.cpy`, read in place from the parity oracle.
    const COMEN02Y: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../app/cpy/COMEN02Y.cpy"
    ));

    /// The main menu's programs in table order (COMEN01C-020).
    const PROGRAMS: [&str; 11] = [
        "COACTVWC", "COACTUPC", "COCRDLIC", "COCRDSLC", "COCRDUPC", "COTRN00C", "COTRN01C",
        "COTRN02C", "CORPT00C", "COBIL00C", "COPAUS0C",
    ];

    type TestResult = Result<(), String>;

    /// The `VALUE` clauses of `CDEMO-MENU-OPTIONS-DATA`, comment lines (`*` in column 7)
    /// skipped: every option number, and every quoted literal in source order.
    fn copybook_values() -> (Vec<u64>, Vec<&'static str>) {
        let mut numbers = Vec::new();
        let mut literals = Vec::new();
        for line in COMEN02Y
            .lines()
            .filter(|line| line.chars().nth(6) != Some('*'))
        {
            if line.contains("FILLER") && line.contains("PIC 9(02) VALUE") {
                let value = line
                    .split("VALUE")
                    .nth(1)
                    .and_then(|value| value.trim().trim_end_matches('.').parse::<u64>().ok());
                numbers.extend(value);
            }
            literals.extend(line.split('\'').skip(1).step_by(2));
        }
        (numbers, literals)
    }

    /// An option outside the shipped table, built as the table builds its own.
    fn option_named(name: &[u8; 35], program: [u8; 8]) -> MenuOption {
        table_option(3, name, program, b'U')
    }

    fn text(value: &[u8]) -> String {
        String::from_utf8_lossy(value).into_owned()
    }

    // audit: COMEN01C-020
    #[test]
    fn table_is_the_copybook_byte_for_byte() -> TestResult {
        let (numbers, literals) = copybook_values();
        assert_eq!(numbers.len(), MAIN_MENU_OPTIONS.len());
        assert_eq!(literals.len(), 3 * MAIN_MENU_OPTIONS.len());
        let mut literals = literals.into_iter();
        for (option, number) in MAIN_MENU_OPTIONS.iter().zip(numbers) {
            let name = literals.next().ok_or("missing name")?;
            let program = literals.next().ok_or("missing program")?;
            let user_type = literals.next().ok_or("missing user type")?;
            assert_eq!(option.number().get(), number);
            assert_eq!(option.name().as_bytes().as_slice(), name.as_bytes());
            assert_eq!(option.program().as_bytes().as_slice(), program.as_bytes());
            assert_eq!(user_type, "U");
            assert_eq!(
                option.user_type().map(|code| vec![code]),
                Some(user_type.as_bytes().to_vec()),
                "user type of option {number}"
            );
            assert_eq!(option.access(), Access::AnyUser);
        }
        Ok(())
    }

    // audit: COMEN01C-020
    #[test]
    fn eleven_options_numbered_one_to_eleven_in_table_order() {
        assert_eq!(MAIN_MENU_OPTIONS.len(), usize::from(OPTION_COUNT));
        let numbers: Vec<u64> = MAIN_MENU_OPTIONS
            .iter()
            .map(|option| option.number().get())
            .collect();
        assert_eq!(numbers, (1..=11).collect::<Vec<u64>>());
        let programs: Vec<String> = MAIN_MENU_OPTIONS
            .iter()
            .map(|option| option.program().to_text())
            .collect();
        assert_eq!(programs, PROGRAMS);
        let eighth = MAIN_MENU_OPTIONS
            .get(7)
            .map(|option| option.name().to_text());
        assert_eq!(eighth.as_deref(), Some("Transaction Add"));
        assert!(
            MAIN_MENU_OPTIONS
                .iter()
                .all(|option| !option.access().is_admin_only() && option.user_type() == Some(b'U'))
        );
    }

    // audit: COMEN01C-020
    #[test]
    fn only_user_type_a_is_admin_only() {
        for (code, access) in [
            (b'A', Access::AdminOnly),
            (b'U', Access::AnyUser),
            (b'a', Access::AnyUser),
            (b' ', Access::AnyUser),
        ] {
            assert_eq!(Access::from_code(code), access, "user type {code}");
            assert_eq!(access.is_admin_only(), code == b'A');
            let option = table_option(
                5,
                b"Credit Card Update                 ",
                *b"COCRDUPC",
                code,
            );
            assert_eq!(option.user_type(), Some(code), "user type {code}");
            assert_eq!(option.access(), access, "user type {code}");
        }
    }

    // audit: COMEN01C-013
    // audit: COMEN01C-017
    #[test]
    fn every_shipped_option_is_installed_and_dummy_programs_come_soon() {
        assert!(
            MAIN_MENU_OPTIONS
                .iter()
                .all(|option| option.installation() == Installation::Installed)
        );
        let dummy = option_named(b"Credit Card List                   ", *b"DUMMY001");
        assert_eq!(dummy.installation(), Installation::ComingSoon);
        for (installation, installed) in [
            (Installation::Installed, true),
            (Installation::NotInstalled, false),
            (Installation::ComingSoon, false),
        ] {
            assert_eq!(installation.is_installed(), installed);
        }
    }

    // audit: COMEN01C-021
    #[test]
    fn display_text_is_number_point_space_and_the_full_name() {
        let texts: Vec<PicX<OPTION_TEXT_WIDTH>> = MAIN_MENU_OPTIONS
            .iter()
            .map(MenuOption::display_text)
            .collect();
        assert_eq!(
            texts.first(),
            Some(&PicX::from_array(
                *b"01. Account View                        "
            ))
        );
        assert_eq!(
            texts.last(),
            Some(&PicX::from_array(
                *b"11. Pending Authorization View          "
            ))
        );
        for (shown, option) in texts.iter().zip(&MAIN_MENU_OPTIONS) {
            let expected = format!(
                "{:02}. {} ",
                option.number().get(),
                text(option.name().as_bytes())
            );
            assert_eq!(text(shown.as_bytes()), expected);
        }
    }

    // audit: COMEN01C-011
    #[test]
    fn rejection_texts_are_the_literals() {
        assert_eq!(INVALID_OPTION, "Please enter a valid option number...");
        assert_eq!(INVALID_OPTION.len(), 37);
        assert_eq!(ADMIN_ONLY, "No access - Admin Only option... ");
        assert_eq!(ADMIN_ONLY.len(), 33);
    }

    // audit: COMEN01C-015
    #[test]
    fn not_installed_text_keeps_the_name_up_to_two_spaces() {
        for (name, expected) in [
            (
                b"Pending Authorization View         ",
                "This option Pending Authorization View is not installed...",
            ),
            (
                b"Credit Card List                   ",
                "This option Credit Card List is not installed...",
            ),
            (
                b"Account  View                      ",
                "This option Account is not installed...",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZ 12345678",
                "This option ABCDEFGHIJKLMNOPQRSTUVWXYZ 12345678 is not installed...",
            ),
        ] {
            assert_eq!(
                not_installed_message(&option_named(name, *b"COPAUS0C")),
                expected
            );
        }
        let eleventh = MAIN_MENU_OPTIONS.last().map(not_installed_message);
        assert_eq!(eleventh.as_ref().map(String::len), Some(58));
    }

    // audit: COMEN01C-017
    #[test]
    fn coming_soon_text_keeps_the_name_up_to_one_space() {
        for (name, expected) in [
            (
                b"Credit Card List                   ",
                "This option Creditis coming soon ...",
            ),
            (
                b" Leading space                     ",
                "This option is coming soon ...",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZ012345678",
                "This option ABCDEFGHIJKLMNOPQRSTUVWXYZ012345678is coming soon ...",
            ),
        ] {
            assert_eq!(
                coming_soon_message(&option_named(name, *b"DUMMY001")),
                expected
            );
        }
    }

    // audit: COMEN01C-015
    // audit: COMEN01C-017
    #[test]
    fn only_the_inquired_and_dummy_programs_have_a_text() {
        let messages: Vec<Option<String>> =
            MAIN_MENU_OPTIONS.iter().map(unavailable_message).collect();
        let (last, rest) = messages.split_last().unzip();
        assert_eq!(
            last.cloned().flatten().as_deref(),
            Some("This option Pending Authorization View is not installed...")
        );
        assert!(rest.is_some_and(|rest| rest.iter().all(Option::is_none)));
        let dummy = option_named(b"Credit Card List                   ", *b"DUMMY001");
        assert_eq!(
            unavailable_message(&dummy).as_deref(),
            Some("This option Creditis coming soon ...")
        );
    }

    // audit: COMEN01C-004
    // audit: COMEN01C-011
    // audit: COMEN01C-020
    #[test]
    fn every_user_type_gets_the_same_menu_and_its_own_type_stated() {
        let user = main_menu(&UserType::USER);
        assert_eq!(user.user_type(), Some(&UserType::USER));
        assert_eq!(user.invalid_option(), INVALID_OPTION);
        assert_eq!(user.admin_only(), Some(ADMIN_ONLY));
        let options: Vec<&MenuOption> = user.entries().iter().map(MenuEntry::option).collect();
        assert_eq!(options, MAIN_MENU_OPTIONS.iter().collect::<Vec<_>>());
        let texts: Vec<Option<&str>> = user
            .entries()
            .iter()
            .map(MenuEntry::unavailable_message)
            .collect();
        let expected: Vec<Option<String>> =
            MAIN_MENU_OPTIONS.iter().map(unavailable_message).collect();
        assert_eq!(
            texts,
            expected.iter().map(Option::as_deref).collect::<Vec<_>>()
        );
        for other in [
            UserType::ADMIN,
            UserType::from_code(PicX::from_array([b' '])),
        ] {
            let menu = main_menu(&other);
            assert_eq!(menu.user_type(), Some(&other));
            let restated = Menu::new(
                user.entries().to_vec(),
                Some(other.clone()),
                INVALID_OPTION,
                Some(ADMIN_ONLY),
            );
            assert_eq!(menu, restated);
        }
    }
}
