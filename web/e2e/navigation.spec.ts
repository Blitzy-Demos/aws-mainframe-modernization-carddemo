// NAV (AAP 0.10.2, o7): every screen is navigable. This spec proves the
// function map (docs/parity/function-map.md, the #1 lane BMS key → UI control
// contract) end to end:
//
// - it cites every F-ID the map defines, and the map defines every F-ID this
//   spec and forms.spec.ts cite (AAP 0.9.1, "After implementation: coverage");
// - each map row's control is found by its role and accessible name on its
//   route, a link or navigation button lands on its target, a button (or a
//   link without a target) whose endpoint is a GET sends that GET, a dialog
//   Cancel closes its dialog without a request, and the PF3 sign-off item
//   ends the session; a row without a control is asserted to have none, with
//   its escalation;
// - every route opens for both roles, an admin route turns a regular user
//   back, and the sidebar shows each role its menu sections;
// - every crumb of a nested route's breadcrumb trail (the PF3 replacement)
//   opens its screen;
// - the @smoke test signs ADMIN001 on and loads the dashboard (AAP 0.10.4,
//   startup in both modes). It reads no database and runs under no hook.
//
// Controls whose request writes (Confirm, Save, Add, …) are only located
// here; forms.spec.ts drives them and restores what they change. The paging
// rows need a second page on the user, transaction-type and authorization
// lists, so the seeded-keys describe adds the rows that takes and restores
// them afterwards.
import { readFileSync } from "node:fs";

import type { Locator, Page } from "@playwright/test";

import { adminMenu, MENU_SECTION_LABELS, userMenu, type MenuEntry } from "../src/nav/menu";

import {
  APP_ROUTES,
  assertFixtureKeys,
  assertMessagesMatchCatalog,
  checkpoint,
  citationReport,
  confirmDialog,
  content,
  controlLocator,
  expect,
  expectMessage,
  expectScreen,
  FIXTURE_KEYS,
  functionMap,
  isRequestTo,
  latestTranId,
  mapControl,
  mapInput,
  MESSAGES,
  nameMatcher,
  psql,
  repoPath,
  resolveRoute,
  routePattern,
  rowEndpoint,
  rowInput,
  routeScope,
  seedPendingAuthorization,
  sidebar,
  sidebarLink,
  signIn,
  signOut,
  sqlLiteral,
  test,
  UNMAPPED_LABELS,
  urlPath,
  USERS,
  type Checkpoint,
  type Endpoint,
  type MapControl,
  type MapRow,
  type Role,
  type RouteParams,
  type SeededAuthorization,
} from "./fixtures";

/** The 21 BMS maps of the carried online programs (AAP 0.9.1). */
const BMS_MAPS = [
  "COSGN00",
  "COMEN01",
  "COACTVW",
  "COACTUP",
  "COCRDLI",
  "COCRDSL",
  "COCRDUP",
  "COTRN00",
  "COTRN01",
  "COTRN02",
  "CORPT00",
  "COBIL00",
  "COPAU00",
  "COPAU01",
  "COADM01",
  "COUSR00",
  "COUSR01",
  "COUSR02",
  "COUSR03",
  "COTRTUP",
  "COTRTLI",
] as const;

/** A type code no TRANSACTION_TYPE row holds (7 seeded types, 01 to 07). */
const MISSING_TYPE_CODE = "99";

/**
 * The rows of one page of the lists whose paging rows need a second page:
 * COUSR00 shows 10 users, COTRTLI 7 types and COPAU00 5 authorizations (the
 * screen rows of COUSR00C, COTRTLIC and COPAUS0C, the API's page defaults).
 */
const USER_PAGE_ROWS = "10";
const TYPE_PAGE_ROWS = "7";
const AUTH_PAGE_ROWS = 5;
/** The user IDs and type codes the seeded-keys describe adds for a second page. */
const EXTRA_USER_PREFIX = "E2ENAV";
const EXTRA_TYPE_CODES = "tr_type BETWEEN '90' AND '97'";

