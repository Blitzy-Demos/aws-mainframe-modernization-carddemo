// o8 rewards (AAP 0.3.5, 0.10.1): the reward screens over the running stack.
//
// - A stored balance of 9007199254740993 (2^53 + 1, the first integer a
//   JavaScript number cannot hold) arrives exactly: as the API's decimal
//   string, in RewardsPage's "Available points" and "Points earned" cards and
//   in the dashboard's "Reward Points" card, with its rounded neighbour
//   nowhere. Each of those cards shows the 16 digits on one line, inside the
//   card. The balance of account 1 is set through `docker compose exec
//   postgres psql` and restored afterwards; every accrual-producing step uses
//   the reward account (50) instead, so nothing else moves that balance.
// - An account that has earned no points (no balance row, no accrual) shows
//   "0" in each balance card and the empty-history text, with Previous and
//   Next both rendered and disabled.
// - The reward history pages newest first, HISTORY_PAGE_SIZE rows at a time:
//   Next shows the next page and Previous returns to the first, where it is
//   disabled. The spec tops the account's history up through CT02 adds when
//   it holds too few accruals for a second page, and removes them again.
// - A CT02 add, through the API and through the screens, accrues
//   ceil(amount × rate) under the best active rule (highest rate, lowest
//   rule_id on a tie), and the newest history row shows it against the
//   transaction's row identity.
// - Administrators list every rule, active and soft-deleted, in rule order,
//   create, edit and soft-delete a rule through the screens with the
//   requests they send, and a second delete of an ended rule changes nothing.
// - The new-rule form, submitted with Save, answers each rule check of the
//   rewards specification on its own field: the exact text in the field's
//   FieldError, the input marked invalid, focus moved from Save to it, and no
//   rule stored.
import type { APIRequestContext, Locator, Page } from "@playwright/test";
import { z } from "zod";

import type { components } from "../src/api/schema";

import {
  addTransactionViaApi,
  apiPath,
  apiToken,
  bearer,
  captureResponse,
  cellUnder,
  checkpoint,
  confirmDialog,
  content,
  controlLocator,
  dataRows,
  escapeRegExp,
  expect,
  expectMessage,
  FIXTURE_KEYS,
  gotoApp,
  isRequestTo,
  mapControl,
  mapInput,
  MESSAGES,
  nameMatcher,
  notifications,
  processingClock,
  processingDate,
  psql,
  readJson,
  REWARD_SCREENS,
  rewardBalanceResponseSchema,
  rewardRuleResponseSchema,
  rowEndpoint,
  rowInput,
  setRewardBalance,
  signIn,
  sqlLiteral,
  tableByCaption,
  test,
  topUpAccruals,
  transactionAddResponseSchema,
  transactionWriteCheckpoint,
  UNMAPPED_LABELS,
  urlPath,
  type AccrualTopUp,
  type AddedTransaction,
  type Checkpoint,
  type LabelText,
} from "./fixtures";

type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];

// 2^53 + 1: a JavaScript number would show it as 9007199254740992.
const EXACT_POINTS = "9007199254740993";
const ROUNDED_POINTS = "9007199254740992";

// The CT02 amount the accrual tests add, and its audited screen form
// (V-COTRN02C-021).
const ADD_AMOUNT = "12.34";
const ADD_AMOUNT_ENTERED = "+00000012.34";

const SCREENS = REWARD_SCREENS;

// A text input by its label, in the page content.
function textbox(page: Page, label: LabelText): Locator {
  return content(page).getByRole("textbox", { name: nameMatcher(label) });
}

// RewardsPage for `accountId`: the account lookup, submitted with "Look up".
async function openRewards(page: Page, accountId: string): Promise<void> {
  await gotoApp(page, SCREENS.rewards.route);
  await textbox(page, UNMAPPED_LABELS.rewards.accountId).fill(accountId);
  await content(page).getByRole("button", { name: SCREENS.rewards.lookUp, exact: true }).click();
}

// The dashboard account lookup (F-COACTVW-001) for `accountId`.
async function lookUpOnDashboard(page: Page, accountId: string): Promise<void> {
  await gotoApp(page, mapControl("F-COACTVW-001").route);
  await mapInput(content(page), rowInput("F-COACTVW-001", 0)).fill(accountId);
  await controlLocator(page, mapControl("F-COACTVW-001")).click();
}

// The SummaryCard titled `title` (a group named by its title).
function summaryCard(page: Page, title: string): Locator {
  return content(page).getByRole("group", { name: title, exact: true });
}

