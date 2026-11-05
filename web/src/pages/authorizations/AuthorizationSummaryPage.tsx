// CPVS, the pending authorizations of one account: COPAUS0C with map COPAU00,
// at /authorizations (App.tsx owns the route and its handle).
//
// The 3270 screen becomes a search form, the account header and a data table:
// - ENTER is the "Search" submit of the "Account ID" form (F-COPAU00-001). The
//   account is kept in the URL (`?account_id=`), so a reload, the browser
//   history and the detail page's Back return to the same account. Searching
//   the account already shown reads it again from the first page, as ENTER
//   re-runs GATHER-DETAILS.
// - PF3 is the "Back" link to the main menu `/` (F-COPAU00-002).
// - PF7 and PF8 are the data table's "Previous" and "Next" (F-COPAU00-003,
//   F-COPAU00-004), five authorizations a page, newest first.
// - The `S` selector is each row's "View" link to /authorizations/:authId
//   (F-COPAU00-005), carrying the account so the detail can come back here.
//
// The client edits the account ID only to avoid sending what the server would
// reject (schemas/authorizations.ts); every message, the not-found and system
// texts included, comes from the server and is shown by lib/api-errors.ts,
// with the cursor on the field as `MOVE -1 TO ACCTIDL OF COPAU0AI` puts it.
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
  type JSX,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm } from "react-hook-form";
import { Link, useSearchParams } from "react-router";

import {
  useAuthorizationSummary,
  type AuthSummary,
  type AuthSummaryParams,
} from "@/api/queries/authorizations";
import type { components } from "@/api/schema";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { BackLink, PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError, showMessage } from "@/lib/api-errors";
import {
  authSummaryBmsFieldMap,
  authSummaryFilterSchema,
  type AuthSummaryFilterValues,
} from "@/schemas/authorizations";

type AuthSummaryItem = components["schemas"]["AuthSummaryItem"];

type AuthColumns = DataTableProps<AuthSummaryItem>["columns"];

/** The page position: none for the first page (ENTER), a key and a direction for PF7 and PF8. */
type Cursor = Pick<AuthSummaryParams, "start_key" | "direction">;

const FIRST_PAGE: Cursor = {};

/** The rows of the table while the first page of an account loads. */
const NO_ITEMS: AuthSummaryItem[] = [];

/** The five rows of a COPAU00 list page, the server's default page size. */
const PAGE_ROWS = 5;

/** `ACCTIDI PIC X(11)` (COPAU00.cpy:L60). */
const ACCOUNT_ID_LENGTH = 11;

const ACCOUNT_FIELD_ID = "account_id";
const ACCOUNT_ERROR_ID = "account_id-error";

/** Whether `accountId` passes the ENTER edits, so that it may be requested. */
function isSearchable(accountId: string): boolean {
  return authSummaryFilterSchema.safeParse({ account_id: accountId }).success;
}

// ---------------------------------------------------------------------------
// Screen text the COBOL builds from the record fields
// ---------------------------------------------------------------------------

/**
 * `CNAMEO`, `ADDR001O` and `ADDR002O` are `PIC X(25)` (COPAU00.cpy:L440,
 * L452, L464): STRING stops filling them at their last byte.
 */
const SCREEN_LINE_LENGTH = 25;

/** A STRING operand `DELIMITED BY delimiter`: the text before the delimiter's first occurrence. */
function delimitedBy(value: string, delimiter: string): string {
  const at = value.indexOf(delimiter);
  return at === -1 ? value : value.slice(0, at);
}

/**
 * A STRING operand `DELIMITED BY SIZE` of a `length`-byte field: the value
 * as the field holds it. The API right-trims text, so the field's trailing
 * spaces are put back.
 */
function bySize(value: string, length: number): string {
  return value.padEnd(length, " ").slice(0, length);
}

/** The STRING of `parts` INTO a 25-byte screen field. */
function intoScreenLine(...parts: string[]): string {
  return parts.join("").slice(0, SCREEN_LINE_LENGTH);
}

/**
 * COPAUS0C GATHER-ACCOUNT-DETAILS — P-COPAUS0C-006: `CNAMEO`, the first name
 * DELIMITED BY SPACES, the middle initial, the last name DELIMITED BY SPACES,
 * one space between each (COPAUS0C.cbl:L758-764).
 */
