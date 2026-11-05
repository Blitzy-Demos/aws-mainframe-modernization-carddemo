// o5 admin gate (AAP 0.6.1, 0.10.1): usr_type A is required for every
// /admin/** endpoint and route, reward-rule management included.
//
// - The server decides: without a token every admin endpoint answers 401
//   'Authentication required', a regular user's token 403 'Admin access
//   required' with nothing written, and an administrator's token reads every
//   admin list.
// - RequireAdmin (web/src/routes/guards.tsx) keeps the screens consistent: a
//   regular user sent to an admin route lands on / with one 'Admin access
//   required' toast before any admin request leaves the browser, and a
//   signed-out visit to any user or admin route lands on the sign-on screen
//   with no such toast, also after a sign-on and sign-off.
// - The sidebar shows a regular user the "Card management" section and
//   nothing of "Administration"; an administrator reaches every admin route
//   (the route table's and the function map's) with that section shown.
//   Section labels collapse in the md icon rail, so presence is asserted by
//   attachment and count, not visibility.
// - The 403 stays authoritative when the client is wrong: a regular user's
//   tab that claims type A still gets the server's refusal.
import type { APIRequestContext, Locator, Page } from "@playwright/test";

import { adminMenu, MENU_SECTION_LABELS } from "../src/nav/menu";
import type { components } from "../src/api/schema";

import {
  apiPath,
  apiToken,
  APP_ROUTES,
  bearer,
  checkpoint,
  errorBodySchema,
  expect,
  expectScreen,
  FIXTURE_KEYS,
  functionMap,
  mapInput,
  MESSAGES,
  notifications,
  psql,
  readJson,
  resolveRoute,
  rowInput,
  shown,
  sidebar,
  signIn,
  signOut,
  sqlLiteral,
  test,
  urlPath,
  USERS,
  type Checkpoint,
  type RouteParams,
} from "./fixtures";

type ErrorBody = components["schemas"]["ErrorBody"];
type UserAddRequest = components["schemas"]["UserAddRequest"];
type UserUpdateRequest = components["schemas"]["UserUpdateRequest"];
type TranTypeCreateRequest = components["schemas"]["TranTypeCreateRequest"];
type TranTypeUpdateRequest = components["schemas"]["TranTypeUpdateRequest"];
type RewardRuleRequest = components["schemas"]["RewardRuleRequest"];

type Method = "GET" | "POST" | "PUT" | "DELETE";
type AdminCall = { method: Method; path: string; data?: object };

// A seeded regular user the refused writes name (DUSRSECJ).
const TARGET_USER = "USER0002";
// A type code and a user ID no seeded row holds, for the refused adds.
const UNUSED_TYPE_CODE = "98";
const UNUSED_USER_ID = "E2EGATE1";

// Every admin operation of the API (AAP 0.3.1, 0.3.5), with a body the
// endpoint accepts, so the gate alone decides the answer.
function adminCalls(ruleId: string): AdminCall[] {
  const user: UserAddRequest = {
    first_name: "Gate",
    last_name: "Probe",
    user_id: UNUSED_USER_ID,
    password: "PASS1234",
    usr_type: "U",
  };
  const userUpdate: UserUpdateRequest = { first_name: "Gate", last_name: "Probe", usr_type: "U" };
  const tranType: TranTypeCreateRequest = { type_code: UNUSED_TYPE_CODE, description: "GATE PROBE" };
  const tranTypeUpdate: TranTypeUpdateRequest = { description: "GATE PROBE" };
  const rule: RewardRuleRequest = {
    transaction_type_cd: FIXTURE_KEYS.tranType,
    transaction_category_cd: FIXTURE_KEYS.tranCategory,
    reward_rate: "9.0000",
    active_from: "2022-01-01T00:00:00Z",
  };
  return [
    { method: "GET", path: "/admin/users" },
    { method: "POST", path: "/admin/users", data: user },
    { method: "GET", path: `/admin/users/${TARGET_USER}` },
    { method: "PUT", path: `/admin/users/${TARGET_USER}`, data: userUpdate },
    { method: "DELETE", path: `/admin/users/${TARGET_USER}` },
    { method: "GET", path: "/admin/transaction-types" },
    { method: "POST", path: "/admin/transaction-types", data: tranType },
    { method: "PUT", path: `/admin/transaction-types/${FIXTURE_KEYS.tranType}`, data: tranTypeUpdate },
    { method: "DELETE", path: `/admin/transaction-types/${FIXTURE_KEYS.tranType}` },
    { method: "GET", path: "/admin/reward-rules" },
    { method: "POST", path: "/admin/reward-rules", data: rule },
    { method: "PUT", path: `/admin/reward-rules/${ruleId}`, data: rule },
    { method: "DELETE", path: `/admin/reward-rules/${ruleId}` },
  ];
}

