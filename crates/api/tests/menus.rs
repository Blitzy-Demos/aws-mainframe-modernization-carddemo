//! CM00 · COMEN01C · COMEN01 and CA00 · COADM01C · COADM01: the two menus,
//! proved against their replacements. Neither program becomes an endpoint.
//!
//! - The main menu becomes the sidebar section "Card management", built from
//!   the eleven `COMEN02Y` options [app/cpy/COMEN02Y.cpy:L21-90]; the admin
//!   menu becomes the section "Administration" and the `/admin` landing, built
//!   from the six `COADM02Y` options [app/cpy/COADM02Y.cpy:L22-53]. Both are
//!   declared once, in `web/src/nav/menu.ts`, which these tests parse: each
//!   option's number, text and program against the audit row that lists the
//!   options, its route and link name against its function-map row.
//! - Typing an option number and pressing ENTER becomes following a link
//!   (E-56), so the option-number, invalid-key, admin-only, not-installed and
//!   coming-soon texts have no web path, and the API publishes no menu
//!   operation, no `OPTION` input and no screen-header field.
//! - Sign-on lands a type `U` user on the main menu's route and a type `A`
//!   user on the admin menu's [app/cbl/COSGN00C.cbl:L230-240]. A task started
//!   with `EIBCALEN = 0` is a request without a token: 401 `Authentication
//!   required` on the operation behind every option. PF3 is the Log out menu
//!   item, `POST /auth/logout`.
//! - The COBOL keeps a `U` user out of the admin options only through that
//!   routing [app/cbl/COMEN01C.cbl:L136-143]. The server's 403 `Admin access
//!   required` on every admin option replaces it, a mandated deviation, and
//!   admits a `U` user to every main-menu option.
//!
//! Expected values are read at run time: option lists, message texts, header
//! fields and the 401 and 403 statuses from the parity audit; routes, link
//! names and the PF3 endpoint from the function map. Only the HTTP test needs
//! Docker, for one `postgres:16.15` container.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, Repo, Row, Table, cell_code_spans,
    code_span, ids_with_prefix, key_values,
};
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use common::{Resp, TestApp, assert_error, parity_audit};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The single navigation definition of the SPA, relative to the repository
/// root.
const MENU_TS: &str = "web/src/nav/menu.ts";

/// The parity-audit column holding a row's expected result.
const EXPECTED: &str = "Expected result";

/// The parity-audit column holding a row's input condition.
const CONDITION: &str = "Input condition";

/// The parity-audit column naming the fields a row reads or writes.
const FIELDS: &str = "Copybook field or PIC clause";

/// The `Endpoint` cell of a function-map row that only navigates.
const NAVIGATION: &str = "— (navigation)";

/// The cell of a function-map row that has no control or no endpoint.
const NONE: &str = "—";

/// The `Key or field` prefix of a map's PF3 row.
const PF3_KEY: &str = "PF3 ";

/// The `Key or field` prefix of a map's row for every other AID key.
const OTHER_KEYS: &str = "other AID keys";

/// The program-name prefix of an option `COMEN01C` and `COADM01C` announce
/// instead of transferring to it [app/cbl/COMEN01C.cbl:L169-176]
/// [app/cbl/COADM01C.cbl:L140-141].
const DUMMY: &str = "DUMMY";

/// The option input field of both maps, `OPTION` [app/bms/COMEN01.bms:L145]
/// [app/bms/COADM01.bms:L145], as an API field name would spell it.
const OPTION_INPUT: &str = "option";

/// The route prefix of the admin operations and screens.
const ADMIN_PREFIX: &str = "/admin";

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The `code` of a 403 for a valid token that is not an admin's.
const FORBIDDEN: &str = "FORBIDDEN";

/// The sign-on route.
const LOGIN_PATH: &str = "/auth/login";

/// The password of every `DUSRSECJ` user row [app/jcl/DUSRSECJ.jcl:L35-44].
const SEED_PASSWORD: &str = "PASSWORD";

/// Every `users` row in key order: the only file sign-on and sign-off read.
const USERS_TABLE: &str = "SELECT * FROM users ORDER BY user_id";

/// One of the two menus and what replaces it.
struct Menu {
    /// The COBOL program.
    program: &'static str,
    /// The BMS map whose function-map rows give the option links.
    bms: &'static str,
    /// The options copybook, also the prefix of each option's `menu.ts` id.
    copybook: &'static str,
    /// The `menu.ts` array that declares the options.
    array: &'static str,
    /// The `MenuSection` of the options, also their `MENU_SECTION_LABELS` key.
    section: &'static str,
    /// The sidebar group title.
    title: &'static str,
    /// The `MenuGroup` constant of the section.
    group: &'static str,
    /// The seeded user whose sign-on lands on this menu.
    user: &'static str,
    /// That user's type, `CDEMO-USER-TYPE` [app/cpy/COCOM01Y.cpy:L26-28].
    usr_type: &'static str,
}

/// The main menu, `COMEN01C`.
const MAIN: Menu = Menu {
    program: "COMEN01C",
    bms: "COMEN01",
    copybook: "COMEN02Y",
    array: "userMenu",
    section: "user",
    title: "Card management",
    group: "userGroup",
    user: "USER0001",
    usr_type: "U",
};

/// The admin menu, `COADM01C`.
const ADMIN: Menu = Menu {
    program: "COADM01C",
    bms: "COADM01",
    copybook: "COADM02Y",
    array: "adminMenu",
    section: "admin",
    title: "Administration",
    group: "adminGroup",
    user: "ADMIN001",
    usr_type: "A",
};

