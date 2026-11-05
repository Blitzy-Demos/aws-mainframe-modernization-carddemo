// o7 responsive layout (AAP 0.5.4.4): the breakpoints the SPA targets, at
// the viewport of each Playwright project (md-768x1024 and lg-1280x800 in
// playwright.config.ts; this spec sets no viewport of its own).
//
// - md (768px to 1023px): the sidebar is an icon rail (the generated
//   sidebar's data-state "collapsed", data-collapsible "icon") whose links
//   show their label as a tooltip, forms use one column, and tables scroll
//   inside their card (the generated table container's overflow-x auto), so
//   no page scrolls sideways.
// - lg (1024px and wider): the sidebar is expanded (data-state "expanded")
//   and forms use two columns.
// - Ctrl/Cmd+B and the "Toggle Sidebar" trigger switch the sidebar at both.
// - The skip link is the first Tab stop and Enter on it moves focus to the
//   page content.
//
// Every route is opened at the project's viewport, for the role that may open
// it, with every key its page reads (path parameters and search keys), and
// is measured only once it shows its real content: the record, editor or
// list rows the keys select, or the empty entry form of a screen that reads
// nothing before a key is entered, with nothing loading and no error shown.
// It must then render without horizontal overflow.
import type { Locator, Page } from "@playwright/test";

import { adminMenu, MENU_SECTION_LABELS, userMenu, type MenuEntry } from "../src/nav/menu";

import {
  APP_ROUTES,
  cellUnder,
  content,
  dataRows,
  escapeRegExp,
  expect,
  expectScreen,
  FIXTURE_KEYS,
  gotoApp,
  mapControl,
  mapInput,
  nameMatcher,
  psql,
  resolveRoute,
  REWARD_SCREENS,
  routeScope,
  seedPendingAuthorization,
  sidebar,
  sidebarLink,
  signIn,
  sqlLiteral,
  tabTo,
  tableByCaption,
  test,
  UNMAPPED_LABELS,
  urlPath,
  USERS,
  type RouteParams,
  type SeededAuthorization,
} from "./fixtures";

// The narrowest viewport the SPA targets and the lg breakpoint (Tailwind 4
// defaults).
const MD_MIN_WIDTH = 768;
const LG_MIN_WIDTH = 1024;
// Bounds on the generated sidebar container: the 3rem icon rail and the 16rem
// panel.
const ICON_RAIL_MAX_WIDTH = 100;
const EXPANDED_MIN_WIDTH = 200;

type SidebarState = "collapsed" | "expanded";

// The generated sidebar's root, the element web/src/components/ui/sidebar.tsx
// gives data-state and data-collapsible (its data-slot is "sidebar").
function sidebarRoot(page: Page): Locator {
  return page.locator('[data-slot="sidebar"][data-state]');
}

// The fixed panel inside it whose width is the rail's or the panel's.
function sidebarContainer(page: Page): Locator {
  return sidebarRoot(page).locator('[data-slot="sidebar-container"]');
}

function viewportWidth(page: Page): number {
  const size = page.viewportSize();
  if (size === null) {
    throw new Error("the project sets no viewport");
  }
  return size.width;
}

const isLg = (page: Page): boolean => viewportWidth(page) >= LG_MIN_WIDTH;

async function widthOf(element: Locator): Promise<number> {
  const box = await element.boundingBox();
  if (box === null) {
    throw new Error("the element is not rendered");
  }
  return box.width;
}

// Asserts the sidebar mode `state`: its root's data-state (and, collapsed,
// data-collapsible "icon"), and the container's width once the transition
// has run.
async function expectSidebar(page: Page, state: SidebarState): Promise<void> {
  const root = sidebarRoot(page);
  await expect(root).toHaveAttribute("data-state", state);
  const width = (): Promise<number> => widthOf(sidebarContainer(page));
  if (state === "collapsed") {
    await expect(root).toHaveAttribute("data-collapsible", "icon");
    await expect.poll(width).toBeLessThan(ICON_RAIL_MAX_WIDTH);
  } else {
    await expect.poll(width).toBeGreaterThan(EXPANDED_MIN_WIDTH);
  }
}