async function send(
  request: APIRequestContext,
  call: AdminCall,
  token: string | null,
): Promise<{ status: number; body: ErrorBody | null }> {
  const url = apiPath(call.path);
  const options = {
    headers: token === null ? {} : bearer(token),
    ...(call.data === undefined ? {} : { data: call.data }),
  };
  const response =
    call.method === "GET"
      ? await request.get(url, options)
      : call.method === "POST"
        ? await request.post(url, options)
        : call.method === "PUT"
          ? await request.put(url, options)
          : await request.delete(url, options);
  const status = response.status();
  return { status, body: status >= 400 ? await readJson(response, errorBodySchema) : null };
}

// A digest of every row the admin endpoints write, read before and after the
// refused calls.
const ADMIN_DATA_DIGEST = `SELECT md5(
  (SELECT coalesce(string_agg(to_jsonb(u)::text, ',' ORDER BY user_id), '') FROM users u) || '|' ||
  (SELECT coalesce(string_agg(to_jsonb(t)::text, ',' ORDER BY tr_type), '') FROM transaction_types t) || '|' ||
  (SELECT coalesce(string_agg(to_jsonb(r)::text, ',' ORDER BY rule_id), '') FROM reward_rules r))`;

async function firstRuleId(): Promise<string> {
  const ruleId = await psql("SELECT min(rule_id) FROM reward_rules");
  expect(ruleId, "a seeded reward rule").toMatch(/^[0-9]+$/);
  return ruleId;
}

test.describe("server gate", () => {
  // Every row the probes address, taken before each test and put back after
  // it, failed or not, so an admitted write cannot outlast the test that
  // caught it: the probed and the added user, the probed and the added type
  // with their categories (listed after the type they reference), and every
  // reward rule, since an admitted add takes a new rule ID. Each test compares
  // ADMIN_DATA_DIGEST itself, before this restore runs. The admin endpoints
  // write no other table: no table references users, and the reward-rule
  // delete only ends a rule, leaving the accruals that reference it as they
  // are.
  let saved: Checkpoint | undefined;

  test.beforeEach(async () => {
    const typeCodes = `${sqlLiteral(FIXTURE_KEYS.tranType)}, ${sqlLiteral(UNUSED_TYPE_CODE)}`;
    saved = await checkpoint([
      { table: "users", where: `user_id IN (${sqlLiteral(TARGET_USER)}, ${sqlLiteral(UNUSED_USER_ID)})` },
      { table: "transaction_types", where: `tr_type IN (${typeCodes})` },
      { table: "transaction_categories", where: `trc_type_code IN (${typeCodes})` },
      { table: "reward_rules", where: "TRUE" },
    ]);
  });

  test.afterEach(async () => {
    await saved?.restore();
  });

  test("every admin endpoint answers 401 without a token", async ({ request }) => {
    const before = await psql(ADMIN_DATA_DIGEST);
    for (const call of adminCalls(await firstRuleId())) {
      const { status, body } = await send(request, call, null);
      expect(status, `${call.method} ${call.path}`).toBe(401);
      expect(body?.message, `${call.method} ${call.path}`).toBe(MESSAGES.AUTHENTICATION_REQUIRED);
    }
    expect(await psql(ADMIN_DATA_DIGEST)).toBe(before);
  });

  test("every admin endpoint answers 403 to a regular user and writes nothing", async ({ request }) => {
    const token = await apiToken(request, "user");
    const before = await psql(ADMIN_DATA_DIGEST);
    for (const call of adminCalls(await firstRuleId())) {
      const { status, body } = await send(request, call, token);
      expect(status, `${call.method} ${call.path}`).toBe(403);
      expect(body?.message, `${call.method} ${call.path}`).toBe(MESSAGES.ADMIN_ACCESS_REQUIRED);
    }
    expect(await psql(ADMIN_DATA_DIGEST)).toBe(before);
  });

  test("an administrator reads every admin resource", async ({ request }) => {
    const token = await apiToken(request, "admin");
    for (const call of adminCalls(await firstRuleId()).filter((c) => c.method === "GET")) {
      const { status } = await send(request, call, token);
      expect(status, `${call.method} ${call.path}`).toBe(200);
    }
  });
});