// The keys the routes of the map rows are resolved with.
type NavParams = Required<
  Pick<RouteParams, "accountId" | "cardNumber" | "tranId" | "authId" | "userId" | "code" | "ruleId">
> & {
  /** A pending authorization of account 1 already reported as fraud. */
  reportedAuthId: string;
};

// Steps on the opened route that bring a row's control on screen.
type Reveal = (page: Page, params: NavParams) => Promise<void>;

type NavCase =
  /** The row's key has no web control; `escalation` is the E-ID that says why. */
  | { kind: "none"; escalation: string }
  | {
      kind: "control";
      /** The route to open in place of the row's own, for a key carried by state. */
      start?: (params: NavParams) => string;
      reveal?: Reveal;
    };

const none = (escalation: string): NavCase => ({ kind: "none", escalation });
const GENERIC: NavCase = { kind: "control" };
const at = (start: (params: NavParams) => string, reveal?: Reveal): NavCase =>
  reveal === undefined ? { kind: "control", start } : { kind: "control", start, reveal };
const after = (reveal: Reveal): NavCase => ({ kind: "control", reveal });

async function press(page: Page, id: string): Promise<void> {
  await controlLocator(page, mapControl(id)).click();
}

/** CAVW lookup on the dashboard (F-COACTVW-001) for account 1. */
async function lookUpAccount(page: Page): Promise<void> {
  await mapInput(content(page), rowInput("F-COACTVW-001", 0)).fill(FIXTURE_KEYS.account);
  await press(page, "F-COACTVW-001");
}

/** The card list filtered to account 1 (F-COCRDLI-001), then a row action. */
function openCardWith(rowActionId: string): Reveal {
  return async (page) => {
    await mapInput(content(page), rowInput("F-COCRDLI-001", 0)).fill(FIXTURE_KEYS.account);
    await press(page, "F-COCRDLI-001");
    await press(page, rowActionId);
    await expect(page).toHaveURL(routePattern(mapControl(rowActionId).target ?? "/cards"));
  };
}

/** The pending authorizations of account 1 (F-COPAU00-001). */
async function searchAuthorizations(page: Page): Promise<void> {
  await mapInput(content(page), rowInput("F-COPAU00-001", 0)).fill(FIXTURE_KEYS.account);
  await press(page, "F-COPAU00-001");
}

/** Presses the control of row `openerId`, which opens the confirm dialog. */
function openDialogWith(openerId: string): Reveal {
  return async (page) => {
    await press(page, openerId);
    await expect(confirmDialog(page)).toBeVisible();
  };
}

/** CT02 with account 1 entered, the key PF5 validates before it copies. */
async function enterAddAccount(page: Page): Promise<void> {
  await content(page)
    .getByRole("textbox", { name: nameMatcher(UNMAPPED_LABELS.transactionAdd.accountId) })
    .fill(FIXTURE_KEYS.account);
}

/** CT02 with account 1, PF5 copy of the last transaction, then the add submit. */
async function copyLastAndAdd(page: Page): Promise<void> {
  await enterAddAccount(page);
  await press(page, "F-COTRN02-006");
  await press(page, "F-COTRN02-001");
  await expect(confirmDialog(page)).toBeVisible();
}

/** The CR00 report type radio `index` of F-CORPT00-001, checked. */
function chooseReport(index: number): Reveal {
  return async (page) => {
    await mapInput(content(page), rowInput("F-CORPT00-001", index)).check();
  };
}

/** CR00 monthly report submitted, which asks for the confirmation. */
async function monthlyReport(page: Page, params: NavParams): Promise<void> {
  await chooseReport(0)(page, params);
  await press(page, "F-CORPT00-001");
  await expect(confirmDialog(page)).toBeVisible();
}

/** CB00 review of account 1's balance, which asks for the confirmation. */
async function reviewPayment(page: Page): Promise<void> {
  await mapInput(content(page), rowInput("F-COBIL00-001", 0)).fill(FIXTURE_KEYS.account);
  await press(page, "F-COBIL00-001");
  await expect(confirmDialog(page)).toBeVisible();
}