// The sidebar mode of the project's breakpoint: collapsed at md, expanded at
// lg.
const initialState = (page: Page): SidebarState => (isLg(page) ? "expanded" : "collapsed");
const flipped = (state: SidebarState): SidebarState => (state === "expanded" ? "collapsed" : "expanded");

// Asserts that the document is no wider than the viewport.
async function expectNoHorizontalOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow, "page scrolls sideways").toBeLessThanOrEqual(0);
}

// The left and top edges of every text input of the page's form, in DOM order.
async function inputEdges(page: Page): Promise<{ left: number; top: number }[]> {
  const inputs = content(page).locator("form").first().getByRole("textbox");
  await expect(inputs.first()).toBeVisible();
  const edges: { left: number; top: number }[] = [];
  for (const field of await inputs.all()) {
    const box = await field.boundingBox();
    if (box !== null) {
      edges.push({ left: Math.round(box.x), top: Math.round(box.y) });
    }
  }
  return edges;
}

// The one row `sql` selects, as its `width` column values (psql separates
// them with `|`). Throws on no row, more than one, or another column count.
async function storedRow(sql: string, width: number): Promise<string[]> {
  const out = await psql(sql);
  const values = out.split("|");
  if (out === "" || out.includes("\n") || values.length !== width) {
    throw new Error(`expected one row of ${width} columns from ${sql}, got ${JSON.stringify(out)}`);
  }
  return values;
}

// The lowest card number, the first row of the card list's unfiltered first
// page.
async function lowestCardNumber(): Promise<string> {
  const [card] = await storedRow("SELECT min(card_number) FROM cards", 1);
  return card;
}

// Asserts that `scope` has settled on what it shows: nothing in it is busy (a
// DataTable's table, a page's loading card, the lazy route's fallback), no
// Skeleton placeholder is left (data-slot "skeleton",
// web/src/components/ui/skeleton.tsx), no key or field error is shown
// (FieldError's data-slot "field-error"), and no request failure is toasted.
async function expectSettled(page: Page, scope: Locator): Promise<void> {
  await expect(scope.locator('[aria-busy="true"]'), "nothing is loading").toHaveCount(0);
  await expect(scope.locator('[data-slot="skeleton"]'), "no placeholder is shown").toHaveCount(0);
  await expect(scope.locator('[data-slot="field-error"]'), "no key or field error is shown").toHaveCount(0);
  await expect(page.locator('[data-sonner-toast][data-type="error"]'), "no request failure is toasted").toHaveCount(0);
}

// Asserts that `table` shows exactly one data row matching `has` (by default
// a cell reading `value`), that its `column` cell reads `value`, and that the
// table is no longer busy. DataTable's loading rows are aria-hidden skeletons
// that hold no text, so only a row of the page's answer can match.
async function expectRow(table: Locator, column: string, value: string, has?: Locator): Promise<void> {
  const row = dataRows(table).filter({
    has: has ?? table.page().getByRole("cell", { name: value, exact: true }),
  });
  await expect(row, `the row of ${value}`).toHaveCount(1);
  await expect(await cellUnder(table, row, column)).toHaveText(value);
  await expect(table, "the table is not loading").not.toHaveAttribute("aria-busy", "true");
}

// The value (dd) of the detail list entry captioned (dt) exactly `caption`.
function detailValue(scope: Locator, caption: string): Locator {
  return scope
    .locator("dt")
    .filter({ hasText: new RegExp(`^${escapeRegExp(caption)}$`) })
    .locator("xpath=following-sibling::dd[1]");
}

// The textbox of the page content named exactly `name`.
function textbox(page: Page, name: string): Locator {
  return content(page).getByRole("textbox", { name, exact: true });
}

