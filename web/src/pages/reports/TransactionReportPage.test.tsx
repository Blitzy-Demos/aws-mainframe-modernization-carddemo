// FN tests of TransactionReportPage (CR00: CORPT00C, map CORPT00). Valid input
// reaches GET /reports/transactions only through the confirmation dialog,
// input the form schema rejects is never sent, and a server rejection lands on
// the form: its message as a toast, each `fields[]` entry on its form field,
// and the cursor on the first of them. Controls are found by the accessible
// names of the F-CORPT00 rows in docs/parity/function-map.md; expected texts
// are the CORPT00C rows of docs/parity/validation-audit.md and
// docs/parity/parity-oracle-audit.md.
//
// The network is the global the API client looks up on every request, stubbed
// per test; the client passes it the Request it built.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
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

/** The F-CORPT00-001 radio items: the map's MONTHLY, YEARLY and CUSTOM. */
const MONTHLY = "Monthly (Current Month)";
const YEARLY = "Yearly (Current Year)";
const CUSTOM = "Custom (Date Range)";

/** The width of every CVTRA07Y report record CBTRN03C writes. */
const REPORT_WIDTH = 133;

/**
 * The first records of a TRANREPT report for a range, as the server returns
 * them: fixed width, the trailing spaces kept and one record entirely spaces
 * (crates/api/tests/snapshots/cr00_reports__report_lines.snap).
 */
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

/** A report answer: WS-REPORT-NAME and the P-CORPT00C-014 text after it. */
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

/** The one request the page sent: its method and URL. */
function onlyRequest(): { method: string; url: URL } {
  expect(fetchMock).toHaveBeenCalledTimes(1);
  const [input, init] = fetchMock.mock.calls[0];
  const request = input instanceof Request ? input : new Request(input, init);
  return { method: request.method, url: new URL(request.url) };
}

/** The query the one request carried, every parameter included. */
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

/** Selects CUSTOM and types both dates, returning the two inputs. */
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

/** ENTER, then the dialog that stands for CONFIRM. */
async function runReport(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: "Run report" }));
  return screen.findByRole("alertdialog", { name: "Confirm the report" });
}

/** ENTER, then CONFIRM Y through the dialog's "Confirm" (F-CORPT00-002). */
async function runAndConfirm(user: User): Promise<void> {
  const dialog = await runReport(user);
  await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
}

/** The texts of every field error shown. */
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

/** An edit the form schema runs itself, with what it marks. */
interface ClientRejection {
  readonly id: string;
  readonly enter: (user: User) => Promise<unknown>;
  /** The control that carries `aria-invalid`. */
  readonly invalid: () => HTMLElement;
  /** Where the cursor lands, the COBOL `MOVE -1 TO <field>L`. */
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

/** A server answer to a request every client edit accepts. */
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

  it("returns to the main menu through Back", async () => {
    // F-CORPT00-004: F3=Back, XCTL to COMEN01C [app/cbl/CORPT00C.cbl:L187-189].
    const { user, router } = renderPage();

    await user.click(screen.getByRole("link", { name: "Back" }));

    expect(await screen.findByText("Main menu")).not.toBeNull();
    expect(router.state.location.pathname).toBe("/");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