/** CTTU lookup of a type code not on file, which opens the add path. */
async function lookUpMissingType(page: Page): Promise<void> {
  await mapInput(content(page), rowInput("F-COTRTUP-001", 0)).fill(MISSING_TYPE_CODE);
  await press(page, "F-COTRTUP-001");
}

function pressFirst(id: string): Reveal {
  return async (page) => {
    await press(page, id);
  };
}

/**
 * One entry per map row, keyed by its F-ID. A control entry names only what
 * the row itself cannot: a different starting route or the steps that bring
 * the control on screen. Everything else comes from the row.
 */
const CASES: Readonly<Record<string, NavCase>> = {
  // COSGN00: sign-on
  "F-COSGN00-001": GENERIC,
  "F-COSGN00-002": GENERIC,
  "F-COSGN00-003": none("E-56"),
  // COMEN01: main menu options and sign-off
  "F-COMEN01-001": GENERIC,
  "F-COMEN01-002": GENERIC,
  "F-COMEN01-003": GENERIC,
  "F-COMEN01-004": GENERIC,
  "F-COMEN01-005": GENERIC,
  "F-COMEN01-006": GENERIC,
  "F-COMEN01-007": GENERIC,
  "F-COMEN01-008": GENERIC,
  "F-COMEN01-009": GENERIC,
  "F-COMEN01-010": GENERIC,
  "F-COMEN01-011": GENERIC,
  "F-COMEN01-012": GENERIC,
  "F-COMEN01-013": none("E-56"),
  // COACTVW: account view
  "F-COACTVW-001": GENERIC,
  "F-COACTVW-002": after(lookUpAccount),
  "F-COACTVW-003": GENERIC,
  "F-COACTVW-004": none("E-56"),
  // COACTUP: account update
  "F-COACTUP-001": after(lookUpAccount),
  "F-COACTUP-002": GENERIC,
  "F-COACTUP-003": none("E-56"),
  "F-COACTUP-004": GENERIC,
  "F-COACTUP-005": GENERIC,
  "F-COACTUP-006": GENERIC,
  "F-COACTUP-007": none("E-56"),
  // COCRDLI: card list
  "F-COCRDLI-001": GENERIC,
  "F-COCRDLI-002": GENERIC,
  "F-COCRDLI-003": GENERIC,
  "F-COCRDLI-004": GENERIC,
  "F-COCRDLI-005": GENERIC,
  "F-COCRDLI-006": GENERIC,
  "F-COCRDLI-007": none("E-56"),
  // COCRDSL: card view, reached from the list with its account
  "F-COCRDSL-001": GENERIC,
  "F-COCRDSL-002": at(() => "/cards", openCardWith("F-COCRDLI-005")),
  "F-COCRDSL-003": none("E-56"),
  // COCRDUP: card update, reached from the list with its account
  "F-COCRDUP-001": GENERIC,
  "F-COCRDUP-002": at(() => "/cards", openCardWith("F-COCRDLI-006")),
  "F-COCRDUP-003": none("E-56"),
  "F-COCRDUP-004": at(() => "/cards", openCardWith("F-COCRDLI-006")),
  "F-COCRDUP-005": at(() => "/cards", openCardWith("F-COCRDLI-006")),
  "F-COCRDUP-006": at(() => "/cards", openCardWith("F-COCRDLI-006")),
  "F-COCRDUP-007": none("E-56"),
  // COTRN00: transaction list
  "F-COTRN00-001": GENERIC,
  "F-COTRN00-002": GENERIC,
  "F-COTRN00-003": GENERIC,
  "F-COTRN00-004": GENERIC,
  "F-COTRN00-005": GENERIC,
  "F-COTRN00-006": none("E-56"),
  // COTRN01: transaction view
  "F-COTRN01-001": GENERIC,
  "F-COTRN01-002": GENERIC,
  "F-COTRN01-003": none("E-62"),
  "F-COTRN01-004": GENERIC,
  "F-COTRN01-005": none("E-56"),
  // COTRN02: transaction add
  "F-COTRN02-001": GENERIC,
  "F-COTRN02-002": after(copyLastAndAdd),
  "F-COTRN02-003": after(copyLastAndAdd),
  "F-COTRN02-004": GENERIC,
  "F-COTRN02-005": GENERIC,
  "F-COTRN02-006": after(enterAddAccount),
  "F-COTRN02-007": none("E-56"),
  // CORPT00: transaction reports; the date inputs show for a custom report
  "F-CORPT00-001": after(chooseReport(2)),
  "F-CORPT00-002": after(monthlyReport),
  "F-CORPT00-003": after(monthlyReport),
  "F-CORPT00-004": GENERIC,
  "F-CORPT00-005": none("E-56"),
  // COBIL00: bill payment
  "F-COBIL00-001": GENERIC,
  "F-COBIL00-002": after(reviewPayment),
  "F-COBIL00-003": after(reviewPayment),
  "F-COBIL00-004": GENERIC,
  "F-COBIL00-005": GENERIC,
  "F-COBIL00-006": none("E-56"),
  // COPAU00: pending authorization summary of account 1
  "F-COPAU00-001": GENERIC,
  "F-COPAU00-002": GENERIC,
  "F-COPAU00-003": after(searchAuthorizations),
  "F-COPAU00-004": after(searchAuthorizations),
  "F-COPAU00-005": after(searchAuthorizations),
  "F-COPAU00-006": none("E-56"),
  // COPAU01: authorization detail; Remove fraud and Next on the reported one
  "F-COPAU01-001": after(searchAuthorizations),
  "F-COPAU01-002": GENERIC,
  "F-COPAU01-003": GENERIC,
  "F-COPAU01-004": at((p) => `/authorizations/${p.reportedAuthId}`),
  "F-COPAU01-005": after(openDialogWith("F-COPAU01-003")),
  "F-COPAU01-006": at((p) => `/authorizations/${p.reportedAuthId}`),
  "F-COPAU01-007": none("E-56"),
  // COADM01: admin menu options and sign-off
  "F-COADM01-001": GENERIC,
  "F-COADM01-002": GENERIC,
  "F-COADM01-003": GENERIC,
  "F-COADM01-004": GENERIC,
  "F-COADM01-005": GENERIC,
  "F-COADM01-006": GENERIC,
  "F-COADM01-007": GENERIC,
  "F-COADM01-008": none("E-56"),
  // COUSR00: user list
  "F-COUSR00-001": GENERIC,
  "F-COUSR00-002": GENERIC,
  "F-COUSR00-003": GENERIC,
  "F-COUSR00-004": GENERIC,
  "F-COUSR00-005": GENERIC,
  "F-COUSR00-006": GENERIC,
  "F-COUSR00-007": none("E-56"),
  // COUSR01: user add
  "F-COUSR01-001": GENERIC,
  "F-COUSR01-002": GENERIC,
  "F-COUSR01-003": GENERIC,
  "F-COUSR01-004": none("E-57"),
  "F-COUSR01-005": none("E-56"),
  // COUSR02: user update
  "F-COUSR02-001": GENERIC,
  "F-COUSR02-002": GENERIC,
  "F-COUSR02-003": GENERIC,
  "F-COUSR02-004": GENERIC,
  "F-COUSR02-005": GENERIC,
  "F-COUSR02-006": none("E-56"),
  // COUSR03: user delete, the confirm dialog of the list's Delete
  "F-COUSR03-001": GENERIC,
  "F-COUSR03-002": after(openDialogWith("F-COUSR03-001")),
  "F-COUSR03-003": after(openDialogWith("F-COUSR03-001")),
  "F-COUSR03-004": after(openDialogWith("F-COUSR03-001")),
  "F-COUSR03-005": after(openDialogWith("F-COUSR03-001")),
  "F-COUSR03-006": none("E-56"),
  // COTRTUP: transaction type maintenance; the add path needs a missing code
  "F-COTRTUP-001": GENERIC,
  "F-COTRTUP-002": GENERIC,
  "F-COTRTUP-003": after(lookUpMissingType),
  "F-COTRTUP-004": none("E-56"),
  "F-COTRTUP-005": GENERIC,
  "F-COTRTUP-006": GENERIC,
  "F-COTRTUP-007": after(openDialogWith("F-COTRTUP-006")),
  "F-COTRTUP-008": after(lookUpMissingType),
  "F-COTRTUP-009": GENERIC,
  "F-COTRTUP-010": none("E-63"),
  "F-COTRTUP-011": after(lookUpMissingType),
  "F-COTRTUP-012": after(openDialogWith("F-COTRTUP-006")),
  "F-COTRTUP-013": none("E-56"),
  "F-COTRTUP-014": GENERIC,
  "F-COTRTUP-015": after(lookUpMissingType),
  "F-COTRTUP-016": none("E-56"),
  // COTRTLI: transaction type list with inline update and delete
  "F-COTRTLI-001": GENERIC,
  "F-COTRTLI-002": GENERIC,
  "F-COTRTLI-003": GENERIC,
  "F-COTRTLI-004": GENERIC,
  "F-COTRTLI-005": GENERIC,
  "F-COTRTLI-006": GENERIC,
  "F-COTRTLI-007": after(pressFirst("F-COTRTLI-006")),
  "F-COTRTLI-008": GENERIC,
  "F-COTRTLI-009": after(openDialogWith("F-COTRTLI-008")),
  "F-COTRTLI-010": none("E-56"),
};

