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
// the rows when the page is empty.
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
import { PageHeader } from "@/components/page-header";
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
 * The COTRN00 columns in screen order ('Transaction ID', 'Date',
 * 'Description', 'Amount'), then the row action that replaces the 'Sel'
 * column. Values are shown exactly as the server sent them: the date is the
 * first ten characters of the timestamp text, never parsed, and the amount is
 * the decimal string as received.
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
    },
    {
      id: "tran_orig_date",
      header: "Date",
      cell: ({ row }) => (
        <span className="tabular-nums">
          {row.original.tran_orig_ts.slice(0, DATE_LENGTH)}
        </span>
      ),
    },
    { accessorKey: "tran_desc", header: "Description" },
    {
      accessorKey: "tran_amt",
      header: () => <span className="block text-end">Amount</span>,
      cell: ({ row }) => (
        <span className="block text-end tabular-nums">
          {row.original.tran_amt}
        </span>
      ),
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
      // skeleton rows DataTable shows while a page loads, and paging moves
      // neither the table nor the scroll position.
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

  // Whether the next page received answers a key press, whose page text is
  // then shown once. A page a cached result is refreshed with answers none.
  const announceRef = useRef(true);
  const shownRef = useRef<{ data: TransactionPage; at: number } | null>(null);
  const shownErrorRef = useRef<unknown>(null);
  const [failureText, setFailureText] = useState<string | undefined>(
    undefined,
  );

  const send = (cursor: Cursor): void => {
    const next = toParams(cursor);
    announceRef.current = true;
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
    send({ start_key: values.start_key });
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
    // table's status region announces.
    if (data.items.length > 0) {
      showMessage(data.message);
    }
  }, [data, dataUpdatedAt, isPlaceholderData, isSuccess]);

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

  const prevKey = page?.prev_key;
  const nextKey = page?.next_key;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description="Browse the transactions in Transaction ID order, ten per page, from the first or from a Transaction ID you enter."
        actions={
          // PF3: XCTL COMEN01C, the main menu [app/cbl/COTRN00C.cbl:L122-124].
          <Button variant="outline" asChild>
            <Link to="/">Back</Link>
          </Button>
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
                <Field data-invalid={fieldState.invalid} className="max-w-sm">
                  <FieldLabel htmlFor={TRNIDIN}>Transaction ID</FieldLabel>
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
                    aria-describedby={fieldState.invalid ? errorId : undefined}
                  />
                  {fieldState.invalid ? (
                    <FieldError id={errorId} errors={[fieldState.error]} />
                  ) : null}
                </Field>
              )}
            />
            <div className="flex">
              <Button type="submit">Search</Button>
            </div>
          </form>
        </CardContent>
      </Card>
      <DataTable<TransactionResponse>
        columns={columns}
        data={page?.items ?? []}
        prevKey={prevKey}
        nextKey={nextKey}
        onPrevious={() => {
          if (typeof prevKey === "string") {
            send({ start_key: prevKey, direction: "backward" });
          }
        }}
        onNext={() => {
          if (typeof nextKey === "string") {
            send({ start_key: nextKey, direction: "forward" });
          }
        }}
        emptyMessage={query.isError ? failureText : emptyPageText(page?.message)}
        caption="Transactions"
        loading={query.isFetching}
      />
    </div>
  );
}
