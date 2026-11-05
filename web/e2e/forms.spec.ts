// FN (AAP 0.10.2) end to end, keyboard only (AAP 0.5.4.4): every form of the
// SPA accepts valid input and rejects invalid input, and is driven with Tab,
// typing, Space, arrow keys, Enter and Escape alone. Enter submits the
// focused form (ENTER), every former PF key is a button or link reached by
// Tab, Escape closes a dialog and a dialog traps focus. Rejections assert the
// exact COBOL text (crates/domain/src/messages.rs, docs/parity/validation-audit.md).
//
// Controls and inputs come from docs/parity/function-map.md by F-ID; inputs
// the map does not list come from UNMAPPED_LABELS. Each describe that writes
// takes a checkpoint first and restores it in afterAll; the user, transaction
// type and reward rule describes take one before each test and restore it in
// afterEach, and each of their tests seeds through psql the row it edits or
// deletes, so every test runs alone, in any order and after a failed one.
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
//   DELETE), except a write held at `halt`, whose 500 is asserted instead.
// - Keys this spec creates (a user ID, a type code) are chosen per run so no
//   seeded row and no row of an interrupted earlier run holds them.
import type { APIResponse, Locator, Page, Request, Response, Route } from "@playwright/test";

import {
  apiPath,
  assertDigits,
  checkpoint,
  confirmDialog,
  content,
  controlLocator,
  expect,
  expectMessage,
  expectScreen,
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
  processingDate,
  psql,
  readJson,
  REWARD_SCREENS,
  rewardRuleResponseSchema,
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

const SUFFIX_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/** `length` random upper-case letters and digits. */
function uniqueSuffix(length: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(length)), (byte) =>
    SUFFIX_ALPHABET.charAt(byte % SUFFIX_ALPHABET.length),
  ).join("");
}

/**
 * The user ID (SEC-USR-ID X(08)) this spec creates and deletes: `E2E` and a
 * per-run suffix, so no seeded user and no user an interrupted run left
 * behind holds it.
 */
const NEW_USER_ID = `E2E${uniqueSuffix(5)}`;

/**
 * The highest type code from 99 down to 10 that no TRANSACTION_TYPE row
 * holds, for the type this spec creates and deletes.
 */
async function freeTypeCode(): Promise<string> {
  const taken = new Set((await psql("SELECT tr_type FROM transaction_types")).split("\n"));
  for (let code = 99; code >= 10; code -= 1) {
    const candidate = String(code);
    if (!taken.has(candidate)) {
      return candidate;
    }
  }
  throw new Error("every type code from 10 to 99 is taken: re-seed the database with `batch seed`");
}

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
 * Space to check it (a radio group is a single Tab stop). Each ArrowDown
 * waits until focus has left the radio it was pressed on, so every key moves
 * focus exactly one radio before the next comparison.
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
    // The roving-focus group moves focus in a timer after the keydown, not
    // during it: without this wait the next comparison sees the old radio,
    // the next key stacks a second move, and a move still pending after the
    // loop shifts focus (and Space) off the target.
    const pressedOn = await page.evaluateHandle(() => document.activeElement);
    try {
      await page.keyboard.press("ArrowDown");
      await expect
        .poll(() => pressedOn.evaluate((element) => element !== document.activeElement), {
          message: "ArrowDown moves focus off the focused radio",
        })
        .toBe(true);
    } finally {
      await pressedOn.dispose();
    }
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
 * response after asserting its `status`, by default the success status AAP
 * 0.3.1 gives a write: 201 for POST, 200 for PUT and DELETE.
 */
async function expectWrite(
  page: Page,
  endpoint: Endpoint,
  action: () => Promise<void>,
  status: number = endpoint.method === "POST" ? 201 : 200,
): Promise<Response> {
  const [response] = await Promise.all([
    page.waitForResponse((r) => isRequestTo(r.request(), endpoint)),
    action(),
  ]);
  expect(response.status(), `${endpoint.method} ${endpoint.path}`).toBe(status);
  return response;
}

/**
 * Runs `action` and asserts that no `endpoint` request was sent while it ran,
 * as for input a form's schema rejects (AAP 0.5.4.3). `action` ends on the
 * state the page reaches, so a request it would send goes out before the spy
 * is removed.
 */
