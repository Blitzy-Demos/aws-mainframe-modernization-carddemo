import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  ADMIN_PATH_PATTERN,
  ADMIN_USER,
  PASSWORD,
  REGULAR_USER,
  chooseOption,
  expectMessage,
  expectScreen,
  expectValueNotShown,
  labelPattern,
  openSignon,
  pressEnter,
  pressKey,
  select,
  signOn,
  trackPaths,
  type ScreenExpectation,
} from "./helpers";

/*
 * NAV (AAP 0.9.1): the 21 CardDemo screens reached through their on-screen controls only, along
 * the navigation graph of AAP 0.3.5, USER0001 for the user branch and ADMIN001 for the admin
 * branch. Reported as the 19 maps of o7 and the 2 Db2 maps (E21).
 *
 * Each test starts at `openSignon`, the suite's only direct navigation, and nothing else opens a
 * URL, calls the API or reads the page through script. The walk sends reads only (the list loads,
 * the fetch a selection triggers on arrival, the COPAU00 search) and never a write: no save,
 * delete, add, payment, report or fraud key, no confirmation `Y`, no ENTER on COTRN02, CORPT00,
 * COBIL00, COUSR01 or COTRTLI, and COUSR02 is left by `F12=Cancel`, because its F3 saves first
 * (`app/cbl/COUSR02C.cbl:111-119`). The database therefore stays as `scripts/e2e.sh` prepared it.
 *
 * Screens can hold cardholder data, so nothing here logs, attaches or dumps a page, and every
 * card-number check asserts a verdict only, never the value (Cardholder Data Redaction).
 */

// A missing control fails its step after this bound instead of waiting for the test timeout, and
// `transfer` relies on it to retry a menu choice that a finished navigation has overtaken.
test.use({ actionTimeout: 15_000 });

/** The member names of the 21 maps. */
type MapName =
  | "COSGN00"
  | "COMEN01"
  | "COADM01"
  | "COACTVW"
  | "COACTUP"
  | "COCRDLI"
  | "COCRDSL"
  | "COCRDUP"
  | "COTRN00"
  | "COTRN01"
  | "COTRN02"
  | "CORPT00"
  | "COBIL00"
  | "COUSR00"
  | "COUSR01"
  | "COUSR02"
  | "COUSR03"
  | "COPAU00"
  | "COPAU01"
  | "COTRTLI"
  | "COTRTUP";

/**
 * Each map as its screen renders it: the route of AAP 0.3.5, the level-1 heading (the map's title
 * row, padding trimmed), the label of its first labelled field and the program of its `PGMNAME`
 * header (`modern/audit/screens.md`, `src/screens/<MAP>.tsx`). COSGN00 has no title row, so its
 * heading is the row-5 literal. COTRTLI and COTRTUP share a title; route and first field tell
 * them apart.
 */
const SCREEN: Record<MapName, ScreenExpectation> = {
  COSGN00: {
    route: "/signon",
    title: "This is a Credit Card Demo Application for Mainframe Modernization",
    firstField: "User ID",
    program: "COSGN00C",
  },
  COMEN01: {
    route: "/menu",
    title: "Main Menu",
    firstField: "Please select an option",
    program: "COMEN01C",
  },
  COADM01: {
    route: "/admin",
    title: "Admin Menu",
    firstField: "Please select an option",
    program: "COADM01C",
  },
  COACTVW: {
    route: "/accounts/view",
    title: "View Account",
    firstField: "Account Number",
    program: "COACTVWC",
  },
  COACTUP: {
    route: "/accounts/update",
    title: "Update Account",
    firstField: "Account Number",
    program: "COACTUPC",
  },
  COCRDLI: {
    route: "/cards",
    title: "List Credit Cards",
    firstField: "Account Number",
    program: "COCRDLIC",
  },
  COCRDSL: {
    route: "/cards/view",
    title: "View Credit Card Detail",
    firstField: "Account Number",
    program: "COCRDSLC",
  },
  COCRDUP: {
    route: "/cards/update",
    title: "Update Credit Card Details",
    firstField: "Account Number",
    program: "COCRDUPC",
  },
  COTRN00: {
    route: "/transactions",
    title: "List Transactions",
    firstField: "Search Tran ID",
    program: "COTRN00C",
  },
  COTRN01: {
    route: "/transactions/view",
    title: "View Transaction",
    firstField: "Enter Tran ID",
    program: "COTRN01C",
  },
  COTRN02: {
    route: "/transactions/add",
    title: "Add Transaction",
    firstField: "Enter Acct #",
    program: "COTRN02C",
  },
  CORPT00: {
    route: "/reports",
    title: "Transaction Reports",
    firstField: "Monthly (Current Month)",
    program: "CORPT00C",
  },
  COBIL00: {
    route: "/bill-payment",
    title: "Bill Payment",
    firstField: "Enter Acct ID",
    program: "COBIL00C",
  },
  COUSR00: {
    route: "/users",
    title: "List Users",
    firstField: "Search User ID",
    program: "COUSR00C",
  },
  COUSR01: {
    route: "/users/add",
    title: "Add User",
    firstField: "First Name",
    program: "COUSR01C",
  },
  COUSR02: {
    route: "/users/update",
    title: "Update User",
    firstField: "Enter User ID",
    program: "COUSR02C",
  },
  COUSR03: {
    route: "/users/delete",
    title: "Delete User",
    firstField: "Enter User ID",
    program: "COUSR03C",
  },
  COPAU00: {
    route: "/authorizations",
    title: "View Authorizations",
    firstField: "Search Acct Id",
    program: "COPAUS0C",
  },
  COPAU01: {
    route: "/authorizations/detail",
    title: "View Authorization Details",
    firstField: "Card #",
    program: "COPAUS1C",
  },
  COTRTLI: {
    route: "/transaction-types",
    title: "Maintain Transaction Type",
    firstField: "Type Filter",
    program: "COTRTLIC",
  },
  COTRTUP: {
    route: "/transaction-types/maintain",
    title: "Maintain Transaction Type",
    firstField: "Transaction Type",
    program: "COTRTUPC",
  },
};