type Check =
  | "opens-option"
  | "signs-off"
  | "navigates"
  | "requests"
  | "closes-dialog"
  | "submit"
  | "present";

// What a row's control is checked for. Controls whose endpoint writes are
// only located; a dialog action without a request is Cancel; a button, or a
// link without a target, whose endpoint is a GET (Previous, Next, Copy last
// transaction, the re-reading Cancel and Delete) is checked for that GET.
function checkOf(row: MapRow, control: MapControl): Check {
  if (control.role === "menuitem") {
    return "signs-off";
  }
  if (row.key.startsWith("option ")) {
    return "opens-option";
  }
  const writes = /^(POST|PUT|DELETE) /.test(row.endpoint);
  if (control.role === "dialog-action") {
    return row.endpoint === "—" ? "closes-dialog" : "present";
  }
  if (control.target !== null && !writes) {
    return "navigates";
  }
  if (/^GET /.test(row.endpoint) && (control.role === "button" || control.role === "link")) {
    return "requests";
  }
  return control.role === "form-submit" ? "submit" : "present";
}

/** Runs `action` and asserts that the page sends a request of `endpoint` meanwhile. */
async function expectRequest(page: Page, endpoint: Endpoint, action: () => Promise<void>): Promise<void> {
  await Promise.all([page.waitForRequest((request) => isRequestTo(request, endpoint)), action()]);
}