test.describe("UI gate", () => {
  let params: RouteParams | undefined;

  test.beforeAll(async () => {
    params = {
      userId: USERS.user.userId,
      code: FIXTURE_KEYS.tranType,
      ruleId: await firstRuleId(),
    };
  });

  // Every admin route: the route table's and every /admin route or target of
  // the function map, resolved with the seeded keys.
  function adminPaths(): string[] {
    if (params === undefined) {
      throw new Error("the admin route keys were not set up");
    }
    const keys = params;
    const isAdminRoute = (path: string): boolean => path === "/admin" || path.startsWith("/admin/");
    const mapRoutes = functionMap().rows.flatMap((row) =>
      row.control === null ? [] : [row.control.route, row.control.target ?? row.control.route],
    );
    const routes = [...APP_ROUTES.filter((route) => route.access === "admin").map((route) => route.path), ...mapRoutes];
    return [...new Set(routes.filter(isAdminRoute).map((route) => resolveRoute(route, keys)))];
  }

  // The user routes the fixture keys resolve; the TRAN-ID and authorization
  // routes need seeded rows, and navigation.spec.ts opens them signed out.
  function userPaths(): string[] {
    return APP_ROUTES.filter((route) => route.access === "user" && !/:(?:tranId|authId)\b/.test(route.path)).map(
      (route) => resolveRoute(route.path, { accountId: FIXTURE_KEYS.account, cardNumber: FIXTURE_KEYS.card }),
    );
  }

  // The sign-on screen's User ID textbox (F-COSGN00-001).
  const signOnUserId = (page: Page): Locator => mapInput(page, rowInput("F-COSGN00-001", 0));

  const adminToast = (page: Page): Locator =>
    notifications(page).getByText(shown(MESSAGES.ADMIN_ACCESS_REQUIRED), { exact: true });

  // Every /api/admin request the page sends from now on.
  function recordAdminRequests(page: Page): string[] {
    const sent: string[] = [];
    page.on("request", (request) => {
      if (new URL(request.url()).pathname.startsWith(apiPath("/admin"))) {
        sent.push(`${request.method()} ${request.url()}`);
      }
    });
    return sent;
  }

  test("a regular user sent to any admin route lands on / with one toast and no admin request", async ({ page }) => {
    await signIn(page, "user");
    const sent = recordAdminRequests(page);
    for (const path of adminPaths()) {
      // page.goto loads a fresh document, so the previous route's toast is
      // gone and the one asserted here is this route's own; the session
      // survives the reload in sessionStorage.
      await page.goto(path);
      await expect(page, path).toHaveURL(urlPath(USERS.user.landing));
      await expect(adminToast(page), path).toBeVisible();
      await expect(adminToast(page), path).toHaveCount(1);
    }
    expect(sent).toEqual([]);
  });

  test("USER0001's sidebar has Card management and nothing of Administration", async ({ page }) => {
    await signIn(page, "user");
    const nav = sidebar(page);
    await expect(nav.getByText(MENU_SECTION_LABELS.user, { exact: true })).toBeAttached();
    await expect(nav.getByRole("list", { name: MENU_SECTION_LABELS.user, exact: true })).toBeAttached();
    await expect(nav.getByText(MENU_SECTION_LABELS.admin)).toHaveCount(0);
    for (const entry of adminMenu) {
      await expect(nav.getByRole("link", { name: entry.label, exact: true }), entry.label).toHaveCount(0);
    }
  });

  test("ADMIN001 reaches every admin route with the Administration section", async ({ page }) => {
    await signIn(page, "admin");
    for (const path of adminPaths()) {
      await page.goto(path);
      await expect(page, path).toHaveURL(urlPath(path));
      await expectScreen(page);
      await expect(sidebar(page).getByText(MENU_SECTION_LABELS.admin, { exact: true }), path).toBeAttached();
      await expect(adminToast(page), path).toHaveCount(0);
    }
  });

  test("a signed-out visit to any user or admin route lands on the sign-on screen without the admin toast", async ({ page }) => {
    // Each test opens a fresh browser context, so this one holds no session.
    for (const path of [...userPaths(), ...adminPaths()]) {
      await page.goto(path);
      await expect(page, path).toHaveURL(urlPath("/login"));
      await expect(signOnUserId(page), path).toBeVisible();
      await expect(page.getByText(shown(MESSAGES.ADMIN_ACCESS_REQUIRED), { exact: true })).toHaveCount(0);
    }
  });

  test("after a sign-on and sign-off, /admin lands on the sign-on screen", async ({ page }) => {
    await signIn(page, "admin");
    await signOut(page, "admin");
    await page.goto("/admin");
    await expect(page).toHaveURL(urlPath("/login"));
    await expect(signOnUserId(page)).toBeVisible();
  });

  test("a regular user whose tab claims type A still gets the server's 403", async ({ page }) => {
    await signIn(page, "user");
    // Rewrite the client's copy of the login response, wherever the session
    // module keeps it, so RequireAdmin lets the user through.
    const tampered = await page.evaluate(() => {
      for (let index = 0; index < window.sessionStorage.length; index += 1) {
        const key = window.sessionStorage.key(index);
        const text = key === null ? null : window.sessionStorage.getItem(key);
        if (key !== null && text !== null && text.includes('"usr_type"')) {
          const value: unknown = JSON.parse(text);
          if (typeof value === "object" && value !== null) {
            window.sessionStorage.setItem(key, JSON.stringify({ ...value, usr_type: "A" }));
            return true;
          }
        }
      }
      return false;
    });
    expect(tampered, "the session is kept in sessionStorage").toBe(true);
    const refused = page.waitForResponse(
      (response) => new URL(response.url()).pathname === apiPath("/admin/users"),
    );
    await page.goto("/admin/users");
    expect((await refused).status()).toBe(403);
    await expect(
      notifications(page).getByText(shown(MESSAGES.ADMIN_ACCESS_REQUIRED), { exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("cell", { name: USERS.admin.userId, exact: true })).toHaveCount(0);
  });
});
