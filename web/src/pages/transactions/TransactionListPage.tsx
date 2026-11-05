// CT00 transaction list (COTRN00C, map COTRN00) at /transactions: the start
// key search, ten transactions per page in TRAN-ID order over the whole live
// master, and the row action that replaces the S selector
// (docs/parity/function-map.md, F-COTRN00-001 to F-COTRN00-005; the invalid
// AID keys of F-COTRN00-006 have no control, E-56).
//
// Search (ENTER) reads forward from the Transaction ID typed in TRNIDIN, or
// from the first key when it is blank, as PROCESS-ENTER-KEY does
// [app/cbl/COTRN00C.cbl:L206-225]. Next (PF8) reads forward from the page's
// `next_key` and Previous (PF7) backward from its `prev_key`; neither reads the
// search field, because PROCESS-PF7-KEY and PROCESS-PF8-KEY page from the keys
// shown [app/cbl/COTRN00C.cbl:L234-278]. A key pressed again with the same
// request reads the file again, as every 3270 key press did. The first entry
// lists from the lowest key.
//
// The REST resource's `account_id` filter (E-17) is not offered: COTRN00C has
// none, the map lists no such input, and its errors name no BMS field, so the
// browse stays unfiltered, as CT00 parity is measured.
//
// The texts COTRN00C shows come from the server and reach the user only
// through lib/api-errors.ts: an error as a toast and a TRNIDIN field error,
// and the page-boundary text of a 200 page as a neutral toast, or in place of
// the rows when an ENTER finds none. A PF7 or PF8 answered with no rows keeps
// the page shown and toasts its text (pageShown).
import { useEffect, useId, useMemo, useRef, useState, type JSX } from "react";
import { Link } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";

