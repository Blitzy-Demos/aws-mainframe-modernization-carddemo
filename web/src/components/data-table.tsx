// The application's single data table: one page of a COBOL key-ordered browse
// (card list, transaction list, pending authorizations, reward history and the
// admin lists) with the Previous and Next controls that replace PF7 and PF8.
//
// Paging is the API's cursor paging: a request names `start_key`, `direction`
// and `page_size`, and the page answers `{ items, next_key, prev_key }`. The
// keys are opaque strings handed back to the caller's callbacks unread, so no
// page number is ever computed and no key is ever converted. Rows render in the
// order the API returns them, which is the COBOL key order, so the table has no
// client-side sorting, filtering or pagination row models.
//
// Built on the pinned TanStack Table 8 API (useReactTable, getCoreRowModel,
// flexRender, ColumnDef). The current shadcn Data Table guide targets
// TanStack Table 9 and does not apply.
import {
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
 * - `data`: the page's `items`, in the order the API returned them.
 * - `prevKey`, `nextKey`: the page's `prev_key` and `next_key`. A non-empty
 *   string enables Previous (PF7) or Next (PF8); `null` or absent disables it.
 * - `onPrevious`, `onNext`: called when the enabled control is activated. The
 *   caller requests the adjacent page with the matching key and direction.
 * - `emptyMessage`: the COBOL message the API returned with an empty page,
 *   shown verbatim in place of the rows; "No records" when it is absent.
 * - `caption`: the table's accessible caption, rendered under the table.
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
};

// Shown in the single row of an empty page when the API sent no message.
const DEFAULT_EMPTY_MESSAGE = "No records";

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
// A disabled control stays rendered, so every list offers both PF keys: it is
// aria-disabled, out of the Tab order, ignores the pointer, and its
// activations only cancel the navigation and call nothing.
function pageControl(
  name: PageControlName,
  key: string | null | undefined,
  onActivate: () => void,
): PageControlProps {
  const enabled = hasKey(key);
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
 * track) and has no content-based width of its own. The paging controls are
 * hidden only for an empty page that has neither key.
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
}: DataTableProps<TData>): JSX.Element {
  const table = useReactTable({
    data,
    columns,
    getCoreRowModel: getCoreRowModel(),
    manualPagination: true,
  });

  const isEmpty = data.length === 0;
  const showPaging = !isEmpty || hasKey(prevKey) || hasKey(nextKey);
  // The leaf count equals columns.length for flat definitions and stays
  // correct when the caller groups columns under a shared header.
  const columnCount = table.getVisibleLeafColumns().length;

  let body: ReactNode;
  if (isEmpty) {
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
      <Table>
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
      {showPaging ? (
        // The generated CardFooter gives a bordered footer pt-6; pb-6 matches
        // it, so the controls sit centred between the table and the card edge.
        <CardFooter className="border-t pb-6">
          <Pagination>
            <PaginationContent>
              <PaginationItem>
                <PaginationPrevious
                  {...pageControl("Previous", prevKey, onPrevious)}
                />
              </PaginationItem>
              <PaginationItem>
                <PaginationNext {...pageControl("Next", nextKey, onNext)} />
              </PaginationItem>
            </PaginationContent>
          </Pagination>
        </CardFooter>
      ) : null}
    </Card>
  );
}