function customerName(summary: AuthSummary): string {
  return intoScreenLine(
    delimitedBy(summary.cust_first_name, " "),
    " ",
    bySize(summary.cust_middle_name, 1),
    " ",
    delimitedBy(summary.cust_last_name, " "),
  );
}

/** `ADDR001O`: address lines 1 and 2, each DELIMITED BY two spaces, joined by a comma (COPAUS0C.cbl:L766-770). */
function addressLine1(summary: AuthSummary): string {
  return intoScreenLine(
    delimitedBy(summary.cust_addr_line_1, "  "),
    ",",
    delimitedBy(summary.cust_addr_line_2, "  "),
  );
}

/** `ADDR002O`: address line 3 DELIMITED BY two spaces, the state and the first five ZIP characters (COPAUS0C.cbl:L771-777). */
function addressLine2(summary: AuthSummary): string {
  return intoScreenLine(
    delimitedBy(summary.cust_addr_line_3, "  "),
    ",",
    bySize(summary.cust_addr_state_cd, 2),
    ",",
    bySize(summary.cust_addr_zip, 5),
  );
}

/** The scale-2 decimal text the API sends for an `S9(n)V99` field. */
const DECIMAL_TEXT = /^(-?)([0-9]+)\.([0-9]{2})$/;

/** `WS-AUTH-AMT` and `WS-DISPLAY-AMT12 PIC -zzzzzzz9.99`: 8 integer positions (COPAUS0C.cbl:L55-56). */
const AMOUNT_12_INTEGER_DIGITS = 8;

/** `WS-DISPLAY-AMT9 PIC -zzzz9.99`: 5 integer positions (COPAUS0C.cbl:L57). */
const AMOUNT_9_INTEGER_DIGITS = 5;

/**
 * A scale-2 amount MOVEd to an edited picture `-z…z9.99` with
 * `integerDigits` integer positions (P-COPAUS0C-006, P-COPAUS0C-013). The
 * MOVE drops the high-order digits the picture has no position for, as the
 * COBOL does; leading zeros are suppressed down to the units digit, and a
 * negative amount gets its `-`. The suppressed positions are blanks, which
 * HTML collapses, so they are left out. The text is edited as text, never
 * converted to a JavaScript number; a value that is not the declared decimal
 * text is shown as sent.
 */
function editedAmount(value: string, integerDigits: number): string {
  const match = DECIMAL_TEXT.exec(value);
  if (match === null) {
    return value;
  }
  const [, sign, whole, cents] = match;
  const digits = whole.padStart(integerDigits, "0").slice(-integerDigits);
  const integer = digits.replace(/^0+(?=[0-9])/, "");
  const negative = sign === "-" && /[1-9]/.test(whole + cents);
  return `${negative ? "-" : ""}${integer}.${cents}`;
}

/**
 * `WS-DISPLAY-COUNT PIC 9(03)` (COPAUS0C.cbl:L58), the field APPRCNTO and
 * DECLCNTO are moved through: the count's low-order three digits, unsigned
 * and zero-filled.
 */
function displayCount(count: number): string {
  return String(Math.abs(count)).padStart(3, "0").slice(-3);
}

/** COPAUS0C POPULATE-AUTH-LIST — P-COPAUS0C-013: `PA-AUTH-ORIG-DATE` (`YYMMDD`) shown `MM/DD/YY`. */
function screenDate(yymmdd: string): string {
  const date = bySize(yymmdd, 6);
  return `${date.slice(2, 4)}/${date.slice(4, 6)}/${date.slice(0, 2)}`;
}

/** COPAUS0C POPULATE-AUTH-LIST — P-COPAUS0C-013: `PA-AUTH-ORIG-TIME` (`HHMMSS`) shown `HH:MM:SS`. */
function screenTime(hhmmss: string): string {
  const time = bySize(hhmmss, 6);
  return `${time.slice(0, 2)}:${time.slice(2, 4)}:${time.slice(4, 6)}`;
}

// ---------------------------------------------------------------------------
// Account header (COPAU00 rows 6 to 12)
// ---------------------------------------------------------------------------

type HeaderField = {
  /** The COPAU00 output field or fields the entry shows. */
  bms: string;
  /** The BMS caption. */
  label: string;
  /** What an abbreviated caption stands for. */
  expansion?: string;
  /** The entry's lines, as the screen shows them. */
  lines: (summary: AuthSummary) => readonly string[];
  /** Figures line up in tabular digits. */
  figure?: boolean;
};

