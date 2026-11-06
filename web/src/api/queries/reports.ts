/**
 * CR00 transaction report request (`CORPT00C`, map `CORPT00`): the hook of
 * `TransactionReportPage` over `GET /reports/transactions`, the synchronous
 * replacement of the TDQ `JOBS` → JES `TRANREPT` submission.
 *
 * Each confirmed ENTER on `CORPT00` submits a new job, so the report runs on
 * submit as a mutation and nothing is cached: no query key holds a report,
 * and asking for the same range again prints it again.
 */
import { type DefaultError, useMutation } from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

/** The key root of the report requests. */
export const reportKeys = { all: ["reports"] } as const;

/**
 * The query of `GET /reports/transactions`: `report_type` (`monthly`,
 * `yearly` or `custom`, the map's `MONTHLY`/`YEARLY`/`CUSTOM` selectors), the
 * custom `start_date` and `end_date` as `YYYY-MM-DD` text (`SDTYYYY`,
 * `SDTMM`, `SDTDD` and `EDTYYYY`, `EDTMM`, `EDTDD`), and `confirm`
 * (`CONFIRM X(1)`, `Y` or `N`).
 */
export type ReportParams = NonNullable<
  paths["/reports/transactions"]["get"]["parameters"]["query"]
>;

/**
 * The answer to a confirmed request: the `CVTRA07Y` records `CBTRN03C` writes
 * for the range, untrimmed and in order, and the
 * `<name> report submitted for printing ...` message.
 */
export type TransactionReport = components["schemas"]["ReportResponse"];

/**
 * Prints the transaction report of `params`.
 *
 * - The dates go to the server exactly as the form holds them. The server
 *   splits each at its hyphens into the year, month and day parts as entered
 *   and runs every `CORPT00C` edit on them, so the text is neither padded,
 *   reformatted nor parsed here.
 * - The report resolves unchanged. Its `lines` are fixed-width records whose
 *   trailing spaces are part of the `TRANREPT` parity, so they are neither
 *   trimmed, padded nor mapped; the page shows them in a monospace block.
 * - Every answer other than the printed report rejects with the `ApiError`
 *   `unwrap` raises, untouched, for the page's `applyApiError`: a failed date
 *   edit, an unselected report type, and the confirmation outcomes as well.
 *   The server answers the confirmation prompt (blank `confirm`), the form
 *   `N` clears and an invalid `confirm` with 400 and their `CORPT00C` message
 *   on their BMS field, since each sets `WS-ERR-FLG`; a 200 is always a
 *   printed report, so no message-only report reaches the page as data.
 */
export function useTransactionReportMutation() {
  return useMutation<TransactionReport, DefaultError, ReportParams>({
    mutationKey: [...reportKeys.all, "transactions"],
    mutationFn: async (params) =>
      unwrap(
        await api.GET("/reports/transactions", { params: { query: params } }),
      ),
  });
}