async function expectNoRequest(page: Page, endpoint: Endpoint, action: () => Promise<void>): Promise<void> {
  const sent: string[] = [];
  const spy = (request: Request): void => {
    if (isRequestTo(request, endpoint)) {
      sent.push(`${request.method()} ${request.url()}`);
    }
  };
  page.on("request", spy);
  try {
    await action();
  } finally {
    page.off("request", spy);
  }
  expect(sent, `no ${endpoint.method} ${endpoint.path}`).toEqual([]);
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
// first page back. The row is compared by innerText on both sides: its
// textContent runs the cells together, so it never equals the innerText taken
// here, and the not-equal check would pass vacuously.
async function pagesForwardAndBack(page: Page, nextId: string, previousId: string): Promise<void> {
  const firstRow = firstDataRow(page);
  await expect(firstRow).toBeVisible();
  const firstPage = (await firstRow.innerText()).trim();
  await enterOn(page, nextId);
  await expect(firstRow).not.toHaveText(firstPage, { useInnerText: true });
  await enterOn(page, previousId);
  await expect(firstRow).toHaveText(firstPage, { useInnerText: true });
}

// ---------------------------------------------------------------------------
// Keyboard path of the app shell
// ---------------------------------------------------------------------------

test.describe("keyboard path", () => {
  test("skip link first, then the sidebar, then the page content", async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await page.goto("/");
    // Pages load lazily inside the shell: a Tab pressed while the content is
    // still empty finds nothing after it and wraps back to the skip link.
    await expectScreen(page);
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
    await typeInto(page, password(page), "WRONGPWD");
    // Sent from Sign on, with the answer held until the cursor is seen off
    // the password, so only the rejection can move it there.
    const login = "**/api/auth/login";
    let release: () => void = () => undefined;
    const released = new Promise<void>((resolve) => {
      release = resolve;
    });
    let arrive: () => void = () => undefined;
    const arrived = new Promise<void>((resolve) => {
      arrive = resolve;
    });
    await page.route(login, async (route) => {
      arrive();
      await released;
      await route.continue();
    });
    try {
      await pressControl(page, control(page, "F-COSGN00-001"));
      await arrived;
      await expect(password(page)).not.toBeFocused();
      release();
      await expectFieldError(page, password(page), MESSAGES.WRONG_PASSWORD_TRY_AGAIN);
    } finally {
      release();
      await page.unroute(login);
    }
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
// CAVW dashboard lookup and account view, CAUP account update
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

// The account view keeps ACCTSID on screen with the lookup's label and submit
// names (F-COACTVW-001), so ENTER there looks up another account in place: a
// new key opens its view, the key on screen is read again, and a key the
// edits reject is refused with no read (web/src/pages/accounts/AccountViewPage.tsx).
test.describe("account view key form (F-COACTVW-001, F-COACTVW-002)", () => {
  const viewRoute = (key: string): string => `/accounts/${key}`;
  const accountKey = (page: Page): Locator => input(page, "F-COACTVW-001", 0);
  // The one value of the Card section, the cross-referenced card number of
  // the account read, rendered only once its GET has answered.
  const cardNumber = (page: Page): Locator =>
    content(page)
      .locator('[data-slot="card"]')
      .filter({ has: page.getByRole("heading", { level: 2, name: "Card", exact: true }) })
      .getByRole("definition");

  /** `GET /accounts/{id}` of F-COACTVW-001 for account `key`. */
  const accountRead = (key: string): Endpoint => {
    const { method, path } = rowEndpoint("F-COACTVW-001");
    return { method, path: path.replace("{id}", key) };
  };

  /**
   * Runs `action` and asserts that it sent a GET of account `key` answered
   * 200. The wait starts before the action, so a read answered earlier does
   * not satisfy it.
   */
  const expectRead = async (page: Page, key: string, action: () => Promise<void>): Promise<void> => {
    await expectWrite(page, accountRead(key), action, 200);
  };

  /** Asserts that the view shows account `key`, whose card is `card`. */
  const expectAccountShown = async (page: Page, key: string, card: string): Promise<void> => {
    await expect(page).toHaveURL(urlPath(viewRoute(key)));
    await expect(content(page).getByRole("heading", { level: 2, name: "Account", exact: true })).toBeVisible();
    await expect(cardNumber(page)).toHaveText(card);
    await expect(content(page).getByRole("status")).toHaveText(`Account ${key} loaded`);
    await expect(accountKey(page)).toHaveValue(key);
  };

  test.beforeEach(async ({ page }) => {
    await signInWithKeyboard(page, "user");
    await expectRead(page, FIXTURE_KEYS.account, async () => {
      await page.goto(viewRoute(FIXTURE_KEYS.account));
    });
    await expectAccountShown(page, FIXTURE_KEYS.account, FIXTURE_KEYS.card);
  });

  test("ENTER with another account number opens that account's view", async ({ page }) => {
    await expectRead(page, FIXTURE_KEYS.otherAccount, () =>
      submitFrom(page, accountKey(page), FIXTURE_KEYS.otherAccount),
    );
    await expectAccountShown(page, FIXTURE_KEYS.otherAccount, FIXTURE_KEYS.otherCard);
  });

  test("ENTER on the account number on screen reads the account again", async ({ page }) => {
    // A reload of the page reads the account too, and gives the document a
    // new time origin: an unchanged origin proves ENTER sent the read.
    const loadedAt = await page.evaluate(() => performance.timeOrigin);
    await tabTo(page, accountKey(page));
    await expect(accountKey(page)).toHaveValue(FIXTURE_KEYS.account);
    await expectRead(page, FIXTURE_KEYS.account, () => page.keyboard.press("Enter"));
    expect(await page.evaluate(() => performance.timeOrigin), "read by ENTER, not by a reload").toBe(loadedAt);
    await expectAccountShown(page, FIXTURE_KEYS.account, FIXTURE_KEYS.card);
  });

  for (const { entered, key, vId, message } of [
    { entered: "a blank account number", key: "", vId: "V-COACTVWC-001", message: MESSAGES.NO_INPUT_RECEIVED },
    {
      entered: "an all-zero account number",
      key: "00000000000",
      vId: "V-COACTVWC-002",
      message: MESSAGES.ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT,
    },
    {
      entered: "a non-numeric account number",
      key: "0000000000A",
      vId: "V-COACTVWC-002",
      message: MESSAGES.ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT,
    },
    {
      entered: "an account number shorter than 11 digits",
      key: "0000000002",
      vId: "V-COACTVWC-002",
      message: MESSAGES.ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT,
    },
  ]) {
    // Submitted from the "Look up account" button, so the cursor placement on
    // ACCTSID is the rejection's own and not focus left in the field.
    test(`rejects ${entered} with no read and puts the cursor there (${vId}, ACCTSID)`, async ({ page }) => {
      await expectNoRequest(page, rowEndpoint("F-COACTVW-001"), async () => {
        await typeInto(page, accountKey(page), key);
        await enterOn(page, "F-COACTVW-001");
        await expectFieldError(page, accountKey(page), message);
      });
      await expect(page).toHaveURL(urlPath(viewRoute(FIXTURE_KEYS.account)));
    });
  }
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

  // A change that passes every edit is not saved: its Open Date then reaches
  // CSUTLDTC through CSUTLDPY EDIT-DATE-LE, an outcome E-26 holds at `halt`,
  // so the save answers 500 with the unexpected-error text and writes
  // nothing, as crates/api/tests/caup_account_update.rs pins. The
  // 'Changes committed to database' save is therefore not asserted here.
  test("a valid new credit limit is held at halt (E-26): 500, nothing written", async ({ page }) => {
    const stored = `SELECT acct_credit_limit FROM accounts WHERE account_id = ${sqlLiteral(FIXTURE_KEYS.account)}::bigint`;
    const before = await psql(stored);
    await typeInto(page, creditLimit(page), "2500.00");
    await expectWrite(page, rowEndpoint("F-COACTUP-005"), () => enterOn(page, "F-COACTUP-005"), 500);
    await expectMessage(page, MESSAGES.UNEXPECTED_ERROR);
    expect(await psql(stored)).toBe(before);
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
    // Letters and spaces only: 1230-EDIT-NAME refuses any other character
    // (V-COCRDUPC-011), a digit included.
    await typeInto(page, nameOnCard(page), "Endtoend Card Holder");
    await expectWrite(page, rowEndpoint("F-COCRDUP-005"), () => enterOn(page, "F-COCRDUP-005"));
    await expectMessage(page, MESSAGES.CHANGES_COMMITTED_TO_DATABASE);
    expect(
      await psql(`SELECT card_embossed_name FROM cards WHERE card_number = ${sqlLiteral(FIXTURE_KEYS.card)}`),
    ).toMatch(/^Endtoend Card Holder$/i);
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
  const labels = UNMAPPED_LABELS.transactionAdd;
  const accountId = (page: Page): Locator => textbox(page, labels.accountId);
  const amount = (page: Page): Locator => textbox(page, labels.amount);
  let saved: Checkpoint | undefined;

  test.beforeAll(async () => {
    saved = await transactionWriteCheckpoint([FIXTURE_KEYS.rewardAccount]);
  });

  // PF5 for `account`: the highest-keyed transaction, read by the map row's
  // backward browse of one row, fills the data fields, and the server's
  // blank-confirm check moves the cursor to the confirmation, whose place the
  // form's submit takes (MOVE -1 TO CONFIRML).
  async function copyLastTransaction(page: Page, account: string): Promise<void> {
    await typeInto(page, accountId(page), account);
    const [read] = await Promise.all([
      page.waitForRequest((request) => isRequestTo(request, rowEndpoint("F-COTRN02-006"))),
      enterOn(page, "F-COTRN02-006"),
    ]);
    const query = new URL(read.url()).searchParams;
    expect([query.get("direction"), query.get("page_size")]).toEqual(["backward", "1"]);
    await expect(control(page, "F-COTRN02-001")).toBeFocused();
    await expect(amount(page)).not.toHaveValue("");
  }

  test("rejects an amount not in the -99999999.99 form (V-COTRN02C-021, TRNAMT)", async ({ page }) => {
    await copyLastTransaction(page, FIXTURE_KEYS.rewardAccount);
    await typeInto(page, amount(page), "12.50");
    await enterOn(page, "F-COTRN02-001");
    await expectFieldError(page, amount(page), MESSAGES.AMOUNT_SHOULD_BE_IN_FORMAT);
    await expect(confirmDialog(page)).toBeHidden();
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

  test("copies the last transaction as a 01/0001 purchase; the dialog traps focus, Cancel writes nothing, Confirm adds it", async ({ page }) => {
    const count = "SELECT count(*) FROM transaction_master";
    const before = await psql(count);
    await copyLastTransaction(page, FIXTURE_KEYS.rewardAccount);
    await typeInto(page, textbox(page, labels.typeCode), FIXTURE_KEYS.tranType);
    await typeInto(page, textbox(page, labels.categoryCode), FIXTURE_KEYS.tranCategory);
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
      tran_type_cd: FIXTURE_KEYS.tranType,
      tran_cat_cd: FIXTURE_KEYS.tranCategory,
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

  test("rejects a report without a type (V-CORPT00C-001, MONTHLY)", async ({ page }) => {
    // V-CORPT00C-001 names MONTHLY, the first radio of the report-type group.
    // The group and each of its radios are aria-invalid, and the cursor of
    // MOVE -1 TO MONTHLYL lands on the Monthly radio (AAP 0.5.4.3). The
    // FieldError belongs to the group's FieldSet, not to the per-radio Field
    // that expectFieldError searches, so the text is asserted on the page.
    await enterOn(page, "F-CORPT00-001");
    await expectMessage(page, MESSAGES.SELECT_A_REPORT_TYPE_TO_PRINT_REPORT);
    const group = content(page).getByRole("radiogroup", { name: "Report type", exact: true });
    await expect(group).toHaveAttribute("aria-invalid", "true");
    const monthly = input(page, "F-CORPT00-001", 0);
    await expect(monthly).toHaveAttribute("aria-invalid", "true");
    await expect(monthly).toBeFocused();
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
    // The dialog opens once the blank-confirm POST /payments has answered,
    // so the write awaited next is the confirmed one.
    await expect(confirmDialog(page)).toBeVisible();
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
  const labels = UNMAPPED_LABELS.userForm;
  const newUserPassword = (page: Page): Locator => content(page).getByLabel(nameMatcher(labels.password));
  /** PUT (F-COUSR02-002, F-COUSR02-004) or DELETE (F-COUSR03-004) of the new user. */
  const userEndpoint = (method: "PUT" | "DELETE"): Endpoint => ({
    method,
    path: `/admin/users/${NEW_USER_ID}`,
  });
  /** The new user's row before the current test, restored after it. */
  let saved: Checkpoint | undefined;

  test.beforeEach(async ({ page }) => {
    saved = await checkpoint([{ table: "users", where: `user_id = ${sqlLiteral(NEW_USER_ID)}` }]);
    await signInWithKeyboard(page, "admin");
  });

  test.afterEach(async () => {
    await saved?.restore();
  });

  // The new user as "adds the user" stores it (Edith Tester, type U), for the
  // update and delete tests: written through psql, with USER0001's password
  // hash, so each of them runs alone. The test's checkpoint removes it.
  async function seedNewUser(): Promise<void> {
    await psql(
      `INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type)
       SELECT ${sqlLiteral(NEW_USER_ID)}, 'Edith', 'Tester', u.password_hash, 'U'
         FROM users u
        WHERE u.user_id = ${sqlLiteral(USERS.user.userId)}`,
    );
    expect(
      await psql(`SELECT count(*) FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`),
      `the new user ${NEW_USER_ID} is seeded`,
    ).toBe("1");
  }

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
    await seedNewUser();
    await openEdit(page);
    // V-COUSR02C-010: a message-only 200 that rejects nothing and writes nothing.
    await enterOn(page, "F-COUSR02-004");
    await expectMessage(page, MESSAGES.PLEASE_MODIFY_TO_UPDATE);
    await typeInto(page, textbox(page, labels.lastName), "Retested");
    await expectWrite(page, userEndpoint("PUT"), () => enterOn(page, "F-COUSR02-004"));
    await expectMessage(page, `${MESSAGES.USER_LABEL}${NEW_USER_ID}${MESSAGES.HAS_BEEN_UPDATED}`);
    expect(await psql(`SELECT sec_usr_lname FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("Retested");
  });

  test("update: Change password, checked with Space, needs a password (V-COUSR02C-007, PASSWD)", async ({ page }) => {
    await seedNewUser();
    await openEdit(page);
    const changePassword = content(page).getByRole("checkbox", { name: labels.changePassword, exact: true });
    await expect(newUserPassword(page)).toBeDisabled();
    await tabTo(page, changePassword);
    await page.keyboard.press("Space");
    await expect(changePassword).toBeChecked();
    await expect(newUserPassword(page)).toBeEnabled();
    await expect(newUserPassword(page)).toHaveValue("");
    await enterOn(page, "F-COUSR02-004");
    await expectFieldError(page, newUserPassword(page), MESSAGES.PASSWORD_CAN_NOT_BE_EMPTY);
    await tabTo(page, changePassword);
    await page.keyboard.press("Space");
    await expect(changePassword).not.toBeChecked();
    await expect(newUserPassword(page)).toBeDisabled();
  });

  test("update: Reset clears the fields, Save and exit returns to the list without a PUT for them, Cancel returns to the list, Save and exit saves valid input and returns", async ({ page }) => {
    const exitTarget = urlPath(mapControl("F-COUSR02-002").target ?? "/admin/users");
    await seedNewUser();
    await openEdit(page);
    await typeInto(page, textbox(page, labels.firstName), "Changed");
    await enterOn(page, "F-COUSR02-003");
    await expect(textbox(page, labels.firstName)).toHaveValue("");
    // P-COUSR02C-008: PF3 returns whatever the update outcome; the blank
    // fields the schema rejects are not sent.
    await expectNoRequest(page, userEndpoint("PUT"), async () => {
      await enterOn(page, "F-COUSR02-002");
      await expect(page).toHaveURL(exitTarget);
    });
    await openEdit(page);
    await enterOn(page, "F-COUSR02-005");
    await expect(page).toHaveURL(urlPath(mapControl("F-COUSR02-005").target ?? "/admin/users"));
    await openEdit(page);
    await typeInto(page, textbox(page, labels.firstName), "Edwina");
    await expectWrite(page, userEndpoint("PUT"), () => enterOn(page, "F-COUSR02-002"));
    await expect(page).toHaveURL(exitTarget);
    expect(await psql(`SELECT sec_usr_fname FROM users WHERE user_id = ${sqlLiteral(NEW_USER_ID)}`)).toBe("Edwina");
  });

  test("delete: the dialog traps focus, Escape and Cancel keep the user, Confirm deletes it", async ({ page }) => {
    const deleteButton = async (): Promise<Locator> =>
      (await listFromNewUser(page)).getByRole("button", { name: mapControl("F-COUSR00-006").name, exact: true });
    await seedNewUser();
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
  const typeCode = (page: Page): Locator => input(page, "F-COTRTUP-001", 0);
  const description = (scope: Page | Locator): Locator => textbox(scope, UNMAPPED_LABELS.tranType.description);
  const storedDescription = (): Promise<string> =>
    psql(`SELECT coalesce((SELECT tr_description FROM transaction_types WHERE tr_type = ${sqlLiteral(newTypeCode)}), '<none>')`);
  /** PUT (F-COTRTUP-002, F-COTRTLI-007) or DELETE (F-COTRTUP-007, F-COTRTLI-009) of the new type. */
  const typeEndpoint = (method: "PUT" | "DELETE"): Endpoint => ({
    method,
    path: `/admin/transaction-types/${newTypeCode}`,
  });
  /** The new type's rows before the current test, restored after it. */
  let saved: Checkpoint | undefined;
  /** The type this describe creates and deletes, chosen by freeTypeCode. */
  let newTypeCode = "";

  test.beforeAll(async () => {
    newTypeCode = await freeTypeCode();
  });

  test.beforeEach(async ({ page }) => {
    saved = await checkpoint([
      { table: "transaction_types", where: `tr_type = ${sqlLiteral(newTypeCode)}` },
      { table: "transaction_categories", where: `trc_type_code = ${sqlLiteral(newTypeCode)}` },
    ]);
    await signInWithKeyboard(page, "admin");
  });

  test.afterEach(async () => {
    await saved?.restore();
  });

  // The new type as "adds the type, then the edit rejects and saves
  // descriptions" leaves it, for the delete test: written through psql, so
  // that test runs alone. The test's checkpoint removes it.
  async function seedNewType(text: string): Promise<void> {
    await psql(
      `INSERT INTO transaction_types (tr_type, tr_description)
       VALUES (${sqlLiteral(newTypeCode)}, ${sqlLiteral(text)})`,
    );
    expect(await storedDescription(), `the new type ${newTypeCode} is seeded`).toBe(text);
  }

  async function addNewType(page: Page, text: string): Promise<void> {
    await page.goto(mapControl("F-COTRTUP-001").route);
    await submitFrom(page, typeCode(page), newTypeCode);
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
    await submitFrom(page, typeCode(page), newTypeCode);
    await expect(control(page, "F-COTRTUP-003")).toBeVisible();
    await enterOn(page, "F-COTRTUP-015");
    await expect(control(page, "F-COTRTUP-003")).toBeHidden();
    expect(await storedDescription()).toBe("<none>");
  });

  test("add rejects a blank description (V-COTRTUPC-017, TRTYDSC)", async ({ page }) => {
    await page.goto(mapControl("F-COTRTUP-001").route);
    await submitFrom(page, typeCode(page), newTypeCode);
    await expect(control(page, "F-COTRTUP-003")).toBeVisible();
    await typeInto(page, description(page), "");
    await enterOn(page, "F-COTRTUP-003");
    await expectFieldError(page, description(page), `${MESSAGES.FIELD_TRANSACTION_DESC}${MESSAGES.MUST_BE_SUPPLIED}`);
    expect(await storedDescription()).toBe("<none>");
  });

  test("adds the type, then the edit rejects and saves descriptions (V-COTRTUPC-008, TRTYDSC)", async ({ page }) => {
    await addNewType(page, "E2E TEST TYPE");
    await page.goto(`/admin/transaction-types/${newTypeCode}/edit`);
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
    await seedNewType("E2E TEST TYPE TWO");
    await page.goto(`/admin/transaction-types/${newTypeCode}/edit`);
    await enterOn(page, "F-COTRTUP-006");
    await expectDialogTrapsFocus(page, confirmDialog(page), typeEndpoint("DELETE"));
    await enterOn(page, "F-COTRTUP-006");
    await answerDialog(page, "F-COTRTUP-012");
    expect(await storedDescription()).not.toBe("<none>");
    await enterOn(page, "F-COTRTUP-014");
    await expect(page).toHaveURL(urlPath(mapControl("F-COTRTUP-014").target ?? "/admin/transaction-types/new"));
    await page.goto(`/admin/transaction-types/${newTypeCode}/edit`);
    await enterOn(page, "F-COTRTUP-006");
    await expectWrite(page, typeEndpoint("DELETE"), () => answerDialog(page, "F-COTRTUP-007"));
    await expect(page).toHaveURL(urlPath(mapControl("F-COTRTUP-007").target ?? "/admin/transaction-types/new"));
    await expect.poll(storedDescription).toBe("<none>");
  });

  test("list: rejects a bad type filter (V-COTRTLIC-001, TRTYPE) and a blank row description (V-COTRTLIC-013), updates the row in place and deletes it", async ({ page }) => {
    await addNewType(page, "E2E LIST TYPE");
    await page.goto(mapControl("F-COTRTLI-001").route);
    await submitFrom(page, input(page, "F-COTRTLI-001", 0), "AB");
    await expectFieldError(page, input(page, "F-COTRTLI-001", 0), MESSAGES.TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT);
    await submitFrom(page, input(page, "F-COTRTLI-001", 0), newTypeCode);
    const row = rowsWith(page, newTypeCode);
    await expect(row).toHaveCount(1);
    await pressControl(page, row.getByRole("button", { name: mapControl("F-COTRTLI-006").name, exact: true }));
    // V-COTRTLIC-013 names no BMS field: the row's own description input
    // carries the error and the cursor, as COTRTLIC places it on TRTYPDn.
    await typeInto(page, description(row), "");
    await enterOn(page, "F-COTRTLI-007");
    await expectFieldError(page, description(row), `${MESSAGES.FIELD_TRANSACTION_DESC}${MESSAGES.MUST_BE_SUPPLIED}`);
    expect(await storedDescription()).toBe("E2E LIST TYPE");
    await typeInto(page, description(row), "E2E LIST TYPE TWO");
    const updated = await expectWrite(page, typeEndpoint("PUT"), () => enterOn(page, "F-COTRTLI-007"));
    expect(new URL(updated.url()).searchParams.get("origin"), "the inline update is the list's").toBe("list");
    await expectMessage(page, MESSAGES.HIGHLIGHTED_ROW_WAS_UPDATED);
    await expect.poll(storedDescription).toBe("E2E LIST TYPE TWO");
    const deleteButton = (): Locator =>
      rowsWith(page, newTypeCode).getByRole("button", { name: mapControl("F-COTRTLI-008").name, exact: true });
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
  const labels = UNMAPPED_LABELS.rewardRule;
  const screens = REWARD_SCREENS;
  const ruleEndpoint = (method: "POST" | "PUT" | "DELETE", ruleId?: string): Endpoint => ({
    method,
    path: ruleId === undefined ? "/admin/reward-rules" : `/admin/reward-rules/${ruleId}`,
  });
  /** Every reward rule before the current test, restored after it. */
  let saved: Checkpoint | undefined;
  let today = "";

  test.beforeAll(async () => {
    // The processing clock's UTC day: the created rule starts no later than
    // the instant its soft delete stamps as active_to.
    today = await processingDate();
  });

  test.beforeEach(async ({ page }) => {
    saved = await checkpoint([{ table: "reward_rules", where: "TRUE" }]);
    await signInWithKeyboard(page, "admin");
  });

  test.afterEach(async () => {
    await saved?.restore();
  });

  // The rule "creates a valid rule" stores (the rule-test pair at 2.5000 from
  // midnight UTC of `today`, no amount bounds, no end), for the edit and soft
  // delete tests: written through psql, so each of them runs alone. Returns
  // its rule_id; the test's checkpoint removes it. The insert sits in a CTE so
  // psql prints the new key alone, without the INSERT tag.
  async function seedRule(): Promise<string> {
    const ruleId = await psql(
      `WITH inserted AS (
         INSERT INTO reward_rules (transaction_type_cd, transaction_category_cd, reward_rate, active_from)
         VALUES (${sqlLiteral(FIXTURE_KEYS.ruleTestTypeCode)}, ${sqlLiteral(FIXTURE_KEYS.ruleTestCategoryCode)}::integer,
                 2.5000, ${sqlLiteral(`${today}T00:00:00Z`)}::timestamptz)
         RETURNING rule_id
       )
       SELECT rule_id FROM inserted`,
    );
    return assertDigits(ruleId);
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
    // The 201 body is read from the copy route.fetch() keeps on the Node side
    // until the context closes, never from the browser's: Playwright asks
    // Chromium for that copy only when it is read, and once the form has
    // navigated to the rule list Chromium may hold it no longer ("No data
    // found for resource"). The route passes every other method on.
    const write = ruleEndpoint("POST");
    const writeUrl = urlPath(apiPath(write.path));
    let resolveFetched: (response: APIResponse) => void = () => undefined;
    let rejectFetched: (error: Error) => void = () => undefined;
    const fetched = new Promise<APIResponse>((resolve, reject) => {
      resolveFetched = resolve;
      rejectFetched = reject;
    });
    const fetchWrite = async (route: Route): Promise<void> => {
      if (route.request().method() !== write.method) {
        await route.fallback();
        return;
      }
      let response: APIResponse;
      try {
        response = await route.fetch();
      } catch (error) {
        rejectFetched(new Error(`${write.method} ${write.path} got no response`, { cause: error }));
        await route.abort();
        return;
      }
      resolveFetched(response);
      await route.fulfill({ response });
    };
    await page.route(writeUrl, fetchWrite);
    let created: APIResponse;
    try {
      [created] = await Promise.all([
        fetched,
        expectWrite(page, write, () => enterRule(page, { rate: "2.5000", min: "", max: "", from: today, to: "" })),
      ]);
    } finally {
      await page.unroute(writeUrl, fetchWrite);
    }
    const ruleId = (await readJson(created, rewardRuleResponseSchema)).rule_id;
    expect(ruleId, "the 201 body carries the new rule_id").toMatch(/^[0-9]+$/);
    await expect(page).toHaveURL(urlPath(screens.ruleList.route));
    expect(
      await psql(
        `SELECT concat_ws('|', transaction_type_cd, lpad(transaction_category_cd::text, 4, '0'), reward_rate,
                          coalesce(min_transaction_amount::text, '-'), coalesce(max_transaction_amount::text, '-'),
                          to_char(active_from AT TIME ZONE 'UTC', 'YYYY-MM-DD'), coalesce(active_to::text, '-'))
           FROM reward_rules WHERE rule_id = ${sqlLiteral(ruleId)}::bigint`,
      ),
    ).toBe(`${FIXTURE_KEYS.ruleTestTypeCode}|${FIXTURE_KEYS.ruleTestCategoryCode}|2.5000|-|-|${today}|-`);
  });

  test("edits the rule's rate", async ({ page }) => {
    const ruleId = await seedRule();
    await page.goto(`/admin/reward-rules/${ruleId}/edit`);
    const rate = textbox(page, labels.rate);
    await expect(rate).toHaveValue("2.5000");
    await typeInto(page, rate, "2.7500");
    await expectWrite(page, ruleEndpoint("PUT", ruleId), () => page.keyboard.press("Enter"));
    await expect(page).toHaveURL(urlPath(screens.ruleList.route));
    expect(await psql(`SELECT reward_rate FROM reward_rules WHERE rule_id = ${sqlLiteral(ruleId)}::bigint`)).toBe("2.7500");
  });

  test("soft-deletes the rule from the list; the dialog traps focus first", async ({ page }) => {
    const ruleId = await seedRule();
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
