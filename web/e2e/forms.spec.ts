// FN (AAP 0.10.2) end to end, keyboard only (AAP 0.5.4.4): every form of the
// SPA accepts valid input and rejects invalid input, and is driven with Tab,
// typing, Space, arrow keys, Enter and Escape alone. Enter submits the
// focused form (ENTER), every former PF key is a button or link reached by
// Tab, Escape closes a dialog and a dialog traps focus. Rejections assert the
// exact COBOL text (crates/domain/src/messages.rs, docs/parity/validation-audit.md).
//
// Controls and inputs come from docs/parity/function-map.md by F-ID; inputs
// the map does not list come from UNMAPPED_LABELS. Each describe that writes
// takes a checkpoint first and restores it in afterAll.
//
// - A rejection whose docs/parity/validation-audit.md row names a BMS field
//   the page renders is asserted through expectFieldError: the message, the
//   field's aria-invalid and the focus the COBOL `MOVE -1 TO <field>L` gives
//   (AAP 0.5.4.3). A row without a field (`—`) or a message-only 200 is
//   asserted by its text, with its V-ID beside it.
// - Every confirm dialog is first checked by expectDialogTrapsFocus (Tab and
//   Shift+Tab stay inside, Escape closes it, its request is not sent), then
//   reopened and answered.
// - Every write the forms confirm records its request and response on the
//   map row's endpoint under /api (AAP 0.3.1: 201 for POST, 200 for PUT and
//   DELETE).
import type { Locator, Page, Request, Response } from "@playwright/test";

import {
  checkpoint,
  confirmDialog,
  content,
  controlLocator,
  expect,
  expectMessage,
  FIXTURE_KEYS,
  isRequestTo,
  latestTranId,
  mapControl,
  mapInput,
  MESSAGES,
  nameMatcher,
  nextTranId,
  notifications,
  pressControl,
  psql,
  REWARD_SCREENS,
  rowEndpoint,
  rowInput,
  seedPendingAuthorization,
  shown,
  sidebar,
  signInWithKeyboard,
  sqlLiteral,
  tabTo,
  test,
  transactionWriteCheckpoint,
  typeInto,
  UNMAPPED_LABELS,
  urlPath,
  USERS,
  type Checkpoint,
  type Endpoint,
  type LabelText,
  type SeededAuthorization,
} from "./fixtures";

/** A user ID no seeded user holds (X(8)), created and removed by this spec. */
const NEW_USER_ID = "E2EUSR01";
/** A type code no seeded type holds, created and removed by this spec. */
const NEW_TYPE_CODE = "99";

// ---------------------------------------------------------------------------
// Keyboard helpers
// ---------------------------------------------------------------------------

/** The control of map row `id` on the current page. */
function control(page: Page, id: string): Locator {
  return controlLocator(page, mapControl(id));
}

/** The `index`-th input of map row `id`, in the page content. */
function input(page: Page, id: string, index: number): Locator {
  return mapInput(content(page), rowInput(id, index));
}

/** A text input the map does not list, by its accessible name. */
function textbox(scope: Page | Locator, label: LabelText): Locator {
  const root = "goto" in scope ? content(scope) : scope;
  return root.getByRole("textbox", { name: nameMatcher(label) });
}

/** Tabs to the control of map row `id` and presses Enter. */
async function enterOn(page: Page, id: string): Promise<void> {
  await pressControl(page, control(page, id));
}

/** Types `text` into `field` and presses Enter there, which submits its form. */
async function submitFrom(page: Page, field: Locator, text: string): Promise<void> {
  await typeInto(page, field, text);
  await page.keyboard.press("Enter");
}

// Whether the focused element is a radio button.
async function focusOnRadio(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const active = document.activeElement;
    return (
      active?.getAttribute("role") === "radio" ||
      (active instanceof HTMLInputElement && active.type === "radio")
    );
  });
}

/**
 * Checks the radio `target`: Tab into its group, arrow keys to the target,
 * Space to check it (a radio group is a single Tab stop).
 */
async function chooseRadio(page: Page, target: Locator): Promise<void> {
  await expect(target).toBeVisible();
  for (let presses = 0; presses < 200 && !(await focusOnRadio(page)); presses += 1) {
    await page.keyboard.press("Tab");
  }
  for (let moves = 0; moves < 10; moves += 1) {
    if (await target.evaluate((element) => element === document.activeElement)) {
      break;
    }
    await page.keyboard.press("ArrowDown");
  }
  await expect(target).toBeFocused();
  await page.keyboard.press("Space");
  await expect(target).toBeChecked();
}

/** Tabs from the open dialog's initial focus to its button `name` and presses Enter. */
async function answerDialog(page: Page, id: string): Promise<void> {
  const dialog = confirmDialog(page);
  await expect(dialog).toBeVisible();
  await pressControl(page, control(page, id));
  await expect(dialog).toBeHidden();
}

/**
 * Asserts a rejection of `field`: `message` shows, as a toast or as the
 * field's FieldError alert, and the field is aria-invalid and focused, the
 * cursor placement of the COBOL `MOVE -1 TO <field>L` (AAP 0.5.4.3).
 */
async function expectFieldError(page: Page, field: Locator, message: string): Promise<void> {
  const text = shown(message);
  const fieldError = field
    .locator('xpath=ancestor::*[@data-slot="field"][1]')
    .getByRole("alert")
    .filter({ hasText: text });
  const toast = notifications(page).getByText(text, { exact: true });
  await expect(toast.or(fieldError).first()).toBeVisible();
  await expect(field).toHaveAttribute("aria-invalid", "true");
  await expect(field).toBeFocused();
}

/**
 * Asserts that the open confirm `dialog` traps focus and that Escape closes
 * it without sending `write`, the request its confirming action sends (AAP
 * 0.5.4.4): Tab, then Shift+Tab, each pressed once more than the dialog has
 * buttons, keep focus inside it, and Escape hides it with no `write` request
 * sent meanwhile.
 */
async function expectDialogTrapsFocus(page: Page, dialog: Locator, write: Endpoint): Promise<void> {
  await expect(dialog).toBeVisible();
  const sent: string[] = [];
  const spy = (request: Request): void => {
    if (isRequestTo(request, write)) {
      sent.push(`${request.method()} ${request.url()}`);
    }
  };
  page.on("request", spy);
  try {
    const presses = (await dialog.getByRole("button").count()) + 1;
    for (const key of ["Tab", "Shift+Tab"]) {
      for (let press = 1; press <= presses; press += 1) {
        await page.keyboard.press(key);
        expect(
          await dialog.evaluate((element) => element.contains(document.activeElement)),
          `${key} ${press} of ${presses} keeps focus in the dialog`,
        ).toBe(true);
      }
    }
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    expect(sent, `no ${write.method} ${write.path} while the dialog was open`).toEqual([]);
  } finally {
    page.off("request", spy);
  }
}

