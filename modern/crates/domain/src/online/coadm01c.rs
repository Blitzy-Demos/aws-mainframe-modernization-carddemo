//! COADM01C, the admin menu: the content of endpoint 3, `GET /api/v1/menus/admin` (transaction
//! `CA00`, map `COADM1A`; AAP 0.3.4.1, 0.3.5).
//!
//! This module ports the `server` rows of `modern/audit/online/COADM01C.md`, the values the
//! endpoint's `MenuResponse` carries:
//!
//! | Rows | Paragraph | Implemented by |
//! |---|---|---|
//! | COADM01C-005 | `BUILD-MENU-OPTIONS` | [`ADMIN_MENU_OPTIONS`] and [`ADMIN_OPTION_COUNT`]: six options; the `OCCURS 9` has no more |
//! | COADM01C-006 to COADM01C-011 | `BUILD-MENU-OPTIONS` | one [`ADMIN_MENU_OPTIONS`] entry each, shown through [`MenuOption::display_text`] |
//! | COADM01C-012 | `BUILD-MENU-OPTIONS`, `PROCESS-ENTER-KEY` | [`Access::AdminOnly`] and no [`MenuOption::user_type`] on every option; no admin-only text |
//! | COADM01C-013 | `PROCESS-ENTER-KEY`, `PGMIDERR-ERR-PARA` | [`Installation::Installed`] on every option, from the program-name test |
//! | COADM01C-014 | `PROCESS-ENTER-KEY`, `PGMIDERR-ERR-PARA` | [`INVALID_OPTION`] and [`not_installed_message`], carried by [`admin_menu`] |
//!
//! COADM01C-001, the 401 without a valid token and the 403 for a `usr_type` other than `A`, is
//! the API's `AdminUser` extractor over the verified token (AAP 0.3.4.6, 0.5.1): COADM01C tests
//! no user type, so nothing here takes an identity, and building this menu grants nothing. The
//! `client` rows send no request and belong to the `COADM01` screen, which applies them with this
//! content: the sign-on redirect, the first entry, the cleared message line, the option edit and
//! its range check, the not-installed and `PGMIDERR` displays, every transfer, F3, the invalid
//! key and the header (COADM01C-002 to COADM01C-004, COADM01C-015 to COADM01C-022). The halt
//! candidates of the audit lie on those client paths, so no path of this module is halted.
//!
//! # Flow
//!
//! `api::services::coadm01c` calls [`admin_menu`] once the `AdminUser` gate has passed, and the
//! handler maps the [`Menu`] it returns through the main menu's `MenuResponse`; nothing is read
//! or written. The menu is a single resource, the fixed table of `COADM02Y`, not a collection,
//! so it takes no page size (Bounded Pagination; COADM01C-005). The response is the same for
//! every admin and every request (COADM01C-001).
//!
//! # Shared shape
//!
//! [`Menu`], [`MenuEntry`], [`MenuOption`], [`Access`] and [`Installation`] are the main menu's
//! (`comen01c`), so one `MenuResponse` maps both menus. What this program contributes is data:
//! the six options, the invalid-option text, the not-installed text, no admin-only text and no
//! stated user type.
//!
//! # Literals
//!
//! Every text is the program's literal byte for byte, trailing spaces included (Behaviour
//! constraint). The not-installed text is the `STRING` the program compiles: the option-name
//! operand between `'This option '` and `'is not installed ...'` is a comment
//! (`COADM01C.cbl:153-154`, `274-275`), so the text names no option, and the one space before
//! `is` is the space `'This option '` ends with. Nothing here parses request input.

use copybook::{PicU, PicX};

use super::comen01c::{Access, Installation, Menu, MenuEntry, MenuOption};

/// `CDEMO-ADMIN-OPT-COUNT PIC 9(02) VALUE 6` (`COADM02Y.cpy:22`): the number of options
/// `BUILD-MENU-OPTIONS` shows and the highest option the range edit accepts. The `VALUE 4` on
/// the line above is a comment from before the Db2 release (COADM01C-005).
pub const ADMIN_OPTION_COUNT: u8 = 6;