// Asserts the initial state of the entry form of function-map row `id`: its
// submit is enabled and its first input, when the row lists one, is enabled.
async function expectEntryForm(page: Page, id: string): Promise<void> {
  const control = mapControl(id);
  const scope = routeScope(page, control.route);
  await expect(scope.getByRole("button", { name: control.name, exact: true })).toBeEnabled();
  const [first] = control.inputs;
  if (first !== undefined) {
    await expect(mapInput(scope, first)).toBeEnabled();
  }
}

// Asserts that the menu landmark `landmark` of the page content links every
// option of `entries`.
async function expectMenuOptions(page: Page, landmark: string, entries: readonly MenuEntry[]): Promise<void> {
  const nav = content(page).getByRole("navigation", { name: landmark, exact: true });
  for (const entry of entries) {
    await expect(nav.getByRole("link", { name: entry.label, exact: true })).toBeVisible();
  }
}

// What the every-route checks read from the store before they open the routes.
type RouteKeys = {
  params: RouteParams;
  // The lowest card number, the first row of the unfiltered card list.
  firstCard: string;
  // The card number of params.tranId, the lowest TRAN-ID.
  tranCard: string;
  // Account 1's card (FIXTURE_KEYS.card) as stored: CARD-ACTIVE-STATUS and the
  // expiry date's parts.
  cardActive: string;
  cardExpiryYear: string;
  cardExpiryMonth: string;
  // The stored description of FIXTURE_KEYS.tranType.
  tranTypeDescription: string;
  // The type code of params.ruleId, the lowest rule.
  ruleTypeCode: string;
  // PA-TRANSACTION-ID of params.authId, the seeded authorization.
  authTranId: string;
};

type RouteCheck = {
  // The search keys the route's page reads, after its path.
  search?: (keys: RouteKeys) => Record<string, string>;
  // Waits until the route shows the content the keys select.
  ready: (page: Page, keys: RouteKeys) => Promise<void>;
};

// A route key the every-route checks set up, read by each check that needs it.
function routeKey(keys: RouteKeys, name: keyof RouteParams): string {
  const value = keys.params[name];
  if (value === undefined) {
    throw new Error(`the route key ${name} was not set up`);
  }
  return value;
}

const tranIdOf = (keys: RouteKeys): string => routeKey(keys, "tranId");
const authIdOf = (keys: RouteKeys): string => routeKey(keys, "authId");
const ruleIdOf = (keys: RouteKeys): string => routeKey(keys, "ruleId");

// The account the card and authorization pages are opened for.
const forAccount = (): Record<string, string> => ({ account_id: FIXTURE_KEYS.account });