/**
 * Runs `action`, which sends one `endpoint` request, and returns its
 * response after asserting the success status AAP 0.3.1 gives a write: 201
 * for POST, 200 for PUT and DELETE.
 */
async function expectWrite(page: Page, endpoint: Endpoint, action: () => Promise<void>): Promise<Response> {
  const [response] = await Promise.all([
    page.waitForResponse((r) => isRequestTo(r.request(), endpoint)),
    action(),
  ]);
  expect(response.status(), `${endpoint.method} ${endpoint.path}`).toBe(endpoint.method === "POST" ? 201 : 200);
  return response;
}

/** The data rows of the page's table that hold `text`. */
function rowsWith(page: Page, text: string): Locator {
  return content(page).getByRole("row").filter({ hasText: text });
}

/** The first data row of the page's table (row 0 is the header). */
function firstDataRow(page: Page): Locator {
  return content(page).getByRole("row").nth(1);
}

// Next (PF8) changes the first row of a browse and Previous (PF7) brings the
// first page back.
async function pagesForwardAndBack(page: Page, nextId: string, previousId: string): Promise<void> {
  const firstRow = firstDataRow(page);
  await expect(firstRow).toBeVisible();
  const firstPage = (await firstRow.innerText()).trim();
  await enterOn(page, nextId);
  await expect(firstRow).not.toHaveText(firstPage);
  await enterOn(page, previousId);
  await expect(firstRow).toHaveText(firstPage);
}

// ---------------------------------------------------------------------------
// Keyboard path of the app shell
// ---------------------------------------------------------------------------

test.describe("keyboard path", () => {
  test("skip link first, then the sidebar, then the page content", async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto("/");
    const skip = page.getByRole("link", { name: "Skip to content", exact: true });
    await page.keyboard.press("Tab");
    await expect(skip).toBeFocused();
    await page.keyboard.press("Tab");
    expect(await sidebar(page).evaluate((nav) => nav.contains(document.activeElement))).toBe(true);
    await tabTo(page, skip, { backward: true });
    await page.keyboard.press("Enter");
    await expect(content(page)).toBeFocused();
    await page.keyboard.press("Tab");
    expect(await content(page).evaluate((main) => main.contains(document.activeElement))).toBe(true);
  });

  test("a dialog traps focus and Escape closes it, back on its opener (F-COBIL00-001)", async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COBIL00-001").route);
    await submitFrom(page, input(page, "F-COBIL00-001", 0), FIXTURE_KEYS.account);
    await expectDialogTrapsFocus(page, confirmDialog(page), rowEndpoint("F-COBIL00-002"));
    await expect(input(page, "F-COBIL00-001", 0)).toBeFocused();
  });
});

// ---------------------------------------------------------------------------
// CC00 sign-on (F-COSGN00-001)
// ---------------------------------------------------------------------------

test.describe("sign-on form (F-COSGN00-001)", () => {
  const userId = (page: Page): Locator => mapInput(page, rowInput("F-COSGN00-001", 0));
  const password = (page: Page): Locator => mapInput(page, rowInput("F-COSGN00-001", 1));

  test.beforeEach(async ({ page }) => {
    await page.goto(mapControl("F-COSGN00-001").route);
  });

  test("rejects a blank user ID and puts the cursor there (V-COSGN00C-001, USERID)", async ({ page }) => {
    await pressControl(page, control(page, "F-COSGN00-001"));
    await expectFieldError(page, userId(page), MESSAGES.PLEASE_ENTER_USER_ID);
  });

  test("rejects a blank password (V-COSGN00C-002, PASSWD)", async ({ page }) => {
    await typeInto(page, userId(page), USERS.admin.userId);
    await pressControl(page, control(page, "F-COSGN00-001"));
    await expectFieldError(page, password(page), MESSAGES.PLEASE_ENTER_PASSWORD);
  });

  test("rejects an unknown user (V-COSGN00C-003, USERID)", async ({ page }) => {
    await typeInto(page, userId(page), "USER0099");
    await submitFrom(page, password(page), USERS.user.password);
    await expectFieldError(page, userId(page), MESSAGES.USER_NOT_FOUND_TRY_AGAIN);
    await expect(page).toHaveURL(urlPath("/login"));
  });

  test("rejects a wrong password (V-COSGN00C-005, PASSWD)", async ({ page }) => {
    await typeInto(page, userId(page), USERS.user.userId);
    await submitFrom(page, password(page), "WRONGPWD");
    await expectFieldError(page, password(page), MESSAGES.WRONG_PASSWORD_TRY_AGAIN);
    await expect(page).toHaveURL(urlPath("/login"));
  });

  test("upper-cases the user ID and password, then lands a user on /", async ({ page }) => {
    await typeInto(page, userId(page), USERS.user.userId.toLowerCase());
    await submitFrom(page, password(page), USERS.user.password.toLowerCase());
    await expect(page).toHaveURL(urlPath(USERS.user.landing));
  });

  test("lands an administrator on /admin", async ({ page }) => {
    await signInWithKeyboard(page, "admin");
    await expect(page).toHaveURL(urlPath(USERS.admin.landing));
  });
});

// ---------------------------------------------------------------------------
// CAVW dashboard lookup and CAUP account update
// ---------------------------------------------------------------------------

test.describe("account lookup (F-COACTVW-001, F-COACTVW-002, F-COACTUP-001)", () => {
  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COACTVW-001").route);
  });

  test("rejects an all-zero account number (V-COACTVWC-002, ACCTSID)", async ({ page }) => {
    await submitFrom(page, input(page, "F-COACTVW-001", 0), "00000000000");
    await expectFieldError(page, input(page, "F-COACTVW-001", 0), MESSAGES.ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT);
  });

  test("finds account 1 and opens its view", async ({ page }) => {
    await submitFrom(page, input(page, "F-COACTVW-001", 0), FIXTURE_KEYS.account);
    await expect(control(page, "F-COACTUP-001")).toBeVisible();
    await enterOn(page, "F-COACTVW-002");
    await expect(page).toHaveURL(urlPath(`/accounts/${FIXTURE_KEYS.account}`));
  });
});