/** The README transaction each map runs under, which its `TRNNAME` header shows. */
const TRANSACTION: Record<MapName, string> = {
  COSGN00: "CC00",
  COMEN01: "CM00",
  COADM01: "CA00",
  COACTVW: "CAVW",
  COACTUP: "CAUP",
  COCRDLI: "CCLI",
  COCRDSL: "CCDL",
  COCRDUP: "CCUP",
  COTRN00: "CT00",
  COTRN01: "CT01",
  COTRN02: "CT02",
  CORPT00: "CR00",
  COBIL00: "CB00",
  COUSR00: "CU00",
  COUSR01: "CU01",
  COUSR02: "CU02",
  COUSR03: "CU03",
  COPAU00: "CPVS",
  COPAU01: "CPVD",
  COTRTLI: "CTLI",
  COTRTUP: "CTTU",
};

/*
 * The account and card of the first cross-reference record (`app/data/ASCII/cardxref.txt:1`),
 * which `e2e/fixtures/pending-auth-seed.sql` gives one pending authorization, with that
 * authorization's transaction ID and merchant name. The full card number is used only to assert
 * that it is never shown. COPAU01 masks the digit run of the transaction ID as free text (E32),
 * so the detail is recognised by its merchant name instead.
 */
const SEED_ACCOUNT = "00000000050";
const SEED_CARD = "0500024453765740";
const SEED_AUTH_TRAN_ID = "NAV000000000001";
const SEED_MERCHANT = /^CARDDEMO NAV MERCHANT\s*$/;

/** A redacted card number: twelve `*` and the last four digits (E32). */
const REDACTED_CARD = /^\*{12}\d{4}$/;

/** The lowest user ID of the `dusrsecj` load (`app/jcl/DUSRSECJ.jcl:35`), row 1 of COUSR00. */
const FIRST_USER_ID = ADMIN_USER;

/** The invalid-option text of COMEN01C and COADM01C (`COMEN01C.cbl:131`, `COADM01C.cbl:135`). */
const INVALID_OPTION = "Please enter a valid option number...";

/** `CCDA-MSG-INVALID-KEY` (`app/cpy/CSMSG01Y.cpy:20-21`), padded to 50 bytes on the screen. */
const INVALID_KEY = /^\s*Invalid key pressed\. Please see below\.\.\./;

/*
 * `CCDA-MSG-THANK-YOU` (`app/cpy/CSMSG01Y.cpy:18-19`), which COSGN00C's PF3 sends
 * (`app/cbl/COSGN00C.cbl:88-90`). `Thank you for using CCDA application...` is COTTL01Y's
 * `CCDA-THANK-YOU`, which no program sends; the source decides, as COSGN00C-004 and
 * `src/lib/header.ts` `THANK_YOU` record.
 */
const THANK_YOU = /^\s*Thank you for using CardDemo application\.\.\./;

/** The read prompts COUSR02C and COUSR03C show after their read of endpoint 18. */
const UPDATE_PROMPT = "Press PF5 key to save your updates ...";
const DELETE_PROMPT = "Press PF5 key to delete this user ...";

/** COTRTUPC's information row on a fresh screen (COTRTUPC-003). */
const MAINTAIN_PROMPT = "Enter transaction type to be maintained";

