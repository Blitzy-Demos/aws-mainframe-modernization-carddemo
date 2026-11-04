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
// it, and must render without horizontal overflow.
import type { Locator, Page } from "@playwright/test";

import { MENU_SECTION_LABELS, userMenu } from "../src/nav/menu";

import {
  APP_ROUTES,
  content,
  expect,
  expectScreen,
  FIXTURE_KEYS,
  latestTranId,
  psql,
  resolveRoute,
  seedPendingAuthorization,
  sidebar,
  sidebarLink,
  signIn,
  tabTo,
  test,
  urlPath,
  USERS,
  type RouteParams,
  type SeededAuthorization,
} from "./fixtures";

/** The narrowest viewport the SPA targets and the lg breakpoint (Tailwind 4 defaults). */
const MD_MIN_WIDTH = 768;
const LG_MIN_WIDTH = 1024;
/** Bounds on the generated sidebar container: the 3rem icon rail and the 16rem panel. */
const ICON_RAIL_MAX_WIDTH = 100;
const EXPANDED_MIN_WIDTH = 200;

type SidebarState = "collapsed" | "expanded";

/**
 * The generated sidebar's root, the element web/src/components/ui/sidebar.tsx
 * gives data-state and data-collapsible (its data-slot is "sidebar").
 */
function sidebarRoot(page: Page): Locator {
  return page.locator('[data-slot="sidebar"][data-state]');
}

/** The fixed panel inside it whose width is the rail's or the panel's. */
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

/**
 * Asserts the sidebar mode `state`: its root's data-state (and, collapsed,
 * data-collapsible "icon"), and the container's width once the transition
 * has run.
 */
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

/** The sidebar mode of the project's breakpoint: collapsed at md, expanded at lg. */
const initialState = (page: Page): SidebarState => (isLg(page) ? "expanded" : "collapsed");
const flipped = (state: SidebarState): SidebarState => (state === "expanded" ? "collapsed" : "expanded");

/** Asserts that the document is no wider than the viewport. */
async function expectNoHorizontalOverflow(page: Page): Promise<void> {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  expect(overflow, "page scrolls sideways").toBeLessThanOrEqual(0);
}

/** The left and top edges of every text input of the page's form, in DOM order. */
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
    await page.goto("/");
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
  for (const { route, role } of [
    { route: "/admin/users", role: "admin" },
    { route: `/cards?account_id=${FIXTURE_KEYS.account}`, role: "user" },
  ] as const) {
    test(`${route}: the table container scrolls and stays inside its card`, async ({ page }) => {
      await signIn(page, role);
      await page.goto(route);
      // The generated Table's scroll container inside the DataTable card
      // (web/src/components/ui/table.tsx, web/src/components/data-table.tsx).
      const container = content(page).locator('[data-slot="card"] [data-slot="table-container"]').first();
      await expect(container.getByRole("row").nth(1)).toBeVisible();
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
      await page.goto(route);
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
  let params: RouteParams | undefined;
  let seeded: SeededAuthorization | undefined;

  test.beforeAll(async () => {
    seeded = await seedPendingAuthorization(FIXTURE_KEYS.account);
    params = {
      accountId: FIXTURE_KEYS.account,
      cardNumber: FIXTURE_KEYS.card,
      tranId: await latestTranId(),
      authId: seeded.authId,
      userId: USERS.user.userId,
      code: FIXTURE_KEYS.tranType,
      ruleId: await psql("SELECT min(rule_id) FROM reward_rules"),
    };
  });

  test.afterAll(async () => {
    await seeded?.restore();
  });

  for (const route of APP_ROUTES) {
    test(`${route.path} has no horizontal overflow`, async ({ page }) => {
      if (params === undefined) {
        throw new Error("the route keys were not set up");
      }
      const path = resolveRoute(route.path, params);
      if (route.access !== "public") {
        await signIn(page, route.access === "admin" ? "admin" : "user");
      }
      await page.goto(path);
      await expect(page).toHaveURL(urlPath(path));
      // The sign-on screen renders outside the app shell, so its h1 is not
      // inside #main-content.
      if (route.access === "public") {
        await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      } else {
        await expectScreen(page);
      }
      await expectNoHorizontalOverflow(page);
    });
  }
});