// The real content of every route of APP_ROUTES. A screen that reads nothing
// until a key is entered is measured in that initial state, its entry form;
// each entry says why.
const ROUTE_CHECKS: ReadonlyMap<string, RouteCheck> = new Map<string, RouteCheck>([
  // Sign-on reads nothing until it is submitted.
  ["/login", { ready: (page) => expectEntryForm(page, "F-COSGN00-001") }],
  [
    // CM00: the account summary needs a looked-up account, so the screen is
    // its lookup and the menu options.
    "/",
    {
      ready: async (page) => {
        await expectEntryForm(page, "F-COACTVW-001");
        await expectMenuOptions(page, "Main Menu", userMenu);
      },
    },
  ],
  [
    "/accounts/:accountId",
    {
      ready: async (page) => {
        await expect(detailValue(content(page), "Customer id")).toHaveText(FIXTURE_KEYS.customer);
        await expect(detailValue(content(page), "Card Number")).toHaveText(FIXTURE_KEYS.card);
      },
    },
  ],
  [
    "/accounts/:accountId/edit",
    {
      ready: async (page) => {
        await expect(textbox(page, "Account Number")).toHaveValue(FIXTURE_KEYS.account);
        await expect(textbox(page, "Customer id")).toHaveValue(FIXTURE_KEYS.customer);
      },
    },
  ],
  [
    "/cards",
    {
      ready: (page, keys) =>
        expectRow(tableByCaption(content(page), "Credit cards"), "Card Number", keys.firstCard),
    },
  ],
  [
    // CCDL reads the card for the account of `account_id` (E-55).
    "/cards/:cardNumber",
    {
      search: forAccount,
      ready: async (page, keys) => {
        await expect(detailValue(content(page), "Card Active Y/N")).toHaveText(keys.cardActive);
        await expect(detailValue(content(page), "Expiry Date")).toHaveText(
          `${keys.cardExpiryMonth}/${keys.cardExpiryYear}`,
        );
      },
    },
  ],
  [
    // CCUP reads the card for the account of `account_id` (E-55).
    "/cards/:cardNumber/edit",
    {
      search: forAccount,
      ready: async (page, keys) => {
        await expect(textbox(page, UNMAPPED_LABELS.cardEdit.nameOnCard)).toBeEditable();
        await expect(textbox(page, "Card Active Y/N")).toHaveValue(keys.cardActive);
        await expect(textbox(page, "Expiry Month")).toHaveValue(keys.cardExpiryMonth);
        await expect(textbox(page, "Expiry Year")).toHaveValue(keys.cardExpiryYear);
      },
    },
  ],
  [
    "/transactions",
    {
      ready: (page, keys) =>
        expectRow(tableByCaption(content(page), "Transactions"), "Transaction ID", tranIdOf(keys)),
    },
  ],
  [
    // CT02 starts from an empty form; "Copy last transaction" fills it only
    // when pressed.
    "/transactions/new",
    {
      ready: async (page) => {
        await expectEntryForm(page, "F-COTRN02-001");
        await expect(
          content(page).getByRole("textbox", { name: nameMatcher(UNMAPPED_LABELS.transactionAdd.accountId) }),
        ).toBeEditable();
      },
    },
  ],
  [
    "/transactions/:tranId",
    {
      ready: async (page, keys) => {
        await expect(detailValue(content(page), "Transaction ID")).toHaveText(tranIdOf(keys));
        await expect(detailValue(content(page), "Card Number")).toHaveText(keys.tranCard);
      },
    },
  ],
  // CR00 runs a report only once its type is chosen and confirmed.
  ["/reports/transactions", { ready: (page) => expectEntryForm(page, "F-CORPT00-001") }],
  // CB00 previews a balance only for an entered account.
  ["/payments", { ready: (page) => expectEntryForm(page, "F-COBIL00-001") }],
  [
    // CPVS lists the authorizations of the account in `account_id`.
    "/authorizations",
    {
      search: forAccount,
      ready: (page, keys) =>
        expectRow(
          tableByCaption(content(page), "Pending authorizations"),
          "Transaction ID",
          keys.authTranId,
          page.locator(`a[href^="/authorizations/${encodeURIComponent(authIdOf(keys))}?"]`),
        ),
    },
  ],
  [
    // CPVD, opened as the summary's View link opens it.
    "/authorizations/:authId",
    {
      search: forAccount,
      ready: async (page, keys) => {
        await expect(detailValue(content(page), "Tran Id")).toHaveText(keys.authTranId);
      },
    },
  ],
  [
    // The balance and history are read only for an entered account.
    "/rewards",
    {
      ready: async (page) => {
        await expect(textbox(page, UNMAPPED_LABELS.rewards.accountId)).toBeEditable();
        await expect(
          content(page).getByRole("button", { name: REWARD_SCREENS.rewards.lookUp, exact: true }),
        ).toBeEnabled();
      },
    },
  ],
  ["/admin", { ready: (page) => expectMenuOptions(page, "Admin Menu", adminMenu) }],
  [
    "/admin/users",
    {
      ready: (page) =>
        expectRow(tableByCaption(content(page), "Users, in user ID order"), "User ID", USERS.user.userId),
    },
  ],
  [
    // CU01 adds from an empty form.
    "/admin/users/new",
    {
      ready: async (page) => {
        await expectEntryForm(page, "F-COUSR01-001");
        await expect(textbox(page, UNMAPPED_LABELS.userForm.firstName)).toBeEditable();
      },
    },
  ],
  [
    "/admin/users/:userId/edit",
    {
      ready: async (page) => {
        await expect(textbox(page, UNMAPPED_LABELS.userForm.userId)).toHaveValue(USERS.user.userId);
        await expect(textbox(page, UNMAPPED_LABELS.userForm.userType)).toHaveValue(USERS.user.usrType);
      },
    },
  ],
  [
    "/admin/transaction-types",
    {
      ready: (page) =>
        expectRow(tableByCaption(content(page), "Transaction types"), "Type", FIXTURE_KEYS.tranType),
    },
  ],
  // CTTU's key entry reads a type only on Look up.
  ["/admin/transaction-types/new", { ready: (page) => expectEntryForm(page, "F-COTRTUP-001") }],
  [
    "/admin/transaction-types/:code/edit",
    {
      ready: async (page, keys) => {
        const description = textbox(page, UNMAPPED_LABELS.tranType.description);
        await expect(description).toBeEditable();
        await expect(description).toHaveValue(keys.tranTypeDescription);
      },
    },
  ],
  [
    "/admin/reward-rules",
    {
      ready: (page, keys) =>
        expectRow(
          tableByCaption(content(page), REWARD_SCREENS.ruleList.caption),
          "Rule ID",
          ruleIdOf(keys),
        ),
    },
  ],
  [
    // A new rule starts from an empty form.
    "/admin/reward-rules/new",
    {
      ready: async (page) => {
        const form = content(page).getByRole("form", { name: REWARD_SCREENS.ruleForm.newTitle, exact: true });
        await expect(
          form.getByRole("textbox", { name: UNMAPPED_LABELS.rewardRule.typeCode, exact: true }),
        ).toBeEditable();
        await expect(form.getByRole("button", { name: REWARD_SCREENS.ruleForm.save, exact: true })).toBeEnabled();
      },
    },
  ],
  [
    "/admin/reward-rules/:ruleId/edit",
    {
      ready: async (page, keys) => {
        const form = content(page).getByRole("form", {
          name: `${REWARD_SCREENS.ruleForm.editTitle} ${ruleIdOf(keys)}`,
          exact: true,
        });
        await expect(
          form.getByRole("textbox", { name: UNMAPPED_LABELS.rewardRule.typeCode, exact: true }),
        ).toHaveValue(keys.ruleTypeCode);
      },
    },
  ],
]);