/// The invalid-option text: an option of `00`, above [`ADMIN_OPTION_COUNT`] or not numeric
/// (`COADM01C.cbl:135`). 37 characters, carried by the admin menu for the screen's range edit
/// (COADM01C-014, COADM01C-016).
pub const INVALID_OPTION: &str = "Please enter a valid option number...";

/// The first operand of both not-installed `STRING`s, `'This option '` with its trailing space
/// (`COADM01C.cbl:152`, `273`; COADM01C-014).
const THIS_OPTION: &str = "This option ";

/// The last operand of both not-installed `STRING`s, with no leading space
/// (`COADM01C.cbl:155`, `276`; COADM01C-014).
const IS_NOT_INSTALLED: &str = "is not installed ...";

/// The six options of `CDEMO-ADMIN-OPTIONS-DATA` in table order, transcribed byte for byte from
/// `COADM02Y.cpy:26-53`: number, 35-byte name with its trailing spaces, and program. The table
/// has no user-type column, so no option has a [`MenuOption::user_type`] and every option is
/// [`Access::AdminOnly`]: it is offered only on this menu, which only an `A` token reaches
/// (COADM01C-012). No program begins with `DUMMY` and every option's screen exists, so every
/// option is [`Installation::Installed`] (COADM01C-013). The `OCCURS 9` redefinition's
/// occurrences 7 to 9 hold no values and are never read, so they are not options (COADM01C-005
/// to COADM01C-011).
pub const ADMIN_MENU_OPTIONS: [MenuOption; 6] = [
    admin_option(1, b"User List (Security)               ", *b"COUSR00C"),
    admin_option(2, b"User Add (Security)                ", *b"COUSR01C"),
    admin_option(3, b"User Update (Security)             ", *b"COUSR02C"),
    admin_option(4, b"User Delete (Security)             ", *b"COUSR03C"),
    admin_option(5, b"Transaction Type List/Update (Db2) ", *b"COTRTLIC"),
    admin_option(6, b"Transaction Type Maintenance (Db2) ", *b"COTRTUPC"),
];

/// One `COADM02Y` occurrence from its three `VALUE` clauses. The table has no user-type column,
/// so the option has no user-type byte and is [`Access::AdminOnly`] (COADM01C-012). Only constant
/// items call it, so a number that did not fit `PIC 9(02)` would stop the build, never a running
/// program.
const fn admin_option(number: u64, name: &[u8; 35], program: [u8; 8]) -> MenuOption {
    let Ok(number) = PicU::new(number) else {
        panic!("a COADM02Y option number fits PIC 9(02)")
    };
    MenuOption::new(
        number,
        PicX::from_array(*name),
        PicX::from_array(program),
        None,
        Access::AdminOnly,
        installation_of(program),
    )
}

/// The installation `PROCESS-ENTER-KEY` finds for a program: not installed when the name begins
/// with `DUMMY`, the test at `COADM01C.cbl:141` that skips the transfer and shows the
/// not-installed text (COADM01C-018); installed otherwise. The other not-installed path, an
/// `XCTL` that raises `PGMIDERR`, needs a program the region does not define, and the
/// replacement defines every option's screen (COADM01C-013, COADM01C-019).
const fn installation_of(program: [u8; 8]) -> Installation {
    if matches!(program, [b'D', b'U', b'M', b'M', b'Y', ..]) {
        Installation::NotInstalled
    } else {
        Installation::Installed
    }
}

/// The not-installed text `option` shows, as `COADM01C.cbl:152-156` and `273-277` string it
/// into the cleared `WS-MESSAGE PIC X(80)`: `'This option '` then `'is not installed ...'`,
/// both `DELIMITED BY SIZE`, giving `'This option is not installed ...'`, 32 characters. The
/// option-name operand once strung between them is commented out, so every option, a `DUMMY`
/// program's included, gets the same text (COADM01C-014, COADM01C-018, COADM01C-019).
#[must_use]
pub fn not_installed_message(_option: &MenuOption) -> String {
    [THIS_OPTION, IS_NOT_INSTALLED].concat()
}