// The PF8 "Next" row of the screen a PF7 "Previous" row belongs to.
function nextRowOf(row: MapRow): MapRow & { control: MapControl } {
  const next = functionMap().rows.find(
    (candidate) =>
      candidate.map === row.map &&
      candidate.control?.role === "button" &&
      candidate.control.name === "Next" &&
      candidate.control.route === row.control?.route,
  );
  if (next?.control === undefined || next.control === null) {
    throw new Error(`${row.id} has no Next row on its screen`);
  }
  return { ...next, control: next.control };
}

// Presses a GET control and asserts its request. A Previous that is disabled
// on the first page is asserted disabled, then pressed after Next has paged
// on (and sent its own GET).
async function exerciseRequest(page: Page, row: MapRow, element: Locator): Promise<void> {
  if (row.control?.name === "Previous" && (await element.isDisabled())) {
    await expect(element, `${row.id} on the first page`).toBeDisabled();
    const next = nextRowOf(row);
    await expectRequest(page, rowEndpoint(next.id), () => controlLocator(page, next.control).click());
    await expect(element, `${row.id} after ${next.id}`).toBeEnabled();
  }
  await expectRequest(page, rowEndpoint(row.id), () => element.click());
}

// Who signs on for a row: nobody for the sign-on screen, an administrator
// for /admin routes, a regular user otherwise.
function roleOf(route: string): Role | null {
  if (route === "/login") {
    return null;
  }
  return route === "/admin" || route.startsWith("/admin/") ? "admin" : "user";
}