test("the project viewport is a supported breakpoint", async ({ page }) => {
  expect(viewportWidth(page)).toBeGreaterThanOrEqual(MD_MIN_WIDTH);
});

test.describe("app shell", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page, "user");
  });

  test("the sidebar is an icon rail at md and expanded at lg", async ({ page }) => {
    await expectSidebar(page, initialState(page));
    if (isLg(page)) {
      await expect(sidebar(page).getByText(MENU_SECTION_LABELS.user, { exact: true })).toBeVisible();
    }
    // The link names stay in the icon rail, so every entry is still reachable.
    for (const entry of userMenu) {
      await expect(sidebarLink(page, entry.label)).toBeVisible();
    }
  });

  test("a sidebar link shows its label as a tooltip on keyboard focus and on hover only in the icon rail", async ({
    page,
  }) => {
    const entry = userMenu[0];
    if (entry === undefined) {
      throw new Error("menu.ts holds no user entry");
    }
    const link = sidebarLink(page, entry.label);
    const tooltip = page.getByRole("tooltip", { name: entry.label, exact: true });
    const expectTooltip = async (): Promise<void> => {
      if (isLg(page)) {
        await expect(tooltip).toBeHidden();
      } else {
        await expect(tooltip).toBeVisible();
      }
    };
    await tabTo(page, link);
    await expectTooltip();
    // Escape dismisses the focus tooltip, so the hover below opens it afresh.
    await page.keyboard.press("Escape");
    await expect(tooltip).toBeHidden();
    await link.hover();
    await expectTooltip();
  });

  test("Ctrl/Cmd+B and the sidebar trigger toggle the sidebar", async ({ page }) => {
    const initial = initialState(page);
    await expectSidebar(page, initial);
    await page.keyboard.press("ControlOrMeta+b");
    await expectSidebar(page, flipped(initial));
    await page.keyboard.press("ControlOrMeta+b");
    await expectSidebar(page, initial);
    await page.getByRole("button", { name: "Toggle Sidebar", exact: true }).first().click();
    await expectSidebar(page, flipped(initial));
  });

  test("the skip link shows when it takes focus, and Enter moves focus to the page content", async ({ page }) => {
    await gotoApp(page, "/");
    // Tab is pressed only once the screen has rendered, from a document in
    // which nothing has focus yet, so it starts at the top of the page.
    await expectScreen(page);
    await expect.poll(() => page.evaluate(() => document.activeElement === document.body)).toBe(true);
    await page.keyboard.press("Tab");
    const skip = page.getByRole("link", { name: "Skip to content", exact: true });
    await expect(skip).toBeFocused();
    await expect(skip).toBeInViewport();
    const main = page.locator("main#main-content");
    await expect(main).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect
      .poll(() => main.evaluate((element) => element === document.activeElement || element.contains(document.activeElement)))
      .toBe(true);
  });
});