/// The parity rows of one menu program, each by the behaviour it proves. A
/// row that proves two behaviours fills two fields.
struct MenuRows {
    /// `MAIN-PARA`, `EIBCALEN = 0`: no session, so sign-on.
    no_session: &'static str,
    /// The first entry from sign-on: the menu built from the copybook.
    first_entry: &'static str,
    /// `BUILD-MENU-OPTIONS`: the options, their numbers, texts and programs.
    options: &'static str,
    /// `MAIN-PARA`, PF3: back to sign-on.
    pf3: &'static str,
    /// `MAIN-PARA`, any other AID key: the invalid-key text.
    other_keys: &'static str,
    /// `PROCESS-ENTER-KEY`: a number that is no option.
    numbers: &'static str,
    /// `PROCESS-ENTER-KEY`: who may choose an option.
    gate: &'static str,
    /// `PROCESS-ENTER-KEY` or `PGMIDERR-ERR-PARA`: an option program that is
    /// not installed.
    installed: &'static str,
    /// `PROCESS-ENTER-KEY`: an option program whose name starts `DUMMY`.
    dummy: &'static str,
    /// `PROCESS-ENTER-KEY`: a valid option, the `XCTL` its link replaces.
    links: &'static str,
    /// `RETURN-TO-SIGNON-SCREEN`: the `XCTL` without a COMMAREA.
    return_to_signon: &'static str,
    /// `SEND-MENU-SCREEN`: the section built from the options.
    section: &'static str,
    /// `RECEIVE-MENU-SCREEN`: the received map.
    receive: &'static str,
    /// `POPULATE-HEADER-INFO`: the screen header.
    header: &'static str,
}

impl MenuRows {
    /// Every row ID the fields name, with repeats.
    const fn ids(&self) -> [&'static str; 14] {
        [
            self.no_session,
            self.first_entry,
            self.options,
            self.pf3,
            self.other_keys,
            self.numbers,
            self.gate,
            self.installed,
            self.dummy,
            self.links,
            self.return_to_signon,
            self.section,
            self.receive,
            self.header,
        ]
    }
}

/// The `COMEN01C` rows.
const MAIN_ROWS: MenuRows = MenuRows {
    no_session: "P-COMEN01C-001",
    first_entry: "P-COMEN01C-002",
    options: "P-COMEN01C-003",
    pf3: "P-COMEN01C-004",
    other_keys: "P-COMEN01C-005",
    numbers: "P-COMEN01C-006",
    gate: "P-COMEN01C-007",
    installed: "P-COMEN01C-008",
    dummy: "P-COMEN01C-009",
    links: "P-COMEN01C-010",
    return_to_signon: "P-COMEN01C-011",
    section: "P-COMEN01C-012",
    receive: "P-COMEN01C-013",
    header: "P-COMEN01C-014",
};

/// The `COADM01C` rows. `PROCESS-ENTER-KEY` transfers to a valid option
/// unless its name starts `DUMMY` and tests no user type, one row; its first
/// entry is a case of `SEND-MENU-SCREEN`.
const ADMIN_ROWS: MenuRows = MenuRows {
    no_session: "P-COADM01C-001",
    first_entry: "P-COADM01C-008",
    options: "P-COADM01C-002",
    pf3: "P-COADM01C-003",
    other_keys: "P-COADM01C-004",
    numbers: "P-COADM01C-005",
    gate: "P-COADM01C-006",
    installed: "P-COADM01C-011",
    dummy: "P-COADM01C-006",
    links: "P-COADM01C-006",
    return_to_signon: "P-COADM01C-007",
    section: "P-COADM01C-008",
    receive: "P-COADM01C-009",
    header: "P-COADM01C-010",
};

/// The HTTP method of an operation behind a menu route.
#[derive(Clone, Copy)]
enum Verb {
    Get,
    Post,
}

/// The operation behind one menu route, from the screen domains of the route
/// table: the read the screen opens with, or its write. A write is sent with
/// the body `{}`: the token is checked before the body is read, so the gate's
/// answer does not depend on it, and any other answer writes nothing.
struct RouteRequest {
    route: &'static str,
    verb: Verb,
    path: &'static str,
}

impl RouteRequest {
    /// Sends the operation, with `token` as the bearer token when given.
    async fn send(&self, app: &TestApp, token: Option<&str>) -> Resp {
        match self.verb {
            Verb::Get => app.get(self.path, token).await,
            Verb::Post => app.post(self.path, token, &json!({})).await,
        }
    }
}

/// Every route a `COMEN02Y` or `COADM02Y` option links to, with its operation.
/// Account View and Account Update link to the dashboard account lookup on
/// `/` (E-61), whose operation is the CAVW read of account 1.
const ROUTE_REQUESTS: [RouteRequest; 11] = [
    RouteRequest {
        route: "/",
        verb: Verb::Get,
        path: "/accounts/00000000001",
    },
    RouteRequest {
        route: "/cards",
        verb: Verb::Get,
        path: "/cards",
    },
    RouteRequest {
        route: "/transactions",
        verb: Verb::Get,
        path: "/transactions",
    },
    RouteRequest {
        route: "/transactions/new",
        verb: Verb::Post,
        path: "/transactions",
    },
    RouteRequest {
        route: "/reports/transactions",
        verb: Verb::Get,
        path: "/reports/transactions?report_type=monthly&confirm=Y",
    },
    RouteRequest {
        route: "/payments",
        verb: Verb::Post,
        path: "/payments",
    },
    RouteRequest {
        route: "/authorizations",
        verb: Verb::Get,
        path: "/authorizations?account_id=00000000001",
    },
    RouteRequest {
        route: "/admin/users",
        verb: Verb::Get,
        path: "/admin/users",
    },
    RouteRequest {
        route: "/admin/users/new",
        verb: Verb::Post,
        path: "/admin/users",
    },
    RouteRequest {
        route: "/admin/transaction-types",
        verb: Verb::Get,
        path: "/admin/transaction-types",
    },
    RouteRequest {
        route: "/admin/transaction-types/new",
        verb: Verb::Post,
        path: "/admin/transaction-types",
    },
];

/// One `MenuEntry` of `menu.ts`; its icon is not read.
#[derive(Debug)]
struct Entry {
    id: String,
    label: String,
    program: Option<String>,
    route: String,
    section: String,
}