/** The option names of `app/cpy/COMEN02Y.cpy:25-89`, in option order. */
const MAIN_MENU_OPTIONS = [
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
] as const;

/** The option names of `app/cpy/COADM02Y.cpy:26-53`, in option order. */
const ADMIN_MENU_OPTIONS = [
  "User List (Security)",
  "User Add (Security)",
  "User Update (Security)",
  "User Delete (Security)",
  "Transaction Type List/Update (Db2)",
  "Transaction Type Maintenance (Db2)",
] as const;

/** `OPTN001` to `OPTN012`: the option lines of both menu maps. */
const MENU_LINES = 12;

/** One header label of map rows 1 and 2: its text, its `POS` and `LENGTH`, the value it labels. */
interface HeaderLabel {
  readonly text: string;
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly field: "TRNNAME" | "CURDATE" | "PGMNAME" | "CURTIME";
}

/** `Tran:`, `Date:`, `Prog:` and `Time:` of every map but COSGN00 (rows 001, 004, 006, 009). */
const STANDARD_HEADER: readonly HeaderLabel[] = [
  { text: "Tran:", row: 1, col: 1, span: 5, field: "TRNNAME" },
  { text: "Date:", row: 1, col: 65, span: 5, field: "CURDATE" },
  { text: "Prog:", row: 2, col: 1, span: 5, field: "PGMNAME" },
  { text: "Time:", row: 2, col: 65, span: 5, field: "CURTIME" },
];

/** COSGN00's own header literals, one column wider (COSGN00-001, -004, -006, -009). */
const SIGNON_HEADER: readonly HeaderLabel[] = [
  { text: "Tran :", row: 1, col: 1, span: 6, field: "TRNNAME" },
  { text: "Date :", row: 1, col: 64, span: 6, field: "CURDATE" },
  { text: "Prog :", row: 2, col: 1, span: 6, field: "PGMNAME" },
  { text: "Time :", row: 2, col: 64, span: 6, field: "CURTIME" },
];

/** The path the page shows now. */
function pathOf(page: Page): string {
  return new URL(page.url()).pathname;
}

/** The two-digit option number of the 0-based menu line `index`. */
function optionNumber(index: number): string {
  return String(index + 1).padStart(2, "0");
}

/**
 * Asserts the header labels of `map` in the screen's `CardHeader`: each literal in BLUE
 * (`text-muted-foreground`) in the grid cell of its map `POS` and `LENGTH`, and, as rendered by
 * the browser, on the line of the value it labels and ending before it. The values themselves are
 * the transaction, the program, the date as `MM/DD/YY` and the time as `HH:MM:SS`.
 */
async function expectHeader(page: Page, map: MapName): Promise<void> {
  const header = page.locator('[data-slot="card-header"]');
  for (const label of map === "COSGN00" ? SIGNON_HEADER : STANDARD_HEADER) {
    const text = header.getByText(label.text, { exact: true });
    const value = header.locator(`[data-field="${label.field}"]`);
    await expect(text).toBeVisible();
    await expect(text).toHaveClass(/(^|\s)text-muted-foreground(\s|$)/);
    await expect(text.locator("xpath=..")).toHaveClass(
      `row-start-${label.row} col-start-${label.col} col-span-${label.span}`,
    );
    await expect(value).toBeVisible();
    const labelBox = await text.boundingBox();
    const valueBox = await value.boundingBox();
    const sameLineBefore =
      labelBox !== null &&
      valueBox !== null &&
      labelBox.x + labelBox.width <= valueBox.x &&
      Math.abs(labelBox.y + labelBox.height / 2 - (valueBox.y + valueBox.height / 2)) <
        Math.max(labelBox.height, valueBox.height) / 2;
    expect(sameLineBefore, `${map} ${label.text} stands before ${label.field} on its line`).toBe(
      true,
    );
  }
  await expect(header.locator('[data-field="TRNNAME"]')).toHaveText(TRANSACTION[map]);
  await expect(header.locator('[data-field="PGMNAME"]')).toHaveText(SCREEN[map].program ?? "");
  await expect(header.locator('[data-field="CURDATE"]')).toHaveText(/^\d{2}\/\d{2}\/\d{2}$/);
  await expect(header.locator('[data-field="CURTIME"]')).toHaveText(/^\d{2}:\d{2}:\d{2}$/);
}

/**
 * Asserts the option lines of a menu: line n shows option n as `NN. <name>`, and the lines after
 * the last option are blank. Waiting for line 1 also waits for the menu's option table to load.
 */
async function expectMenuOptions(page: Page, options: readonly string[]): Promise<void> {
  for (let index = 0; index < MENU_LINES; index += 1) {
    const line = page.locator(`[data-field="OPTN${String(index + 1).padStart(3, "0")}"]`);
    const name = options.at(index);
    await expect(line).toHaveText(name === undefined ? "" : `${optionNumber(index)}. ${name}`);
  }
}

