import { expect, type Locator, type Page } from "@playwright/test";

/*
 * Shared helpers of the NAV suite (`nav.spec.ts`), which walks the 21 screens through the controls
 * they render: labels, the level-1 heading, the row-24 key buttons and the message rows. Nothing
 * here imports the application, calls the API or reads the DOM through `page.evaluate`, and
 * nothing logs, attaches or dumps page content: screens can hold cardholder data.
 */

/**
 * The two sign-ons NAV uses, from the in-stream USRSEC records that `carddemo-batch dusrsecj`
 * loads (`app/jcl/DUSRSECJ.jcl:35-44`): `ADMIN001` has user type `A` (line 35) and `USER0001`
 * type `U` (line 40), both with password `PASSWORD` (`README.md:203-204`).
 */
export const ADMIN_USER = "ADMIN001";
export const REGULAR_USER = "USER0001";
export const PASSWORD = "PASSWORD";

/** Route of COSGN00, the only route a test opens directly. */
export const SIGNON_ROUTE = "/signon";

/**
 * The admin routes of AAP 0.3.5: `/admin`, `/users`, `/users/add`, `/users/update`,
 * `/users/delete`, `/transaction-types` and `/transaction-types/maintain`.
 */
export const ADMIN_PATH_PATTERN = /^\/(admin|users|transaction-types)(\/|$)/;

/** A list screen's row selection: `S` view, `U` update, `D` delete. */
export type SelectionCode = "S" | "U" | "D";

/**
 * One screen as it is rendered: its route, its level-1 heading, the label of its first input or
 * output field and, optionally, the program its `PGMNAME` header shows. This describes the UI,
 * not an API payload.
 */
export interface ScreenExpectation {
  route: string;
  title: string | RegExp;
  firstField: string | RegExp;
  program?: string;
}

/** Characters that carry meaning in a regular expression. */
const REGEX_SPECIAL = /[.*+?^${}()|[\]\\/]/g;

/**
 * A pattern matching a map label however the screen pads it: the text is trimmed, a trailing
 * `:` is dropped, every run of whitespace matches any run (`\s*`) and the pattern is anchored at
 * the start. `User ID     :` and `User ID` therefore both give `/^User\s*ID/`.
 *
 * @throws Error when nothing is left of the text to match.
 */
export function labelPattern(text: string): RegExp {
  const words = text.trim().replace(/:$/, "").trim();
  if (words === "") {
    throw new Error("labelPattern needs label text other than spaces and a colon");
  }
  const parts = words.split(/\s+/).map((part) => part.replace(REGEX_SPECIAL, "\\$&"));
  return new RegExp(`^${parts.join("\\s*")}`);
}

/** `labelPattern` for label text; a `RegExp` is used as given. */
function labelMatcher(label: string | RegExp): RegExp {
  return typeof label === "string" ? labelPattern(label) : label;
}

/**
 * Opens the sign-on screen. This is the suite's only direct navigation: each test calls it once,
 * at its start, and reaches every other screen through on-screen controls.
 */
export async function openSignon(page: Page): Promise<void> {
  await page.goto(SIGNON_ROUTE);
}

/**
 * Types the user ID and password into COSGN00 and clicks `ENTER=Sign-on` (`KEY_ROWS.COSGN00`,
 * `modern/audit/screens.md` COSGN00-046). The outcome is left to the caller's `expectScreen`.
 */
export async function signOn(page: Page, userId: string, password: string): Promise<void> {
  await page.getByLabel(labelPattern("User ID")).fill(userId);
  await page.getByLabel(labelPattern("Password")).fill(password);
  await page.getByRole("button", { name: "ENTER=Sign-on", exact: true }).click();
}

/**
 * Clicks the row-24 key button whose label is exactly `label` (`F3=Exit`, `F12=Cancel`). Keys
 * are clicked, never typed, because browsers keep F5 and F12 for themselves (AAP 0.4.4). A dark
 * key is `hidden` and so never visible, which makes it unpressable here, as intended.
 */
export async function pressKey(page: Page, label: string): Promise<void> {
  const button = page.getByRole("button", { name: label, exact: true });
  await expect(button).toBeVisible();
  await button.click();
}

