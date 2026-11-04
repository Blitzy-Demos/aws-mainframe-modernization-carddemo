// CCLI card list (COCRDLIC, map COCRDLI) at /cards: the optional account and
// card filters, seven cards per page in card-number order, and the row actions
// that replace the S and U selectors (docs/parity/function-map.md,
// F-COCRDLI-001 to F-COCRDLI-006).
//
// Every key sends the filters as they stand on screen, because COCRDLIC
// receives and edits the map on ENTER, PF7 and PF8 alike (P-COCRDLIC-021): a
// filter the schema rejects sends no request and shows its FieldError, as
// INPUT-ERROR answers each of those keys. Search (ENTER) reads forward from
// the first card shown, the COBOL WS-CA-FIRST-CARD-NUM, which the server
// returns as each page's `prev_key`; Next (PF8) reads forward from `next_key`
// and Previous (PF7) backward from `prev_key`. A key pressed again with the
// same request reads the file again, as every 3270 key press did.
//
// The texts COCRDLIC shows come from the server and reach the user only
// through lib/api-errors.ts: an error as a toast and field errors, and the
// page-boundary text of a 200 page as a neutral toast, or in place of the rows
// when the page is empty.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type BaseSyntheticEvent,
  type JSX,
} from "react";
import { Link } from "react-router";
import { Controller, useForm, type Control } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";

import { useCards, type CardListParams } from "@/api/queries/cards";
import type { components } from "@/api/schema";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { applyApiError, showMessage } from "@/lib/api-errors";
import {
  cardListBmsFieldMap,
  cardListFilterSchema,
  type CardListFilterQuery,
  type CardListFilterValues,
} from "@/schemas/cards";

type CardSummary = components["schemas"]["CardSummary"];
type CardListResult = components["schemas"]["Page_CardSummary"];

/** The page a key asks for: `start_key` and `direction` of `GET /cards`. */
type Cursor = Pick<CardListParams, "start_key" | "direction">;

/** `ACCTSIDI X(11)` and `CARDSIDI X(16)` [app/cpy-bms/COCRDLI.CPY]. */
const ACCOUNT_ID_LENGTH = 11;
const CARD_NUMBER_LENGTH = 16;

const DEFAULT_VALUES: CardListFilterValues = { account_id: "", card_number: "" };

/**
 * The query of one key press: the filters the schema let through (a blank
 * filter is absent, so it is no filter) and the page asked for. Absent and
 * empty values are left out, so two presses of one request share one key.
 */
function toParams(filters: CardListFilterQuery, cursor: Cursor): CardListParams {
  const params: CardListParams = {};
  if (filters.account_id !== undefined) {
    params.account_id = filters.account_id;
  }
  if (filters.card_number !== undefined) {
    params.card_number = filters.card_number;
  }
  if (cursor.start_key !== undefined && cursor.start_key !== "") {
    params.start_key = cursor.start_key;
  }
  if (cursor.direction !== undefined) {
    params.direction = cursor.direction;
  }
  return params;
}

function sameParams(a: CardListParams, b: CardListParams): boolean {
  return (
    a.account_id === b.account_id &&
    a.card_number === b.card_number &&
    a.start_key === b.start_key &&
    a.direction === b.direction
  );
}

/**
 * The View (S, XCTL COCRDSLC) and Edit (U, XCTL COCRDUPC) actions of one
 * row. Both carry the row's account as `account_id`, the CDEMO-ACCT-ID the
 * COBOL passes with the card (E-55). The accessible names are exactly the
 * function map's; the card number cell describes them, since every row has
 * the same two names.
 */
function RowActions({
  card,
  describedBy,
}: {
  card: CardSummary;
  describedBy: string;
}): JSX.Element {
  const pathname = `/cards/${encodeURIComponent(card.card_number)}`;
  const search = `?account_id=${encodeURIComponent(card.account_id)}`;
  // BLITZY [LAYOUT]: the link buttons take the height of the cell's text line
  // (overrode the default h-9 px-4), so a row is as tall as the skeleton rows
  // DataTable shows while a page loads, and paging moves neither the table
  // nor the scroll position.
  // BLITZY [A11Y]: each link is therefore a 20px-high target, below the 24px
  // WCAG 2.2 minimum; the gap and cell padding keep neighbouring targets
  // apart. Flagged for designer review.
  return (
    <div className="flex items-center gap-4">
      <Button variant="link" asChild className="h-auto p-0">
        <Link to={{ pathname, search }} aria-describedby={describedBy}>
          View
        </Link>
      </Button>
      <Button variant="link" asChild className="h-auto p-0">
        <Link
          to={{ pathname: `${pathname}/edit`, search }}
          aria-describedby={describedBy}
        >
          Edit
        </Link>
      </Button>
    </div>
  );
}

/**
 * The COCRDLI columns in screen order ('Account Number', 'Card Number',
 * 'Active'), then the row actions that replace the 'Select' column. Values are
 * shown exactly as the server sent them.
 */
function cardColumns(idPrefix: string): DataTableProps<CardSummary>["columns"] {
  const cardCellId = (index: number): string => `${idPrefix}-card-${index}`;
  return [
    { accessorKey: "account_id", header: "Account Number" },
    {
      accessorKey: "card_number",
      header: "Card Number",
      cell: ({ row }) => (
        <span id={cardCellId(row.index)}>{row.original.card_number}</span>
      ),
    },
    {
      accessorKey: "card_active_status",
      header: "Active",
      // BLITZY [LAYOUT]: the badge drops its block padding (overrode the
      // default py-0.5) to fit the cell's text line, like the row actions, so
      // a row keeps the height of DataTable's loading skeleton rows.
      cell: ({ row }) => (
        <Badge variant="secondary" className="py-0">
          {row.original.card_active_status}
        </Badge>
      ),
    },
    {
      id: "actions",
      header: "Actions",
      cell: ({ row }) => (
        <RowActions card={row.original} describedBy={cardCellId(row.index)} />
      ),
    },
  ];
}