/// One option of an audit row's option list: `<number> <program> <name>`.
#[derive(Debug)]
struct AuditedOption {
    number: u8,
    program: String,
    name: String,
}

#[test]
fn the_main_menu_is_the_comen02y_options_of_the_audit() {
    let entries = assert_menu(&MAIN, &MAIN_ROWS);
    let parity = parity_audit();
    let id = MAIN_ROWS.gate;
    let admin_only = expected(&parity, id);
    for entry in &entries {
        assert!(
            !entry.route.starts_with(ADMIN_PREFIX),
            "{id}: {} links under {ADMIN_PREFIX}, which would make it an admin-only option: \
             {admin_only}",
            entry.id
        );
    }
}

#[test]
fn the_admin_menu_is_the_coadm02y_options_of_the_audit() {
    let entries = assert_menu(&ADMIN, &ADMIN_ROWS);
    let id = ADMIN_ROWS.gate;
    for entry in &entries {
        assert!(
            entry.route.starts_with(ADMIN_PREFIX),
            "{id}: {} links outside {ADMIN_PREFIX}, so the 403 gate that replaces the menu \
             routing does not cover it",
            entry.id
        );
    }
}

/// The checks both menus share; returns the menu's COBOL options of
/// `menu.ts`.
fn assert_menu(menu: &Menu, rows: &MenuRows) -> Vec<Entry> {
    let parity = parity_audit();
    let map = function_map();
    let source = menu_source();
    let entries = cobol_entries(&source, menu);
    assert_options(&parity, rows.options, menu, &entries);
    assert_numbers(&parity, rows, menu, &entries);
    assert_links(&parity, &map, rows, menu, &entries);
    assert_no_dummy(&parity, rows, &entries);
    assert_other_keys(&parity, &map, rows.other_keys, menu);
    assert_unshown(&parity, rows, &source);
    assert_section(&parity, rows.section, menu, &source);
    assert_visibility(&source, rows.section);
    entries
}

/// `BUILD-MENU-OPTIONS`: the options of `menu.ts` are those the audit row
/// lists, in order, each with its number in its id, its text as its label
/// and its program, all in the menu's section.
fn assert_options(parity: &Document, id: &str, menu: &Menu, entries: &[Entry]) {
    let options = audited_options(parity, id);
    let listed: Vec<(String, &str, Option<&str>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.id.clone(),
                entry.label.as_str(),
                entry.program.as_deref(),
            )
        })
        .collect();
    let audited: Vec<(String, &str, Option<&str>)> = options
        .iter()
        .map(|option| {
            (
                format!("{}-{:02}", menu.copybook, option.number),
                option.name.as_str(),
                Some(option.program.as_str()),
            )
        })
        .collect();
    assert_eq!(
        listed, audited,
        "{id}: the options of `{}` in {MENU_TS}, as (id, label, program)",
        menu.array
    );
    for entry in entries {
        assert_eq!(
            entry.section, menu.section,
            "{id}: {} is not in the `{}` section",
            entry.id, menu.section
        );
    }
    if let Some(usrtype) = value_of(expected(parity, id), "usrtype") {
        assert_eq!(
            usrtype,
            format!("{} for all {}", MAIN.usr_type, options.len()),
            "{id}: every option of the `{}` section is open to user type {}",
            menu.section,
            MAIN.usr_type
        );
    }
}

/// `PROCESS-ENTER-KEY`, a number that is no option: the `menu.ts` options are
/// numbered 1 to the option count the edit tests against, so every valid
/// number has a link and no link carries 0 or a higher number.
fn assert_numbers(parity: &Document, rows: &MenuRows, menu: &Menu, entries: &[Entry]) {
    let id = rows.numbers;
    let prefix = format!("{}-", menu.copybook);
    let numbers: Vec<u8> = entries
        .iter()
        .map(|entry| {
            entry
                .id
                .strip_prefix(&prefix)
                .and_then(|digits| digits.parse().ok())
                .unwrap_or_else(|| panic!("{id}: {} carries no option number", entry.id))
        })
        .collect();
    let valid: Vec<u8> = (1..).take(numbers.len()).collect();
    assert_eq!(
        numbers, valid,
        "{id}: the options of {MENU_TS} are numbered 1 to their count"
    );
    let condition = row_cell(parity, id, CONDITION);
    let above = format!("> {}", numbers.len());
    assert!(
        condition.contains(&above),
        "{id}: the edit rejects a number {above}, the option count: {condition}"
    );
}

/// A valid option: its link replaces the `XCTL` to its program. Each option's
/// function-map row names the route `menu.ts` gives it and a link named by
/// its label, and no endpoint: following the link is SPA navigation.
fn assert_links(
    parity: &Document,
    map: &Document,
    rows: &MenuRows,
    menu: &Menu,
    entries: &[Entry],
) {
    let id = rows.links;
    let action = required_value(expected(parity, id), "action", id);
    assert!(
        action.starts_with("XCTL"),
        "{id}: a valid option transfers to its program: {action}"
    );
    let options = audited_options(parity, rows.options);
    assert_eq!(entries.len(), options.len(), "{id}: one link per option");
    for (option, entry) in options.iter().zip(entries) {
        let number = option.number;
        let (table, row) = map_row(map, menu.bms, &format!("option {number},"));
        let control = cell(table, row, "UI control");
        assert_eq!(
            required_value(control, "route", id),
            entry.route,
            "{id}: the route of option {number}"
        );
        assert_eq!(
            required_value(control, "control", id),
            format!("link \"{}\"", entry.label),
            "{id}: the control of option {number}"
        );
        assert_eq!(
            cell(table, row, "Endpoint"),
            NAVIGATION,
            "{id}: option {number} sends no request of its own"
        );
        assert!(
            cell(table, row, "COBOL action").contains(rows.options),
            "{id}: the function-map row of option {number} is built from {}",
            rows.options
        );
    }
    assert_eq!(
        option_rows(map, menu.bms),
        options.len(),
        "{id}: every option row of {} in {FUNCTION_MAP_PATH} is a `menu.ts` option",
        menu.bms
    );
}