/**
 * Chooses option `nn` on the menu shown and waits for its transfer to `target`.
 *
 * COMEN01 decides ENTER from endpoint 2's option table and ignores ENTER while that table loads,
 * which it does again on every arrival with nothing on screen to show it; COADM01 ignores ENTER
 * until its table first loads. An ignored ENTER sends nothing and changes nothing, so the choice is
 * repeated until the transfer happens. A choice that a finished transfer overtakes fails within
 * the action timeout, and the next attempt then finds the target shown.
 */
async function transfer(page: Page, nn: string, target: ScreenExpectation): Promise<void> {
  await expect(async () => {
    if (pathOf(page) !== target.route) {
      await chooseOption(page, nn);
    }
    await expect(page).toHaveURL((url) => url.pathname === target.route, { timeout: 2_000 });
  }).toPass({ timeout: 60_000 });
  await expectScreen(page, target);
}

/**
 * Types into the field labelled `label`, presses `F4=Clear` and asserts the screen-clearing PF4:
 * the field is empty again and holds the cursor. The key sends nothing to the server.
 */
async function expectClearedByF4(page: Page, label: string, typed: string): Promise<void> {
  const field = page.getByLabel(labelPattern(label));
  await field.fill(typed);
  await expect(field).toHaveValue(typed);
  await pressKey(page, "F4=Clear");
  await expect(field).toHaveValue("");
  await expect(field).toBeFocused();
}

/**
 * Asserts that `shown`, a card number as a screen shows it, is the redacted form. Only the verdict
 * is asserted, so a failure never prints the number.
 */
function expectRedacted(shown: string | null, reason: string): void {
  expect(REDACTED_CARD.test((shown ?? "").trim()), reason).toBe(true);
}

/** Data row `row` (1-based) of the screen's list. */
function listRow(page: Page, row: number): Locator {
  return page.locator("tbody tr").nth(row - 1);
}

/** Cell `column` (0-based; 0 is the selector) of data row `row` of the screen's list. */
function listCell(page: Page, row: number, column: number): Locator {
  return listRow(page, row).getByRole("cell").nth(column);
}

/**
 * Waits until data row 1 of a list shows a value in column `column` and returns it trimmed. A list
 * screen loads on arrival, and an ENTER pressed before that answer arrives is not served, so a
 * selection waits for it.
 */
async function firstListed(page: Page, column: number): Promise<string> {
  const cell = listCell(page, 1, column);
  await expect(cell).not.toHaveText(/^\s*$/);
  return ((await cell.textContent()) ?? "").trim();
}

/** Asserts that the screen's error row shows no message. */
async function expectNoMessage(page: Page): Promise<void> {
  await expect(page.locator('[data-message-kind="error"]')).toHaveText("");
}

/** Waits until COTRTLI shows its first listed type, whose selector is named after the type. */
async function expectTypesListed(page: Page): Promise<void> {
  await expect(page.getByRole("textbox", { name: /^Select type / }).first()).toBeVisible();
}

/**
 * Asserts COTRTUP's fresh screen (COTRTUPC-003): code and description empty, the code editable
 * with the cursor, the description protected, no error, the search prompt in the information row,
 * and only `ENTER=Process` and `F3=Exit` shown.
 */
async function expectFreshMaintenance(page: Page): Promise<void> {
  const code = page.getByLabel(labelPattern("Transaction Type"));
  const description = page.getByLabel(labelPattern("Description"));
  await expect(code).toHaveValue("");
  await expect(code).toBeEditable();
  await expect(code).toBeFocused();
  await expect(description).toHaveValue("");
  await expect(description).not.toBeEditable();
  await expectNoMessage(page);
  await expectMessage(page, "info", MAINTAIN_PROMPT);
  await expect(page.getByRole("button")).toHaveText(["ENTER=Process", "F3=Exit"]);
}

/** A main-menu option whose screen the user branch enters and leaves with no further edge. */
interface LeafOption {
  readonly option: string;
  readonly map: MapName;
  /** The map's F3 label, which returns to COMEN01. */
  readonly exit: "F3=Exit" | "F3=Back";
  /** The label of the field the screen-clearing `F4=Clear` empties, on the maps that have it. */
  readonly clears?: string;
}