async function exercise(page: Page, row: MapRow, nav: Extract<NavCase, { kind: "control" }>, params: NavParams): Promise<void> {
  const control = row.control;
  if (control === null) {
    throw new Error(`${row.id} has no UI control, so its case must be none(…)`);
  }
  const role = roleOf(control.route);
  if (role !== null) {
    await signIn(page, role);
  }
  const check = checkOf(row, control);
  if (check === "signs-off" && role !== null) {
    await page.goto(control.route);
    await signOut(page, role);
    return;
  }
  if (check === "opens-option") {
    await expect(sidebarLink(page, control.name)).toBeVisible();
    await sidebarLink(page, control.name).click();
    await expect(page).toHaveURL(urlPath(control.route));
    return;
  }
  await page.goto(resolveRoute(nav.start?.(params) ?? control.route, params));
  if (nav.reveal !== undefined) {
    await nav.reveal(page, params);
  }
  const element = controlLocator(page, control);
  await expect(element).toBeVisible();
  switch (check) {
    case "navigates":
      await element.click();
      await expect(page).toHaveURL(routePattern(control.target ?? control.route));
      await expectScreen(page);
      return;
    case "requests":
      await exerciseRequest(page, row, element);
      return;
    case "closes-dialog": {
      const url = page.url();
      await element.click();
      await expect(confirmDialog(page)).toBeHidden();
      expect(page.url()).toBe(url);
      return;
    }
    case "submit":
      await expect(element).toHaveAttribute("type", "submit");
      for (const input of control.inputs) {
        await expect(mapInput(routeScope(page, control.route), input)).toBeVisible();
      }
      return;
    case "present":
      return;
  }
}

// ---------------------------------------------------------------------------

test.describe("function map coverage", () => {
  test("the map holds one table for each of the 21 BMS maps", () => {
    const map = functionMap();
    expect(map.maps).toEqual([...BMS_MAPS]);
    expect(map.byId.size).toBe(map.rows.length);
    for (const name of BMS_MAPS) {
      expect(map.rows.some((row) => row.map === name), `${name} has rows`).toBe(true);
    }
  });

  test("cites every F-ID of the map, and no spec cites an F-ID the map lacks", () => {
    const map = functionMap();
    const report = citationReport(
      map.rows.map((row) => row.id),
      readFileSync(repoPath("web/e2e/navigation.spec.ts"), "utf8"),
      readFileSync(repoPath("web/e2e/forms.spec.ts"), "utf8"),
    );
    expect(report).toEqual({ uncited: [], unknown: [] });
    expect(Object.keys(CASES).sort()).toEqual(map.rows.map((row) => row.id).sort());
  });

  test("menu.ts options are the CM00 and CA00 option rows of the map", () => {
    const optionRows = (map: string): { label: string; route: string }[] =>
      functionMap()
        .rows.filter((row) => row.map === map && row.key.startsWith("option "))
        .map((row) => ({ label: row.control?.name ?? "", route: row.control?.route ?? "" }));
    const carried = (entries: readonly MenuEntry[]): { label: string; route: string }[] =>
      entries
        .filter((entry) => entry.program !== null)
        .map((entry) => ({ label: entry.label, route: entry.route }));
    expect(carried(userMenu)).toEqual(optionRows("COMEN01"));
    expect(carried(adminMenu)).toEqual(optionRows("COADM01"));
  });

  test("fixture keys and asserted messages agree with the corpus and messages.rs", () => {
    assertFixtureKeys();
    assertMessagesMatchCatalog();
  });
});