/// An option program whose name starts `DUMMY`: no audited option and no
/// `menu.ts` option names one.
fn assert_no_dummy(parity: &Document, rows: &MenuRows, entries: &[Entry]) {
    let id = rows.dummy;
    let (_, row) = audit_row(parity, id);
    assert!(
        row.cells.iter().any(|text| text.contains(DUMMY)),
        "{id} is the {DUMMY} test"
    );
    for option in audited_options(parity, rows.options) {
        assert!(
            !option.program.starts_with(DUMMY),
            "{id}: option {} names {}",
            option.number,
            option.program
        );
    }
    for entry in entries {
        let program = entry.program.as_deref().unwrap_or_default();
        assert!(
            !program.starts_with(DUMMY),
            "{id}: {} names {program}",
            entry.id
        );
    }
}

/// Any other AID key: its function-map row has no control and no endpoint
/// (E-56), and the invalid-key text it would send stays the catalog's
/// `CCDA-MSG-INVALID-KEY`, its `VALUE` literal byte for byte, which fits the
/// audited width of its `X(50)` field.
fn assert_other_keys(parity: &Document, map: &Document, id: &str, menu: &Menu) {
    let (table, row) = map_row(map, menu.bms, OTHER_KEYS);
    assert!(
        cell(table, row, "COBOL action").contains(id),
        "{id}: the function-map row of the other AID keys of {}",
        menu.bms
    );
    assert_eq!(
        cell(table, row, "UI control"),
        NONE,
        "{id}: no control replaces the other AID keys"
    );
    assert_eq!(
        cell(table, row, "Endpoint"),
        NONE,
        "{id}: no request carries an AID key"
    );
    let span = code_span(expected(parity, id))
        .unwrap_or_else(|| panic!("{id}: the expected result quotes no text"));
    assert_eq!(
        unquoted(&span.text),
        messages::CCDA_MSG_INVALID_KEY,
        "{id}: the invalid-key text"
    );
    let width = span
        .width
        .unwrap_or_else(|| panic!("{id}: the invalid-key text has no audited width"));
    assert!(
        messages::CCDA_MSG_INVALID_KEY.len() <= width,
        "{id}: the invalid-key text fits its {width}-byte field"
    );
}

/// The texts the program sends for an AID key, a number that is no option, an
/// admin-only option, a program that is not installed or a `DUMMY` program:
/// none of them appears in `menu.ts`, whose links offer none of those cases.
fn assert_unshown(parity: &Document, rows: &MenuRows, source: &str) {
    for id in [
        rows.other_keys,
        rows.numbers,
        rows.gate,
        rows.installed,
        rows.dummy,
    ] {
        for span in cell_code_spans(expected(parity, id)) {
            let text = unquoted(&span.text).trim();
            assert!(
                text.is_empty() || !source.contains(text),
                "{id}: {MENU_TS} shows the 3270 text {text:?}"
            );
        }
    }
}

/// `SEND-MENU-SCREEN`: the sidebar section titled for the menu is built from
/// the menu's options alone, and `menu.ts` imports only icons, so building
/// it sends no request.
fn assert_section(parity: &Document, id: &str, menu: &Menu, source: &str) {
    let replacement = required_value(expected(parity, id), "replacement", id);
    assert!(
        replacement.contains(menu.copybook) && replacement.contains("no request"),
        "{id}: the section is built from {} with no request: {replacement}",
        menu.copybook
    );
    let labels = statement(source, "export const MENU_SECTION_LABELS");
    let title = format!("{}: \"{}\"", menu.section, menu.title);
    assert!(
        labels.contains(&title),
        "{id}: the `{}` section is titled \"{}\": {labels}",
        menu.section,
        menu.title
    );
    let group = statement(source, &format!("const {}: MenuGroup = {{", menu.group));
    let built = format!(
        "const {group}: MenuGroup = {{ section: \"{section}\", label: MENU_SECTION_LABELS.{section}, \
         entries: {array}, }};",
        group = menu.group,
        section = menu.section,
        array = menu.array
    );
    assert_eq!(group, built, "{id}: the `{}` group", menu.section);
    let imports = imports(source);
    assert!(!imports.is_empty(), "{id}: {MENU_TS} imports its icons");
    for import in &imports {
        assert!(
            import.ends_with("from \"lucide-react\";"),
            "{id}: {MENU_TS} imports only icons: {import}"
        );
    }
    assert!(
        !source.contains("fetch("),
        "{id}: {MENU_TS} sends no request"
    );
}

/// Which sections a signed-on user sees: a type `U` user the main menu's, a
/// type `A` user both, the main menu's first.
fn assert_visibility(source: &str, id: &str) {
    assert_eq!(
        groups_shown(source, MAIN.usr_type),
        [MAIN.group],
        "{id}: the sections of user type {}",
        MAIN.usr_type
    );
    assert_eq!(
        groups_shown(source, ADMIN.usr_type),
        [MAIN.group, ADMIN.group],
        "{id}: the sections of user type {}",
        ADMIN.usr_type
    );
}