/**
 * Presses Enter in `field`, which submits the screen's form as ENTER does on the terminal, also
 * on maps whose key row labels no ENTER key (COACTVW, COCRDLI, COPAU01, COTRTLI).
 *
 * @throws Error when `field` belongs to a page other than `page`.
 */
export async function pressEnter(page: Page, field: Locator): Promise<void> {
  if (field.page() !== page) {
    throw new Error("pressEnter was given a field of another page");
  }
  await field.press("Enter");
}

/**
 * Chooses menu option `nn` on COMEN01 or COADM01: types it into `Please select an option :`
 * (COMEN01-024, COADM01-024) and presses Enter.
 *
 * @throws Error when `nn` is not two digits, the width of the map's `OPTION` field.
 */
export async function chooseOption(page: Page, nn: string): Promise<void> {
  if (!/^\d{2}$/.test(nn)) {
    throw new Error(`Menu option must be two digits, got ${JSON.stringify(nn)}`);
  }
  const option = page.getByLabel(labelPattern("Please select an option"));
  await option.fill(nn);
  await pressEnter(page, option);
}

/**
 * Selects data row `row` (1-based) of the screen's list with `code` and presses Enter. The
 * row's first textbox is its 1-character selector.
 *
 * @throws Error when `row` is not a whole number of at least 1.
 */
export async function select(page: Page, row: number, code: SelectionCode): Promise<void> {
  if (!Number.isInteger(row) || row < 1) {
    throw new Error(`List rows are numbered from 1, got ${String(row)}`);
  }
  const line = page.locator("tbody tr").nth(row - 1);
  await expect(line).toBeVisible();
  const selector = line.getByRole("textbox").first();
  await selector.fill(code);
  await pressEnter(page, selector);
}

/**
 * Asserts that `e` is the screen shown: the route, the level-1 heading, the first field and, when
 * given, the `PGMNAME` header. Playwright normalises whitespace in string expectations, so the
 * caller passes map texts without their padding.
 */
export async function expectScreen(page: Page, e: ScreenExpectation): Promise<void> {
  await expect(page).toHaveURL((url) => url.pathname === e.route);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(e.title);
  await expect(page.getByLabel(labelMatcher(e.firstField)).first()).toBeVisible();
  if (e.program !== undefined) {
    await expect(page.locator('[data-field="PGMNAME"]')).toHaveText(e.program);
  }
}

/**
 * Asserts the text of the screen's first message row of `kind`: `error` is the `ERRMSG` row,
 * `info` the `INFOMSG` row or an `ERRMSG` text the program shows as information.
 */
export async function expectMessage(
  page: Page,
  kind: "error" | "info",
  text: string | RegExp,
): Promise<void> {
  await expect(page.locator(`[data-message-kind="${kind}"]`).first()).toHaveText(text);
}

/**
 * Asserts that `value` (a full card number, for example) appears neither in the page's text nor
 * in any field's value. Only the verdict is asserted, under `reason`, so a failure never prints
 * the value or the page contents (Cardholder Data Redaction).
 *
 * @throws Error when `value` is empty, which every page would contain.
 */
export async function expectValueNotShown(
  page: Page,
  value: string,
  reason: string,
): Promise<void> {
  if (value === "") {
    throw new Error("expectValueNotShown needs a non-empty value");
  }
  const shownInText = (await page.getByText(value, { exact: false }).count()) > 0;
  const values: string[] = [];
  for (const field of await page.locator("input, textarea, select").all()) {
    values.push(await field.inputValue());
  }
  const shownInInputs = values.some((v) => v.includes(value));
  expect(shownInText || shownInInputs, reason).toBe(false);
}

/**
 * Records the path of every main-frame navigation from now on, client-side route changes
 * included: Playwright reports history-API navigations as `framenavigated` too. The returned
 * array keeps growing; call this before `openSignon` to see every path a test visits.
 */
export function trackPaths(page: Page): string[] {
  const paths: string[] = [];
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) {
      paths.push(new URL(frame.url()).pathname);
    }
  });
  return paths;
}