test("ADMIN001 signs on and loads the dashboard", { tag: "@smoke" }, async ({ page }) => {
  await signIn(page, "admin");
  await expectScreen(page);
  await page.goto("/");
  const lookup = controlLocator(page, mapControl("F-COACTVW-001"));
  await expect(lookup).toBeVisible();
  await expectScreen(page);
  // The session lives in sessionStorage, so a reload of the tab keeps it.
  await page.reload();
  await expect(page).toHaveURL(urlPath("/"));
  await expect(lookup).toBeVisible();
});

test.describe("sidebar menus", () => {
  const sections: Record<Role, readonly { label: string; entries: readonly MenuEntry[] }[]> = {
    user: [{ label: MENU_SECTION_LABELS.user, entries: userMenu }],
    admin: [
      { label: MENU_SECTION_LABELS.user, entries: userMenu },
      { label: MENU_SECTION_LABELS.admin, entries: adminMenu },
    ],
  };

  for (const role of ["user", "admin"] as const) {
    test(`${USERS[role].userId} sees the ${role === "admin" ? "two sections" : "card management section"}`, async ({ page }) => {
      await signIn(page, role);
      for (const section of sections[role]) {
        const list = sidebar(page).getByRole("list", { name: section.label, exact: true });
        await expect(list.getByRole("link")).toHaveText(section.entries.map((entry) => entry.label));
      }
      if (role === "user") {
        await expect(
          sidebar(page).getByRole("list", { name: MENU_SECTION_LABELS.admin, exact: true }),
        ).toHaveCount(0);
      }
    });

    test(`every sidebar entry of ${USERS[role].userId} opens its route`, async ({ page }) => {
      await signIn(page, role);
      for (const section of sections[role]) {
        for (const entry of section.entries) {
          await sidebarLink(page, entry.label).click();
          await expect(page).toHaveURL(urlPath(entry.route));
          await expectScreen(page);
        }
      }
    });
  }
});

test.describe("breadcrumb trail", () => {
  // The crumb links of the page's breadcrumb navigation; the current page's
  // crumb is a BreadcrumbPage, not a link with a target.
  const crumbLinks = (page: Page): Locator =>
    page
      .getByRole("navigation", { name: "breadcrumb", exact: true })
      .locator('[data-slot="breadcrumb-link"]');

  for (const { route, role } of [
    { route: `/accounts/${FIXTURE_KEYS.account}/edit`, role: "user" },
    { route: `/admin/users/${USERS.user.userId}/edit`, role: "admin" },
  ] as const) {
    test(`every crumb of ${route} opens its screen (PF3)`, async ({ page }) => {
      await signIn(page, role);
      await page.goto(route);
      await expect(crumbLinks(page).first()).toBeVisible();
      const targets = await crumbLinks(page).evaluateAll((links) =>
        links.map((link) => new URL(link.getAttribute("href") ?? "", document.baseURI).pathname),
      );
      expect(targets.length, "a nested route has crumbs above it").toBeGreaterThan(0);
      for (const [index, target] of targets.entries()) {
        await page.goto(route);
        await crumbLinks(page).nth(index).click();
        await expect(page, `crumb ${index + 1} of ${route}`).toHaveURL(urlPath(target));
        await expectScreen(page);
      }
    });
  }
});