#[test]
fn the_api_publishes_no_menu_operation_option_input_or_screen_header() {
    let parity = parity_audit();
    let map = function_map();
    let text = carddemo_api::spec_json();
    let spec: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let paths: BTreeSet<&str> = spec["paths"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no paths object"))
        .keys()
        .map(String::as_str)
        .collect();
    let mut names = BTreeSet::new();
    api_field_names(&spec, &mut names);
    assert!(
        names.contains("landing"),
        "the walk reads the document's field names: {names:?}"
    );
    for (menu, rows) in [(&MAIN, &MAIN_ROWS), (&ADMIN, &ADMIN_ROWS)] {
        let screen = menu_route(&map, menu);
        let id = rows.receive;
        assert!(
            !paths.contains(screen.as_str()),
            "{id}: the menu route {screen} is an API operation"
        );
        for path in &paths {
            assert!(
                !path.split('/').any(|segment| segment == "menu"),
                "{id}: {path} is a menu operation"
            );
        }
        assert!(
            !names.contains(OPTION_INPUT),
            "{id}, {}: an API operation takes the `OPTION` input",
            rows.numbers
        );
        let id = rows.header;
        let header = header_fields(&parity, id);
        for name in &names {
            assert!(
                !header.contains(&normalized(name)),
                "{id}: the API field `{name}` carries the screen header"
            );
        }
        for title in titles(&parity, id) {
            assert!(
                !text.contains(&title),
                "{id}: the OpenAPI document carries the header title {title:?}"
            );
        }
    }
}

#[test]
fn sign_on_no_session_log_out_and_the_gate_replace_the_menu_programs() {
    let parity = parity_audit();
    let map = function_map();
    let source = menu_source();
    let main_entries = cobol_entries(&source, &MAIN);
    let main = requests_of(&main_entries);
    let admin = requests_of(&cobol_entries(&source, &ADMIN));
    let routed: BTreeSet<&str> = main.iter().chain(&admin).map(|req| req.route).collect();
    let tabled: BTreeSet<&str> = ROUTE_REQUESTS.iter().map(|req| req.route).collect();
    assert_eq!(routed, tabled, "ROUTE_REQUESTS is every menu route");
    let app = TestApp::start();
    let run = Run {
        app: &app,
        parity: &parity,
        map: &map,
        no_session: replacement_status(&parity, MAIN_ROWS.no_session),
    };
    block_on(async {
        let user = run.first_entry(&MAIN, &MAIN_ROWS).await;
        let admin_token = run.first_entry(&ADMIN, &ADMIN_ROWS).await;
        run.without_session(&MAIN_ROWS, &main).await;
        run.without_session(&ADMIN_ROWS, &admin).await;
        run.gate(&main, &admin, &user, &admin_token).await;
        run.installed(&main_entries, &admin, &user, &admin_token)
            .await;
        run.sign_off(&MAIN, &MAIN_ROWS, &user, main[0]).await;
        run.sign_off(&ADMIN, &ADMIN_ROWS, &admin_token, admin[0])
            .await;
    });
}

/// What the HTTP test shares: the app, both audit documents and the status of
/// a request without a session.
struct Run<'a> {
    app: &'a TestApp,
    parity: &'a Document,
    map: &'a Document,
    no_session: StatusCode,
}