// Asserts that the card `title` shows exactly `value`, and `roundedAway`
// nowhere in it.
async function expectCardValue(page: Page, title: string, value: string, roundedAway?: string): Promise<void> {
  const card = summaryCard(page, title);
  await expect(card).toHaveCount(1);
  await expect(card.getByText(value, { exact: true })).toBeVisible();
  if (roundedAway !== undefined) {
    await expect(card).not.toContainText(roundedAway);
  }
}

// Asserts that the card `title` shows `value` as one number: its text lies on
// a single line (one line box, however many client rects the range returns
// for it), and neither the figure nor the card is wider than its box.
async function expectValueOnOneLine(page: Page, title: string, value: string): Promise<void> {
  const figure = summaryCard(page, title).getByText(value, { exact: true });
  await expect(figure).toBeVisible();
  const layout = await figure.evaluate((element) => {
    const range = document.createRange();
    range.selectNodeContents(element);
    const lineTops = new Set(Array.from(range.getClientRects(), (rect) => Math.round(rect.top)));
    const card = element.closest('[data-slot="card"]');
    return {
      lines: lineTops.size,
      figureOverflow: element.scrollWidth - element.clientWidth,
      cardOverflow: card === null ? null : card.scrollWidth - card.clientWidth,
    };
  });
  expect(layout, `the "${title}" card's figure ${value}`).toEqual({ lines: 1, figureOverflow: 0, cardOverflow: 0 });
}

// RewardsPage's history table.
function history(page: Page): Locator {
  return tableByCaption(content(page), SCREENS.rewards.historyCaption);
}

// A paging control of the page's DataTable (PF7 "Previous", PF8 "Next").
function pagingControl(page: Page, name: "Previous" | "Next"): Locator {
  return content(page).getByRole("button", { name, exact: true });
}

// The points and rule the RewardEngine credits for a `typeCode`/`categoryCode`
// add of `amount` at the processing instant (processingClock), computed by
// PostgreSQL: among the rules active at that instant whose bounds hold the
// amount, the highest rate wins and a tie goes to the lowest rule_id;
// points = ceil(amount × rate). "points|rule_id".
async function expectedAccrual(typeCode: string, categoryCode: string, amount: string): Promise<string> {
  if (!/^[0-9]+\.[0-9]{2}$/.test(amount)) {
    throw new Error(`not an amount: ${amount}`);
  }
  const value = `${sqlLiteral(amount)}::numeric`;
  const clock = processingClock();
  const expected = await psql(
    `SELECT ceil(${value} * reward_rate)::bigint || '|' || rule_id
       FROM reward_rules
      WHERE transaction_type_cd = ${sqlLiteral(typeCode)}
        AND transaction_category_cd = ${sqlLiteral(categoryCode)}::integer
        AND active_from <= ${clock.instant} AND (active_to IS NULL OR ${clock.instant} < active_to)
        AND (min_transaction_amount IS NULL OR min_transaction_amount <= ${value})
        AND (max_transaction_amount IS NULL OR max_transaction_amount >= ${value})
      ORDER BY reward_rate DESC, rule_id
      LIMIT 1`,
  );
  expect(
    expected,
    `a ${typeCode}/${categoryCode} rule active at ${clock.label} accrues for ${amount}`,
  ).toMatch(/^[0-9]+\|[0-9]+$/);
  return expected;
}

// PostgreSQL's clock_timestamp() as text: one bound of an add the system
// clock stamps.
async function databaseClock(): Promise<string> {
  return psql("SELECT clock_timestamp()");
}

// Asserts that the accrual of the live TRAN-ID `tranId` was stamped at the
// processing clock expectedAccrual chose its rule at: exactly
// CARDDEMO_CLOCK_OVERRIDE when this run sets it, else between `before` and
// `after`, databaseClock readings taken just before and just after the add.
// A mismatch means the API under test and this Playwright run do not share
// CARDDEMO_CLOCK_OVERRIDE, so the expected rule was chosen at another instant.
async function expectAccruedAtProcessingClock(tranId: string, before: string, after: string): Promise<void> {
  const clock = processingClock();
  const stamped =
    clock.override === null
      ? `a.created_at BETWEEN ${sqlLiteral(before)}::timestamptz AND ${sqlLiteral(after)}::timestamptz`
      : `a.created_at = ${clock.instant}`;
  const row = await psql(
    `SELECT ${stamped}, a.created_at
       FROM transaction_master t JOIN reward_accruals a ON a.transaction_id = t.transaction_id
      WHERE t.tran_id = ${sqlLiteral(tranId)}`,
  );
  const [holds = "", createdAt = ""] = row.split("|");
  const found = row === "" ? "no accrual" : `an accrual stamped ${createdAt}`;
  const wanted = clock.override === null ? `within the add (${before} to ${after})` : `at ${clock.label}`;
  const agreement =
    clock.override === null
      ? "CARDDEMO_CLOCK_OVERRIDE unset"
      : `CARDDEMO_CLOCK_OVERRIDE=${clock.override}`;
  expect(
    holds,
    `TRAN-ID ${tranId} has ${found}, not one ${wanted}: the API under test must run with ${agreement}, as this Playwright run does`,
  ).toBe("t");
}

