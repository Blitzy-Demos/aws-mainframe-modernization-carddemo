// FN tests of TransactionReportPage (CR00: CORPT00C, map CORPT00). Valid input
// reaches GET /reports/transactions only through the confirmation dialog,
// input the form schema rejects is never sent, and a server rejection lands on
// the form: its message as a toast, each `fields[]` entry on its form field,
// and the cursor on the first of them. An answer that arrives after the entry
// was edited is still shown but leaves the newer entry as typed. While a
// report runs, Run report keeps the focus the dialog returns to it and sends
// nothing more. Controls are found by the accessible names of the F-CORPT00
// rows in docs/parity/function-map.md; expected texts are the CORPT00C rows of
// docs/parity/validation-audit.md and docs/parity/parity-oracle-audit.md.
//
// The network is the global the API client looks up on every request, stubbed
// per test; the client passes it the Request it built.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createMemoryRouter, RouterProvider } from "react-router";
import { toast } from "sonner";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type Mock,
  type MockInstance,
} from "vitest";

import type { components, paths } from "@/api/schema";

import TransactionReportPage from "./TransactionReportPage";

type ReportResponse = components["schemas"]["ReportResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];
type ReportRequestField = components["schemas"]["ReportRequestField"];
type ReportQuery = NonNullable<
  paths["/reports/transactions"]["get"]["parameters"]["query"]
>;
type ReportType = NonNullable<ReportQuery["report_type"]>;
type User = ReturnType<typeof userEvent.setup>;
type Fetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

// The F-CORPT00-001 radio items: the map's MONTHLY, YEARLY and CUSTOM.
const MONTHLY = "Monthly (Current Month)";
const YEARLY = "Yearly (Current Year)";
const CUSTOM = "Custom (Date Range)";

// The width of every CVTRA07Y report record CBTRN03C writes.
const REPORT_WIDTH = 133;

// The first records of a TRANREPT report for a range, as the server returns
// them: fixed width, the trailing spaces kept and one record entirely spaces
// (crates/api/tests/snapshots/cr00_reports__report_lines.snap).
function reportLines(startDate: string, endDate: string): string[] {
  return [
    "DALYREPT".padEnd(38) +
      "Daily Transaction Report".padEnd(41) +
      `Date Range: ${startDate} to ${endDate}`,
    "",
    "Transaction ID   Account ID  Transaction Type   Tran Category                      Tran Source            Amount",
    "-".repeat(REPORT_WIDTH),
    "0000000058866561 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM               183.88",
  ].map((line) => line.padEnd(REPORT_WIDTH));
}

// A report answer: WS-REPORT-NAME and the P-CORPT00C-014 text after it.
function report(
  reportName: string,
  startDate: string,
  endDate: string,
): ReportResponse {
  return {
    report_name: reportName,
    start_date: startDate,
    end_date: endDate,
    lines: reportLines(startDate, endDate),
    message: `${reportName} report submitted for printing ...`,
  };
}

let fetchMock: Mock<Fetch>;

function jsonResponse(
  status: number,
  body: ReportResponse | ErrorBody,
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

// The one request the page sent: its method and URL.
function onlyRequest(): { method: string; url: URL } {
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [input, init] = fetchMock.mock.calls[0];
  const request = input instanceof Request ? input : new Request(input, init);
  return { method: request.method, url: new URL(request.url) };
}

// The query the one request carried, every parameter included.
function onlyQuery(): Record<string, string> {
  const { method, url } = onlyRequest();
  expect(method).toBe("GET");
  expect(url.pathname).toBe("/api/reports/transactions");
  return Object.fromEntries(url.searchParams);
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      { path: "/", element: <p>Main menu</p>, handle: { crumb: "Home" } },
      {
        path: "/reports/transactions",
        element: <TransactionReportPage />,
        handle: { title: "Transaction Reports", crumb: "Transaction Reports" },
      },
    ],
    { initialEntries: ["/reports/transactions"] },
  );
  const user = userEvent.setup();
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { user, router };
}

function textbox(name: string): HTMLInputElement {
  const element = screen.getByRole("textbox", { name });
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`the ${name} textbox is not an input element`);
  }
  return element;
}