/**
 * One COCRDLI filter field. `field.ref` reaches the Input, through which
 * react-hook-form and applyApiError place the cursor on an errored filter, and
 * the input is described by its error while it shows one.
 */
function FilterField({
  control,
  name,
  id,
  label,
  maxLength,
  autoFocus = false,
}: {
  control: Control<CardListFilterValues, unknown, CardListFilterQuery>;
  name: keyof CardListFilterValues;
  id: string;
  label: string;
  maxLength: number;
  autoFocus?: boolean;
}): JSX.Element {
  const errorId = `${id}-error`;
  return (
    <Controller
      name={name}
      control={control}
      render={({ field, fieldState }) => (
        <Field data-invalid={fieldState.invalid}>
          <FieldLabel htmlFor={id}>{label}</FieldLabel>
          <Input
            {...field}
            id={id}
            maxLength={maxLength}
            inputMode="numeric"
            autoComplete="off"
            autoFocus={autoFocus}
            aria-invalid={fieldState.invalid}
            aria-describedby={fieldState.invalid ? errorId : undefined}
          />
          {fieldState.invalid ? (
            <FieldError id={errorId} errors={[fieldState.error]} />
          ) : null}
        </Field>
      )}
    />
  );
}

export default function CardListPage(): JSX.Element {
  const idPrefix = useId();
  const accountInputId = `${idPrefix}-account`;
  const cardInputId = `${idPrefix}-card`;
  const columns = useMemo(() => cardColumns(idPrefix), [idPrefix]);

  const form = useForm<CardListFilterValues, unknown, CardListFilterQuery>({
    resolver: zodResolver(cardListFilterSchema),
    defaultValues: DEFAULT_VALUES,
  });

  // A first entry lists every card from the lowest card number.
  const [params, setParams] = useState<CardListParams>({});
  const query = useCards(params);
  const page = query.isError ? undefined : query.data;

  // The first card shown (WS-CA-FIRST-CARD-NUM): the `prev_key` of the last
  // page received, kept while a failed request shows no page.
  const firstShownRef = useRef<string | undefined>(undefined);
  // Whether the next page received answers a key press, whose page text is
  // then shown once. A page a cached result is refreshed with answers none.
  const announceRef = useRef(true);
  const shownRef = useRef<{ data: CardListResult; at: number } | null>(null);
  const failedAtRef = useRef(0);
  const [failureText, setFailureText] = useState<string | undefined>(
    undefined,
  );

  const send = (next: CardListParams): void => {
    announceRef.current = true;
    if (sameParams(next, params)) {
      void query.refetch();
    } else {
      setParams(next);
    }
  };

  // One key press: the filters on screen go through the schema first, so a
  // rejected filter sends nothing whichever key was pressed.
  const pressKey = (cursor: Cursor, event?: BaseSyntheticEvent): void => {
    void form.handleSubmit((filters) => {
      send(toParams(filters, cursor));
    })(event);
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
    firstShownRef.current = data.prev_key ?? undefined;
    if (!announceRef.current) {
      return;
    }
    announceRef.current = false;
    // An empty page shows its text in place of the rows instead.
    if (data.items.length > 0) {
      showMessage(data.message);
    }
  }, [data, dataUpdatedAt, isPlaceholderData, isSuccess]);

  const { error, errorUpdatedAt, isError } = query;
  useEffect(() => {
    if (!isError || errorUpdatedAt === failedAtRef.current) {
      return;
    }
    failedAtRef.current = errorUpdatedAt;
    announceRef.current = false;
    setFailureText(applyApiError(error, form, cardListBmsFieldMap).message);
  }, [error, errorUpdatedAt, isError, form]);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="List Credit Cards"
        actions={
          <Button variant="outline" asChild>
            <Link to="/">Back</Link>
          </Button>
        }
      />
      <Card>
        <CardContent>
          <form
            noValidate
            aria-label="Card filters"
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              pressKey(
                { start_key: firstShownRef.current, direction: "forward" },
                event,
              );
            }}
          >
            <div className="grid gap-4 lg:grid-cols-2">
              {/* The cursor starts on the account: ACCTSID carries IC. */}
              <FilterField
                control={form.control}
                name="account_id"
                id={accountInputId}
                label="Account number"
                maxLength={ACCOUNT_ID_LENGTH}
                autoFocus
              />
              <FilterField
                control={form.control}
                name="card_number"
                id={cardInputId}
                label="Card number"
                maxLength={CARD_NUMBER_LENGTH}
              />
            </div>
            <div className="flex">
              <Button type="submit">Search</Button>
            </div>
          </form>
        </CardContent>
      </Card>
      <DataTable<CardSummary>
        columns={columns}
        data={page?.items ?? []}
        prevKey={page?.prev_key}
        nextKey={page?.next_key}
        onPrevious={() => {
          pressKey({
            start_key: page?.prev_key ?? undefined,
            direction: "backward",
          });
        }}
        onNext={() => {
          pressKey({
            start_key: page?.next_key ?? undefined,
            direction: "forward",
          });
        }}
        emptyMessage={query.isError ? failureText : page?.message}
        caption="Credit cards"
        loading={query.isFetching}
      />
    </div>
  );
}