// "transaction_id|points_earned|rule_id" of the accrual of the live TRAN-ID
// `tranId`.
async function accrualOf(tranId: string): Promise<string> {
  return psql(
    `SELECT t.transaction_id || '|' || a.points_earned || '|' || a.rule_id
       FROM transaction_master t JOIN reward_accruals a ON a.transaction_id = t.transaction_id
      WHERE t.tran_id = ${sqlLiteral(tranId)}`,
  );
}

async function rules(request: APIRequestContext): Promise<RewardRuleResponse[]> {
  const token = await apiToken(request, "admin");
  const response = await request.get(apiPath("/admin/reward-rules"), { headers: bearer(token) });
  expect(response.status()).toBe(200);
  return readJson(response, z.array(rewardRuleResponseSchema));
}

// Orders decimal integer strings by value, without converting them to numbers.
function byDecimal(a: string, b: string): number {
  if (a.length !== b.length) {
    return a.length - b.length;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

// The Status badge text the rule list shows for each server `status`.
const RULE_STATUS_LABELS = {
  active: SCREENS.ruleList.active,
  scheduled: SCREENS.ruleList.scheduled,
  ended: SCREENS.ruleList.ended,
} as const satisfies Record<RewardRuleResponse["status"], string>;

// The table row of the rule `ruleId` on the rule list.
function ruleRow(page: Page, ruleId: string): Locator {
  return tableByCaption(content(page), SCREENS.ruleList.caption)
    .getByRole("row")
    .filter({ has: page.getByRole("cell", { name: ruleId, exact: true }) });
}

type RuleField = keyof typeof UNMAPPED_LABELS.rewardRule;

// The reward-rule form's fields in screen order.
const RULE_FIELDS = [
  "typeCode",
  "categoryCode",
  "rate",
  "minAmount",
  "maxAmount",
  "activeFrom",
  "activeTo",
] as const satisfies readonly RuleField[];

// A rule the server accepts: the rule-test pair, unbounded and open-ended.
const VALID_RULE: Readonly<Record<RuleField, string>> = {
  typeCode: FIXTURE_KEYS.ruleTestTypeCode,
  categoryCode: FIXTURE_KEYS.ruleTestCategoryCode,
  rate: "2.5000",
  minAmount: "",
  maxAmount: "",
  activeFrom: "2022-01-01",
  activeTo: "",
};

// The three rule checks (AAP 0.3.5), each as the field values that fail it,
// the field it is reported on and its messages.rs text. The form's schema
// answers a signed rate before any request is sent; the server answers the
// two cross-field checks with 400 and the field in `fields[]`. The text is
// the same on either path.
const INVALID_RULES = [
  {
    check: "a negative reward rate",
    entered: { rate: "-1" },
    field: "rate",
    message: MESSAGES.REWARD_RATE_NEGATIVE,
  },
  {
    check: "a minimum above the maximum",
    entered: { minAmount: "100.00", maxAmount: "10.00" },
    field: "minAmount",
    message: MESSAGES.REWARD_MIN_EXCEEDS_MAX,
  },
  {
    check: "an end before the start",
    entered: { activeFrom: "2022-02-01", activeTo: "2022-01-01" },
    field: "activeTo",
    message: MESSAGES.REWARD_ACTIVE_TO_BEFORE_FROM,
  },
] as const satisfies readonly {
  check: string;
  entered: Partial<Record<RuleField, string>>;
  field: RuleField;
  message: string;
}[];

// Fills every field of the reward-rule form with `values`, in screen order.
async function fillRuleForm(page: Page, values: Readonly<Record<RuleField, string>>): Promise<void> {
  for (const field of RULE_FIELDS) {
    await textbox(page, UNMAPPED_LABELS.rewardRule[field]).fill(values[field]);
  }
}

// The FieldError of `input`: the alert inside the input's Field.
function fieldErrorOf(input: Locator): Locator {
  return input.locator('xpath=ancestor::*[@data-slot="field"][1]').getByRole("alert");
}

test.describe("a balance above 2^53", () => {
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await setRewardBalance(FIXTURE_KEYS.account, EXACT_POINTS);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test("the API returns it as the exact decimal string", async ({ request }) => {
    const token = await apiToken(request, "user");
    const response = await request.get(apiPath(`/rewards/balance/${FIXTURE_KEYS.account}`), {
      headers: bearer(token),
    });
    expect(response.status()).toBe(200);
    expect(await response.text()).toContain(`"available_points":"${EXACT_POINTS}"`);
    const balance = await readJson(response, rewardBalanceResponseSchema);
    expect(balance).toEqual({
      account_id: FIXTURE_KEYS.account,
      total_points_earned: EXACT_POINTS,
      total_points_redeemed: "0",
      available_points: EXACT_POINTS,
    });
  });

  test("RewardsPage shows it exactly in its balance cards", async ({ page }) => {
    await signIn(page, "user");
    await openRewards(page, FIXTURE_KEYS.account);
    await expectCardValue(page, SCREENS.rewards.available, EXACT_POINTS, ROUNDED_POINTS);
    await expectCardValue(page, SCREENS.rewards.earned, EXACT_POINTS, ROUNDED_POINTS);
    await expectCardValue(page, SCREENS.rewards.redeemed, "0");
    await expect(content(page)).not.toContainText(ROUNDED_POINTS);
    await expectValueOnOneLine(page, SCREENS.rewards.available, EXACT_POINTS);
    await expectValueOnOneLine(page, SCREENS.rewards.earned, EXACT_POINTS);
  });

  test("the dashboard's Reward Points card shows it exactly", async ({ page }) => {
    await signIn(page, "user");
    await lookUpOnDashboard(page, FIXTURE_KEYS.account);
    await expectCardValue(page, SCREENS.dashboardPoints, EXACT_POINTS, ROUNDED_POINTS);
    await expect(content(page)).not.toContainText(ROUNDED_POINTS);
    await expectValueOnOneLine(page, SCREENS.dashboardPoints, EXACT_POINTS);
  });
});

test("RewardsPage answers an account not on file with the API's text", async ({ page }) => {
  await signIn(page, "user");
  await openRewards(page, FIXTURE_KEYS.missingAccount);
  await expectMessage(page, MESSAGES.ACCOUNT_NOT_FOUND);
});

test("RewardsPage shows the zero balance and the empty history of an account that has earned no points", async ({ page }) => {
  const account = `${sqlLiteral(FIXTURE_KEYS.zeroRewardAccount)}::bigint`;
  expect(
    await psql(
      `SELECT (SELECT count(*) FROM reward_balances WHERE account_id = ${account})
           || '|' || (SELECT count(*) FROM reward_accruals WHERE account_id = ${account})`,
    ),
    "reward balance rows|accruals of the zero-reward account",
  ).toBe("0|0");
  await signIn(page, "user");
  await openRewards(page, FIXTURE_KEYS.zeroRewardAccount);
  for (const title of [SCREENS.rewards.earned, SCREENS.rewards.redeemed, SCREENS.rewards.available]) {
    await expectCardValue(page, title, "0");
  }
  await expect(dataRows(history(page))).toHaveText([SCREENS.rewards.emptyHistory]);
  // An empty history has neither key, yet both paging controls are offered,
  // as on every list: each is rendered once and disabled (aria-disabled).
  for (const name of ["Previous", "Next"] as const) {
    const control = pagingControl(page, name);
    await expect(control, `${name} on an empty history`).toHaveCount(1);
    await expect(control, `${name} on an empty history`).toBeDisabled();
    await expect(control, `${name} on an empty history`).toHaveAttribute("aria-disabled", "true");
  }
});

test.describe("reward history pagination", () => {
  const pageSize = SCREENS.rewards.historyPageSize;
  const account = `${sqlLiteral(FIXTURE_KEYS.rewardAccount)}::bigint`;
  let topUp: AccrualTopUp | undefined;

  // The accrual IDs of the reward account, newest first (the history order),
  // `offset` rows on.
  const newestIds = async (offset: string): Promise<string[]> =>
    (
      await psql(
        `SELECT accrual_id FROM reward_accruals WHERE account_id = ${account}
          ORDER BY created_at DESC, accrual_id DESC
          LIMIT ${sqlLiteral(pageSize)}::bigint OFFSET ${sqlLiteral(offset)}::bigint`,
      )
    ).split("\n");

  test.beforeAll(async ({ request }) => {
    // A second page needs more than one page of accruals.
    const minimum = await psql(`SELECT ${sqlLiteral(pageSize)}::bigint + 1`);
    topUp = await topUpAccruals(request, await apiToken(request, "user"), FIXTURE_KEYS.rewardAccount, minimum);
  });

  test.afterAll(async () => {
    await topUp?.restore();
  });

  test("shows HISTORY_PAGE_SIZE rows newest first; Next pages on, Previous returns and is disabled on page one", async ({ page }) => {
    const firstPage = await newestIds("0");
    expect(String(firstPage.length), "accruals on the first page").toBe(pageSize);
    const secondPageFirst = (await newestIds(pageSize))[0];
    if (secondPageFirst === undefined || secondPageFirst === "") {
      throw new Error("the reward account has no second page of accruals");
    }
    await signIn(page, "user");
    await openRewards(page, FIXTURE_KEYS.rewardAccount);
    const table = history(page);
    await expect(table.getByRole("columnheader")).toHaveText([...SCREENS.rewards.historyColumns]);
    const rows = dataRows(table);
    await expect(rows).toHaveCount(firstPage.length);
    const shownIds: string[] = [];
    for (let index = 0; index < firstPage.length; index += 1) {
      shownIds.push((await (await cellUnder(table, rows.nth(index), "Accrual ID")).innerText()).trim());
    }
    expect(shownIds, "the first page, newest first").toEqual(firstPage);

    const previous = pagingControl(page, "Previous");
    const next = pagingControl(page, "Next");
    const firstAccrual = await cellUnder(table, rows.first(), "Accrual ID");
    await expect(previous).toBeDisabled();
    await expect(next).toBeEnabled();
    await next.click();
    await expect(firstAccrual).toHaveText(secondPageFirst);
    await expect(previous).toBeEnabled();
    await previous.click();
    await expect(firstAccrual).toHaveText(firstPage[0] ?? "");
    await expect(previous).toBeDisabled();
  });
});

test.describe("a CT02 add through the API", () => {
  let added: AddedTransaction | undefined;

  test.afterAll(async () => {
    await added?.restore();
  });

  test("accrues ceil(amount × rate) under the best active rule", async ({ request }) => {
    const expected = await expectedAccrual(FIXTURE_KEYS.tranType, FIXTURE_KEYS.tranCategory, ADD_AMOUNT);
    const token = await apiToken(request, "user");
    const before = await databaseClock();
    added = await addTransactionViaApi(request, token, {
      account_id: FIXTURE_KEYS.rewardAccount,
      tran_amt: ADD_AMOUNT_ENTERED,
    });
    const after = await databaseClock();
    expect(added.status).toBe(201);
    expect(added.tranId).toMatch(/^[0-9]{16}$/);
    await expectAccruedAtProcessingClock(added.tranId ?? "", before, after);
    const accrual = await accrualOf(added.tranId ?? "");
    expect(accrual.split("|").slice(1).join("|"), "points|rule_id of the new accrual").toBe(expected);
  });
});

test.describe("a CT02 add through the screens", () => {
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await transactionWriteCheckpoint([FIXTURE_KEYS.rewardAccount]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test("accrues for the reward account and the newest history row shows it", async ({ page }) => {
    const expected = await expectedAccrual(FIXTURE_KEYS.tranType, FIXTURE_KEYS.tranCategory, ADD_AMOUNT);
    const labels = UNMAPPED_LABELS.transactionAdd;
    await signIn(page, "user");
    await gotoApp(page, mapControl("F-COTRN02-001").route);
    for (const [label, value] of [
      [labels.accountId, FIXTURE_KEYS.rewardAccount],
      [labels.cardNumber, FIXTURE_KEYS.rewardCard],
      [labels.typeCode, FIXTURE_KEYS.tranType],
      [labels.categoryCode, FIXTURE_KEYS.tranCategory],
      [labels.source, "POS TERM"],
      [labels.description, "E2E SCREEN PURCHASE"],
      [labels.amount, ADD_AMOUNT_ENTERED],
      [labels.origDate, "2022-07-01"],
      [labels.procDate, "2022-07-01"],
      [labels.merchantId, "800000001"],
      [labels.merchantName, "E2E FIXTURE MERCHANT"],
      [labels.merchantCity, "SEATTLE"],
      [labels.merchantZip, "98101"],
    ] as const) {
      await textbox(page, label).fill(value);
    }
    await controlLocator(page, mapControl("F-COTRN02-001")).click();
    await expect(confirmDialog(page)).toBeVisible();
    const before = await databaseClock();
    const [response] = await Promise.all([
      page.waitForResponse((r) => isRequestTo(r.request(), rowEndpoint("F-COTRN02-002"))),
      controlLocator(page, mapControl("F-COTRN02-002")).click(),
    ]);
    const after = await databaseClock();
    expect(response.status()).toBe(201);
    expect(response.request().postDataJSON()).toMatchObject({
      account_id: FIXTURE_KEYS.rewardAccount,
      tran_amt: ADD_AMOUNT_ENTERED,
      confirm: "Y",
    });
    const body = await readJson(response, transactionAddResponseSchema);

    // The success toast is the COTRN02C text with the new TRAN-ID.
    const added = new RegExp(
      `${escapeRegExp(MESSAGES.TRANSACTION_ADDED_SUCCESSFULLY.trim())}\\s+${escapeRegExp(MESSAGES.YOUR_TRAN_ID_IS.trim())}\\s+([0-9]{16})\\.`,
    );
    const toast = notifications(page).getByText(added);
    await expect(toast).toBeVisible();
    const tranId = added.exec(await toast.innerText())?.[1];
    if (tranId === undefined) {
      throw new Error("the success toast names no TRAN-ID");
    }
    expect(tranId).toBe(body.transaction.tran_id);
    await expectAccruedAtProcessingClock(tranId, before, after);
    const [transactionId = "", ...credited] = (await accrualOf(tranId)).split("|");
    expect(credited.join("|"), "points|rule_id of the new accrual").toBe(expected);
    const [points = ""] = expected.split("|");

    await openRewards(page, FIXTURE_KEYS.rewardAccount);
    const table = history(page);
    const newest = dataRows(table).first();
    await expect(await cellUnder(table, newest, "Points earned")).toHaveText(points);
    await expect(await cellUnder(table, newest, "Transaction row")).toHaveText(transactionId);
  });
});

test.describe("admin reward rules", () => {
  test.describe.configure({ mode: "serial" });
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([{ table: "reward_rules", where: "TRUE" }]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  const ruleTable = (page: Page): Locator => tableByCaption(content(page), SCREENS.ruleList.caption);
  const formButton = (page: Page, name: string): Locator =>
    content(page).getByRole("button", { name, exact: true });
  const ruleEndpoint = (method: "POST" | "PUT" | "DELETE", ruleId?: string) => ({
    method,
    path: ruleId === undefined ? "/admin/reward-rules" : `/admin/reward-rules/${ruleId}`,
  });

  test("the list shows every rule in rule order", async ({ page, request }) => {
    const expected = await rules(request);
    expect(expected.length, "seeded rules").toBeGreaterThan(0);
    await signIn(page, "admin");
    await gotoApp(page, SCREENS.ruleList.route);
    await expect(content(page).getByRole("heading", { level: 1 })).toHaveText(SCREENS.ruleList.title);
    const table = ruleTable(page);
    await expect(table.getByRole("columnheader")).toHaveText([...SCREENS.ruleList.columns]);
    const ids = expected.map((rule) => rule.rule_id);
    expect(ids, "the API lists the rules by rule_id").toEqual([...ids].sort(byDecimal));
    // The table shows the list one window at a time, and Next moves to the
    // following window without a request, so the windows are walked until
    // every rule has been seen, each window holding the next rules in order.
    const rows = dataRows(table);
    const next = pagingControl(page, "Next");
    const seen: string[] = [];
    while (seen.length < expected.length) {
      const firstId = expected[seen.length]?.rule_id ?? "";
      if (seen.length > 0) {
        await expect(next, `Next after rule ${seen.at(-1)}`).toBeEnabled();
        await next.click();
      }
      await expect(await cellUnder(table, rows.first(), "Rule ID")).toHaveText(firstId);
      const shown = await rows.count();
      const window = expected.slice(seen.length, seen.length + shown);
      expect(window.length, `the window from rule ${firstId} holds only listed rules`).toBe(shown);
      const tops: number[] = [];
      for (const rule of window) {
        const row = ruleRow(page, rule.rule_id);
        await expect(row).toHaveCount(1);
        await expect(row).toContainText(rule.reward_rate);
        if (rule.min_transaction_amount === null) {
          await expect(await cellUnder(table, row, "Min amount")).toHaveText(SCREENS.ruleList.none);
        }
        if (rule.max_transaction_amount === null) {
          await expect(await cellUnder(table, row, "Max amount")).toHaveText(SCREENS.ruleList.none);
        }
        await expect(await cellUnder(table, row, "Status")).toHaveText(RULE_STATUS_LABELS[rule.status]);
        const box = await row.boundingBox();
        if (box === null) {
          throw new Error(`the row of rule ${rule.rule_id} is not rendered`);
        }
        tops.push(box.y);
        seen.push(rule.rule_id);
      }
      expect(tops, `the window from rule ${firstId} follows rule_id order`).toEqual(
        [...tops].sort((a, b) => a - b),
      );
    }
    expect(seen, "every rule, in rule order").toEqual(ids);
    await expect(next, "Next on the last window").toBeDisabled();
  });

  test("the edit route loads the rule", async ({ page, request }) => {
    const [rule] = await rules(request);
    if (rule === undefined) {
      throw new Error("no reward rule to edit");
    }
    await signIn(page, "admin");
    await gotoApp(page, `/admin/reward-rules/${rule.rule_id}/edit`);
    await expect(
      content(page).getByRole("textbox", { name: nameMatcher(UNMAPPED_LABELS.rewardRule.rate) }),
    ).toHaveValue(rule.reward_rate);
  });

  test("a rule is created, edited and soft-deleted through the screens", async ({ page }) => {
    const labels = UNMAPPED_LABELS.rewardRule;
    // The processing clock's UTC day: the rule starts no later than the
    // instant the soft delete below stamps as its active_to.
    const today = await processingDate();
    await signIn(page, "admin");
    await gotoApp(page, SCREENS.ruleList.route);
    const table = ruleTable(page);

    const ruleId = await test.step("New rule: 07/0001 at 2.5000 from today, unbounded and open-ended", async () => {
      await content(page).getByRole("link", { name: SCREENS.ruleList.newRule, exact: true }).click();
      await expect(page).toHaveURL(urlPath(SCREENS.ruleForm.newRoute));
      await expect(content(page).getByRole("heading", { level: 1 })).toHaveText(SCREENS.ruleForm.newTitle);
      await textbox(page, labels.typeCode).fill(FIXTURE_KEYS.ruleTestTypeCode);
      await textbox(page, labels.categoryCode).fill(FIXTURE_KEYS.ruleTestCategoryCode);
      await textbox(page, labels.rate).fill("2.5000");
      await textbox(page, labels.activeFrom).fill(today);
      // Save navigates to the rule list, after which Chromium may no longer
      // hold the browser's copy of the 201 body, so the body is read from the
      // copy captureResponse takes through route.fetch().
      const created = await captureResponse(page, ruleEndpoint("POST"), () =>
        formButton(page, SCREENS.ruleForm.save).click(),
      );
      expect(created.status()).toBe(201);
      const rule = await readJson(created, rewardRuleResponseSchema);
      expect(rule).toMatchObject({
        transaction_type_cd: FIXTURE_KEYS.ruleTestTypeCode,
        transaction_category_cd: FIXTURE_KEYS.ruleTestCategoryCode,
        reward_rate: "2.5000",
        min_transaction_amount: null,
        max_transaction_amount: null,
        active_to: null,
        status: "active",
      });
      expect(rule.active_from.startsWith(`${today}T`), `active_from ${rule.active_from} is today`).toBe(true);
      await expect(page).toHaveURL(urlPath(SCREENS.ruleList.route));
      const row = ruleRow(page, rule.rule_id);
      await expect(row).toHaveCount(1);
      await expect(await cellUnder(table, row, "Status")).toHaveText(SCREENS.ruleList.active);
      return rule.rule_id;
    });
    const row = ruleRow(page, ruleId);

    await test.step("Edit: the rate becomes 2.7500", async () => {
      await row.getByRole("link", { name: SCREENS.ruleList.edit, exact: true }).click();
      await expect(page).toHaveURL(urlPath(`/admin/reward-rules/${ruleId}/edit`));
      await expect(content(page).getByRole("heading", { level: 1 })).toHaveText(
        `${SCREENS.ruleForm.editTitle} ${ruleId}`,
      );
      await textbox(page, labels.rate).fill("2.7500");
      const [updated] = await Promise.all([
        page.waitForResponse((r) => isRequestTo(r.request(), ruleEndpoint("PUT", ruleId))),
        formButton(page, SCREENS.ruleForm.save).click(),
      ]);
      expect(updated.status()).toBe(200);
      await expect(page).toHaveURL(urlPath(SCREENS.ruleList.route));
      await expect(await cellUnder(table, row, "Rate")).toHaveText("2.7500");
    });

    await test.step("Delete: Cancel sends nothing, Delete soft-deletes and the rule stays listed", async () => {
      const deletes: string[] = [];
      page.on("request", (request) => {
        if (isRequestTo(request, ruleEndpoint("DELETE", ruleId))) {
          deletes.push(request.url());
        }
      });
      const dialog = confirmDialog(page);
      const deleteButton = row.getByRole("button", { name: SCREENS.ruleList.delete, exact: true });
      await deleteButton.click();
      await expect(dialog).toBeVisible();
      await expect(dialog).toHaveAccessibleName(SCREENS.deleteDialog.title);
      await dialog.getByRole("button", { name: SCREENS.deleteDialog.cancel, exact: true }).click();
      await expect(dialog).toBeHidden();
      expect(deletes, "Cancel sends no DELETE").toEqual([]);
      await expect(await cellUnder(table, row, "Status")).toHaveText(SCREENS.ruleList.active);
      await deleteButton.click();
      await expect(dialog).toBeVisible();
      const [deleted] = await Promise.all([
        page.waitForResponse((r) => isRequestTo(r.request(), ruleEndpoint("DELETE", ruleId))),
        dialog.getByRole("button", { name: SCREENS.deleteDialog.confirm, exact: true }).click(),
      ]);
      expect(deleted.status()).toBe(200);
      await expect(dialog).toBeHidden();
      await expect(row).toHaveCount(1);
      await expect(await cellUnder(table, row, "Status")).toHaveText(SCREENS.ruleList.ended);
      const activeTo = await cellUnder(table, row, "Active to");
      await expect(activeTo).not.toHaveText(SCREENS.ruleList.none);
      await expect(activeTo).toHaveText(/\S/);
    });
  });

  test("deleting a rule that has already ended changes nothing", async ({ request }) => {
    const token = await apiToken(request, "admin");
    // An ended rule of the spec's own: the one the screens soft-deleted when
    // that test ran, else one created and soft-deleted here through the API.
    let ended = (await rules(request)).find((rule) => rule.status === "ended");
    if (ended === undefined) {
      const created = await request.post(apiPath("/admin/reward-rules"), {
        headers: bearer(token),
        data: {
          transaction_type_cd: FIXTURE_KEYS.ruleTestTypeCode,
          transaction_category_cd: FIXTURE_KEYS.ruleTestCategoryCode,
          reward_rate: "2.5000",
          active_from: "2022-01-01T00:00:00Z",
        },
      });
      expect(created.status()).toBe(201);
      const ruleId = (await readJson(created, rewardRuleResponseSchema)).rule_id;
      const first = await request.delete(apiPath(`/admin/reward-rules/${ruleId}`), { headers: bearer(token) });
      expect(first.status()).toBe(200);
      ended = (await rules(request)).find((rule) => rule.rule_id === ruleId);
    }
    if (ended === undefined || ended.status !== "ended" || ended.active_to === null) {
      throw new Error("no ended reward rule to delete again");
    }
    const { rule_id: endedId, active_to: endedAt } = ended;
    const response = await request.delete(apiPath(`/admin/reward-rules/${endedId}`), {
      headers: bearer(token),
    });
    expect(response.status()).toBe(200);
    const after = (await rules(request)).find((rule) => rule.rule_id === endedId);
    expect(after?.active_to).toBe(endedAt);
    expect(after?.status).toBe("ended");
  });

  // forms.spec.ts submits these with Enter from the last field, keyboard only.
  // Here Save is clicked, so focus has to move from the button to the
  // rejected field, and the text has to be the field's FieldError rather than
  // a toast alone.
  for (const invalid of INVALID_RULES) {
    test(`the new-rule form rejects ${invalid.check} on its field`, async ({ page }) => {
      const answers: number[] = [];
      page.on("response", (response) => {
        if (isRequestTo(response.request(), ruleEndpoint("POST"))) {
          answers.push(response.status());
        }
      });
      await signIn(page, "admin");
      await gotoApp(page, SCREENS.ruleForm.newRoute);
      await expect(content(page).getByRole("heading", { level: 1 })).toHaveText(SCREENS.ruleForm.newTitle);
      await fillRuleForm(page, { ...VALID_RULE, ...invalid.entered });
      await formButton(page, SCREENS.ruleForm.save).click();

      const input = textbox(page, UNMAPPED_LABELS.rewardRule[invalid.field]);
      await expect(fieldErrorOf(input)).toHaveText(invalid.message);
      await expect(input).toHaveAttribute("aria-invalid", "true");
      await expect(input).toBeFocused();
      await expect(page).toHaveURL(urlPath(SCREENS.ruleForm.newRoute));
      expect(
        answers.filter((status) => status !== 400),
        "the rejected rule is answered by the form or by a 400 only",
      ).toEqual([]);
    });
  }
});