import {
  useTransactions,
  type TransactionListParams,
} from "@/api/queries/transactions";
import type { components } from "@/api/schema";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { BackLink, PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { applyApiError, showMessage } from "@/lib/api-errors";
import {
  transactionListBmsFieldMap,
  transactionListFilterSchema,
  type TransactionListFilterQuery,
  type TransactionListFilterValues,
} from "@/schemas/transactions";

type TransactionResponse = components["schemas"]["TransactionResponse"];
type TransactionPage = components["schemas"]["Page_TransactionResponse"];

/** The page a key asks for: `start_key` and `direction` of `GET /transactions`. */
type Cursor = Pick<TransactionListParams, "start_key" | "direction">;

/** The BMS name of the search field, also its element id. */
const TRNIDIN = "TRNIDIN";

/** `TRNIDINI X(16)` [app/cpy-bms/COTRN00.CPY:L66]. */
const TRAN_ID_LENGTH = 16;

/** The date part of `TRAN-ORIG-TS X(26)`, `YYYY-MM-DD`. */
const DATE_LENGTH = 10;

/** Shown in place of the rows of an empty page that carries no text. */
const NO_TRANSACTIONS = "No transactions found";

/** The ten rows of a COTRN00 page, the server's default page size. */
const PAGE_ROWS = 10;

const DEFAULT_VALUES: TransactionListFilterValues = { start_key: "" };

/**
 * The query of one key press, absent and empty values left out, so two
 * presses of one request share one query key. `page_size` is never sent: the
 * server's default is the ten rows of COTRN00.
 */
function toParams(cursor: Cursor): TransactionListParams {
  const params: TransactionListParams = {};
  if (cursor.start_key !== undefined && cursor.start_key !== "") {
    params.start_key = cursor.start_key;
  }
  if (cursor.direction !== undefined) {
    params.direction = cursor.direction;
  }
  return params;
}

function sameParams(a: TransactionListParams, b: TransactionListParams): boolean {
  return a.start_key === b.start_key && a.direction === b.direction;
}

/** The text of an empty page: the server's, unless it sent none. */
function emptyPageText(message: string | null | undefined): string {
  return typeof message === "string" && message.trim() !== ""
    ? message
    : NO_TRANSACTIONS;
}

/**
 * An answer with no rows, only its page-boundary text: for PF7 from a page
 * nothing precedes, 'You are already at the top of the page...'
 * (V-COTRN00C-009); for a key no record is at or after any more, the STARTBR
 * NOTFND text 'You are at the top of the page...' (V-COTRN00C-005,
 * V-COTRN00C-010).
 */
function isBoundaryAnswer(page: TransactionPage): boolean {
  return (
    page.items.length === 0 &&
    typeof page.message === "string" &&
    page.message.trim() !== ""
  );
}

/**
 * What the table showed when the user last pressed a key: the page, the
 * query's answer it was shown for, and whether the key is PF7 or PF8 rather
 * than ENTER.
 */
type OnScreen = {
  answer: TransactionPage | undefined;
  page: TransactionPage | undefined;
  press: boolean;
};

/**
 * The page the table shows for the query's answer `page`.
 *
 * COTRN00C answers PF7 on the first page and PF8 without a next page with
 * SEND-ERASE-NO, so the rows on screen stay and only the message line is
 * written (P-COTRN00C-009, P-COTRN00C-010, P-COTRN00C-018)
 * [app/cbl/COTRN00C.cbl:L247-251] [app/cbl/COTRN00C.cbl:L269-273], and a PF7
 * or PF8 browse whose STARTBR finds no record blanks no row (P-COTRN00C-016).
 * The API answers such a key with a page that has no rows and carries the
 * text (E-54), so that answer to a press keeps the page that was on screen,
 * with its keys, which Previous and Next page from. The empty page of an
 * ENTER, a search that finds nothing or the empty master, stands in the
 * table (E-27). While a new page loads, a page kept stays in place of the
 * empty answer the query holds over it.
 */
function pageShown(
  page: TransactionPage | undefined,
  onScreen: OnScreen,
  loadingNext: boolean,
): TransactionPage | undefined {
  if (page === undefined || onScreen.page === undefined) {
    return page;
  }
  if (loadingNext) {
    return page === onScreen.answer ? onScreen.page : page;
  }
  return onScreen.press && isBoundaryAnswer(page) ? onScreen.page : page;
}

/**
 * The COTRN00 columns in screen order ('Transaction ID', 'Date',
 * 'Description', 'Amount'), then the row action that replaces the 'Sel'
 * column. Values are shown exactly as the server sent them: the date is the
 * first ten characters of the timestamp text, never parsed, and the amount is
 * the decimal string as received. Each placeholder is its cell's markup
 * around a value as wide as the column's on a first page: the 16-digit
 * TRAN-ID, a `YYYY-MM-DD` date, a description and an amount as wide as the
 * widest the seeded first page shows, and the row action.
 */
function transactionColumns(
  idPrefix: string,
): DataTableProps<TransactionResponse>["columns"] {
  const tranIdCellId = (index: number): string => `${idPrefix}-tran-${index}`;
  return [
    {
      accessorKey: "tran_id",
      header: "Transaction ID",
      cell: ({ row }) => (
        <span id={tranIdCellId(row.index)} className="tabular-nums">
          {row.original.tran_id}
        </span>
      ),
      meta: {
        placeholder: <span className="tabular-nums">0000000000000000</span>,
      },
    },
    {
      id: "tran_orig_date",
      header: "Date",
      cell: ({ row }) => (
        <span className="tabular-nums">
          {row.original.tran_orig_ts.slice(0, DATE_LENGTH)}
        </span>
      ),
      meta: { placeholder: <span className="tabular-nums">0000-00-00</span> },
    },
    {
      accessorKey: "tran_desc",
      header: "Description",
      // BLITZY [LAYOUT]: the description may break anywhere (overrode the
      // cell's whitespace-nowrap), so a TRAN-DESC X(100) without spaces no
      // longer sets the column's minimum width: the column takes the width the
      // other columns leave, and Amount and View stay inside the card. It is
      // clamped to one line, so a row stays as tall as the skeleton and filler
      // rows, and the title gives the full text the clamp hides.
      cell: ({ row }) => (
        <span
          title={row.original.tran_desc}
          className="line-clamp-1 whitespace-normal wrap-anywhere"
        >
          {row.original.tran_desc}
        </span>
      ),
      meta: {
        placeholder: (
          <span className="line-clamp-1 whitespace-normal wrap-anywhere">
            Return item at Merchant, Jones and Company
          </span>
        ),
      },
    },
    {
      accessorKey: "tran_amt",
      header: () => <span className="block text-end">Amount</span>,
      cell: ({ row }) => (
        <span className="block text-end tabular-nums">
          {row.original.tran_amt}
        </span>
      ),
      meta: {
        placeholder: (
          <span className="block text-end tabular-nums">-999.99</span>
        ),
      },
    },
    {
      id: "actions",
      header: "Actions",
      // S on a row XCTLs to COTRN01C with its TRAN-ID
      // [app/cbl/COTRN00C.cbl:L184-196]. The view's Back returns to the list
      // named in `from`, as COTRN01C's F3 returns to its caller. The
      // accessible name is exactly the function map's; the Transaction ID
      // cell describes it, since every row has the same name.
      // BLITZY [LAYOUT]: the link button takes the height of the cell's text
      // line (overrode the default h-9 px-4), so a row is as tall as the
      // skeleton and filler rows DataTable reserves a full page with at its
      // default text row height, and paging moves neither the table nor the
      // scroll position.
      // BLITZY [A11Y]: the link is therefore a 20px-high target, below the
      // 24px WCAG 2.2 minimum; the cell padding keeps neighbouring targets
      // apart. Flagged for designer review.
      cell: ({ row }) => (
        <Button variant="link" asChild className="h-auto p-0">
          <Link
            to={`/transactions/${encodeURIComponent(row.original.tran_id)}`}
            state={{ from: "/transactions" }}
            aria-describedby={tranIdCellId(row.index)}
          >
            View
          </Link>
        </Button>
      ),
      meta: {
        placeholder: (
          <Button variant="link" asChild className="h-auto p-0">
            <span>View</span>
          </Button>
        ),
      },
    },
  ];
}

export default function TransactionListPage(): JSX.Element {
  const idPrefix = useId();
  const errorId = `${idPrefix}-${TRNIDIN}-error`;
  const columns = useMemo(() => transactionColumns(idPrefix), [idPrefix]);

  const form = useForm<
    TransactionListFilterValues,
    unknown,
    TransactionListFilterQuery
  >({
    resolver: zodResolver(transactionListFilterSchema),
    defaultValues: DEFAULT_VALUES,
  });

  // The first entry browses from LOW-VALUES, the lowest key.
  const [params, setParams] = useState<TransactionListParams>({});
  const query = useTransactions(params);
  const page = query.isError ? undefined : query.data;

  // The page the table shows (pageShown): the answer, or the page kept on
  // screen in its place after a PF7 or PF8 that found no page. `kept` tells
  // the two apart.
  const [onScreen, setOnScreen] = useState<OnScreen>({
    answer: undefined,
    page: undefined,
    press: false,
  });
  const shownPage = pageShown(page, onScreen, query.isPlaceholderData);
  const kept = shownPage !== page;

  // Whether the next page received answers a key press, whose page text is
  // then shown once. A page a cached result is refreshed with answers none.
  const announceRef = useRef(true);
  const shownRef = useRef<{ data: TransactionPage; at: number } | null>(null);
  const shownErrorRef = useRef<unknown>(null);
  const [failureText, setFailureText] = useState<string | undefined>(
    undefined,
  );

  // Sends a key press, `press` for PF7 and PF8, noting what the table shows
  // so that an answer with no rows to a press leaves it there.
  const send = (cursor: Cursor, press: boolean): void => {
    const next = toParams(cursor);
    announceRef.current = true;
    setOnScreen({ answer: page, page: shownPage, press });
    if (sameParams(next, params)) {
      void query.refetch();
    } else {
      setParams(next);
    }
  };

  // ENTER: forward from the key entered, `direction` left to the server's
  // forward default. A key the schema rejects sends nothing and shows its
  // FieldError with focus on TRNIDIN.
  const onSubmit = (values: TransactionListFilterQuery): void => {
    send({ start_key: values.start_key }, false);
  };

  // Each page received is handled once, StrictMode's repeated effects and
  // re-renders included: a fetch gives a new `dataUpdatedAt` even when its
  // rows equal the page held, and a cached page shown again is another object.
  const { data, dataUpdatedAt, isPlaceholderData, isSuccess } = query;
  useEffect(() => {
    if (!isSuccess || isPlaceholderData) {
      return;
    }
    const shown = shownRef.current;
    if (shown !== null && shown.data === data && shown.at === dataUpdatedAt) {
      return;
    }
    shownRef.current = { data, at: dataUpdatedAt };
    if (!announceRef.current) {
      return;
    }
    announceRef.current = false;
    // An empty page shows its text in place of the rows instead, which the
    // table's status region announces. The empty answer to a PF7 or PF8
    // leaves the page shown (pageShown), so its text is toasted, once for each
    // press, the message line COTRN00C rewrites (P-COTRN00C-018).
    if (data.items.length > 0 || kept) {
      showMessage(data.message);
    }
  }, [data, dataUpdatedAt, isPlaceholderData, isSuccess, kept]);

  // Each failed read is shown once: every failure is a new error object. The
  // message also stands in place of the rows, since no page arrived.
  const { error, isError } = query;
  useEffect(() => {
    if (!isError || error === shownErrorRef.current) {
      return;
    }
    shownErrorRef.current = error;
    announceRef.current = false;
    setFailureText(
      applyApiError(error, form, transactionListBmsFieldMap).message,
    );
  }, [error, isError, form]);

  // The table waits for the read that is out: the first one, a page loading
  // behind the one kept on screen, or a re-read of a request whose last read
  // failed, since that failure's text does not answer it. A page already
  // read stays shown, not busy, while it is read again: from the cache on a
  // return to the list, or after a key pressed again with the same request.
  const tableLoading =
    query.isPending ||
    query.isPlaceholderData ||
    (query.isError && query.fetchStatus !== "idle");

  // PF7 and PF8 page from the keys of the page shown, a kept page's included.
  const prevKey = shownPage?.prev_key;
  const nextKey = shownPage?.next_key;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description="Browse the transactions in Transaction ID order, ten per page, from the first or from a Transaction ID you enter."
        actions={
          // PF3: XCTL COMEN01C, the main menu [app/cbl/COTRN00C.cbl:L122-124].
          <BackLink to="/" />
        }
      />
      <Card>
        <CardContent>
          <form
            noValidate
            aria-label="Transaction search"
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              void form.handleSubmit(onSubmit)(event);
            }}
          >
            <Controller
              name="start_key"
              control={form.control}
              render={({ field, fieldState }) => (
                <Field data-invalid={fieldState.invalid} className="max-w-md">
                  <FieldLabel htmlFor={TRNIDIN}>Transaction ID</FieldLabel>
                  <div className="flex gap-2">
                    {/* The cursor starts on TRNIDIN: COTRN00C sends every
                        screen with MOVE -1 TO TRNIDINL. */}
                    <Input
                      {...field}
                      value={field.value ?? ""}
                      id={TRNIDIN}
                      maxLength={TRAN_ID_LENGTH}
                      inputMode="numeric"
                      autoComplete="off"
                      autoFocus
                      aria-invalid={fieldState.invalid}
                      aria-describedby={
                        fieldState.invalid ? errorId : undefined
                      }
                    />
                    <Button type="submit">Search</Button>
                  </div>
                  {fieldState.invalid ? (
                    <FieldError id={errorId} errors={[fieldState.error]} />
                  ) : null}
                </Field>
              )}
            />
          </form>
        </CardContent>
      </Card>
      <DataTable<TransactionResponse>
        columns={columns}
        data={shownPage?.items ?? []}
        prevKey={prevKey}
        nextKey={nextKey}
        onPrevious={() => {
          if (typeof prevKey === "string") {
            send({ start_key: prevKey, direction: "backward" }, true);
          }
        }}
        onNext={() => {
          if (typeof nextKey === "string") {
            send({ start_key: nextKey, direction: "forward" }, true);
          }
        }}
        emptyMessage={
          query.isError ? failureText : emptyPageText(shownPage?.message)
        }
        caption="Transactions"
        loading={tableLoading}
        pageSize={PAGE_ROWS}
      />
    </div>
  );
}
