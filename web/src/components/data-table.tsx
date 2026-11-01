// The application's single data table: one page of a COBOL key-ordered browse
// (card list, transaction list, pending authorizations, reward history and the
// admin lists) with the Previous and Next controls that replace PF7 and PF8.
//
// Paging is the API's cursor paging: a request names `start_key`, `direction`
// and `page_size`, and the page answers `{ items, next_key, prev_key }`. The
// keys are opaque strings the table reads only to enable or disable Previous
// and Next. The `onPrevious` and `onNext` callbacks take no arguments: the
// caller requests the adjacent page with the `prev_key` or `next_key` it
// already holds, closing over the matching key and direction, so no page
// number is ever computed and no key is ever converted. Rows render in the
// order the API returns them, which is the COBOL key order, so the table has no
// client-side sorting, filtering or pagination row models.
//
// While the caller's page request is pending (`loading`), skeleton rows stand
// in for the page and paging is suppressed. A visually hidden status region
// announces each completed page by its row count or empty-page text, because
// a page change replaces the rows without moving focus.
//
// Built on the pinned TanStack Table 8 API (useReactTable, getCoreRowModel,
// flexRender, ColumnDef). The current shadcn Data Table guide targets
// TanStack Table 9 and does not apply.
import {
  useEffect,
  useState,
  type JSX,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";

import { Card, CardFooter } from "@/components/ui/card";
import {
  Pagination,
  PaginationContent,
  PaginationItem,
  PaginationNext,
  PaginationPrevious,
} from "@/components/ui/pagination";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

/**
 * Props of {@link DataTable}.
 *
 * - `columns`: TanStack column definitions (`accessorKey`/`header`/`cell`). A
 *   row-action column supplies its own `cell` rendering `Button variant="link"`,
 *   the replacement of the 3270 `S`/`U` selectors.
 * - `data`: the page's `items`, in the order the API returned them. Keep one
 *   non-empty array per result across renders (the query's `items` as
 *   returned): TanStack Table re-derives its rows for every new array, and the
 *   status region announces every new non-empty array as a new result. Every
 *   empty array is the same result, so an inline fallback such as
 *   `page?.items ?? []` is announced again only when the empty-page text
 *   changes or `loading` ends.
 * - `prevKey`, `nextKey`: the page's `prev_key` and `next_key`. A non-empty
 *   string enables Previous (PF7) or Next (PF8); `null` or absent disables it.
 * - `onPrevious`, `onNext`: called when the enabled control is activated. The
 *   caller requests the adjacent page with the matching key and direction.
 * - `emptyMessage`: the COBOL message the API returned with an empty page,
 *   shown verbatim in place of the rows; "No records" when it is absent.
 * - `caption`: the table's accessible caption, rendered under the table, and
 *   the name the status region announces each result under.
 * - `loading`: true while the page's request is pending, the first fetch or a
 *   Previous/Next fetch. Skeleton rows stand in for the rows (as many as the
 *   rows still held, so the table keeps its height) and for the empty
 *   message, the table is busy, and Previous and Next are disabled. Defaults
 *   to false.
 */
export type DataTableProps<TData> = {
  columns: ColumnDef<TData>[];
  data: TData[];
  prevKey?: string | null;
  nextKey?: string | null;
  onPrevious: () => void;
  onNext: () => void;
  emptyMessage?: string | null;
  caption?: string;
  loading?: boolean;
};

// Shown in the single row of an empty page when the API sent no message.
const DEFAULT_EMPTY_MESSAGE = "No records";

// Skeleton rows shown while the first page loads, when no rows are held yet.
const DEFAULT_SKELETON_ROWS = 5;

// How long the status region stays empty before it announces a result. The
// cleared region is committed first, so a screen reader hears a result whose
// text equals the previous one (two full pages of the same size) as a change.
const STATUS_ANNOUNCE_DELAY_MS = 150;

// A cursor key enables its control only when it is a non-empty string.
function hasKey(key: string | null | undefined): key is string {
  return typeof key === "string" && key.length > 0;
}

// The text of an empty page: the server's message as sent, untrimmed, unless
// it holds nothing but blanks (a spaces-filled COBOL message field), which
// would otherwise leave the only row of the table visibly empty.
function emptyText(message: string | null | undefined): string {
  return typeof message === "string" && message.trim().length > 0
    ? message
    : DEFAULT_EMPTY_MESSAGE;
}

// The status text of a completed page: its row count, or the empty-page text
// when it has no rows, after the caption that names the table when there is
// one. It never repeats cell content.
function resultText(
  rowCount: number,
  emptyMessage: string | null | undefined,
  caption: string | undefined,
): string {
  let outcome: string;
  if (rowCount === 0) {
    outcome = emptyText(emptyMessage);
  } else if (rowCount === 1) {
    outcome = "1 row";
  } else {
    outcome = `${rowCount} rows`;
  }
  return typeof caption === "string" && caption.trim().length > 0
    ? `${caption}: ${outcome}`
    : outcome;
}

type PageControlName = "Previous" | "Next";

type PageControlProps = {
  href: string;
  role: "button";
  "aria-label": PageControlName;
  "aria-disabled": "true" | undefined;
  tabIndex: number | undefined;
  className: string | undefined;
  onClick: (event: MouseEvent<HTMLAnchorElement>) => void;
  onKeyDown: (event: KeyboardEvent<HTMLAnchorElement>) => void;
  onKeyUp: (event: KeyboardEvent<HTMLAnchorElement>) => void;
};

// Props for the generated PaginationPrevious and PaginationNext anchors.
//
// The function map names these controls `button "Previous"` and
// `button "Next"`, exactly and case-sensitively (every PF7 and PF8 row), while
// the generated links carry the role link and the names "Go to previous page"
// and "Go to next page". Both are overridden here, since the generated
// component spreads its props after its own aria-label; the name equals the
// visible text.
//
// `href="#"` keeps the anchor in the Tab order with Enter activating it, and
// every activation cancels the navigation to "#". The button role promises
// Space activation too, which an anchor lacks: Space is cancelled on keydown
// (no page scroll) and activates on keyup, as a native button does, so a held
// Space pages once.
//
// A control is disabled when its key is missing and, whatever its key, while
// a page request is pending, so a pending fetch is never followed by another.
// A disabled control stays rendered, so every list offers both PF keys: it is
// aria-disabled, out of the Tab order, ignores the pointer, and its
// activations only cancel the navigation and call nothing.
function pageControl(
  name: PageControlName,
  key: string | null | undefined,
  onActivate: () => void,
  pending: boolean,
): PageControlProps {
  const enabled = hasKey(key) && !pending;
  const activate = (): void => {
    if (enabled) {
      onActivate();
    }
  };
  return {
    href: "#",
    role: "button",
    "aria-label": name,
    "aria-disabled": enabled ? undefined : "true",
    tabIndex: enabled ? undefined : -1,
    className: enabled ? undefined : "pointer-events-none opacity-50",
    onClick: (event) => {
      event.preventDefault();
      activate();
    },
    onKeyDown: (event) => {
      if (event.key === " ") {
        event.preventDefault();
      }
    },
    onKeyUp: (event) => {
      if (event.key === " ") {
        event.preventDefault();
        activate();
      }
    },
  };
}

/**
 * One server-paged page of rows in a card, with the Previous (PF7) and Next
 * (PF8) controls under it.
 *
 * The generated `Table` scrolls horizontally inside its own container, and the
 * card's `min-w-0` and inline-size containment make a wide table scroll inside
 * the card at `md` instead of widening the page. The card therefore takes its
 * width from its container (block flow, a stretched flex column or a grid
 * track) and has no content-based width of its own. The paging controls stay
 * shown while a page loads and are otherwise hidden only for an empty page
 * that has neither key.
 */
export function DataTable<TData>({
  columns,
  data,
  prevKey,
  nextKey,
  onPrevious,
  onNext,
  emptyMessage,
  caption,
  loading = false,
}: DataTableProps<TData>): JSX.Element {
  const table = useReactTable({
    data,
    columns,
    getCoreRowModel: getCoreRowModel(),
    manualPagination: true,
  });

  // The status region is empty while a page loads (the table's aria-busy
  // tells that) and announces each completed result. A new non-empty `data`
  // array, a change of the result text, or the end of loading clears it, then
  // fills it after a short delay, so a result that reads like the previous one
  // is still announced. Every empty array is one result (`resultRows` is null
  // for all of them), so an empty fallback created on every render does not
  // re-announce an unchanged empty page.
  const result = resultText(data.length, emptyMessage, caption);
  const resultRows: readonly TData[] | null = data.length === 0 ? null : data;
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    setAnnouncement("");
    if (loading) {
      return undefined;
    }
    const timer = setTimeout(() => {
      setAnnouncement(result);
    }, STATUS_ANNOUNCE_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [resultRows, loading, result]);

  const isEmpty = data.length === 0;
  const showPaging = loading || !isEmpty || hasKey(prevKey) || hasKey(nextKey);
  // The leaf count equals columns.length for flat definitions and stays
  // correct when the caller groups columns under a shared header.
  const leafColumns = table.getVisibleLeafColumns();
  const columnCount = leafColumns.length;

  let body: ReactNode;
  if (loading) {
    // Hidden from assistive technology: the rows hold no content, and the
    // table's busy state tells that the page is loading. Each skeleton is
    // h-5, the height of the table's text-sm line box, so a skeleton row is
    // exactly as tall as a single-line data row and the held rows keep the
    // table's height.
    const skeletonRows = isEmpty ? DEFAULT_SKELETON_ROWS : data.length;
    body = Array.from({ length: skeletonRows }, (_, index) => (
      <TableRow key={`skeleton-${index}`} aria-hidden="true">
        {leafColumns.map((column) => (
          <TableCell key={column.id}>
            <Skeleton className="h-5 w-full" />
          </TableCell>
        ))}
      </TableRow>
    ));
  } else if (isEmpty) {
    body = (
      <TableRow>
        <TableCell
          colSpan={columnCount}
          className="h-24 text-center whitespace-normal text-muted-foreground"
        >
          {emptyText(emptyMessage)}
        </TableCell>
      </TableRow>
    );
  } else {
    body = table.getRowModel().rows.map((row) => (
      <TableRow key={row.id}>
        {row.getVisibleCells().map((cell) => (
          <TableCell key={cell.id}>
            {flexRender(cell.column.columnDef.cell, cell.getContext())}
          </TableCell>
        ))}
      </TableRow>
    ));
  }

  // min-w-0 lets the card shrink as a flex or grid item. contain-inline-size
  // also keeps the table's width out of the card's intrinsic width, so no
  // ancestor (such as a flex item without min-w-0) is widened past the
  // viewport by a wide table: the card takes its container's width and the
  // table scrolls inside it.
  return (
    <Card className="min-w-0 gap-0 py-0 contain-inline-size">
      <Table aria-busy={loading ? true : undefined}>
        {/* BLITZY [LAYOUT]: caption aligned to the start (overrode the
            centred default), so a table wider than its card shows the
            caption without scrolling. */}
        {caption ? (
          <TableCaption className="mb-4 px-2 text-start">
            {caption}
          </TableCaption>
        ) : null}
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <TableHead
                  key={header.id}
                  colSpan={header.colSpan}
                  scope={header.colSpan > 1 ? "colgroup" : "col"}
                >
                  {header.isPlaceholder
                    ? null
                    : flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>{body}</TableBody>
      </Table>
      {/* Always rendered, and outside the busy table, so assistive technology
          tracks the region from the start and announces its changes
          politely. */}
      <div role="status" className="sr-only">
        {announcement}
      </div>
      {showPaging ? (
        // The generated CardFooter gives a bordered footer pt-6; pb-6 matches
        // it, so the controls sit centred between the table and the card edge.
        <CardFooter className="border-t pb-6">
          <Pagination>
            <PaginationContent>
              <PaginationItem>
                <PaginationPrevious
                  {...pageControl("Previous", prevKey, onPrevious, loading)}
                />
              </PaginationItem>
              <PaginationItem>
                <PaginationNext
                  {...pageControl("Next", nextKey, onNext, loading)}
                />
              </PaginationItem>
            </PaginationContent>
          </Pagination>
        </CardFooter>
      ) : null}
    </Card>
  );
}