/** Options `01`, `02`, `04`, `05` and `07` to `10` of COMEN01 (COMEN01C-018). */
const LEAF_OPTIONS: readonly LeafOption[] = [
  { option: "01", map: "COACTVW", exit: "F3=Exit" },
  { option: "02", map: "COACTUP", exit: "F3=Exit" },
  { option: "04", map: "COCRDSL", exit: "F3=Exit" },
  { option: "05", map: "COCRDUP", exit: "F3=Exit" },
  { option: "07", map: "COTRN01", exit: "F3=Back", clears: "Enter Tran ID" },
  { option: "08", map: "COTRN02", exit: "F3=Back", clears: "Enter Acct #" },
  { option: "09", map: "CORPT00", exit: "F3=Back" },
  { option: "10", map: "COBIL00", exit: "F3=Back", clears: "Enter Acct ID" },
];

test.describe("o7: 19 maps", () => {
  test("USER0001 user branch", async ({ page }) => {
    test.setTimeout(180_000);
    const paths = trackPaths(page);

    await openSignon(page);
    await expectScreen(page, SCREEN.COSGN00);
    // audit: COSGN00-001
    // audit: COSGN00-004
    // audit: COSGN00-006
    // audit: COSGN00-009
    await expectHeader(page, "COSGN00");

    // audit: COSGN00C-017
    await signOn(page, REGULAR_USER, PASSWORD);
    await expectScreen(page, SCREEN.COMEN01);
    // audit: COMEN01-001
    // audit: COMEN01-004
    // audit: COMEN01-006
    // audit: COMEN01-009
    await expectHeader(page, "COMEN01");

    // Each option's screen, entered from the menu and left by its F3, which returns to the menu
    // because the menu transferred there; on three of them F4 first clears the typed field.
    // audit: COMEN01C-018
    // audit: COACTVWC-004
    // audit: COACTUPC-005
    // audit: COCRDSLC-004
    // audit: COCRDUPC-004
    // audit: COTRN01C-004
    // audit: COTRN02C-004
    // audit: CORPT00C-004
    // audit: COBIL00C-004
    // audit: COTRN01C-005
    // audit: COTRN02C-005
    // audit: COBIL00C-005
    // audit: COACTVW-001
    // audit: COACTVW-004
    // audit: COACTVW-006
    // audit: COACTVW-009
    // audit: COACTUP-001
    // audit: COACTUP-004
    // audit: COACTUP-006
    // audit: COACTUP-009
    // audit: COCRDSL-001
    // audit: COCRDSL-004
    // audit: COCRDSL-006
    // audit: COCRDSL-009
    // audit: COCRDUP-001
    // audit: COCRDUP-004
    // audit: COCRDUP-006
    // audit: COCRDUP-009
    // audit: COTRN01-001
    // audit: COTRN01-004
    // audit: COTRN01-006
    // audit: COTRN01-009
    // audit: COTRN02-001
    // audit: COTRN02-004
    // audit: COTRN02-006
    // audit: COTRN02-009
    // audit: CORPT00-001
    // audit: CORPT00-004
    // audit: CORPT00-006
    // audit: CORPT00-009
    // audit: COBIL00-001
    // audit: COBIL00-004
    // audit: COBIL00-006
    // audit: COBIL00-009
    for (const leaf of LEAF_OPTIONS) {
      await transfer(page, leaf.option, SCREEN[leaf.map]);
      await expectHeader(page, leaf.map);
      if (leaf.clears !== undefined) {
        await expectClearedByF4(page, leaf.clears, "1");
      }
      await pressKey(page, leaf.exit);
      await expectScreen(page, SCREEN.COMEN01);
    }

    // Cards: option 03 lists them; `S` and `U` open the card view and the card update, whose F3
    // returns to the list that transferred there, and the list's F3 returns to the menu.
    await transfer(page, "03", SCREEN.COCRDLI);
    // audit: COCRDLI-001
    // audit: COCRDLI-004
    // audit: COCRDLI-006
    // audit: COCRDLI-009
    await expectHeader(page, "COCRDLI");
    const listedAccount = await firstListed(page, 1);
    await expectValueNotShown(page, SEED_CARD, "USER0001 sees only redacted card numbers");
    expectRedacted(await listCell(page, 1, 2).textContent(), "COCRDLI row 1 card is redacted");

    // audit: COCRDLIC-015
    // audit: COCRDSLC-002
    await select(page, 1, "S");
    await expectScreen(page, SCREEN.COCRDSL);
    const viewAccount = page.getByLabel(labelPattern("Account Number"));
    const viewCard = page.getByLabel(labelPattern("Card Number"));
    await expect(page.getByLabel(labelPattern("Name on card"))).not.toHaveValue("");
    await expect(viewAccount).toHaveValue(listedAccount);
    await expect(viewAccount).not.toBeEditable();
    await expect(viewCard).not.toBeEditable();
    expectRedacted(await viewCard.inputValue(), "COCRDSL shows USER0001 a redacted card number");
    await expectValueNotShown(page, SEED_CARD, "COCRDSL discloses no full number to USER0001");

    // audit: COCRDSLC-004
    // audit: COCRDLIC-002
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COCRDLI);
    await firstListed(page, 1);
    await expect(page.getByLabel(labelPattern("Page"))).toHaveValue("1");
    await expect(page.getByLabel(labelPattern("Account Number"))).toHaveValue("");
    await expect(page.getByLabel(labelPattern("Credit Card Number"))).toHaveValue("");
    await expect(page.getByRole("textbox", { name: "Select row 1", exact: true })).toHaveValue("");

    // audit: COCRDLIC-015
    await select(page, 1, "U");
    await expectScreen(page, SCREEN.COCRDUP);
    await expect(page.getByLabel(labelPattern("Name on card"))).not.toHaveValue("");
    await expectValueNotShown(page, SEED_CARD, "COCRDUP shows USER0001 no full card number");
    // audit: COCRDUPC-004
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COCRDLI);
    // audit: COCRDLIC-004
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COMEN01);

    // Transactions: option 06 lists the transactions POSTTRAN posted, which `scripts/e2e.sh`
    // counts against the audited posted count (FIXPOST-901 to FIXPOST-903).
    await transfer(page, "06", SCREEN.COTRN00);
    // audit: COTRN00-001
    // audit: COTRN00-004
    // audit: COTRN00-006
    // audit: COTRN00-009
    await expectHeader(page, "COTRN00");
    const listedTranId = await firstListed(page, 1);
    // audit: COTRN00C-008
    // audit: COTRN01C-003
    await select(page, 1, "S");
    await expectScreen(page, SCREEN.COTRN01);
    await expect(page.getByLabel(labelPattern("Enter Tran ID"))).toHaveValue(listedTranId);
    await expect(page.getByLabel(labelPattern("Transaction ID"))).toHaveValue(listedTranId);
    // audit: COTRN01C-006
    await pressKey(page, "F5=Browse Tran.");
    await expectScreen(page, SCREEN.COTRN00);
    await firstListed(page, 1);
    // audit: COTRN00C-008
    await select(page, 1, "S");
    await expectScreen(page, SCREEN.COTRN01);
    await expect(page.getByLabel(labelPattern("Transaction ID"))).not.toHaveValue("");
    // audit: COTRN01C-004
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COTRN00);
    // audit: COTRN00C-005
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COMEN01);

    // Authorizations: option 11, then the seeded account's list, its one authorization, and back.
    // audit: COMEN01C-014
    await transfer(page, "11", SCREEN.COPAU00);
    // audit: COPAU00-001
    // audit: COPAU00-004
    // audit: COPAU00-006
    // audit: COPAU00-009
    await expectHeader(page, "COPAU00");
    const searchAccount = page.getByLabel(labelPattern("Search Acct Id"));
    await searchAccount.fill(SEED_ACCOUNT);
    await pressEnter(page, searchAccount);
    await expect(listCell(page, 1, 1)).toHaveText(SEED_AUTH_TRAN_ID);
    await expectValueNotShown(page, SEED_CARD, "COPAU00 shows USER0001 no full card number");
    // audit: COPAUS0C-010
    // audit: COPAUS1C-002
    await select(page, 1, "S");
    await expectScreen(page, SCREEN.COPAU01);
    // audit: COPAU01-001
    // audit: COPAU01-004
    // audit: COPAU01-006
    // audit: COPAU01-009
    await expectHeader(page, "COPAU01");
    await expect(page.getByLabel(labelPattern("Name"))).toHaveValue(SEED_MERCHANT);
    expectRedacted(
      await page.getByLabel(labelPattern("Card #")).inputValue(),
      "COPAU01 shows USER0001 a redacted card number",
    );
    await expectValueNotShown(page, SEED_CARD, "COPAU01 shows USER0001 no full card number");
    // audit: COPAUS1C-004
    // audit: COPAUS0C-002
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COPAU00);
    await expect(searchAccount).toHaveValue(SEED_ACCOUNT);
    await expect(listCell(page, 1, 1)).toHaveText(SEED_AUTH_TRAN_ID);
    // audit: COPAUS0C-003
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COMEN01);

    // audit: COMEN01C-006
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COSGN00);
    // audit: COSGN00C-004
    await pressKey(page, "F3=Exit");
    await expect(page.getByText(THANK_YOU)).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).toHaveCount(0);
    await expect(page.getByRole("button")).toHaveCount(0);

    expect(
      paths.some((path) => ADMIN_PATH_PATTERN.test(path)),
      "USER0001 reached an admin route",
    ).toBe(false);
  });

  // The 11 options COMEN01 offers all transfer to non-admin routes, and the user branch above walks
  // every one of them. The route guard `RequireSignon` only hides the admin screens; the control is
  // the API's `AdminUser` gate (n5, `crates/api/tests/auth_gate.rs`). Options 12 to 99 are not
  // entered: their outcome is halt candidate X-COMEN01C-1 (COMEN01C-010), so `00` stands for the
  // invalid option, whose outcome COMEN01C-009 fixes.
  test("USER0001 has no path to an admin screen", async ({ page }) => {
    const paths = trackPaths(page);
    await openSignon(page);
    await signOn(page, REGULAR_USER, PASSWORD);
    await expectScreen(page, SCREEN.COMEN01);

    // audit: COMEN01C-005
    // audit: COMEN01C-022
    await expectMenuOptions(page, MAIN_MENU_OPTIONS);
    const option = page.getByLabel(labelPattern("Please select an option"));
    await expect(option).toHaveValue("");
    await expect(option).toBeFocused();
    await expectNoMessage(page);

    // The only controls are the two row-24 keys and the frame's skip link: none names a screen.
    await expect(page.getByRole("button")).toHaveText(["ENTER=Continue", "F3=Exit"]);
    await expect(page.getByRole("link")).toHaveText(["Skip to main content"]);
    await expect(page.getByRole("link")).toHaveAttribute("href", /^#/);

    // audit: COMEN01C-009
    await chooseOption(page, "00");
    await expectMessage(page, "error", INVALID_OPTION);
    await expect(option).toHaveValue("00");
    await expect(page).toHaveURL((url) => url.pathname === SCREEN.COMEN01.route);

    expect(
      paths.some((path) => ADMIN_PATH_PATTERN.test(path)),
      "USER0001 reached an admin route",
    ).toBe(false);
  });

  test("ADMIN001 admin branch", async ({ page }) => {
    test.setTimeout(180_000);
    await openSignon(page);
    // audit: COSGN00C-017
    await signOn(page, ADMIN_USER, PASSWORD);
    await expectScreen(page, SCREEN.COADM01);
    // audit: COADM01-001
    // audit: COADM01-004
    // audit: COADM01-006
    // audit: COADM01-009
    await expectHeader(page, "COADM01");
    // audit: COADM01C-003
    await expectMenuOptions(page, ADMIN_MENU_OPTIONS);
    const option = page.getByLabel(labelPattern("Please select an option"));
    await expect(option).toHaveValue("");
    await expect(option).toBeFocused();
    await expectNoMessage(page);

    // Option 01, then `U` on the first user: COUSR02 reads that user on arrival and is left by
    // F12, which returns to the admin menu and saves nothing.
    // audit: COADM01C-017
    // audit: COUSR00C-003
    await transfer(page, "01", SCREEN.COUSR00);
    // audit: COUSR00-001
    // audit: COUSR00-004
    // audit: COUSR00-006
    // audit: COUSR00-009
    await expectHeader(page, "COUSR00");
    await expect(page.getByLabel(labelPattern("Search User ID"))).toHaveValue("");
    expect(await firstListed(page, 1)).toBe(FIRST_USER_ID);
    // audit: COUSR00C-008
    // audit: COUSR02C-004
    // audit: COUSR02C-014
    await select(page, 1, "U");
    await expectScreen(page, SCREEN.COUSR02);
    // audit: COUSR02-001
    // audit: COUSR02-004
    // audit: COUSR02-006
    // audit: COUSR02-009
    await expectHeader(page, "COUSR02");
    await expect(page.getByLabel(labelPattern("Enter User ID"))).toHaveValue(FIRST_USER_ID);
    await expectMessage(page, "info", UPDATE_PROMPT);
    // audit: COUSR02C-008
    await pressKey(page, "F12=Cancel");
    await expectScreen(page, SCREEN.COADM01);

    // Option 01 again, then `D` on the first user: COUSR03 reads that user on arrival, and its F3
    // returns to the list that transferred there, which starts again from the lowest user ID.
    await transfer(page, "01", SCREEN.COUSR00);
    expect(await firstListed(page, 1)).toBe(FIRST_USER_ID);
    // audit: COUSR00C-009
    // audit: COUSR03C-004
    // audit: COUSR03C-012
    await select(page, 1, "D");
    await expectScreen(page, SCREEN.COUSR03);
    // audit: COUSR03-001
    // audit: COUSR03-004
    // audit: COUSR03-006
    // audit: COUSR03-009
    await expectHeader(page, "COUSR03");
    await expect(page.getByLabel(labelPattern("Enter User ID"))).toHaveValue(FIRST_USER_ID);
    await expectMessage(page, "info", DELETE_PROMPT);
    // audit: COUSR03C-006
    // audit: COUSR00C-003
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COUSR00);
    expect(await firstListed(page, 1)).toBe(FIRST_USER_ID);
    // audit: COUSR00C-004
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COADM01);

    // Option 02: the empty add screen. F4 clears what was typed, the labelled but unserved F12
    // shows the invalid-key text, and F3 returns to the admin menu; ENTER, which adds, is never
    // pressed.
    await transfer(page, "02", SCREEN.COUSR01);
    // audit: COUSR01-001
    // audit: COUSR01-004
    // audit: COUSR01-006
    // audit: COUSR01-009
    await expectHeader(page, "COUSR01");
    // audit: COUSR01C-004
    for (const label of ["First Name", "Last Name", "User ID", "Password", "User Type"]) {
      await expect(page.getByLabel(labelPattern(label))).toHaveValue("");
    }
    await expect(page.getByLabel(labelPattern("First Name"))).toBeFocused();
    await expectNoMessage(page);
    // audit: COUSR01C-008
    await expectClearedByF4(page, "First Name", "X");
    // audit: COUSR01C-009
    await pressKey(page, "F12=Exit");
    await expectMessage(page, "error", INVALID_KEY);
    await expect(page).toHaveURL((url) => url.pathname === SCREEN.COUSR01.route);
    // audit: COUSR01C-007
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COADM01);

    // Option 03: COUSR02 with no user selected, cleared by F4 and left by F12.
    await transfer(page, "03", SCREEN.COUSR02);
    // audit: COUSR02C-005
    for (const label of ["Enter User ID", "First Name", "Last Name", "Password", "User Type"]) {
      await expect(page.getByLabel(labelPattern(label))).toHaveValue("");
    }
    await expect(page.getByLabel(labelPattern("Enter User ID"))).toBeFocused();
    await expectNoMessage(page);
    // audit: COUSR02C-007
    await expectClearedByF4(page, "Enter User ID", FIRST_USER_ID);
    // audit: COUSR02C-008
    await pressKey(page, "F12=Cancel");
    await expectScreen(page, SCREEN.COADM01);

    // Option 04: COUSR03 with no user selected; its F3 returns to the admin menu that opened it.
    await transfer(page, "04", SCREEN.COUSR03);
    // audit: COUSR03C-005
    for (const label of ["Enter User ID", "First Name", "Last Name", "User Type"]) {
      await expect(page.getByLabel(labelPattern(label))).toHaveValue("");
    }
    await expect(page.getByLabel(labelPattern("Enter User ID"))).toBeFocused();
    await expectNoMessage(page);
    // audit: COUSR03C-006
    await pressKey(page, "F3=Back");
    await expectScreen(page, SCREEN.COADM01);

    // audit: COADM01C-020
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COSGN00);
  });
});