test.describe("with seeded keys", () => {
  let params: NavParams | undefined;
  let seeded: SeededAuthorization[] = [];
  let extraRows: Checkpoint | undefined;

  test.beforeAll(async () => {
    // A second page on the user and transaction-type lists: one row more
    // than a page holds, copied from USER0001 (its password hash included)
    // and numbered down from type 97.
    extraRows = await checkpoint([
      { table: "users", where: `user_id LIKE ${sqlLiteral(`${EXTRA_USER_PREFIX}%`)}` },
      { table: "transaction_types", where: EXTRA_TYPE_CODES },
    ]);
    await psql(
      `INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type)
       SELECT ${sqlLiteral(EXTRA_USER_PREFIX)} || lpad(n::text, 2, '0'), 'Nav', 'Probe', u.password_hash, 'U'
         FROM users u,
              generate_series(1, greatest(0, ${sqlLiteral(USER_PAGE_ROWS)}::bigint + 1 - (SELECT count(*) FROM users))) AS n
        WHERE u.user_id = ${sqlLiteral(USERS.user.userId)}`,
    );
    await psql(
      `INSERT INTO transaction_types (tr_type, tr_description)
       SELECT lpad((98 - n)::text, 2, '0'), 'E2E NAV TYPE ' || n
         FROM generate_series(1, greatest(0, ${sqlLiteral(TYPE_PAGE_ROWS)}::bigint + 1 - (SELECT count(*) FROM transaction_types))) AS n`,
    );
    expect(
      await psql(
        `SELECT (SELECT count(*) FROM users) > ${sqlLiteral(USER_PAGE_ROWS)}::bigint
            AND (SELECT count(*) FROM transaction_types) > ${sqlLiteral(TYPE_PAGE_ROWS)}::bigint`,
      ),
      "the user and type lists have a second page",
    ).toBe("t");
    // A second page of account 1's authorizations: older ones first, so the
    // two the cases name stay the newest under the account.
    for (let older = 1; older < AUTH_PAGE_ROWS; older += 1) {
      seeded = [...seeded, await seedPendingAuthorization(FIXTURE_KEYS.account)];
    }
    const unreported = await seedPendingAuthorization(FIXTURE_KEYS.account);
    seeded = [...seeded, unreported];
    const reported = await seedPendingAuthorization(FIXTURE_KEYS.account, { fraud: "F" });
    seeded = [...seeded, reported];
    params = {
      accountId: FIXTURE_KEYS.account,
      cardNumber: FIXTURE_KEYS.card,
      tranId: await latestTranId(),
      authId: unreported.authId,
      reportedAuthId: reported.authId,
      userId: USERS.user.userId,
      code: FIXTURE_KEYS.tranType,
      ruleId: await psql("SELECT min(rule_id) FROM reward_rules"),
    };
  });

  test.afterAll(async () => {
    for (const authorization of [...seeded].reverse()) {
      await authorization.restore();
    }
    await extraRows?.restore();
  });

  function keys(): NavParams {
    if (params === undefined) {
      throw new Error("the seeded keys were not set up");
    }
    return params;
  }

  test.describe("every route", () => {
    test("/login shows the sign-on form when signed out", async ({ page }) => {
      const control = mapControl("F-COSGN00-001");
      await page.goto("/login");
      await expect(controlLocator(page, control)).toBeVisible();
    });

    for (const route of APP_ROUTES.filter((r) => r.access !== "public")) {
      test(`${route.path} signed out goes to /login`, async ({ page }) => {
        await page.goto(resolveRoute(route.path, keys()));
        await expect(page).toHaveURL(urlPath("/login"));
      });

      for (const role of ["user", "admin"] as const) {
        test(`${route.path} as ${USERS[role].userId}`, async ({ page }) => {
          await signIn(page, role);
          const path = resolveRoute(route.path, keys());
          await page.goto(path);
          if (route.access === "admin" && role === "user") {
            await expect(page).toHaveURL(urlPath("/"));
            await expectMessage(page, MESSAGES.ADMIN_ACCESS_REQUIRED);
            return;
          }
          await expect(page).toHaveURL(urlPath(path));
          await expectScreen(page);
        });
      }
    }
  });

  test.describe("every function map row", () => {
    for (const row of functionMap().rows) {
      const nav = CASES[row.id];
      test(`${row.id} ${row.map} ${row.key.split(" [")[0] ?? row.key}`, async ({ page }) => {
        if (nav === undefined) {
          throw new Error(`${row.id} has no navigation case`);
        }
        if (nav.kind === "none") {
          expect(row.control, `${row.id} has no control`).toBeNull();
          expect(row.endpoint).toBe("—");
          expect(row.action).toContain(nav.escalation);
          return;
        }
        await exercise(page, row, nav, keys());
      });
    }
  });
});