impl Run<'_> {
    /// The first entry from sign-on: the menu's seeded user signs on, and the
    /// answer lands on the menu's route with the user's type and carries no
    /// screen-header field. Returns the user's token.
    async fn first_entry(&self, menu: &Menu, rows: &MenuRows) -> String {
        let id = rows.first_entry;
        let built = expected(self.parity, id);
        assert!(
            built.contains(menu.copybook),
            "{id}: the menu is built from {}: {built}",
            menu.copybook
        );
        let body = json!({ "user_id": menu.user, "password": SEED_PASSWORD });
        let (status, answer, raw) = self.app.post(LOGIN_PATH, None, &body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{id}: sign-on of {}: {raw}",
            menu.user
        );
        assert_eq!(
            answer["usr_type"].as_str(),
            Some(menu.usr_type),
            "{id}: the user type of {}: {raw}",
            menu.user
        );
        let landing = menu_route(self.map, menu);
        assert_eq!(
            answer["landing"].as_str(),
            Some(landing.as_str()),
            "{id}: the sign-on of {} lands on the {} route: {raw}",
            menu.user,
            menu.program
        );
        let header = header_fields(self.parity, rows.header);
        let keys = answer
            .as_object()
            .unwrap_or_else(|| panic!("{id}: the sign-on answer is no object: {raw}"));
        for key in keys.keys() {
            assert!(
                !header.contains(&normalized(key)),
                "{}: the sign-on answer field `{key}` carries the screen header: {raw}",
                rows.header
            );
        }
        answer["token"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: the sign-on answer has no string token: {raw}"))
            .to_owned()
    }

    /// `EIBCALEN = 0`: the operation behind every option, sent without a
    /// token, is the 401 `Authentication required` of a missing session.
    async fn without_session(&self, rows: &MenuRows, requests: &[&RouteRequest]) {
        let id = rows.no_session;
        assert_eq!(
            row_cell(self.parity, id, CONDITION),
            "EIBCALEN = 0",
            "{id}: the no-session case"
        );
        let action = required_value(expected(self.parity, id), "action", id);
        assert!(
            action.starts_with("XCTL COSGN00C"),
            "{id}: no session transfers to sign-on: {action}"
        );
        for request in requests {
            let resp = request.send(self.app, None).await;
            assert_eq!(
                resp.0, self.no_session,
                "{id}: {} ({}) without a token: {}",
                request.route, request.path, resp.2
            );
            assert_error(
                &resp,
                self.no_session,
                UNAUTHENTICATED,
                messages::AUTHENTICATION_REQUIRED,
                &[],
            );
        }
    }

    /// Who may choose an option: a type `U` user passes the gate of every
    /// main-menu option and is refused every admin option with the 403
    /// `Admin access required` that replaces the menu routing; a type `A`
    /// user passes the gate of every admin option, which tests no user type
    /// of its own.
    async fn gate(
        &self,
        main: &[&RouteRequest],
        admin: &[&RouteRequest],
        user: &str,
        admin_token: &str,
    ) {
        let refused = replacement_status(self.parity, MAIN_ROWS.gate);
        for request in main {
            let resp = request.send(self.app, Some(user)).await;
            assert_admitted(&resp, MAIN_ROWS.gate, MAIN.user, request);
        }
        for request in admin {
            let resp = request.send(self.app, Some(user)).await;
            assert_eq!(
                resp.0, refused,
                "{}: {} ({}) with the {} token: {}",
                MAIN_ROWS.gate, request.route, request.path, MAIN.user, resp.2
            );
            assert_error(
                &resp,
                refused,
                FORBIDDEN,
                messages::ADMIN_ACCESS_REQUIRED,
                &[],
            );
            let resp = request.send(self.app, Some(admin_token)).await;
            assert_admitted(&resp, ADMIN_ROWS.gate, ADMIN.user, request);
        }
    }

    /// An option program that is not installed: the screens behind the
    /// pending-authorization option and every admin option are always
    /// installed, so their reads answer 200.
    async fn installed(
        &self,
        main_entries: &[Entry],
        admin: &[&RouteRequest],
        user: &str,
        admin_token: &str,
    ) {
        let id = MAIN_ROWS.installed;
        let condition = row_cell(self.parity, id, CONDITION);
        let inquired: Vec<&Entry> = main_entries
            .iter()
            .filter(|entry| {
                entry
                    .program
                    .as_deref()
                    .is_some_and(|program| condition.contains(program))
            })
            .collect();
        let [entry] = inquired.as_slice() else {
            panic!("{id}: exactly one option is inquired about: {condition}");
        };
        let request = request_for(&entry.route);
        let (status, body, raw) = request.send(self.app, Some(user)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{id}: {} ({}) is installed: {raw}",
            request.route,
            request.path
        );
        assert!(
            body["items"].is_array(),
            "{id}: the list is answered: {raw}"
        );
        let id = ADMIN_ROWS.installed;
        for request in admin.iter().filter(|req| matches!(req.verb, Verb::Get)) {
            let (status, _, raw) = request.send(self.app, Some(admin_token)).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{id}: {} ({}) is installed: {raw}",
                request.route,
                request.path
            );
        }
    }

    /// PF3 and `RETURN-TO-SIGNON-SCREEN`: the Log out item's endpoint answers
    /// the thank-you text and reads no file, and the next request, sent
    /// without the discarded token, is the 401 of a missing session.
    async fn sign_off(&self, menu: &Menu, rows: &MenuRows, token: &str, next: &RouteRequest) {
        let id = rows.pf3;
        let (table, pf3) = map_row(self.map, menu.bms, PF3_KEY);
        assert!(
            cell(table, pf3, "COBOL action").contains(id),
            "{id}: the PF3 row of {}",
            menu.bms
        );
        let endpoint = cell(table, pf3, "Endpoint");
        let path = endpoint
            .strip_prefix("POST ")
            .unwrap_or_else(|| panic!("{id}: PF3 is a POST: {endpoint}"));
        let back = rows.return_to_signon;
        let returned = expected(self.parity, back);
        assert!(
            returned.contains(endpoint) && returned.contains("no file access"),
            "{back}: the return to sign-on is {endpoint} with no file access: {returned}"
        );
        let before = self.app.stack().fetch_text_rows(USERS_TABLE).await;
        let (status, body, raw) = self.app.post(path, Some(token), &json!({})).await;
        assert_eq!(status, StatusCode::OK, "{id}: {endpoint}: {raw}");
        assert_eq!(
            body["message"].as_str(),
            Some(messages::CCDA_MSG_THANK_YOU),
            "{id}: the answer of {endpoint}: {raw}"
        );
        let after = self.app.stack().fetch_text_rows(USERS_TABLE).await;
        assert_eq!(after, before, "{back}: {endpoint} changed `users`");
        let resp = next.send(self.app, None).await;
        assert_eq!(
            resp.0, self.no_session,
            "{back}: {} without the discarded token: {}",
            next.route, resp.2
        );
        assert_error(
            &resp,
            self.no_session,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    }
}

/// Asserts that `resp`, the answer to `request` with the token of `user`,
/// passed the gate: neither 401 nor 403.
fn assert_admitted(resp: &Resp, id: &str, user: &str, request: &RouteRequest) {
    assert!(
        resp.0 != StatusCode::UNAUTHORIZED && resp.0 != StatusCode::FORBIDDEN,
        "{id}: {} ({}) with the {user} token must pass the gate, got {}: {}",
        request.route,
        request.path,
        resp.0,
        resp.2
    );
}

#[test]
fn every_comen01c_and_coadm01c_row_is_proved_here() {
    let parity = parity_audit();
    for (menu, rows) in [(&MAIN, &MAIN_ROWS), (&ADMIN, &ADMIN_ROWS)] {
        let listed: BTreeSet<String> = rows.ids().iter().map(|id| (*id).to_owned()).collect();
        let prefix = format!("P-{}-", menu.program);
        let defined: BTreeSet<String> = ids_with_prefix(&parity, IdKind::Parity)
            .into_iter()
            .filter(|id| id.starts_with(&prefix))
            .collect();
        assert_eq!(
            listed, defined,
            "the rows of {} must be exactly its rows in {PARITY_AUDIT_PATH}",
            menu.program
        );
        for id in &listed {
            assert_eq!(row_cell(&parity, id, "Program"), menu.program, "{id}");
            assert_eq!(
                last_verdict(&parity, id),
                Some("pass"),
                "{id} must have passed its review before a test relies on it"
            );
        }
    }
}

/// The parsed function map.
///
/// # Panics
///
/// Panics, naming the path, if the document cannot be read.
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The text of [`MENU_TS`].
fn menu_source() -> String {
    let path = fixtures::repo_root().join(MENU_TS);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()))
}

/// The data-table row `id` of `doc`, with its table.
fn audit_row<'a>(doc: &'a Document, id: &str) -> (&'a Table, &'a Row) {
    doc.find_row(id)
        .unwrap_or_else(|| panic!("no data table defines the row {id}"))
}

/// The `column` cell of `row`.
fn cell<'a>(table: &'a Table, row: &'a Row, column: &str) -> &'a str {
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("the row at line {} has no `{column}` cell", row.line))
}