test.describe("tables scroll inside their card", () => {
  let firstCard: string | undefined;

  test.beforeAll(async () => {
    firstCard = await lowestCardNumber();
  });

  // Each table is measured with a row of its answer shown: the users list's
  // first page holds USER0001, and the card list, which reads no key from the
  // URL, starts its unfiltered first page at the lowest card number.
  for (const { route, role, caption, column, known } of [
    {
      route: "/admin/users",
      role: "admin",
      caption: "Users, in user ID order",
      column: "User ID",
      known: (): string | undefined => USERS.user.userId,
    },
    {
      route: "/cards",
      role: "user",
      caption: "Credit cards",
      column: "Card Number",
      known: (): string | undefined => firstCard,
    },
  ] as const) {
    test(`${route}: the table container scrolls and stays inside its card`, async ({ page }) => {
      const value = known();
      if (value === undefined) {
        throw new Error("the known row was not set up");
      }
      await signIn(page, role);
      await gotoApp(page, route);
      const table = tableByCaption(content(page), caption);
      await expectRow(table, column, value);
      // The generated Table's scroll container inside the DataTable card
      // (web/src/components/ui/table.tsx, web/src/components/data-table.tsx).
      const container = table.locator('xpath=ancestor::*[@data-slot="table-container"][1]');
      await expect(container.locator('[data-slot="skeleton"]'), "no placeholder row is shown").toHaveCount(0);
      await expect(container).toHaveCSS("overflow-x", "auto");
      const card = container.locator('xpath=ancestor::*[@data-slot="card"][1]');
      const inner = await container.boundingBox();
      const outer = await card.boundingBox();
      if (inner === null || outer === null) {
        throw new Error("the table or its card is not rendered");
      }
      // Half a pixel absorbs sub-pixel layout rounding.
      expect(inner.x, "the table starts inside its card").toBeGreaterThanOrEqual(outer.x - 0.5);
      expect(inner.x + inner.width, "the table ends inside its card").toBeLessThanOrEqual(outer.x + outer.width + 0.5);
      await expectNoHorizontalOverflow(page);
    });
  }
});

test.describe("form columns", () => {
  for (const { route, role } of [
    { route: "/transactions/new", role: "user" },
    { route: "/admin/users/new", role: "admin" },
  ] as const) {
    test(`${route} uses one column at md and two at lg`, async ({ page }) => {
      await signIn(page, role);
      await gotoApp(page, route);
      const edges = await inputEdges(page);
      expect(edges.length, "the form holds more than one input").toBeGreaterThan(1);
      const columns = new Set(edges.map((edge) => edge.left));
      if (isLg(page)) {
        expect(columns.size, "input columns at lg").toBeGreaterThanOrEqual(2);
        const rows = new Set(edges.map((edge) => edge.top));
        expect(rows.size, "some inputs share a row at lg").toBeLessThan(edges.length);
      } else {
        expect(columns.size, "input columns at md").toBe(1);
      }
    });
  }
});