test.describe("account update (F-COACTUP-002, F-COACTUP-004, F-COACTUP-005, F-COACTUP-006)", () => {
  const editRoute = `/accounts/${FIXTURE_KEYS.account}/edit`;
  const creditLimit = (page: Page): Locator => textbox(page, UNMAPPED_LABELS.accountEdit.creditLimit);
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([
      { table: "accounts", where: `account_id = ${sqlLiteral(FIXTURE_KEYS.account)}::bigint` },
      { table: "customers", where: `customer_id = ${sqlLiteral(FIXTURE_KEYS.customer)}::integer` },
    ]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(editRoute);
    await expect(creditLimit(page)).toBeVisible();
  });

  test("answers an unchanged save with the no-change text", async ({ page }) => {
    // V-COACTUPC-010: a message-only 200 that rejects nothing and writes nothing.
    await enterOn(page, "F-COACTUP-002");
    await expectMessage(page, MESSAGES.NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED);
  });

  test("rejects a credit limit that is not a number (V-COACTUPC-025, ACRDLIM)", async ({ page }) => {
    await typeInto(page, creditLimit(page), "abc");
    await enterOn(page, "F-COACTUP-005");
    await expectFieldError(page, creditLimit(page), `${MESSAGES.FIELD_CREDIT_LIMIT}${MESSAGES.IS_NOT_VALID}`);
  });

  test("Cancel drops unsaved changes", async ({ page }) => {
    const fetched = await creditLimit(page).inputValue();
    await typeInto(page, creditLimit(page), "1.00");
    await enterOn(page, "F-COACTUP-006");
    await expect(creditLimit(page)).toHaveValue(fetched);
  });

  test("saves a new credit limit", async ({ page }) => {
    await typeInto(page, creditLimit(page), "2500.00");
    await expectWrite(page, rowEndpoint("F-COACTUP-005"), () => enterOn(page, "F-COACTUP-005"));
    await expectMessage(page, MESSAGES.CHANGES_COMMITTED_TO_DATABASE);
    expect(
      await psql(
        `SELECT acct_credit_limit FROM accounts WHERE account_id = ${sqlLiteral(FIXTURE_KEYS.account)}::bigint`,
      ),
    ).toBe("2500.00");
  });

  test("Back returns to the account view", async ({ page }) => {
    await enterOn(page, "F-COACTUP-004");
    await expect(page).toHaveURL(urlPath(`/accounts/${FIXTURE_KEYS.account}`));
  });
});

// ---------------------------------------------------------------------------
// CCLI card list and CCUP card update
// ---------------------------------------------------------------------------

test.describe("card list (F-COCRDLI-001, F-COCRDLI-003, F-COCRDLI-004, F-COCRDLI-005)", () => {
  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COCRDLI-001").route);
  });

  test("rejects an account filter that is not 11 digits (V-COCRDLIC-001, ACCTSID)", async ({ page }) => {
    await submitFrom(page, input(page, "F-COCRDLI-001", 0), "0000000005A");
    await expectFieldError(page, input(page, "F-COCRDLI-001", 0), MESSAGES.ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT);
  });

  test("rejects a card filter that is not 16 digits (V-COCRDLIC-002, CARDSID)", async ({ page }) => {
    await submitFrom(page, input(page, "F-COCRDLI-001", 1), "050002445376574X");
    await expectFieldError(page, input(page, "F-COCRDLI-001", 1), MESSAGES.CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT);
  });

  test("filters by account and opens the card", async ({ page }) => {
    await submitFrom(page, input(page, "F-COCRDLI-001", 0), FIXTURE_KEYS.account);
    await expect(rowsWith(page, FIXTURE_KEYS.card)).toHaveCount(1);
    await pressControl(page, rowsWith(page, FIXTURE_KEYS.card).getByRole("link", { name: mapControl("F-COCRDLI-005").name, exact: true }));
    await expect(page).toHaveURL(urlPath(`/cards/${FIXTURE_KEYS.card}`));
  });

  test("Next and Previous page through the cards", async ({ page }) => {
    await pagesForwardAndBack(page, "F-COCRDLI-004", "F-COCRDLI-003");
  });
});

test.describe("card update (F-COCRDLI-006, F-COCRDUP-002, F-COCRDUP-004, F-COCRDUP-005, F-COCRDUP-006)", () => {
  const nameOnCard = (page: Page): Locator => textbox(page, UNMAPPED_LABELS.cardEdit.nameOnCard);
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([
      { table: "cards", where: `card_number = ${sqlLiteral(FIXTURE_KEYS.card)}` },
    ]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COCRDLI-001").route);
    await submitFrom(page, input(page, "F-COCRDLI-001", 0), FIXTURE_KEYS.account);
    await pressControl(page, rowsWith(page, FIXTURE_KEYS.card).getByRole("link", { name: mapControl("F-COCRDLI-006").name, exact: true }));
    await expect(page).toHaveURL(urlPath(`/cards/${FIXTURE_KEYS.card}/edit`));
    await expect(nameOnCard(page)).toBeVisible();
  });

  test("rejects a blank name on card (V-COCRDUPC-010, CRDNAME)", async ({ page }) => {
    await typeInto(page, nameOnCard(page), "");
    await enterOn(page, "F-COCRDUP-002");
    await expectFieldError(page, nameOnCard(page), MESSAGES.CARD_NAME_NOT_PROVIDED);
  });

  test("Cancel drops unsaved changes", async ({ page }) => {
    const fetched = await nameOnCard(page).inputValue();
    await typeInto(page, nameOnCard(page), "Changed Name");
    await enterOn(page, "F-COCRDUP-006");
    await expect(nameOnCard(page)).toHaveValue(fetched);
  });

  test("saves a new name on card", async ({ page }) => {
    await typeInto(page, nameOnCard(page), "E2E Card Holder");
    await expectWrite(page, rowEndpoint("F-COCRDUP-005"), () => enterOn(page, "F-COCRDUP-005"));
    await expectMessage(page, MESSAGES.CHANGES_COMMITTED_TO_DATABASE);
    expect(
      await psql(`SELECT card_embossed_name FROM cards WHERE card_number = ${sqlLiteral(FIXTURE_KEYS.card)}`),
    ).toMatch(/^E2E Card Holder$/i);
  });

  test("Back returns to the card view", async ({ page }) => {
    await enterOn(page, "F-COCRDUP-004");
    await expect(page).toHaveURL(urlPath(`/cards/${FIXTURE_KEYS.card}`));
  });
});

// ---------------------------------------------------------------------------
// CT00 list, CT01 view and CT02 add
// ---------------------------------------------------------------------------