// Selects CUSTOM and types both dates, returning the two inputs.
async function enterCustomRange(
  user: User,
  startDate: string,
  endDate: string,
): Promise<{ start: HTMLInputElement; end: HTMLInputElement }> {
  await user.click(screen.getByRole("radio", { name: CUSTOM }));
  const start = textbox("Start date");
  const end = textbox("End date");
  await user.type(start, startDate);
  await user.type(end, endDate);
  return { start, end };
}

// ENTER, then the dialog that stands for CONFIRM.
async function runReport(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: "Run report" }));
  return screen.findByRole("alertdialog", { name: "Confirm the report" });
}

// ENTER, then CONFIRM Y through the dialog's "Confirm" (F-CORPT00-002).
async function runAndConfirm(user: User): Promise<void> {
  const dialog = await runReport(user);
  await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
}

// The texts of every field error shown.
async function shownFieldErrors(): Promise<(string | null)[]> {
  const alerts = await screen.findAllByRole("alert");
  return alerts.map((alert) => alert.textContent);
}

beforeEach(() => {
  fetchMock = vi.fn<Fetch>();
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// An edit the form schema runs itself, with what it marks.
interface ClientRejection {
  readonly id: string;
  readonly enter: (user: User) => Promise<unknown>;
  // The control that carries `aria-invalid`.
  readonly invalid: () => HTMLElement;
  // Where the cursor lands, the COBOL `MOVE -1 TO <field>L`.
  readonly focused: () => HTMLElement;
  readonly message: string;
}

const CLIENT_REJECTIONS: readonly ClientRejection[] = [
  {
    // No selector entered: MOVE -1 TO MONTHLYL [app/cbl/CORPT00C.cbl:L437-442].
    id: "V-CORPT00C-001",
    enter: async () => undefined,
    invalid: () => screen.getByRole("radiogroup", { name: "Report type" }),
    focused: () => screen.getByRole("radio", { name: MONTHLY }),
    message: "Select a report type to print report...",
  },
  {
    // A start date outside YYYY-MM-DD. The server would read the one-digit
    // parts through NUMVAL-C; the client asks for the CSUTLDTC picture and
    // answers with the start date's text instead.
    id: "V-CORPT00C-014",
    enter: (user) => enterCustomRange(user, "2022-1-1", "2022-07-06"),
    invalid: () => textbox("Start date"),
    focused: () => textbox("Start date"),
    message: "Start Date - Not a valid date...",
  },
];

// A server answer to a request every client edit accepts.
interface ServerRejection {
  readonly id: string;
  readonly rejected: "Start date" | "End date";
  readonly startDate: string;
  readonly endDate: string;
  readonly field: ReportRequestField;
  readonly message: string;
}

// The CSUTLDTC calendar check is the server's alone, so these dates pass the
// form schema and only the server rejects them. Each audit row's own example
// is used, with the BMS field its cursor goes to.
const SERVER_REJECTIONS: readonly ServerRejection[] = [
  {
    // 30 February; MOVE -1 TO SDTMML [app/cbl/CORPT00C.cbl:L381-406].
    id: "V-CORPT00C-014",
    rejected: "Start date",
    startDate: "2022-02-30",
    endDate: "2022-07-06",
    field: "SDTMM",
    message: "Start Date - Not a valid date...",
  },
  {
    // 31 April; MOVE -1 TO EDTMML [app/cbl/CORPT00C.cbl:L408-426].
    id: "V-CORPT00C-015",
    rejected: "End date",
    startDate: "2022-01-01",
    endDate: "2022-04-31",
    field: "EDTMM",
    message: "End Date - Not a valid date...",
  },
];

// The range typed over the sent one while its request is still running.
const NEWER_RANGE = ["2023-03-01", "2023-03-31"] as const;

// V-CORPT00C-014, on SDTMM [app/cbl/CORPT00C.cbl:L381-406].
const START_DATE_INVALID = "Start Date - Not a valid date...";

// An answer that arrives once the entry it was sent for has been replaced.
interface LateAnswer {
  readonly outcome: string;
  readonly sent: readonly [startDate: string, endDate: string];
  readonly status: number;
  readonly body: ReportResponse | ErrorBody;
  // The toast that still carries the answer's message.
  readonly toast: "success" | "error";
  readonly message: string;
  // The names of the report regions shown afterwards.
  readonly regions: readonly string[];
}

const LATE_ANSWERS: readonly LateAnswer[] = [
  {
    outcome: "printed report",
    sent: ["2022-01-01", "2022-07-06"],
    status: 200,
    body: report("Custom", "2022-01-01", "2022-07-06"),
    toast: "success",
    message: "Custom report submitted for printing ...",
    regions: ["Custom"],
  },
  {
    outcome: "V-CORPT00C-014 rejection",
    sent: ["2022-02-30", "2022-07-06"],
    status: 400,
    body: {
      code: "INVALID",
      message: START_DATE_INVALID,
      fields: [{ field: "SDTMM", message: START_DATE_INVALID }],
    } satisfies ErrorBody,
    toast: "error",
    message: START_DATE_INVALID,
    regions: [],
  },
];

// The hint shown above a record box narrower than its records.
const SCROLL_HINT = "Scroll sideways to see each line in full.";

// Lays every element out with the given sizes: jsdom performs no layout, so
// each reports 0. vi.restoreAllMocks puts the real getters back.
function stubLayout(
  scrollWidth: number,
  clientWidth: number,
): { scrollLeft: MockInstance<() => number> } {
  vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(
    scrollWidth,
  );
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(
    clientWidth,
  );
  const scrollLeft = vi
    .spyOn(HTMLElement.prototype, "scrollLeft", "get")
    .mockReturnValue(0);
  return { scrollLeft };
}

// Runs a confirmed Monthly report and returns the box holding its records.
async function showMonthlyRecords(): Promise<HTMLPreElement> {
  fetchMock.mockResolvedValueOnce(
    jsonResponse(200, report("Monthly", "2022-07-01", "2022-07-31")),
  );
  const { user } = renderPage();
  await user.click(screen.getByRole("radio", { name: MONTHLY }));
  await runAndConfirm(user);
  const shown = await screen.findByRole("region", { name: "Monthly" });
  const box = shown.querySelector("pre");
  if (box === null) {
    throw new Error("the report shows no record box");
  }
  return box;
}

// The sides of the record box an edge fade covers.
function fadedSides(box: HTMLElement): string[] {
  return Array.from(box.parentElement?.children ?? [])
    .filter((element) => element.getAttribute("aria-hidden") === "true")
    .map((fade) => {
      if (fade.classList.contains("start-0")) {
        return "start";
      }
      return fade.classList.contains("end-0") ? "end" : "unplaced";
    });
}

// The margin-top utility that applies to a field description where it stands
// among its field's children: the generated FieldDescription varies it with
// the `last:` and `nth-last-2:` variants.
function positionalMarginTop(description: Element): string | undefined {
  let variant = "";
  if (description.matches(":last-child")) {
    variant = "last:";
  } else if (description.matches(":nth-last-child(2)")) {
    variant = "nth-last-2:";
  }
  return Array.from(description.classList)
    .filter((name) => name.startsWith(variant))
    .map((name) => name.slice(variant.length))
    .find((utility) => /^-?mt-/.test(utility));
}

// Lets the timer task pass on which react-hook-form's setFocus moves focus.
async function nextTask(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

describe("TransactionReportPage (CR00)", () => {
  it("runs a confirmed custom report and shows its records untrimmed", async () => {
    // F-CORPT00-001 and F-CORPT00-002; the success text is P-CORPT00C-014.
    const toastSuccess = vi.spyOn(toast, "success");
    const answer = report("Custom", "2022-01-01", "2022-07-06");
    fetchMock.mockResolvedValueOnce(jsonResponse(200, answer));
    const { user } = renderPage();

    await enterCustomRange(user, "2022-01-01", "2022-07-06");
    const dialog = await runReport(user);
    // The dialog names the request, and nothing is sent before Confirm.
    expect(
      within(dialog).getByText(`${CUSTOM}: 2022-01-01 to 2022-07-06`).tagName,
    ).toBe("P");
    expect(fetchMock).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    const shown = await screen.findByRole("region", { name: "Custom" });
    expect(onlyQuery()).toEqual({
      report_type: "custom",
      start_date: "2022-01-01",
      end_date: "2022-07-06",
      confirm: "Y",
    });
    // The records exactly as returned: a trimmed or padded record differs.
    expect(shown.querySelector("pre")?.textContent).toBe(
      answer.lines.join("\n"),
    );
    expect(toastSuccess).toHaveBeenCalledWith(
      "Custom report submitted for printing ...",
    );
    // INITIALIZE-ALL-FIELDS, then MOVE -1 TO MONTHLYL
    // [app/cbl/CORPT00C.cbl:L445-454].
    expect(
      screen.getByRole("radio", { name: CUSTOM }).getAttribute("aria-checked"),
    ).toBe("false");
    expect(screen.queryByRole("textbox", { name: "Start date" })).toBeNull();
    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole("radio", { name: MONTHLY }),
      );
    });
  });

  it.each([
    {
      type: "monthly",
      label: MONTHLY,
      answer: report("Monthly", "2022-07-01", "2022-07-31"),
    },
    {
      type: "yearly",
      label: YEARLY,
      answer: report("Yearly", "2022-01-01", "2022-12-31"),
    },
  ] satisfies { type: ReportType; label: string; answer: ReportResponse }[])(
    "sends no dates for a $type report, whose range the server derives",
    async ({ type, label, answer }) => {
      fetchMock.mockResolvedValueOnce(jsonResponse(200, answer));
      const { user } = renderPage();

      await user.click(screen.getByRole("radio", { name: label }));
      // Only CUSTOM takes a range [app/cbl/CORPT00C.cbl:L213-255].
      expect(screen.queryByRole("textbox", { name: "Start date" })).toBeNull();
      await runAndConfirm(user);

      const shown = await screen.findByRole("region", {
        name: answer.report_name,
      });
      expect(onlyQuery()).toEqual({ report_type: type, confirm: "Y" });
      expect(shown.querySelector("pre")?.textContent).toBe(
        answer.lines.join("\n"),
      );
    },
  );

  it.each(CLIENT_REJECTIONS)(
    "rejects input the $id edit fails without sending it",
    async ({ enter, invalid, focused, message }) => {
      const { user } = renderPage();

      await enter(user);
      await user.click(screen.getByRole("button", { name: "Run report" }));

      await waitFor(() => {
        expect(invalid().getAttribute("aria-invalid")).toBe("true");
      });
      expect(await shownFieldErrors()).toEqual([message]);
      await waitFor(() => {
        expect(document.activeElement).toBe(focused());
      });
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it.each(SERVER_REJECTIONS)(
    "maps the server's $id answer onto the $rejected field",
    async ({ rejected, startDate, endDate, field, message }) => {
      const toastError = vi.spyOn(toast, "error");
      fetchMock.mockResolvedValueOnce(
        jsonResponse(400, {
          code: "INVALID",
          message,
          fields: [{ field, message }],
        } satisfies ErrorBody),
      );
      const { user } = renderPage();

      const { start, end } = await enterCustomRange(user, startDate, endDate);
      const [marked, unmarked] =
        rejected === "Start date" ? [start, end] : [end, start];
      await runAndConfirm(user);

      await waitFor(() => {
        expect(toastError).toHaveBeenCalledWith(message);
      });
      // The dates go as entered, for the server's calendar check.
      expect(onlyQuery()).toEqual({
        report_type: "custom",
        start_date: startDate,
        end_date: endDate,
        confirm: "Y",
      });
      await waitFor(() => {
        expect(marked.getAttribute("aria-invalid")).toBe("true");
      });
      expect(unmarked.getAttribute("aria-invalid")).toBe("false");
      expect(await shownFieldErrors()).toEqual([message]);
      await waitFor(() => {
        expect(document.activeElement).toBe(marked);
      });
      // The entries stay for correction, and no report is shown.
      expect([start.value, end.value]).toEqual([startDate, endDate]);
      expect(screen.queryByRole("region")).toBeNull();
    },
  );

  it.each(LATE_ANSWERS)(
    "keeps a range typed while the report runs when its $outcome arrives",
    async ({ sent: [startDate, endDate], status, body, toast: kind, message, regions }) => {
      const toasts = {
        success: vi.spyOn(toast, "success"),
        error: vi.spyOn(toast, "error"),
      };
      // The answer is held until the newer range has been typed.
      let release: (response: Response) => void = () => undefined;
      fetchMock.mockReturnValueOnce(
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
      );
      const { user } = renderPage();

      const { start, end } = await enterCustomRange(user, startDate, endDate);
      await runAndConfirm(user);
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(onlyQuery()).toEqual({
          report_type: "custom",
          start_date: startDate,
          end_date: endDate,
          confirm: "Y",
        });
      });
      await user.clear(start);
      await user.type(start, NEWER_RANGE[0]);
      await user.clear(end);
      await user.type(end, NEWER_RANGE[1]);
      release(jsonResponse(status, body));

      // The outcome of the request sent is still shown.
      await waitFor(() => {
        expect(toasts[kind]).toHaveBeenCalledWith(message);
      });
      await nextTask();
      expect(
        screen
          .queryAllByRole("region")
          .map((region) => within(region).getByRole("heading").textContent),
      ).toEqual(regions);
      // The newer entry is neither cleared nor marked, and the cursor stays
      // where it was typed instead of moving to MONTHLY or a date.
      expect(
        screen.getByRole("radio", { name: CUSTOM }).getAttribute("aria-checked"),
      ).toBe("true");
      expect([start.value, end.value]).toEqual([...NEWER_RANGE]);
      expect(
        [start, end].map((input) => input.getAttribute("aria-invalid")),
      ).toEqual(["false", "false"]);
      expect(screen.queryAllByRole("alert")).toEqual([]);
      expect(document.activeElement).toBe(end);
    },
  );

  it.each<{ how: string; press: (user: User, run: HTMLElement) => Promise<void> }>([
    { how: "a click", press: (user, run) => user.click(run) },
    { how: "Enter", press: (user) => user.keyboard("{Enter}") },
  ])(
    "keeps focus on the inert Run report while the report runs and ignores $how on it",
    async ({ press }) => {
      // The answer is held until the second press has been made.
      let release: (response: Response) => void = () => undefined;
      fetchMock.mockReturnValueOnce(
        new Promise<Response>((resolve) => {
          release = resolve;
        }),
      );
      const { user } = renderPage();

      await user.click(screen.getByRole("radio", { name: MONTHLY }));
      await runAndConfirm(user);
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
      });
      // The dialog's opener takes focus back: it is inert, not disabled.
      const run = screen.getByRole("button", { name: "Run report" });
      expect(run.getAttribute("aria-disabled")).toBe("true");
      expect(run.hasAttribute("disabled")).toBe(false);
      await waitFor(() => {
        expect(document.activeElement).toBe(run);
      });

      await press(user, run);
      await nextTask();
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(screen.queryAllByRole("alert")).toEqual([]);
      expect(onlyQuery()).toEqual({ report_type: "monthly", confirm: "Y" });
      expect(document.activeElement).toBe(run);

      release(jsonResponse(200, report("Monthly", "2022-07-01", "2022-07-31")));
      await screen.findByRole("region", { name: "Monthly" });
      expect(run.getAttribute("aria-disabled")).toBe("false");
      // INITIALIZE-ALL-FIELDS, then MOVE -1 TO MONTHLYL.
      await waitFor(() => {
        expect(document.activeElement).toBe(
          screen.getByRole("radio", { name: MONTHLY }),
        );
      });
      expect(
        screen.getByRole("radio", { name: MONTHLY }).getAttribute("aria-checked"),
      ).toBe("false");
    },
  );

  it("ignores Enter in a date text while the report runs, before any edit", async () => {
    // The date texts stay editable while a report runs; Enter in one submits
    // the form, whose submit is inert until the answer arrives.
    fetchMock.mockReturnValueOnce(new Promise<Response>(() => undefined));
    const { user } = renderPage();

    const { start, end } = await enterCustomRange(
      user,
      "2022-01-01",
      "2022-07-06",
    );
    await runAndConfirm(user);
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    // A blank end date, which the submitted form marks as it changes. The
    // edits of a submit would also move the cursor onto it.
    await user.clear(end);
    await waitFor(() => {
      expect(end.getAttribute("aria-invalid")).toBe("true");
    });
    await user.type(start, "{Enter}");
    await nextTask();

    expect(document.activeElement).toBe(start);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(onlyQuery()).toEqual({
      report_type: "custom",
      start_date: "2022-01-01",
      end_date: "2022-07-06",
      confirm: "Y",
    });
  });

  it("closes the dialog on Cancel without sending, keeping the entries", async () => {
    // F-CORPT00-003: the entries stay, so the same request can run again.
    const { user } = renderPage();

    const { start, end } = await enterCustomRange(
      user,
      "2022-01-01",
      "2022-07-06",
    );
    const dialog = await runReport(user);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(
      screen.getByRole("radio", { name: CUSTOM }).getAttribute("aria-checked"),
    ).toBe("true");
    expect([start.value, end.value]).toEqual(["2022-01-01", "2022-07-06"]);
  });

  it.each(["Start date", "End date"])(
    "hints the %s format as (YYYY-MM-DD), as CT02 does",
    async (name) => {
      const { user } = renderPage();

      await user.click(screen.getByRole("radio", { name: CUSTOM }));

      const input = textbox(name);
      expect(input.placeholder).toBe("YYYY-MM-DD");
      const [descriptionId] = (
        input.getAttribute("aria-describedby") ?? ""
      ).split(" ");
      expect(document.getElementById(descriptionId)?.textContent).toBe(
        "(YYYY-MM-DD)",
      );
    },
  );

  it("returns to the main menu through Back", async () => {
    // F-CORPT00-004: F3=Back, XCTL to COMEN01C [app/cbl/CORPT00C.cbl:L187-189].
    const { user, router } = renderPage();
    const back = screen.getByRole("link", { name: "Back" });
    // In the page header's actions, as on every screen, not the form's keys.
    expect(back.closest("header")).not.toBeNull();
    expect(back.closest("form")).toBeNull();

    await user.click(back);

    expect(await screen.findByText("Main menu")).not.toBeNull();
    expect(router.state.location.pathname).toBe("/");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("keeps both date hints on one line when only the start date has an error", async () => {
    // Empty dates stop at the first edit, the start month, so only the start
    // date is marked [app/cbl/CORPT00C.cbl:L259-265].
    const { user } = renderPage();

    await user.click(screen.getByRole("radio", { name: CUSTOM }));
    await user.click(screen.getByRole("button", { name: "Run report" }));

    expect(await shownFieldErrors()).toEqual([
      "Start Date - Month can NOT be empty...",
    ]);
    const hints = screen.getAllByText("(YYYY-MM-DD)");
    // The error under the start hint makes it its field's second-to-last
    // child, while the end hint stays the last of its field.
    expect(hints.map((hint) => hint.matches(":nth-last-child(2)"))).toEqual([
      true,
      false,
    ]);
    expect(hints.map(positionalMarginTop)).toEqual(["mt-0", "mt-0"]);
  });

  it.each([
    {
      layout: "fits its box",
      scrollWidth: 926,
      clientWidth: 926,
      description: null,
      sides: [],
    },
    {
      layout: "is wider than its box",
      scrollWidth: 1117,
      clientWidth: 926,
      description: SCROLL_HINT,
      sides: ["end"],
    },
  ] satisfies {
    layout: string;
    scrollWidth: number;
    clientWidth: number;
    description: string | null;
    sides: string[];
  }[])(
    "says the records scroll sideways only when a record $layout",
    async ({ scrollWidth, clientWidth, description, sides }) => {
      // The records are never wrapped, so a wide one scrolls inside its box;
      // the hint and the edge fade make the hidden text visible as such.
      stubLayout(scrollWidth, clientWidth);

      const box = await showMonthlyRecords();

      const describedBy = box.getAttribute("aria-describedby");
      expect(
        describedBy === null
          ? null
          : document.getElementById(describedBy)?.textContent,
      ).toBe(description);
      expect(screen.queryAllByText(SCROLL_HINT)).toHaveLength(
        description === null ? 0 : 1,
      );
      expect(fadedSides(box)).toEqual(sides);
    },
  );

  it.each([
    { position: "mid-line", scrolled: 95, sides: ["start", "end"] },
    { position: "to the line ends", scrolled: 191, sides: ["start"] },
  ])(
    "fades the hidden sides of a wide record scrolled $position",
    async ({ scrolled, sides }) => {
      // 1117px of record in a 926px box: 191px lie out of view.
      const { scrollLeft } = stubLayout(1117, 926);
      const box = await showMonthlyRecords();
      expect(fadedSides(box)).toEqual(["end"]);

      scrollLeft.mockReturnValue(scrolled);
      fireEvent.scroll(box);

      expect(fadedSides(box)).toEqual(sides);
      // The hint stays while any record is wider than the box.
      expect(screen.queryAllByText(SCROLL_HINT)).toHaveLength(1);
    },
  );
});