test.describe("every route at this viewport", () => {
  let keys: RouteKeys | undefined;
  let seeded: SeededAuthorization | undefined;

  test.beforeAll(async () => {
    seeded = await seedPendingAuthorization(FIXTURE_KEYS.account);
    // The lowest TRAN-ID is a seeded record and the first row of the
    // transaction list, which starts at the lowest key.
    const [tranId, tranCard] = await storedRow(
      "SELECT tran_id, tran_card_num FROM transaction_master ORDER BY tran_id LIMIT 1",
      2,
    );
    const [cardActive, cardExpiry] = await storedRow(
      `SELECT card_active_status, card_expiraion_date FROM cards WHERE card_number = ${sqlLiteral(FIXTURE_KEYS.card)}`,
      2,
    );
    const [ruleId, ruleTypeCode] = await storedRow(
      "SELECT rule_id, transaction_type_cd FROM reward_rules ORDER BY rule_id LIMIT 1",
      2,
    );
    const [tranTypeDescription] = await storedRow(
      `SELECT tr_description FROM transaction_types WHERE tr_type = ${sqlLiteral(FIXTURE_KEYS.tranType)}`,
      1,
    );
    const [authTranId] = await storedRow(
      `SELECT pa_transaction_id FROM pending_auth_details WHERE auth_id = ${sqlLiteral(seeded.authId)}::bigint`,
      1,
    );
    keys = {
      params: {
        accountId: FIXTURE_KEYS.account,
        cardNumber: FIXTURE_KEYS.card,
        tranId,
        authId: seeded.authId,
        userId: USERS.user.userId,
        code: FIXTURE_KEYS.tranType,
        ruleId,
      },
      firstCard: await lowestCardNumber(),
      tranCard,
      cardActive,
      // CARD-EXPIRAION-DATE is YYYY-MM-DD.
      cardExpiryYear: cardExpiry.slice(0, 4),
      cardExpiryMonth: cardExpiry.slice(5, 7),
      tranTypeDescription,
      ruleTypeCode,
      authTranId,
    };
  });

  test.afterAll(async () => {
    await seeded?.restore();
  });

  const unchecked = APP_ROUTES.filter((route) => !ROUTE_CHECKS.has(route.path)).map((route) => route.path);
  const unrouted = [...ROUTE_CHECKS.keys()].filter((path) => !APP_ROUTES.some((route) => route.path === path));
  if (unchecked.length > 0 || unrouted.length > 0) {
    throw new Error(
      `ROUTE_CHECKS must cover APP_ROUTES exactly: no check for ${JSON.stringify(unchecked)}, no route for ${JSON.stringify(unrouted)}`,
    );
  }

  for (const route of APP_ROUTES) {
    test(`${route.path} has no horizontal overflow`, async ({ page }) => {
      const check = ROUTE_CHECKS.get(route.path);
      if (keys === undefined || check === undefined) {
        throw new Error("the route keys or the route's check were not set up");
      }
      const search = check.search === undefined ? "" : `?${new URLSearchParams(check.search(keys)).toString()}`;
      const target = `${resolveRoute(route.path, keys.params)}${search}`;
      if (route.access !== "public") {
        await signIn(page, route.access === "admin" ? "admin" : "user");
      }
      await gotoApp(page, target);
      await expect(page).toHaveURL(urlPath(target));
      // The sign-on screen renders outside the app shell, so its h1 is not
      // inside #main-content.
      if (route.access === "public") {
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      } else {
        await expectScreen(page);
      }
      await check.ready(page, keys);
      await expectSettled(page, routeScope(page, route.path));
      await expectNoHorizontalOverflow(page);
    });
  }
});