test.describe("transaction list (F-COTRN00-001, F-COTRN00-003, F-COTRN00-004, F-COTRN00-005, F-COTRN01-002, F-COTRN01-004)", () => {
  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COTRN00-001").route);
  });

  test("rejects a start key that is not numeric (V-COTRN00C-004, TRNIDIN)", async ({ page }) => {
    await submitFrom(page, input(page, "F-COTRN00-001", 0), "ABC");
    await expectFieldError(page, input(page, "F-COTRN00-001", 0), MESSAGES.TRAN_ID_MUST_BE_NUMERIC);
  });

  test("browses from a start key and opens the transaction", async ({ page }) => {
    const tranId = await latestTranId();
    await submitFrom(page, input(page, "F-COTRN00-001", 0), tranId);
    await expect(firstDataRow(page)).toContainText(tranId);
    await pressControl(page, firstDataRow(page).getByRole("link", { name: mapControl("F-COTRN00-005").name, exact: true }));
    await expect(page).toHaveURL(urlPath(`/transactions/${tranId}`));
    await enterOn(page, "F-COTRN01-004");
    await expect(page).toHaveURL(urlPath("/transactions"));
    await page.goBack();
    await enterOn(page, "F-COTRN01-002");
    await expect(page).toHaveURL(urlPath("/transactions"));
  });

  test("Next and Previous page through the transactions", async ({ page }) => {
    await pagesForwardAndBack(page, "F-COTRN00-004", "F-COTRN00-003");
  });
});

test.describe("transaction add (F-COTRN02-001, F-COTRN02-002, F-COTRN02-003, F-COTRN02-004, F-COTRN02-005, F-COTRN02-006)", () => {
  const accountId = (page: Page): Locator => textbox(page, UNMAPPED_LABELS.transactionAdd.accountId);
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await transactionWriteCheckpoint([FIXTURE_KEYS.rewardAccount]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COTRN02-001").route);
  });

  test("rejects an add with neither account nor card (V-COTRN02C-003, ACTIDIN)", async ({ page }) => {
    await enterOn(page, "F-COTRN02-001");
    await expectFieldError(page, accountId(page), MESSAGES.ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED);
  });

  test("rejects a copy for an account not on file (V-COTRN02C-004, ACTIDIN)", async ({ page }) => {
    await typeInto(page, accountId(page), FIXTURE_KEYS.missingAccount);
    await enterOn(page, "F-COTRN02-006");
    await expectFieldError(page, accountId(page), MESSAGES.ACCOUNT_ID_NOT_FOUND);
  });

  test("Reset clears the form", async ({ page }) => {
    await typeInto(page, accountId(page), FIXTURE_KEYS.rewardAccount);
    await enterOn(page, "F-COTRN02-005");
    await expect(accountId(page)).toHaveValue("");
  });

  test("copies the last transaction; the dialog traps focus, Cancel writes nothing, Confirm adds it", async ({ page }) => {
    const count = "SELECT count(*) FROM transaction_master";
    const before = await psql(count);
    await typeInto(page, accountId(page), FIXTURE_KEYS.rewardAccount);
    await enterOn(page, "F-COTRN02-006");
    await enterOn(page, "F-COTRN02-001");
    await expectDialogTrapsFocus(page, confirmDialog(page), rowEndpoint("F-COTRN02-002"));
    await enterOn(page, "F-COTRN02-001");
    await answerDialog(page, "F-COTRN02-003");
    expect(await psql(count)).toBe(before);
    const tranId = await nextTranId();
    await enterOn(page, "F-COTRN02-001");
    const added = await expectWrite(page, rowEndpoint("F-COTRN02-002"), () => answerDialog(page, "F-COTRN02-002"));
    expect(added.request().postDataJSON()).toMatchObject({
      account_id: FIXTURE_KEYS.rewardAccount,
      confirm: "Y",
    });
    await expectMessage(
      page,
      `${MESSAGES.TRANSACTION_ADDED_SUCCESSFULLY}${MESSAGES.YOUR_TRAN_ID_IS}${tranId}.`,
    );
    expect(await psql(`SELECT count(*) FROM transaction_master WHERE tran_id = ${sqlLiteral(tranId)}`)).toBe("1");
  });

  test("Back returns to the transaction list", async ({ page }) => {
    await enterOn(page, "F-COTRN02-004");
    await expect(page).toHaveURL(urlPath(mapControl("F-COTRN02-004").target ?? "/transactions"));
  });
});

// ---------------------------------------------------------------------------
// CR00 transaction reports
// ---------------------------------------------------------------------------

test.describe("transaction reports (F-CORPT00-001, F-CORPT00-002, F-CORPT00-003, F-CORPT00-004)", () => {
  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-CORPT00-001").route);
  });

  test("rejects a report without a type", async ({ page }) => {
    // V-CORPT00C-001 names MONTHLY, the first radio of the report-type group:
    // the text is asserted, as a radio group keeps no aria-invalid of its own
    // on one radio and the page brief fixes no focus target inside the group.
    await enterOn(page, "F-CORPT00-001");
    await expectMessage(page, MESSAGES.SELECT_A_REPORT_TYPE_TO_PRINT_REPORT);
  });

  test("rejects a custom report without dates (V-CORPT00C-002, SDTMM)", async ({ page }) => {
    await chooseRadio(page, input(page, "F-CORPT00-001", 2));
    await enterOn(page, "F-CORPT00-001");
    await expectFieldError(page, input(page, "F-CORPT00-001", 3), MESSAGES.START_DATE_MONTH_CAN_NOT_BE_EMPTY);
  });

  test("a monthly report: the dialog traps focus, Cancel runs nothing, Confirm submits it", async ({ page }) => {
    await chooseRadio(page, input(page, "F-CORPT00-001", 0));
    await enterOn(page, "F-CORPT00-001");
    await expectDialogTrapsFocus(page, confirmDialog(page), rowEndpoint("F-CORPT00-002"));
    await enterOn(page, "F-CORPT00-001");
    await answerDialog(page, "F-CORPT00-003");
    await enterOn(page, "F-CORPT00-001");
    await answerDialog(page, "F-CORPT00-002");
    await expectMessage(page, `${MESSAGES.REPORT_NAME_MONTHLY}${MESSAGES.REPORT_SUBMITTED_FOR_PRINTING}`);
  });

  test("a custom report over the TRANREPT range", async ({ page }) => {
    await chooseRadio(page, input(page, "F-CORPT00-001", 2));
    await typeInto(page, input(page, "F-CORPT00-001", 3), "2022-01-01");
    await typeInto(page, input(page, "F-CORPT00-001", 4), "2022-07-06");
    await page.keyboard.press("Enter");
    await answerDialog(page, "F-CORPT00-002");
    await expectMessage(page, `${MESSAGES.REPORT_NAME_CUSTOM}${MESSAGES.REPORT_SUBMITTED_FOR_PRINTING}`);
  });

  test("Back returns to the main menu", async ({ page }) => {
    await enterOn(page, "F-CORPT00-004");
    await expect(page).toHaveURL(urlPath("/"));
  });
});