/// The `column` cell of the row `id` of `doc`.
fn row_cell<'a>(doc: &'a Document, id: &str, column: &str) -> &'a str {
    let (table, row) = audit_row(doc, id);
    cell(table, row, column)
}

/// The expected result of the parity row `id`.
fn expected<'a>(parity: &'a Document, id: &str) -> &'a str {
    row_cell(parity, id, EXPECTED)
}

/// The value of the first `key=value` pair of `text` named `key`.
fn value_of(text: &str, key: &str) -> Option<String> {
    key_values(text)
        .into_iter()
        .find(|pair| pair.key == key)
        .map(|pair| pair.value)
}

/// As [`value_of`], panicking, under `id`, when `text` has no such pair.
fn required_value(text: &str, key: &str, id: &str) -> String {
    value_of(text, key).unwrap_or_else(|| panic!("{id}: no `{key}=` in {text}"))
}

/// A code span's text without the double quotes the audit writes around a
/// COBOL literal.
fn unquoted(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(text)
}

/// The option list of the parity row `id`: each `<number> <program> <name>`
/// part of its expected result, checked against its `options=` count.
fn audited_options(parity: &Document, id: &str) -> Vec<AuditedOption> {
    let text = expected(parity, id);
    let count: usize = required_value(text, "options", id)
        .parse()
        .unwrap_or_else(|error| panic!("{id}: `options=` is no count: {error}"));
    let options: Vec<AuditedOption> = text
        .split(';')
        .filter_map(|part| audited_option(part.trim()))
        .collect();
    assert_eq!(
        options.len(),
        count,
        "{id}: options={count} lists another number of options: {text}"
    );
    options
}

/// One `<number> <program> <name>` part of an option list, or `None` for any
/// other part.
fn audited_option(part: &str) -> Option<AuditedOption> {
    let (number, rest) = part.split_once(' ')?;
    let number: u8 = number.parse().ok()?;
    let (program, name) = rest.split_once(' ')?;
    let named = program.len() == 8
        && program
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    named.then(|| AuditedOption {
        number,
        program: program.to_owned(),
        name: name.trim().to_owned(),
    })
}

/// The status a parity row's `replacement` names: the digits after
/// `replacement=` or `replacement:`.
fn replacement_status(parity: &Document, id: &str) -> StatusCode {
    const KEY: &str = "replacement";
    let text = expected(parity, id);
    let at = text
        .find(KEY)
        .unwrap_or_else(|| panic!("{id} names no {KEY}: {text}"));
    let rest = text[at + KEY.len()..].trim_start_matches(['=', ':', ' ']);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let code: u16 = digits
        .parse()
        .unwrap_or_else(|error| panic!("{id}: the {KEY} names no status: {error}: {text}"));
    StatusCode::from_u16(code).unwrap_or_else(|error| panic!("{id}: status {code}: {error}"))
}

/// The screen-header fields of the parity row `id`, as API field names would
/// spell them: each map output field of its fields cell without its `O`
/// suffix, lower case (`TITLE01O` is `title01`).
fn header_fields(parity: &Document, id: &str) -> BTreeSet<String> {
    let text = row_cell(parity, id, FIELDS);
    let fields: BTreeSet<String> = text
        .split([',', ';'])
        .filter_map(|part| part.split_whitespace().next())
        .map(|name| name.strip_suffix('O').unwrap_or(name).to_ascii_lowercase())
        .collect();
    assert!(!fields.is_empty(), "{id} names no header field: {text}");
    fields
}

/// The header titles of the parity row `id`: the quoted texts of its
/// `title<n>=` pairs.
fn titles(parity: &Document, id: &str) -> Vec<String> {
    let titles: Vec<String> = key_values(expected(parity, id))
        .into_iter()
        .filter(|pair| pair.key.starts_with("title"))
        .filter_map(|pair| code_span(&pair.value))
        .map(|span| unquoted(&span.text).to_owned())
        .collect();
    assert!(!titles.is_empty(), "{id} quotes no header title");
    titles
}

/// `name` without underscores, lower case, so `cur_date` and `CURDATE`
/// compare equal.
fn normalized(name: &str) -> String {
    name.replace('_', "").to_ascii_lowercase()
}

/// Adds every property name and every parameter name of the `OpenAPI`
/// document `value` to `names`.
fn api_field_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Object(properties)) = object.get("properties") {
                names.extend(properties.keys().cloned());
            }
            if let (Some(Value::String(name)), Some(Value::String(_))) =
                (object.get("name"), object.get("in"))
            {
                names.insert(name.clone());
            }
            for child in object.values() {
                api_field_names(child, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                api_field_names(item, names);
            }
        }
        _ => {}
    }
}

/// The one data-table row of the function map for the BMS map `bms` whose
/// `Key or field` starts with `key`, with its table.
fn map_row<'a>(map: &'a Document, bms: &str, key: &str) -> (&'a Table, &'a Row) {
    let found: Vec<(&Table, &Row)> = map
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            table.cell(row, "BMS map") == Some(bms)
                && table
                    .cell(row, "Key or field")
                    .is_some_and(|text| text.starts_with(key))
        })
        .collect();
    let [one] = found.as_slice() else {
        panic!(
            "{FUNCTION_MAP_PATH} has {} rows of {bms} for `{key}`, not one",
            found.len()
        );
    };
    *one
}

/// How many function-map rows of the BMS map `bms` are option rows.
fn option_rows(map: &Document, bms: &str) -> usize {
    map.tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            table.cell(row, "BMS map") == Some(bms)
                && table
                    .cell(row, "Key or field")
                    .is_some_and(|text| text.starts_with("option "))
        })
        .count()
}

/// The route of the menu screen itself: the `route=` of the map's PF3 row,
/// the screen the Log out item is offered on.
fn menu_route(map: &Document, menu: &Menu) -> String {
    let (table, row) = map_row(map, menu.bms, PF3_KEY);
    required_value(cell(table, row, "UI control"), "route", menu.bms)
}