// In BMS order, each value as GATHER-ACCOUNT-DETAILS moves it to its screen
// field (COPAUS0C.cbl:L757-799): the counts through WS-DISPLAY-COUNT, the
// credit limit and balance through WS-DISPLAY-AMT12, the cash limit and
// balance and the approved and declined amounts through WS-DISPLAY-AMT9.
// Without a summary segment the COBOL moves ZERO to the summary fields; the
// API sends 0 and 0.00 for them then, which are shown edited like any other.
// ACCSTAT ('Acct Status:') has no entry: COPAUS0C moves no value to ACCSTATO,
// so the 3270 field is always blank and the response carries no field for it.
const HEADER_FIELDS: readonly HeaderField[] = [
  { bms: "CNAME", label: "Name", lines: (s) => [customerName(s)] },
  { bms: "CUSTID", label: "Customer Id", lines: (s) => [s.customer_id], figure: true },
  {
    bms: "ADDR001 ADDR002",
    label: "Address",
    lines: (s) => [addressLine1(s), addressLine2(s)],
  },
  {
    bms: "PHONE1",
    label: "PH",
    expansion: "Phone",
    lines: (s) => [s.cust_phone_num_1],
    figure: true,
  },
  {
    bms: "APPRCNT",
    label: "Approval #",
    lines: (s) => [displayCount(s.pa_approved_auth_cnt)],
    figure: true,
  },
  {
    bms: "DECLCNT",
    label: "Decline #",
    lines: (s) => [displayCount(s.pa_declined_auth_cnt)],
    figure: true,
  },
  {
    bms: "CREDLIM",
    label: "Credit Lim",
    lines: (s) => [editedAmount(s.acct_credit_limit, AMOUNT_12_INTEGER_DIGITS)],
    figure: true,
  },
  {
    bms: "CASHLIM",
    label: "Cash Lim",
    lines: (s) => [editedAmount(s.acct_cash_credit_limit, AMOUNT_9_INTEGER_DIGITS)],
    figure: true,
  },
  {
    bms: "APPRAMT",
    label: "Appr Amt",
    lines: (s) => [editedAmount(s.pa_approved_auth_amt, AMOUNT_9_INTEGER_DIGITS)],
    figure: true,
  },
  {
    bms: "CREDBAL",
    label: "Credit Bal",
    lines: (s) => [editedAmount(s.pa_credit_balance, AMOUNT_12_INTEGER_DIGITS)],
    figure: true,
  },
  {
    bms: "CASHBAL",
    label: "Cash Bal",
    lines: (s) => [editedAmount(s.pa_cash_balance, AMOUNT_9_INTEGER_DIGITS)],
    figure: true,
  },
  {
    bms: "DECLAMT",
    label: "Decl Amt",
    lines: (s) => [editedAmount(s.pa_declined_auth_amt, AMOUNT_9_INTEGER_DIGITS)],
    figure: true,
  },
];

const HEADER_GRID = "grid grid-cols-1 gap-x-6 gap-y-2 lg:grid-cols-2";

function AccountHeader({ summary }: { summary: AuthSummary }): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Account {summary.account_id}</h2>
        </CardTitle>
      </CardHeader>
      <CardContent>
        <dl className={HEADER_GRID}>
          {HEADER_FIELDS.map((field) => (
            <div key={field.bms} className="flex flex-col gap-1">
              <dt className="text-sm text-muted-foreground">
                {field.expansion === undefined ? (
                  field.label
                ) : (
                  <abbr title={field.expansion}>{field.label}</abbr>
                )}
              </dt>
              <dd
                className={
                  field.figure === true
                    ? "text-sm font-medium tabular-nums wrap-anywhere"
                    : "text-sm font-medium wrap-anywhere"
                }
              >
                {field.lines(summary).map((line, index) => (
                  <span key={index} className="block">
                    {line}
                  </span>
                ))}
              </dd>
            </div>
          ))}
        </dl>
      </CardContent>
    </Card>
  );
}

/** The screen lines of an entry: one per COPAU00 output field it shows. */
function screenLineCount(field: HeaderField): number {
  return field.bms.split(" ").length;
}