// ---------------------------------------------------------------------------
// CB00 bill payment
// ---------------------------------------------------------------------------

test.describe("bill payment (F-COBIL00-001, F-COBIL00-002, F-COBIL00-003, F-COBIL00-004, F-COBIL00-005)", () => {
  const accountId = (page: Page): Locator => input(page, "F-COBIL00-001", 0);
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await transactionWriteCheckpoint([FIXTURE_KEYS.account]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COBIL00-001").route);
  });

  test("rejects a blank account ID (V-COBIL00C-001, ACTIDIN)", async ({ page }) => {
    await enterOn(page, "F-COBIL00-001");
    await expectFieldError(page, accountId(page), MESSAGES.ACCT_ID_CAN_NOT_BE_EMPTY);
  });

  test("rejects an account not on file (V-COBIL00C-004, ACTIDIN)", async ({ page }) => {
    await submitFrom(page, accountId(page), FIXTURE_KEYS.missingAccount);
    await expectFieldError(page, accountId(page), MESSAGES.ACCOUNT_ID_NOT_FOUND);
  });

  test("Reset clears the form", async ({ page }) => {
    await typeInto(page, accountId(page), FIXTURE_KEYS.account);
    await enterOn(page, "F-COBIL00-005");
    await expect(accountId(page)).toHaveValue("");
  });

  test("pays the balance after the confirmation, then has nothing to pay (V-COBIL00C-007, ACTIDIN)", async ({ page }) => {
    const balance = `SELECT acct_curr_bal FROM accounts WHERE account_id = ${sqlLiteral(FIXTURE_KEYS.account)}::bigint`;
    const before = await psql(balance);
    await submitFrom(page, accountId(page), FIXTURE_KEYS.account);
    await expectDialogTrapsFocus(page, confirmDialog(page), rowEndpoint("F-COBIL00-002"));
    await submitFrom(page, accountId(page), FIXTURE_KEYS.account);
    await answerDialog(page, "F-COBIL00-003");
    expect(await psql(balance)).toBe(before);
    const tranId = await nextTranId();
    await submitFrom(page, accountId(page), FIXTURE_KEYS.account);
    const paid = await expectWrite(page, rowEndpoint("F-COBIL00-002"), () => answerDialog(page, "F-COBIL00-002"));
    expect(paid.request().postDataJSON()).toMatchObject({ confirm: "Y" });
    await expectMessage(page, `${MESSAGES.PAYMENT_SUCCESSFUL}${MESSAGES.YOUR_TRANSACTION_ID_IS}${tranId}.`);
    expect(await psql(balance)).toBe("0.00");
    await submitFrom(page, accountId(page), FIXTURE_KEYS.account);
    await expectFieldError(page, accountId(page), MESSAGES.YOU_HAVE_NOTHING_TO_PAY);
  });

  test("Back returns to the main menu", async ({ page }) => {
    await enterOn(page, "F-COBIL00-004");
    await expect(page).toHaveURL(urlPath("/"));
  });
});

// ---------------------------------------------------------------------------
// CPVS summary and CPVD fraud marking
// ---------------------------------------------------------------------------

test.describe("pending authorizations (F-COPAU00-001, F-COPAU00-005, F-COPAU01-003, F-COPAU01-004, F-COPAU01-005)", () => {
  let seeded: SeededAuthorization | undefined;

  test.beforeAll(async () => {
    seeded = await seedPendingAuthorization(FIXTURE_KEYS.account);
  });

  test.afterAll(async () => {
    await seeded?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto(mapControl("F-COPAU00-001").route);
  });

  test("rejects a blank account ID (V-COPAUS0C-001, ACCTID)", async ({ page }) => {
    await enterOn(page, "F-COPAU00-001");
    await expectFieldError(page, input(page, "F-COPAU00-001", 0), MESSAGES.PLEASE_ENTER_ACCT_ID);
  });

  test("rejects an account ID that is not numeric (V-COPAUS0C-002, ACCTID)", async ({ page }) => {
    await submitFrom(page, input(page, "F-COPAU00-001", 0), "0000000000A");
    await expectFieldError(page, input(page, "F-COPAU00-001", 0), MESSAGES.ACCT_ID_MUST_BE_NUMERIC);
  });

  test("marks the authorization as fraud and removes the mark", async ({ page }) => {
    if (seeded === undefined) {
      throw new Error("the pending authorization was not seeded");
    }
    // The seeded authorization is the account's only one, so View opens it.
    const update: Endpoint = { method: "PUT", path: `/authorizations/${seeded.authId}` };
    await submitFrom(page, input(page, "F-COPAU00-001", 0), FIXTURE_KEYS.account);
    await enterOn(page, "F-COPAU00-005");
    await expect(page).toHaveURL(urlPath(`/authorizations/${seeded.authId}`));
    for (const [openerId, message] of [
      ["F-COPAU01-003", MESSAGES.AUTH_MARKED_FRAUD],
      ["F-COPAU01-004", MESSAGES.AUTH_FRAUD_REMOVED],
    ] as const) {
      await enterOn(page, openerId);
      await expectDialogTrapsFocus(page, confirmDialog(page), update);
      await enterOn(page, openerId);
      await expectWrite(page, update, () => answerDialog(page, "F-COPAU01-005"));
      await expectMessage(page, message);
    }
  });
});

// ---------------------------------------------------------------------------
// CU00 to CU03 users
// ---------------------------------------------------------------------------