test.describe("+2: COTRTLI, COTRTUP", () => {
  test("ADMIN001 reaches COTRTLI and COTRTUP", async ({ page }) => {
    test.setTimeout(180_000);
    await openSignon(page);
    await signOn(page, ADMIN_USER, PASSWORD);
    await expectScreen(page, SCREEN.COADM01);

    // Option 05: the Db2 transaction types, listed on arrival (endpoint 24). F2 opens the
    // maintenance screen once the list is shown, as COTRTLIC serves PF2 only after its own screen
    // has been displayed (`app/app-transaction-type-db2/cbl/COTRTLIC.cbl:630-640`). Neither ENTER
    // nor F10 is pressed here, because both act on the list's selections.
    // audit: COADM01C-017
    await transfer(page, "05", SCREEN.COTRTLI);
    // audit: COTRTLI-001
    // audit: COTRTLI-004
    // audit: COTRTLI-006
    // audit: COTRTLI-009
    await expectHeader(page, "COTRTLI");
    await expectTypesListed(page);
    // audit: COTRTLIC-016
    // audit: COTRTUPC-003
    await pressKey(page, "F2=Add");
    await expectScreen(page, SCREEN.COTRTUP);
    // audit: COTRTUP-001
    // audit: COTRTUP-004
    // audit: COTRTUP-006
    // audit: COTRTUP-009
    await expectHeader(page, "COTRTUP");
    await expectFreshMaintenance(page);
    // audit: COTRTUPC-044
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COTRTLI);
    await expectTypesListed(page);
    // audit: COTRTLIC-015
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COADM01);

    // Option 06: the maintenance screen opened from the admin menu, to which its F3 returns.
    // audit: COADM01C-017
    // audit: COTRTUPC-003
    await transfer(page, "06", SCREEN.COTRTUP);
    await expectFreshMaintenance(page);
    // audit: COTRTUPC-044
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COADM01);

    // audit: COADM01C-020
    await pressKey(page, "F3=Exit");
    await expectScreen(page, SCREEN.COSGN00);
  });
});
