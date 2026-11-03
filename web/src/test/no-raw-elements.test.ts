// Raw-element gate for the pages (AAP 0.4.5): no source under src/pages holds a raw
// button, input, select or table element. Pages build those controls from the
// generated shadcn components in components/ui (Button, Input, Select…, Table…) or
// the shared components/data-table.tsx, which own the styling, focus ring and
// aria-invalid wiring the raw elements would bypass.
//
// Each page is read as its source text through Vite's ?raw glob and scanned whole,
// comments included, because a page has no reason to hold a raw-element literal at
// all. The FN tests beside the pages are left out: they render pages and query
// button roles by name.
import { describe, expect, it } from "vitest";

// JSX intrinsic elements are lower-case and components capitalised, so the match is
// case-sensitive: `<button` is a raw element and `<Button` the shadcn component. The
// lookahead requires the tag name to end (whitespace or a newline before the first
// attribute, `/>` or `>`), which keeps `<tbody`, `<inputs`, `<selection`, `<tableX`
// and type arguments such as `<inputValue>` out. The element set is exactly the four
// AAP 0.4.5 names.
const RAW_ELEMENT = /<(button|input|select|table)(?=[\s/>])/g;

/** One raw element found in a source: its 1-based line, tag name and trimmed line. */
interface RawElement {
  readonly line: number;
  readonly element: string;
  readonly text: string;
}

function rawElements(source: string): RawElement[] {
  const lines = source.split("\n");
  return Array.from(source.matchAll(RAW_ELEMENT), (match) => {
    const line = source.slice(0, match.index).split("\n").length;
    return { line, element: match[1], text: lines[line - 1].trim() };
  });
}

// The 23 screens of AAP 0.5.1.2. COUSR03 has no page of its own: it is the delete
// dialog on UserListPage.
const EXPECTED_PAGES: readonly string[] = [
  "/src/pages/auth/LoginPage.tsx",
  "/src/pages/dashboard/DashboardPage.tsx",
  "/src/pages/admin/AdminHomePage.tsx",
  "/src/pages/accounts/AccountViewPage.tsx",
  "/src/pages/accounts/AccountEditPage.tsx",
  "/src/pages/cards/CardListPage.tsx",
  "/src/pages/cards/CardViewPage.tsx",
  "/src/pages/cards/CardEditPage.tsx",
  "/src/pages/transactions/TransactionListPage.tsx",
  "/src/pages/transactions/TransactionViewPage.tsx",
  "/src/pages/transactions/TransactionAddPage.tsx",
  "/src/pages/reports/TransactionReportPage.tsx",
  "/src/pages/payments/BillPaymentPage.tsx",
  "/src/pages/authorizations/AuthorizationSummaryPage.tsx",
  "/src/pages/authorizations/AuthorizationDetailPage.tsx",
  "/src/pages/rewards/RewardsPage.tsx",
  "/src/pages/admin/users/UserListPage.tsx",
  "/src/pages/admin/users/UserAddPage.tsx",
  "/src/pages/admin/users/UserEditPage.tsx",
  "/src/pages/admin/transaction-types/TranTypeListPage.tsx",
  "/src/pages/admin/transaction-types/TranTypeFormPage.tsx",
  "/src/pages/admin/reward-rules/RewardRuleListPage.tsx",
  "/src/pages/admin/reward-rules/RewardRuleFormPage.tsx",
];

// Keys are project-absolute paths such as /src/pages/auth/LoginPage.tsx.
const sources = import.meta.glob<string>("/src/pages/**/*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
});

const pages: readonly (readonly [file: string, source: string])[] =
  Object.entries(sources)
    .filter(([file]) => !file.endsWith(".test.tsx"))
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));

describe("raw elements in pages", () => {
  // Without this check a broken glob, or a page renamed or moved out of src/pages,
  // would leave the scan below passing over nothing.
  it("finds all 23 pages", () => {
    const files = pages.map(([file]) => file);
    expect(files).toEqual([...EXPECTED_PAGES].sort());
    expect(files).toHaveLength(23);
    const empty = pages
      .filter(([, source]) => typeof source !== "string" || source.trim() === "")
      .map(([file]) => file);
    expect(empty).toEqual([]);
  });

  it("matches raw elements but not the shadcn components", () => {
    const flagged = [
      '<button type="submit">',
      '<input\n  name="x"',
      "<select>",
      '<table className="x">',
      "<button/>",
    ];
    expect(
      flagged.map((sample) => rawElements(sample).map((hit) => hit.element)),
    ).toEqual([["button"], ["input"], ["select"], ["table"], ["button"]]);

    const ignored = [
      "<Button",
      "<Input",
      "<Select",
      "<SelectTrigger",
      "<Table",
      "<TableCell",
      "<TableHead",
      "<tbody>",
      "<thead>",
      "<inputs",
      "<selection>",
      "<tableX>",
      "useState<inputValue>(",
      'React.ComponentProps<"input">',
      "button",
      "table",
      "Press the button to refresh the table.",
    ];
    expect(ignored.filter((sample) => rawElements(sample).length > 0)).toEqual(
      [],
    );

    expect(
      rawElements('const a = 1;\n\n    <table className="x">\n  </table>\n'),
    ).toEqual([{ line: 3, element: "table", text: '<table className="x">' }]);
  });

  it("finds no raw button, input, select or table element in any page", () => {
    const violations = pages.flatMap(([file, source]) =>
      rawElements(source).map((hit) => ({ file, ...hit })),
    );
    expect(violations).toEqual([]);
  });
});