test.describe("users (F-COUSR00-001, F-COUSR00-005, F-COUSR00-006, F-COUSR01-001, F-COUSR01-003, F-COUSR02-002, F-COUSR02-003, F-COUSR02-004, F-COUSR02-005, F-COUSR03-002, F-COUSR03-003, F-COUSR03-004, F-COUSR03-005)", () => {
  test.describe.configure({ mode: "serial" });
  const labels = UNMAPPED_LABELS.userForm;
  const newUserPassword = (page: Page): Locator => content(page).getByLabel(nameMatcher(labels.password));
  /** PUT (F-COUSR02-002, F-COUSR02-004) or DELETE (F-COUSR03-004) of the new user. */
  const userEndpoint = (method: "PUT" | "DELETE"): Endpoint => ({
    method,
    path: `/admin/users/${NEW_USER_ID}`,
  });
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([{ table: "users", where: `user_id = ${sqlLiteral(NEW_USER_ID)}` }]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "admin");
  });

  // The list started at the new user (F-COUSR00-001), whose row is first.
  async function listFromNewUser(page: Page): Promise<Locator> {
    await page.goto(mapControl("F-COUSR00-001").route);
    await submitFrom(page, input(page, "F-COUSR00-001", 0), NEW_USER_ID);
    const row = rowsWith(page, NEW_USER_ID);
    await expect(row).toHaveCount(1);
    return row;
  }

  async function openEdit(page: Page): Promise<void> {
    const row = await listFromNewUser(page);
    await pressControl(page, row.getByRole("link", { name: mapControl("F-COUSR00-005").name, exact: true }));
    await expect(page).toHaveURL(urlPath(`/admin/users/${NEW_USER_ID}/edit`));
  }

  async function fillNewUser(page: Page, userId: string): Promise<void> {
    await typeInto(page, textbox(page, labels.firstName), "Edith");
    await typeInto(page, textbox(page, labels.lastName), "Tester");
    await typeInto(page, textbox(page, labels.userId), userId);
    await typeInto(page, newUserPassword(page), "PASS1234");
    await typeInto(page, textbox(page, labels.userType), "U");
  }

  test("add rejects a blank first name (V-COUSR01C-001, FNAME)", async ({ page }) => {
    await page.goto(mapControl("F-COUSR01-001").route);
    await enterOn(page, "F-COUSR01-001");
    await expectFieldError(page, textbox(page, labels.firstName), MESSAGES.FIRST_NAME_CAN_NOT_BE_EMPTY);
  });

  test("add rejects a user ID already on file (V-COUSR01C-006, USERID)", async ({ page }) => {
    await page.goto(mapControl("F-COUSR01-001").route);
    await fillNewUser(page, USERS.admin.userId);
    await page.keyboard.press("Enter");
    await expectFieldError(page, textbox(page, labels.userId), MESSAGES.USER_ID_ALREADY_EXIST);
  });

  test("add: Reset clears the form", async ({ page }) => {
    await page.goto(mapControl("F-COUSR01-001").route);
    await typeInto(page, textbox(page, labels.firstName), "Edith");
    await enterOn(page, "F-COUSR01-003");
    await expect(textbox(page, labels.firstName)).toHaveValue("");
  });

  test("adds the user", async ({ page }) => {
    await page.goto(mapControl("F-COUSR01-001").route);
    await fillNewUser(page, NEW_USER_ID);
    await expectWrite(page, rowEndpoint("F-COUSR01-001"), () => enterOn(page, "F-COUSR01-001"));
    await expectMessage(page, `${MESSAGES.USER_LABEL}${NEW_USER_ID}${MESSAGES.HAS_BEEN_ADDED}`);
    expect(await psql(`SELECT sec_usr_type FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("U");
  });

  test("update answers an unchanged save, then saves a new last name", async ({ page }) => {
    await openEdit(page);
    // V-COUSR02C-010: a message-only 200 that rejects nothing and writes nothing.
    await enterOn(page, "F-COUSR02-004");
    await expectMessage(page, MESSAGES.PLEASE_MODIFY_TO_UPDATE);
    await typeInto(page, textbox(page, labels.lastName), "Retested");
    await expectWrite(page, userEndpoint("PUT"), () => enterOn(page, "F-COUSR02-004"));
    await expectMessage(page, `${MESSAGES.USER_LABEL}${NEW_USER_ID}${MESSAGES.HAS_BEEN_UPDATED}`);
    expect(await psql(`SELECT sec_usr_lname FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("Retested");
  });

  test("update: Reset clears the fields, Cancel and Save and exit return to the list", async ({ page }) => {
    await openEdit(page);
    await typeInto(page, textbox(page, labels.firstName), "Changed");
    await enterOn(page, "F-COUSR02-003");
    await expect(textbox(page, labels.firstName)).toHaveValue("");
    await enterOn(page, "F-COUSR02-005");
    await expect(page).toHaveURL(urlPath(mapControl("F-COUSR02-005").target ?? "/admin/users"));
    await openEdit(page);
    await typeInto(page, textbox(page, labels.firstName), "Edwina");
    await expectWrite(page, userEndpoint("PUT"), () => enterOn(page, "F-COUSR02-002"));
    await expect(page).toHaveURL(urlPath(mapControl("F-COUSR02-002").target ?? "/admin/users"));
    expect(await psql(`SELECT sec_usr_fname FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("Edwina");
  });

  test("delete: the dialog traps focus, Escape and Cancel keep the user, Confirm deletes it", async ({ page }) => {
    const deleteButton = async (): Promise<Locator> =>
      (await listFromNewUser(page)).getByRole("button", { name: mapControl("F-COUSR00-006").name, exact: true });
    await pressControl(page, await deleteButton());
    await expectDialogTrapsFocus(page, confirmDialog(page), userEndpoint("DELETE"));
    for (const cancelId of ["F-COUSR03-002", "F-COUSR03-003", "F-COUSR03-005"]) {
      await pressControl(page, await deleteButton());
      await answerDialog(page, cancelId);
    }
    expect(await psql(`SELECT count(*) FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("1");
    await pressControl(page, await deleteButton());
    await expectWrite(page, userEndpoint("DELETE"), () => answerDialog(page, "F-COUSR03-004"));
    await expectMessage(page, `${MESSAGES.USER_LABEL}${NEW_USER_ID}${MESSAGES.HAS_BEEN_DELETED}`);
    expect(await psql(`SELECT count(*) FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("0");
  });
});

// ---------------------------------------------------------------------------
// CTTU maintenance and CTLI list
// ---------------------------------------------------------------------------

test.describe("transaction types (F-COTRTUP-001, F-COTRTUP-002, F-COTRTUP-003, F-COTRTUP-006, F-COTRTUP-007, F-COTRTUP-012, F-COTRTUP-014, F-COTRTUP-015, F-COTRTLI-001, F-COTRTLI-006, F-COTRTLI-007, F-COTRTLI-008, F-COTRTLI-009)", () => {
  test.describe.configure({ mode: "serial" });
  const typeCode = (page: Page): Locator => input(page, "F-COTRTUP-001", 0);
  const description = (scope: Page | Locator): Locator => textbox(scope, UNMAPPED_LABELS.tranType.description);
  const storedDescription = (): Promise<string> =>
    psql(`SELECT coalesce((SELECT tr_description FROM transaction_types WHERE tr_type = ${sqlLiteral(NEW_TYPE_CODE)}), '<none>')`);
  /** PUT (F-COTRTUP-002, F-COTRTLI-007) or DELETE (F-COTRTUP-007, F-COTRTLI-009) of the new type. */
  const typeEndpoint = (method: "PUT" | "DELETE"): Endpoint => ({
    method,
    path: `/admin/transaction-types/${NEW_TYPE_CODE}`,
  });
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([
      { table: "transaction_types", where: `tr_type = ${sqlLiteral(NEW_TYPE_CODE)}` },
      { table: "transaction_categories", where: `trc_type_code = ${sqlLiteral(NEW_TYPE_CODE)}` },
    ]);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "admin");
  });

  async function addNewType(page: Page, text: string): Promise<void> {
    await page.goto(mapControl("F-COTRTUP-001").route);
    await submitFrom(page, typeCode(page), NEW_TYPE_CODE);
    await typeInto(page, description(page), text);
    await expectWrite(page, rowEndpoint("F-COTRTUP-003"), () => enterOn(page, "F-COTRTUP-003"));
    await expect.poll(storedDescription).toBe(text);
  }

  test("look-up rejects a blank and a non-numeric type code (V-COTRTUPC-001, V-COTRTUPC-002, TRTYPCD)", async ({ page }) => {
    await page.goto(mapControl("F-COTRTUP-001").route);
    await enterOn(page, "F-COTRTUP-001");
    await expectFieldError(page, typeCode(page), `${MESSAGES.FIELD_TRAN_TYPE_CODE}${MESSAGES.MUST_BE_SUPPLIED}`);
    await submitFrom(page, typeCode(page), "AB");
    await expectFieldError(page, typeCode(page), `${MESSAGES.FIELD_TRAN_TYPE_CODE}${MESSAGES.MUST_BE_NUMERIC}`);
  });

  test("Cancel abandons the add path of a code not on file", async ({ page }) => {
    await page.goto(mapControl("F-COTRTUP-001").route);
    await submitFrom(page, typeCode(page), NEW_TYPE_CODE);
    await expect(control(page, "F-COTRTUP-003")).toBeVisible();
    await enterOn(page, "F-COTRTUP-015");
    await expect(control(page, "F-COTRTUP-003")).toBeHidden();
    expect(await storedDescription()).toBe("<none>");
  });

  test("adds the type, then the edit rejects and saves descriptions (V-COTRTUPC-008, TRTYDSC)", async ({ page }) => {
    await addNewType(page, "E2E TEST TYPE");
    await page.goto(`/admin/transaction-types/${NEW_TYPE_CODE}/edit`);
    await typeInto(page, description(page), "E2E#TYPE");
    await enterOn(page, "F-COTRTUP-002");
    await expectFieldError(
      page,
      description(page),
      `${MESSAGES.FIELD_TRANSACTION_DESC}${MESSAGES.CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY}`,
    );
    await typeInto(page, description(page), "E2E TEST TYPE TWO");
    await expectWrite(page, typeEndpoint("PUT"), () => enterOn(page, "F-COTRTUP-002"));
    await expect.poll(storedDescription).toBe("E2E TEST TYPE TWO");
  });

  test("edit: the Delete dialog traps focus, Cancel keeps it, Cancel leaves for key entry, Confirm removes it", async ({ page }) => {
    await page.goto(`/admin/transaction-types/${NEW_TYPE_CODE}/edit`);
    await enterOn(page, "F-COTRTUP-006");
    await expectDialogTrapsFocus(page, confirmDialog(page), typeEndpoint("DELETE"));
    await enterOn(page, "F-COTRTUP-006");
    await answerDialog(page, "F-COTRTUP-012");
    expect(await storedDescription()).not.toBe("<none>");
    await enterOn(page, "F-COTRTUP-014");
    await expect(page).toHaveURL(urlPath(mapControl("F-COTRTUP-014").target ?? "/admin/transaction-types/new"));
    await page.goto(`/admin/transaction-types/${NEW_TYPE_CODE}/edit`);
    await enterOn(page, "F-COTRTUP-006");
    await expectWrite(page, typeEndpoint("DELETE"), () => answerDialog(page, "F-COTRTUP-007"));
    await expect(page).toHaveURL(urlPath(mapControl("F-COTRTUP-007").target ?? "/admin/transaction-types/new"));
    await expect.poll(storedDescription).toBe("<none>");
  });

  test("list: rejects a bad type filter (V-COTRTLIC-001, TRTYPE), updates a row in place and deletes it", async ({ page }) => {
    await addNewType(page, "E2E LIST TYPE");
    await page.goto(mapControl("F-COTRTLI-001").route);
    await submitFrom(page, input(page, "F-COTRTLI-001", 0), "AB");
    await expectFieldError(page, input(page, "F-COTRTLI-001", 0), MESSAGES.TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT);
    await submitFrom(page, input(page, "F-COTRTLI-001", 0), NEW_TYPE_CODE);
    const row = rowsWith(page, NEW_TYPE_CODE);
    await expect(row).toHaveCount(1);
    await pressControl(page, row.getByRole("button", { name: mapControl("F-COTRTLI-006").name, exact: true }));
    await typeInto(page, description(row), "E2E LIST TYPE TWO");
    const updated = await expectWrite(page, typeEndpoint("PUT"), () => enterOn(page, "F-COTRTLI-007"));
    expect(new URL(updated.url()).searchParams.get("origin"), "the inline update is the list's").toBe("list");
    await expectMessage(page, MESSAGES.HIGHLIGHTED_ROW_WAS_UPDATED);
    await expect.poll(storedDescription).toBe("E2E LIST TYPE TWO");
    const deleteButton = (): Locator =>
      rowsWith(page, NEW_TYPE_CODE).getByRole("button", { name: mapControl("F-COTRTLI-008").name, exact: true });
    await pressControl(page, deleteButton());
    await expectDialogTrapsFocus(page, confirmDialog(page), typeEndpoint("DELETE"));
    await pressControl(page, deleteButton());
    await expectWrite(page, typeEndpoint("DELETE"), () => answerDialog(page, "F-COTRTLI-009"));
    await expectMessage(page, MESSAGES.HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE);
    await expect.poll(storedDescription).toBe("<none>");
  });
});

// ---------------------------------------------------------------------------
// Rewards: the account lookup and the admin reward-rule form (no F-IDs)
// ---------------------------------------------------------------------------

test.describe("rewards account lookup", () => {
  test("rejects an all-zero account number", async ({ page }) => {
    // The rewards lookup is new (o8) and has no V-row; it gives the CAVW
    // account edit's text (V-COACTVWC-002) on its account field.
    await signInWithKeyboard(page, "user");
    await page.goto(REWARD_SCREENS.rewards.route);
    const account = textbox(page, UNMAPPED_LABELS.rewards.accountId);
    await submitFrom(page, account, "00000000000");
    await expectFieldError(page, account, MESSAGES.ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT);
  });
});

test.describe("reward rule form", () => {
  test.describe.configure({ mode: "serial" });
  const labels = UNMAPPED_LABELS.rewardRule;
  const screens = REWARD_SCREENS;
  const ruleEndpoint = (method: "POST" | "PUT" | "DELETE", ruleId?: string): Endpoint => ({
    method,
    path: ruleId === undefined ? "/admin/reward-rules" : `/admin/reward-rules/${ruleId}`,
  });
  let saved: Checkpoint | undefined;
  let today = "";
  /** The rule "creates a valid rule" stored, edited and soft-deleted after it. */
  let createdRuleId: string | undefined;

  test.beforeAll(async () => {
    saved = await checkpoint([{ table: "reward_rules", where: "TRUE" }]);
    today = await psql("SELECT current_date");
    expect(today).toMatch(/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/);
  });

  test.afterAll(async () => {
    await saved?.restore();
  });

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "admin");
  });

  function createdRule(): string {
    if (createdRuleId === undefined) {
      throw new Error("no rule was created by this spec");
    }
    return createdRuleId;
  }

  // Types the rule-test pair and `rule` into the new-rule form; Enter in the
  // last field submits it.
  async function enterRule(
    page: Page,
    rule: { rate: string; min: string; max: string; from: string; to: string },
  ): Promise<void> {
    await page.goto(screens.ruleForm.newRoute);
    await typeInto(page, textbox(page, labels.typeCode), FIXTURE_KEYS.ruleTestTypeCode);
    await typeInto(page, textbox(page, labels.categoryCode), FIXTURE_KEYS.ruleTestCategoryCode);
    await typeInto(page, textbox(page, labels.rate), rule.rate);
    await typeInto(page, textbox(page, labels.minAmount), rule.min);
    await typeInto(page, textbox(page, labels.maxAmount), rule.max);
    await typeInto(page, textbox(page, labels.activeFrom), rule.from);
    await typeInto(page, textbox(page, labels.activeTo), rule.to);
    await page.keyboard.press("Enter");
  }

  test("rejects a negative reward rate (reward_rate)", async ({ page }) => {
    await enterRule(page, { rate: "-1", min: "", max: "", from: today, to: "" });
    await expectFieldError(page, textbox(page, labels.rate), MESSAGES.REWARD_RATE_NEGATIVE);
  });

  test("rejects a minimum above the maximum (min_transaction_amount)", async ({ page }) => {
    await enterRule(page, { rate: "2.5000", min: "100.00", max: "10.00", from: today, to: "" });
    await expectFieldError(page, textbox(page, labels.minAmount), MESSAGES.REWARD_MIN_EXCEEDS_MAX);
  });

  test("rejects an end before the start (active_to)", async ({ page }) => {
    await enterRule(page, { rate: "2.5000", min: "", max: "", from: "2022-07-01", to: "2022-01-01" });
    await expectFieldError(page, textbox(page, labels.activeTo), MESSAGES.REWARD_ACTIVE_TO_BEFORE_FROM);
  });

  test("creates a valid rule", async ({ page }) => {
    const created = await expectWrite(page, ruleEndpoint("POST"), () =>
      enterRule(page, { rate: "2.5000", min: "", max: "", from: today, to: "" }),
    );
    const body: unknown = await created.json();
    const ruleId =
      typeof body === "object" && body !== null && "rule_id" in body && typeof body.rule_id === "string"
        ? body.rule_id
        : undefined;
    expect(ruleId, "the 201 body carries the new rule_id").toMatch(/^[0-9]+$/);
    createdRuleId = ruleId;
    await expect(page).toHaveURL(urlPath(screens.ruleList.route));
    expect(
      await psql(
        `SELECT concat_ws('|', transaction_type_cd, lpad(transaction_category_cd::text, 4, '0'), reward_rate,
                          coalesce(min_transaction_amount::text, '-'), coalesce(max_transaction_amount::text, '-'),
                          to_char(active_from AT TIME ZONE 'UTC', 'YYYY-MM-DD'), coalesce(active_to::text, '-'))
           FROM reward_rules WHERE rule_id = ${sqlLiteral(createdRule())}::bigint`,
      ),
    ).toBe(`${FIXTURE_KEYS.ruleTestTypeCode}|${FIXTURE_KEYS.ruleTestCategoryCode}|2.5000|-|-|${today}|-`);
  });

  test("edits the rule's rate", async ({ page }) => {
    const ruleId = createdRule();
    await page.goto(`/admin/reward-rules/${ruleId}/edit`);
    const rate = textbox(page, labels.rate);
    await expect(rate).toHaveValue("2.5000");
    await typeInto(page, rate, "2.7500");
    await expectWrite(page, ruleEndpoint("PUT", ruleId), () => page.keyboard.press("Enter"));
    await expect(page).toHaveURL(urlPath(screens.ruleList.route));
    expect(await psql(`SELECT reward_rate FROM reward_rules WHERE rule_id = ${sqlLiteral(ruleId)}::bigint`)).toBe("2.7500");
  });

  test("soft-deletes the rule from the list; the dialog traps focus first", async ({ page }) => {
    const ruleId = createdRule();
    await page.goto(screens.ruleList.route);
    const row = content(page)
      .getByRole("table", { name: screens.ruleList.caption, exact: true })
      .getByRole("row")
      .filter({ has: page.getByRole("cell", { name: ruleId, exact: true }) });
    const deleteButton = row.getByRole("button", { name: screens.ruleList.delete, exact: true });
    const dialog = confirmDialog(page);
    await pressControl(page, deleteButton);
    await expect(dialog).toHaveAccessibleName(screens.deleteDialog.title);
    await expectDialogTrapsFocus(page, dialog, ruleEndpoint("DELETE", ruleId));
    await pressControl(page, deleteButton);
    await expectWrite(page, ruleEndpoint("DELETE", ruleId), async () => {
      await pressControl(page, dialog.getByRole("button", { name: screens.deleteDialog.confirm, exact: true }));
      await expect(dialog).toBeHidden();
    });
    await expect(row.getByText(screens.ruleList.ended, { exact: true })).toBeVisible();
    expect(
      await psql(`SELECT active_to IS NOT NULL FROM reward_rules WHERE rule_id = ${sqlLiteral(ruleId)}::bigint`),
    ).toBe("t");
  });
});