/**
 * The header card while the first page of an account loads; the table's busy
 * state announces the wait. Each caption and each value line is a skeleton
 * centred in the 20px line box of the header's text-sm (h-4 with my-0.5), so
 * every entry takes the height it takes once read.
 */
function AccountHeaderSkeleton(): JSX.Element {
  return (
    <Card aria-hidden="true">
      <CardHeader>
        <Skeleton className="h-4 w-48" />
      </CardHeader>
      <CardContent>
        <div className={HEADER_GRID}>
          {HEADER_FIELDS.map((field) => (
            <div key={field.bms} className="flex flex-col gap-1">
              <Skeleton className="my-0.5 h-4 w-24" />
              <div className="flex flex-col">
                {Array.from({ length: screenLineCount(field) }, (_, line) => (
                  <Skeleton key={line} className="my-0.5 h-4 w-full" />
                ))}
              </div>
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Authorization list (COPAU00 rows 16 to 20)
// ---------------------------------------------------------------------------

type BadgeVariant = NonNullable<ComponentProps<typeof Badge>["variant"]>;

/** `PAPRV0nI`: `A` approved, `D` declined; the letter is shown, so colour is never the only cue. */
function approvalVariant(approved: string): BadgeVariant {
  if (approved === "A") {
    return "secondary";
  }
  return approved === "D" ? "destructive" : "outline";
}

/**
 * The COPAU00 list columns in BMS order, then the row action that replaces
 * the `S` selector. Each "View" link keeps the exact name of F-COPAU00-005
 * and is described by its row's transaction ID, so links are told apart
 * without changing their name. Each placeholder is its cell's markup around a
 * value of the column's display format: a 15-position alphanumeric
 * transaction ID (letters, then digits), `MM/DD/YY`, `HH:MM:SS`, the type, the
 * A/D and match-status letters, an amount as wide as the widest of the seeded
 * amounts, and the row action.
 */
function authorizationColumns(detailSearch: string, idPrefix: string): AuthColumns {
  const transactionIdOf = (authId: string): string => `${idPrefix}-tran-${authId}`;
  return [
    {
      accessorKey: "pa_transaction_id",
      header: "Transaction ID",
      cell: ({ row }) => (
        <span id={transactionIdOf(row.original.auth_id)} className="tabular-nums">
          {row.original.pa_transaction_id}
        </span>
      ),
      meta: { placeholder: <span className="tabular-nums">AA0000000000000</span> },
    },
    {
      id: "date",
      header: "Date",
      cell: ({ row }) => (
        <span className="tabular-nums">{screenDate(row.original.pa_auth_orig_date)}</span>
      ),
      meta: { placeholder: <span className="tabular-nums">00/00/00</span> },
    },
    {
      id: "time",
      header: "Time",
      cell: ({ row }) => (
        <span className="tabular-nums">{screenTime(row.original.pa_auth_orig_time)}</span>
      ),
      meta: { placeholder: <span className="tabular-nums">00:00:00</span> },
    },
    { accessorKey: "pa_auth_type", header: "Type", meta: { placeholder: "POS" } },
    {
      accessorKey: "approved",
      header: () => <abbr title="Approved or declined">A/D</abbr>,
      // BLITZY [LAYOUT]: the badge drops its block padding (overrode the
      // default py-0.5) to fit the cell's text line, like the View link, so a
      // row is as tall as DataTable's skeleton and filler rows at its default
      // text row height.
      cell: ({ row }) => (
        <Badge variant={approvalVariant(row.original.approved)} className="py-0">
          {row.original.approved}
        </Badge>
      ),
      meta: {
        placeholder: (
          <Badge variant={approvalVariant("A")} className="py-0">
            A
          </Badge>
        ),
      },
    },
    {
      accessorKey: "pa_match_status",
      header: () => <abbr title="Match status">STS</abbr>,
      meta: { placeholder: "P" },
    },
    {
      accessorKey: "pa_approved_amt",
      header: () => <span className="block text-right">Amount</span>,
      cell: ({ row }) => (
        <span className="block text-right tabular-nums">
          {editedAmount(row.original.pa_approved_amt, AMOUNT_12_INTEGER_DIGITS)}
        </span>
      ),
      meta: {
        placeholder: <span className="block text-right tabular-nums">-999.99</span>,
      },
    },
    {
      id: "view",
      header: "Actions",
      // BLITZY [LAYOUT]: the link button takes the height of the cell's text
      // line (overrode the default h-9 px-4), as the row links of the other
      // data tables do, so a row is as tall as its text rather than the 36px
      // button.
      // BLITZY [A11Y]: the link is therefore a 20px-high target, below the
      // 24px WCAG 2.2 minimum; the cell padding keeps neighbouring targets
      // apart. Flagged for designer review.
      cell: ({ row }) => (
        <Button asChild variant="link" className="h-auto p-0">
          <Link
            to={{
              pathname: `/authorizations/${encodeURIComponent(row.original.auth_id)}`,
              search: detailSearch,
            }}
            aria-describedby={transactionIdOf(row.original.auth_id)}
          >
            View
          </Link>
        </Button>
      ),
      meta: {
        placeholder: (
          <Button asChild variant="link" className="h-auto p-0">
            <span>View</span>
          </Button>
        ),
      },
    },
  ];
}

/**
 * The paging callbacks of the table shown before the first page arrives:
 * there is no key to page from yet, and DataTable disables Previous and Next
 * while it loads.
 */
const ignorePageRequest = (): undefined => undefined;

type AuthorizationResultsProps = {
  /** An account ID that passes the ENTER edits. */
  accountId: string;
  /** Counts the searches of this same account; each one reads it again from the first page. */
  search: number;
  /** Shows a failed read on the search form. */
  onError: (error: unknown) => void;
};

/**
 * The header and the list of one account. Mounted per account (keyed by the
 * page), so every account starts on its first page, as COPAUS0C's
 * GATHER-DETAILS resets the page number.
 */
function AuthorizationResults({
  accountId,
  search,
  onError,
}: AuthorizationResultsProps): JSX.Element | null {
  const [cursor, setCursor] = useState<Cursor>(FIRST_PAGE);
  const [servedSearch, setServedSearch] = useState(search);
  const [reread, setReread] = useState(false);
  // A new search of the same account goes back to the first page during this
  // render, so the query below already asks for it.
  if (search !== servedSearch) {
    setServedSearch(search);
    setCursor(FIRST_PAGE);
    setReread(true);
  }

  const {
    data,
    error,
    isError,
    isFetching,
    fetchStatus,
    isPlaceholderData,
    dataUpdatedAt,
    refetch,
  } = useAuthorizationSummary({ account_id: accountId, ...cursor });
  // A failed read sends COPAU00 without the header and list GATHER-DETAILS
  // would have filled, so a page read earlier under this key (a same-account
  // Search, or Previous or Next back to a page already read) is not presented
  // as the answer: no header, no rows and no paging until a read succeeds.
  const current = isError ? undefined : data;

  // ENTER always reads again. Moving back to the first page starts a read of
  // its own when its cached copy is stale (isFetching is already true then);
  // otherwise the page is read here, once.
  useEffect(() => {
    if (!reread) {
      return;
    }
    setReread(false);
    if (!isFetching) {
      void refetch();
    }
  }, [reread, isFetching, refetch]);

  // The COBOL message line of a page, the top- or bottom-of-page text, is
  // shown once per response, never again on a re-render.
  const shownResponse = useRef<{ data: AuthSummary; at: number } | null>(null);
  useEffect(() => {
    if (data === undefined || isPlaceholderData) {
      return;
    }
    const last = shownResponse.current;
    if (last !== null && last.data === data && last.at === dataUpdatedAt) {
      return;
    }
    shownResponse.current = { data, at: dataUpdatedAt };
    showMessage(data.message);
  }, [data, dataUpdatedAt, isPlaceholderData]);

  // Each failed read is shown once: every failure is a new error object. A
  // failure a read again already supersedes, such as the cached one a return
  // to the page reads again, is not shown again.
  const failed = isError && fetchStatus === "idle" ? error : null;
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (failed === null || failed === shownError.current) {
      return;
    }
    shownError.current = failed;
    onError(failed);
  }, [failed, onError]);

  const idPrefix = useId();
  const detailSearch = `?${new URLSearchParams({ account_id: accountId }).toString()}`;
  const columns = useMemo(
    () => authorizationColumns(detailSearch, idPrefix),
    [detailSearch, idPrefix],
  );

  if (current === undefined) {
    // A failed read leaves the screen to the search form, whose field and
    // toast carry the server's message, the first read's and a read again's
    // alike. Read again (a new Search), in flight or waiting offline, it
    // loads as the first read does.
    if (failed !== null) {
      return null;
    }
    return (
      <>
        <AccountHeaderSkeleton />
        <DataTable
          columns={columns}
          data={NO_ITEMS}
          onPrevious={ignorePageRequest}
          onNext={ignorePageRequest}
          caption="Pending authorizations"
          loading
          pageSize={PAGE_ROWS}
        />
      </>
    );
  }

  // The list is busy while any read of the page shown is outstanding, so held
  // rows are shown busy, never presented as its result: a new page held over
  // the previous one (placeholder data, kept too while that read waits
  // offline), a same-account Search reading the first page again, or a page
  // already read, shown from the cache and read again when Previous or Next
  // returns to it.
  const reading = isFetching || isPlaceholderData;
  const prevKey = current.prev_key;
  const nextKey = current.next_key;
  return (
    <>
      <AccountHeader summary={current} />
      <DataTable
        columns={columns}
        data={current.items}
        prevKey={prevKey}
        nextKey={nextKey}
        onPrevious={() => {
          if (prevKey !== null) {
            setCursor({ start_key: prevKey, direction: "backward" });
          }
        }}
        onNext={() => {
          if (nextKey !== null) {
            setCursor({ start_key: nextKey, direction: "forward" });
          }
        }}
        emptyMessage={current.message}
        caption="Pending authorizations"
        loading={reading}
        pageSize={PAGE_ROWS}
      />
    </>
  );
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

export default function AuthorizationSummaryPage(): JSX.Element {
  const [searchParams, setSearchParams] = useSearchParams();
  const accountParam = searchParams.get("account_id") ?? "";
  const searchable = isSearchable(accountParam);
  const [search, setSearch] = useState(0);

  const form = useForm<AuthSummaryFilterValues>({
    resolver: zodResolver(authSummaryFilterSchema),
    defaultValues: { account_id: accountParam },
  });
  const { control, handleSubmit, reset, trigger } = form;

  // The field shows the account in the URL. An account that fails the edits
  // is shown with its message and requested from nobody; an absent one is
  // the empty first entry of the screen.
  useEffect(() => {
    reset({ account_id: accountParam });
    if (accountParam !== "" && !isSearchable(accountParam)) {
      void trigger("account_id", { shouldFocus: true });
    }
  }, [accountParam, reset, trigger]);

  const showSearchError = useCallback(
    (error: unknown) => {
      applyApiError(error, form, authSummaryBmsFieldMap);
    },
    [form],
  );

  const onSubmit = (values: AuthSummaryFilterValues): void => {
    if (values.account_id === accountParam) {
      setSearch((count) => count + 1);
    } else {
      // An update within the page, so the scroll position stays (AppRoot).
      setSearchParams(
        { account_id: values.account_id },
        { preventScrollReset: true },
      );
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description="Enter an account ID to view its pending authorizations, newest first."
        actions={<BackLink to="/" />}
      />
      <Card>
        <CardContent>
          <form noValidate onSubmit={handleSubmit(onSubmit)}>
            <Controller
              control={control}
              name="account_id"
              render={({ field, fieldState }) => (
                <Field data-invalid={fieldState.invalid} className="max-w-md">
                  <FieldLabel htmlFor={ACCOUNT_FIELD_ID}>Account ID</FieldLabel>
                  <div className="flex gap-2">
                    {/* The cursor starts on ACCTID: COPAUS0C sends every screen
                        with MOVE -1 TO ACCTIDL
                        [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L188, L196]. */}
                    <Input
                      {...field}
                      id={ACCOUNT_FIELD_ID}
                      maxLength={ACCOUNT_ID_LENGTH}
                      inputMode="numeric"
                      autoComplete="off"
                      autoFocus
                      aria-invalid={fieldState.invalid}
                      aria-describedby={fieldState.invalid ? ACCOUNT_ERROR_ID : undefined}
                    />
                    <Button type="submit">Search</Button>
                  </div>
                  <FieldError id={ACCOUNT_ERROR_ID} errors={[fieldState.error]} />
                </Field>
              )}
            />
          </form>
        </CardContent>
      </Card>
      {searchable ? (
        <AuthorizationResults
          key={accountParam}
          accountId={accountParam}
          search={search}
          onError={showSearchError}
        />
      ) : null}
    </div>
  );
}