/// The [`ROUTE_REQUESTS`] entry of `route`.
fn request_for(route: &str) -> &'static RouteRequest {
    ROUTE_REQUESTS
        .iter()
        .find(|request| request.route == route)
        .unwrap_or_else(|| panic!("ROUTE_REQUESTS has no operation for the menu route {route}"))
}

/// The operations behind the distinct routes of `entries`, in entry order.
fn requests_of(entries: &[Entry]) -> Vec<&'static RouteRequest> {
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .filter(|entry| seen.insert(entry.route.as_str()))
        .map(|entry| request_for(&entry.route))
        .collect()
}

/// The options of `menu` in `menu.ts`: the entries of its array that name a
/// COBOL program, leaving out the reward entries.
fn cobol_entries(source: &str, menu: &Menu) -> Vec<Entry> {
    let entries: Vec<Entry> = menu_entries(source, menu.array)
        .into_iter()
        .filter(|entry| entry.program.is_some())
        .collect();
    assert!(
        !entries.is_empty(),
        "{MENU_TS}: `{}` names no COBOL program",
        menu.array
    );
    entries
}

/// Every entry of the `menu.ts` array `array`, written one field per line as
/// the file is formatted: `{`, then `key: value,` lines, then `},`.
fn menu_entries(source: &str, array: &str) -> Vec<Entry> {
    let opening = format!("export const {array}: readonly MenuEntry[] = [");
    let mut lines = source
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != opening);
    assert!(lines.next().is_some(), "{MENU_TS} declares no `{opening}`");
    let mut entries = Vec::new();
    let mut open: Option<BTreeMap<&str, &str>> = None;
    for line in lines {
        match (line, open.as_mut()) {
            ("];", None) => return entries,
            ("{", None) => open = Some(BTreeMap::new()),
            ("},", Some(_)) => {
                if let Some(fields) = open.take() {
                    entries.push(entry(array, &fields));
                }
            }
            (_, Some(fields)) => {
                let (key, value) = line.split_once(": ").unwrap_or_else(|| {
                    panic!("{MENU_TS}: an entry of `{array}` holds the line `{line}`")
                });
                fields.insert(key, value.strip_suffix(',').unwrap_or(value));
            }
            (_, None) => assert!(
                line.starts_with("/**") || line.starts_with('*') || line.starts_with("//"),
                "{MENU_TS}: `{array}` holds the line `{line}` between entries"
            ),
        }
    }
    panic!("{MENU_TS}: `{array}` is not closed by `];`")
}

/// The [`Entry`] of the raw `key: value` fields of one `menu.ts` entry.
fn entry(array: &str, fields: &BTreeMap<&str, &str>) -> Entry {
    let text = |key: &str| -> Option<String> {
        let raw = fields.get(key).unwrap_or_else(|| {
            panic!("{MENU_TS}: an entry of `{array}` has no `{key}`: {fields:?}")
        });
        if *raw == "null" {
            return None;
        }
        let quoted = raw
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or_else(|| panic!("{MENU_TS}: `{key}: {raw}` in `{array}` is no plain string"));
        assert!(
            !quoted.contains(['\\', '"']),
            "{MENU_TS}: `{key}: {raw}` in `{array}` holds an escape"
        );
        Some(quoted.to_owned())
    };
    let required = |key: &str| {
        text(key).unwrap_or_else(|| panic!("{MENU_TS}: an entry of `{array}` has a null `{key}`"))
    };
    Entry {
        id: required("id"),
        label: required("label"),
        program: text("program"),
        route: required("route"),
        section: required("section"),
    }
}

/// The statement of `source` whose first line starts with `head`, through
/// the line that ends it with `;`, its words joined by single spaces.
fn statement(source: &str, head: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    for line in source
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with(head))
    {
        words.extend(line.split_whitespace());
        if line.ends_with(';') {
            return words.join(" ");
        }
    }
    panic!("{MENU_TS} has no complete statement starting `{head}`")
}

/// Every `import` statement of `source`, its words joined by single spaces.
fn imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut open: Option<Vec<&str>> = None;
    for line in source.lines().map(str::trim) {
        if open.is_none() && line.starts_with("import ") {
            open = Some(Vec::new());
        }
        if let Some(words) = open.as_mut() {
            words.extend(line.split_whitespace());
            if line.ends_with(';') {
                imports.push(words.join(" "));
                open = None;
            }
        }
    }
    imports
}

/// The `MenuGroup` constants `menuForUserType` returns for `usr_type`, in
/// order: the `adminGroups` list for `A`, the other list for any other type.
fn groups_shown(source: &str, usr_type: &str) -> Vec<String> {
    let choice = statement(source, "return userType ===");
    let branches = choice
        .strip_prefix("return userType === \"A\" ? ")
        .and_then(|rest| rest.strip_suffix(';'))
        .unwrap_or_else(|| panic!("{MENU_TS}: menuForUserType chooses otherwise: {choice}"));
    let (admin, other) = branches
        .split_once(" : ")
        .unwrap_or_else(|| panic!("{MENU_TS}: menuForUserType has no two lists: {choice}"));
    let list = if usr_type == ADMIN.usr_type {
        admin
    } else {
        other
    };
    let declared = statement(source, &format!("const {list}:"));
    declared
        .split_once("= [")
        .and_then(|(_, rest)| rest.strip_suffix("];"))
        .unwrap_or_else(|| panic!("{MENU_TS}: `{list}` is no list: {declared}"))
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The verdict of the last review-log row on `id`, if any.
fn last_verdict<'a>(doc: &'a Document, id: &str) -> Option<&'a str> {
    let mut verdict = None;
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if table.cell(row, "Row ID").map(str::trim) == Some(id) {
                verdict = table.cell(row, "Verdict").map(str::trim);
            }
        }
    }
    verdict
}