/// Endpoint 3's content: the six [`ADMIN_MENU_OPTIONS`] in table order, each carrying its
/// [`not_installed_message`] because both not-installed paths apply to every option the range
/// edit accepts, and the text [`INVALID_OPTION`]. It states no user type and no admin-only text:
/// COADM01C compares no user type, and the `AdminUser` gate has already admitted the caller, so
/// every admin gets the same menu (COADM01C-001, COADM01C-005, COADM01C-012 to COADM01C-014).
#[must_use]
pub fn admin_menu() -> Menu {
    let entries = ADMIN_MENU_OPTIONS
        .into_iter()
        .map(|option| {
            let message = not_installed_message(&option);
            MenuEntry::new(option, Some(message))
        })
        .collect();
    Menu::new(entries, None, INVALID_OPTION, None)
}

#[cfg(test)]
mod tests {
    use copybook::PicX;

    use super::{
        ADMIN_MENU_OPTIONS, ADMIN_OPTION_COUNT, Access, INVALID_OPTION, Installation, Menu,
        MenuEntry, MenuOption, admin_menu, admin_option, not_installed_message,
    };
    use crate::online::comen01c::OPTION_TEXT_WIDTH;

    /// `app/cpy/COADM02Y.cpy`, read in place from the parity oracle.
    const COADM02Y: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../app/cpy/COADM02Y.cpy"
    ));

    /// The admin menu's programs in table order (COADM01C-006 to COADM01C-011).
    const PROGRAMS: [&str; 6] = [
        "COUSR00C", "COUSR01C", "COUSR02C", "COUSR03C", "COTRTLIC", "COTRTUPC",
    ];

    /// The not-installed text every option carries (COADM01C-014).
    const NOT_INSTALLED: &str = "This option is not installed ...";

    type TestResult = Result<(), String>;

    /// The code area of a copybook line, columns 8 to 72: `COADM02Y` carries sequence numbers
    /// in columns 1 to 6 and 73 to 80. `None` for a comment line (`*` in column 7).
    fn code_area(line: &str) -> Option<&str> {
        if line.get(6..7)? == "*" {
            return None;
        }
        line.get(7..line.len().min(72))
    }

    /// The `VALUE` clauses of `CARDDEMO-ADMIN-MENU-OPTIONS`: the option count, every option
    /// number, and every quoted literal in source order.
    fn copybook_values() -> (Option<u8>, Vec<u64>, Vec<&'static str>) {
        let mut count = None;
        let mut numbers = Vec::new();
        let mut literals = Vec::new();
        for area in COADM02Y.lines().filter_map(code_area) {
            let value = area
                .split("VALUE")
                .nth(1)
                .map(|value| value.trim().trim_end_matches('.'));
            if area.contains("CDEMO-ADMIN-OPT-COUNT") {
                count = value.and_then(|value| value.parse::<u8>().ok());
            } else if area.contains("FILLER") && area.contains("PIC 9(02) VALUE") {
                numbers.extend(value.and_then(|value| value.parse::<u64>().ok()));
            }
            literals.extend(area.split('\'').skip(1).step_by(2));
        }
        (count, numbers, literals)
    }

    fn text(value: &[u8]) -> String {
        String::from_utf8_lossy(value).into_owned()
    }

    // audit: COADM01C-005
    // audit: COADM01C-006
    // audit: COADM01C-007
    // audit: COADM01C-008
    // audit: COADM01C-009
    // audit: COADM01C-010
    // audit: COADM01C-011
    #[test]
    fn table_is_the_copybook_byte_for_byte() -> TestResult {
        let (count, numbers, literals) = copybook_values();
        assert_eq!(count, Some(ADMIN_OPTION_COUNT));
        assert_eq!(numbers.len(), ADMIN_MENU_OPTIONS.len());
        assert_eq!(literals.len(), 2 * ADMIN_MENU_OPTIONS.len());
        let mut literals = literals.into_iter();
        for (option, number) in ADMIN_MENU_OPTIONS.iter().zip(numbers) {
            let name = literals.next().ok_or("missing name")?;
            let program = literals.next().ok_or("missing program")?;
            assert_eq!(option.number().get(), number);
            assert_eq!(option.name().as_bytes().as_slice(), name.as_bytes());
            assert_eq!(option.program().as_bytes().as_slice(), program.as_bytes());
        }
        Ok(())
    }

    // audit: COADM01C-005
    // audit: COADM01C-012
    // audit: COADM01C-013
    #[test]
    fn six_admin_only_installed_options_numbered_one_to_six() {
        assert_eq!(ADMIN_MENU_OPTIONS.len(), usize::from(ADMIN_OPTION_COUNT));
        let numbers: Vec<u64> = ADMIN_MENU_OPTIONS
            .iter()
            .map(|option| option.number().get())
            .collect();
        assert_eq!(numbers, (1..=6).collect::<Vec<u64>>());
        let programs: Vec<String> = ADMIN_MENU_OPTIONS
            .iter()
            .map(|option| option.program().to_text())
            .collect();
        assert_eq!(programs, PROGRAMS);
        for option in &ADMIN_MENU_OPTIONS {
            assert_eq!(option.user_type(), None);
            assert_eq!(option.access(), Access::AdminOnly);
            assert!(option.access().is_admin_only());
            assert_eq!(option.installation(), Installation::Installed);
            assert!(option.installation().is_installed());
        }
    }

    // audit: COADM01C-013
    #[test]
    fn only_a_dummy_program_is_not_installed() {
        for (program, installation) in [
            (*b"DUMMY001", Installation::NotInstalled),
            (*b"DUMMYXYZ", Installation::NotInstalled),
            (*b"DUMM0001", Installation::Installed),
            (*b"XDUMMY01", Installation::Installed),
            (*b"COUSR00C", Installation::Installed),
        ] {
            let option = admin_option(3, b"User Update (Security)             ", program);
            assert_eq!(option.installation(), installation, "{}", text(&program));
            assert_eq!(option.access(), Access::AdminOnly);
        }
    }

    // audit: COADM01C-006
    // audit: COADM01C-007
    // audit: COADM01C-008
    // audit: COADM01C-009
    // audit: COADM01C-010
    // audit: COADM01C-011
    #[test]
    fn display_text_is_number_point_space_and_the_full_name() {
        let texts: Vec<PicX<OPTION_TEXT_WIDTH>> = ADMIN_MENU_OPTIONS
            .iter()
            .map(MenuOption::display_text)
            .collect();
        assert_eq!(
            texts.first(),
            Some(&PicX::from_array(
                *b"01. User List (Security)                "
            ))
        );
        assert_eq!(
            texts.last(),
            Some(&PicX::from_array(
                *b"06. Transaction Type Maintenance (Db2)  "
            ))
        );
        for (shown, option) in texts.iter().zip(&ADMIN_MENU_OPTIONS) {
            let expected = format!(
                "{:02}. {} ",
                option.number().get(),
                text(option.name().as_bytes())
            );
            assert_eq!(text(shown.as_bytes()), expected);
        }
    }

    // audit: COADM01C-014
    #[test]
    fn option_texts_are_the_literals_and_name_no_option() {
        assert_eq!(INVALID_OPTION, "Please enter a valid option number...");
        assert_eq!(INVALID_OPTION.len(), 37);
        for option in &ADMIN_MENU_OPTIONS {
            assert_eq!(not_installed_message(option), NOT_INSTALLED);
        }
        let dummy = admin_option(5, b"Transaction Type List/Update (Db2) ", *b"DUMMY005");
        assert_eq!(not_installed_message(&dummy), NOT_INSTALLED);
        assert_eq!(NOT_INSTALLED.len(), 32);
    }

    // audit: COADM01C-005
    // audit: COADM01C-012
    // audit: COADM01C-013
    // audit: COADM01C-014
    #[test]
    fn admin_menu_carries_the_table_and_the_two_option_texts() {
        let menu = admin_menu();
        let options: Vec<&MenuOption> = menu.entries().iter().map(MenuEntry::option).collect();
        assert_eq!(options, ADMIN_MENU_OPTIONS.iter().collect::<Vec<_>>());
        assert!(
            menu.entries()
                .iter()
                .all(|entry| entry.unavailable_message() == Some(NOT_INSTALLED))
        );
        assert_eq!(menu.invalid_option(), INVALID_OPTION);
        assert_eq!(menu.admin_only(), None);
        assert_eq!(menu.user_type(), None);
        let restated = Menu::new(menu.entries().to_vec(), None, INVALID_OPTION, None);
        assert_eq!(menu, restated);
        assert_eq!(admin_menu(), menu);
    }
}
