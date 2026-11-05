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
// order the caller passes them: the order the API returns them, which is the
// COBOL key order, or a screen's reversal of one page. The table has no
// client-side sorting, filtering or pagination row models.
//
// While the caller's page request is pending (`loading`), the rows held stay
// shown, dimmed and ignoring the pointer, until the answer replaces them;
// skeleton rows stand in only for a page with no rows held, and paging is
// suppressed. A column's `meta.placeholder` gives each of its skeleton cells
// the width its values take, so the columns keep their widths when the rows
// arrive; an empty page's filler rows hold the placeholders too, so the
// columns keep them when no rows arrive. Given the rows of a full page
// (`pageSize`), the body keeps the height of a full page in every state, so
// no short page, empty-page text or load moves the controls under it. A
// visually hidden status region announces each completed page by its row
// count or empty-page text, because a page change replaces the rows while
// focus stays on the paging controls.
// Focus moves only off a control that would hold it inert: one the new page
// disables, or the control of a row the table no longer shows. A page change
// scrolls the page only to bring a focus target the user cannot see into
// view, so a visible table and its controls never move under the pointer.
//
// Built on the pinned TanStack Table 8 API (useReactTable, getCoreRowModel,
// flexRender, ColumnDef). The current shadcn Data Table guide targets
// TanStack Table 9 and does not apply.
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
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
  type ColumnPinningPosition,
  type RowData,
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
import { cn } from "@/lib/utils";

// The column metadata DataTable reads, merged into TanStack Table's
// ColumnMeta (its declaration: interface ColumnMeta<TData extends RowData,
// TValue>), so a column definition's `meta` is typed.
declare module "@tanstack/react-table" {
  interface ColumnMeta<TData extends RowData, TValue> {
    /**
     * Static content as wide as the column's cells once the rows arrive: the
     * cell's own markup (its classes, badge or link-button styling) around a
     * value in the column's display format, at the fixed width of a
     * fixed-format value or the representative width of a variable-length
     * one. Each skeleton cell holds it invisibly under its bar, and each
     * filler cell of an empty page holds it invisibly, so the table's
     * automatic layout gives the column, while it loads and when the page is
     * empty, the width the rows give it. It holds no ids, links or controls:
     * it is never shown, and the skeleton and filler rows are hidden from
     * assistive technology. Absent, the skeleton cell is the bar alone, and
     * the column takes its width while it loads, and on an empty page, from
     * its header.
     */
    placeholder?: ReactNode;
  }
}

/**
 * Props of {@link DataTable}.
 *
 * - `columns`: TanStack column definitions (`accessorKey`/`header`/`cell`). A
 *   row-action column supplies its own `cell` rendering `Button variant="link"`,
 *   the replacement of the 3270 `S`/`U` selectors. `meta.placeholder` sizes
 *   the column's skeleton cells and an empty page's filler cells (see
 *   ColumnMeta above).
 * - `data`: the page's `items`, in the order the API returned them or, where
 *   a screen shows a page in reverse, a reversed copy. Keep one non-empty
 *   array per result across renders (the query's `items` as returned, or a
 *   copy memoized on them): TanStack Table re-derives its rows for every new
 *   array, and the status region announces every new non-empty array as a
 *   new result. Every empty array is the same result, so an inline fallback
 *   such as `page?.items ?? []` is announced again only when the empty-page
 *   text changes or `loading` ends.
 * - `prevKey`, `nextKey`: the page's `prev_key` and `next_key`. A non-empty
 *   string enables Previous (PF7) or Next (PF8); `null` or absent disables it.
 * - `onPrevious`, `onNext`: called when the enabled control is activated. The
 *   caller requests the adjacent page with the matching key and direction.
 * - `emptyMessage`: the COBOL message the API returned with an empty page,
 *   shown verbatim in place of the rows; "No records" when it is absent.
 * - `caption`: the table's accessible caption, rendered under the table, and
 *   the name the status region announces each result under.
 * - `loading`: true while the page's request is pending, the first fetch or a
 *   Previous/Next fetch. The rows held stay shown, dimmed and ignoring the
 *   pointer, so a new page never flickers through a placeholder; with no rows
 *   held (the first fetch, or an empty page shown), skeleton rows stand in
 *   for the page and for the empty message. The table is busy, and Previous
 *   and Next are disabled. Defaults to false.
 * - `pageSize`: the rows of a full page, the screen's rows the page is read
 *   with. Skeleton rows number `pageSize`, and hidden filler rows pad a short
 *   page, and the empty-page text, to `pageSize` rows, so the table is as tall
 *   as a full page in every state and paging moves neither the controls under
 *   it nor the scroll position. Absent, nothing is reserved: the table is as
 *   tall as what it shows, and DEFAULT_SKELETON_ROWS skeleton rows stand in
 *   for a page with no rows held.
 * - `rowHeight`: the tallest content of a data row, which sets the content
 *   height of every skeleton and filler row so each is exactly as tall as a
 *   data row: `text` (the default) for text and controls sized to the text
 *   line, `sm` for a `size="sm"` Button, `default` for a default-size Button
 *   or Input.
 * - `getRowId`: the record key of a row, such as the user ID. Rows and cells
 *   are keyed by it, so a row's elements, its focused control included, stay
 *   with their record when a delete or a page change moves other records
 *   into its place, and leave the document with it. Without it rows are keyed
 *   by position, and a row's elements pass to whichever record takes its
 *   place.
 * - `pinnedEndColumn`: the `id` of one column, such as the row actions, held
 *   at the inline end of the card while the other columns scroll under it, so
 *   its controls are never clipped and never hidden when focused, however
 *   wide the table. The column renders last, its skeleton cells included.
 *   Omitted, no column is pinned.
 */
export type DataTableProps<TData> = {
  columns: ColumnDef<TData>[];
  data: TData[];
  getRowId?: (row: TData, index: number) => string;
  prevKey?: string | null;
  nextKey?: string | null;
  onPrevious: () => void;
  onNext: () => void;
  emptyMessage?: string | null;
  caption?: string;
  loading?: boolean;
  pageSize?: number;
  rowHeight?: DataTableRowHeight;
  pinnedEndColumn?: string;
};

/** The tallest content of a data row: `rowHeight` of {@link DataTableProps}. */
export type DataTableRowHeight = "text" | "sm" | "default";

// The content height of a skeleton or filler row, the height of the content
// `rowHeight` names: the table's text-sm line box (h-5), a size="sm" Button
// (h-8), or a default-size Button or Input (h-9). With the cell's padding and
// the row's border it makes the row exactly as tall as a data row. A minimum,
// so the empty-page text may still wrap onto more lines.
const ROW_CONTENT_HEIGHT: Record<DataTableRowHeight, string> = {
  text: "min-h-5",
  sm: "min-h-8",
  default: "min-h-9",
};

// Shown in the one text row of an empty page when the API sent no message.
const DEFAULT_EMPTY_MESSAGE = "No records";

// Skeleton rows shown while a page with no rows held loads, when the caller
// gives no page size.
const DEFAULT_SKELETON_ROWS = 5;

// A held row while the next page loads: dimmed, and ignoring the pointer so a
// click meant for it cannot land on a row of the page about to replace it.
const BUSY_ROW_CLASS = "pointer-events-none opacity-50";

// A row that only reserves height (filler rows, and the empty-page text among
// them): its border is transparent, so it still takes the 1px a data row's
// border does, and it shows no hover background.
const RESERVED_ROW_CLASS = "border-transparent hover:bg-transparent";

// How long the status region stays empty before it announces a result. The
// cleared region is committed first, so a screen reader hears a result whose
// text equals the previous one (two full pages of the same size) as a change.
const STATUS_ANNOUNCE_DELAY_MS = 150;

// No column pinned on a side: one array, so the pinning state keeps its
// identity across renders.
const NO_COLUMNS: string[] = [];

// Each row of a table with a pinned column is a named group, which the pinned
// cell's tint follows.
const PINNED_ROW_CLASS = "group/row";

// The card of a table with a pinned column clips its content to its rounded
// corners.
const PINNED_CARD_CLASS = "overflow-clip";

// The pinned column's header and body cells stick to the inline end of the
// generated table's scroll container, stacked above the cells that scroll
// under them and opaque in the card's colour. The table collapses its borders,
// and a collapsed cell border is painted by the table beneath the cell's
// opaque background, so the start border (the cue that the table continues
// under the column) is drawn on a ::before layer that moves with the cell. That
// layer, under the cell's content within its stacking context, also repeats
// the row's hover and expanded tints (TableRow's bg-muted/50), so a pinned
// cell reads as part of its row.
const PINNED_CELL_CLASS =
  "sticky end-0 z-10 bg-card before:absolute before:inset-0 before:-z-10 before:border-s before:border-border before:transition-colors group-hover/row:before:bg-muted/50 group-has-aria-expanded/row:before:bg-muted/50";

// The class of a header or body cell whose column is pinned at `position`:
// the pinned column's cells stick to the inline end, and every other cell
// keeps the generated styling.
function pinnedCellClass(position: ColumnPinningPosition): string | undefined {
  return position === false ? undefined : PINNED_CELL_CLASS;
}

// The pinned cell of an empty page's filler row keeps its column's pinned
// class, with its start border transparent, as the row's own border is, so
// the reserved row stays blank. The row is no hover group, so the cell shows
// no hover tint either.
const RESERVED_PINNED_CELL_CLASS = "before:border-transparent";

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
  className: string;
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
// activations only cancel the navigation and call nothing. A control disabled
// while it holds focus passes focus on once no page is loading. A focused
// control is scrolled into view only when part of it is off-screen, and then
// keeps a scroll margin (scroll-my-2) beyond its focus ring, so the whole
// ring shows (see revealIfHidden).
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
    className: enabled
      ? "scroll-my-2"
      : "scroll-my-2 pointer-events-none opacity-50",
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

// Scrolls a focus target into view only when the user cannot see it, so a
// page change never moves a table or control the user sees from under the
// pointer, and a focus target off-screen still comes into view. A paging
// control (`whole`) counts as shown only when all of it lies inside the
// viewport: `nearest` would otherwise scroll a control lying within its
// scroll margin of the viewport's edge. The table (`part`), which may be
// taller than the viewport, counts as shown when any part of it does. The
// viewport is the document's client area, which excludes its scrollbars.
function revealIfHidden(element: HTMLElement, shown: "whole" | "part"): void {
  const rect = element.getBoundingClientRect();
  const { clientWidth, clientHeight } = document.documentElement;
  const visible =
    shown === "whole"
      ? rect.top >= 0 &&
        rect.left >= 0 &&
        rect.bottom <= clientHeight &&
        rect.right <= clientWidth
      : rect.bottom > 0 &&
        rect.right > 0 &&
        rect.top < clientHeight &&
        rect.left < clientWidth;
  if (!visible) {
    element.scrollIntoView({ block: "nearest" });
  }
}

// The page change an activated Previous or Next asked for, with the table as
// it stood at the activation: the result rows (null for every empty page, as
// `resultRows` is), both keys, and whether a load has run since.
type PageChange<TData> = {
  control: PageControlName;
  rows: readonly TData[] | null;
  prevKey: string | null | undefined;
  nextKey: string | null | undefined;
  loaded: boolean;
};

/**
 * One server-paged page of rows in a card, with the Previous (PF7) and Next
 * (PF8) controls under it.
 *
 * The generated `Table` scrolls horizontally inside its own container, and the
 * card's `min-w-0` and inline-size containment make a wide table scroll inside
 * the card at `md` instead of widening the page. The card therefore takes its
 * width from its container (block flow, a stretched flex column or a grid
 * track) and has no content-based width of its own. A `pinnedEndColumn` stays
 * in view at the card's inline end while the other columns scroll. The paging
 * controls are always shown, an empty page without keys included, disabled
 * while a page loads and when their key is missing.
 */
export function DataTable<TData>({
  columns,
  data,
  getRowId,
  prevKey,
  nextKey,
  onPrevious,
  onNext,
  emptyMessage,
  caption,
  loading = false,
  pageSize,
  rowHeight = "text",
  pinnedEndColumn,
}: DataTableProps<TData>): JSX.Element {
  // TanStack's column pinning (its `right` side is the inline end of this
  // left-to-right table) places the pinned column last in the header groups
  // and in every row's cells.
  const columnPinning = useMemo(
    () => ({
      left: NO_COLUMNS,
      right: pinnedEndColumn === undefined ? NO_COLUMNS : [pinnedEndColumn],
    }),
    [pinnedEndColumn],
  );
  const table = useReactTable({
    data,
    columns,
    getRowId,
    getCoreRowModel: getCoreRowModel(),
    manualPagination: true,
    state: { columnPinning },
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

  // The table itself is the neutral focus target (tabIndex -1, named by its
  // caption) for focus that has no control left to sit on.
  const tableRef = useRef<HTMLTableElement>(null);
  const previousRef = useRef<HTMLAnchorElement>(null);
  const nextRef = useRef<HTMLAnchorElement>(null);

  // Focus on a row's control that leaves the document with its row (the
  // record was deleted, or the rows were replaced) falls to the body, where a
  // keyboard user would restart from the top of the page. The element last
  // focused inside the table is kept, and when a render finds it gone with
  // focus on the body, the table takes focus in place. Focus the user moved
  // out of the table forgets it: onto another element, or onto nothing (a
  // click on the page background) while the element stays. Chrome reports
  // the blur of a removed focused element while it is still connected, so a
  // blur onto nothing is judged once the removal has completed.
  const focusedInTable = useRef<HTMLElement | null>(null);
  const onTableFocus = (event: FocusEvent<HTMLTableElement>): void => {
    const target = event.target;
    focusedInTable.current =
      target instanceof HTMLElement && target !== event.currentTarget
        ? target
        : null;
  };
  const onTableBlur = (event: FocusEvent<HTMLTableElement>): void => {
    const target = event.target;
    const next = event.relatedTarget;
    if (next instanceof Node && event.currentTarget.contains(next)) {
      return;
    }
    if (next !== null) {
      focusedInTable.current = null;
      return;
    }
    queueMicrotask(() => {
      if (target.isConnected && focusedInTable.current === target) {
        focusedInTable.current = null;
      }
    });
  };
  useLayoutEffect(() => {
    const focused = focusedInTable.current;
    if (focused === null || focused.isConnected) {
      return;
    }
    focusedInTable.current = null;
    const active = document.activeElement;
    if (active === null || active === document.body) {
      tableRef.current?.focus({ preventScroll: true });
    }
  });

  // Focus after a page change. The activated control keeps focus while the
  // page loads, disabled as every control then is. The new page is shown
  // once the load has ended or, for a caller that pages without loading
  // (rows it already holds, a page from its cache), once the rows or keys
  // differ from the activation's. If focus is still on the activated control
  // and the control is still enabled, it keeps focus, and is scrolled into
  // view only when a page of another height has moved part of it off-screen:
  // a control the user sees stays under the pointer that may have clicked it.
  //
  // Outside a load, a control that holds focus while disabled would hold it
  // inert (out of the Tab order, Enter and Space doing nothing): the new page
  // has no key in its direction, or a later answer took the key away (a page
  // shown from the cache, then read again). Focus then moves to the other
  // control when that one is enabled, else to the table, without the scroll
  // focus() would make, and its target is scrolled into view only when the
  // user cannot see it (revealIfHidden). Focus anywhere else is left where
  // it is.
  const pageChange = useRef<PageChange<TData> | null>(null);
  const requestPage =
    (control: PageControlName, request: () => void) => (): void => {
      pageChange.current = {
        control,
        rows: resultRows,
        prevKey,
        nextKey,
        loaded: false,
      };
      request();
    };
  useLayoutEffect(() => {
    const change = pageChange.current;
    if (loading) {
      if (change !== null) {
        change.loaded = true;
      }
      return;
    }
    if (
      change !== null &&
      (change.loaded ||
        change.rows !== resultRows ||
        change.prevKey !== prevKey ||
        change.nextKey !== nextKey)
    ) {
      pageChange.current = null;
      const activated =
        change.control === "Previous" ? previousRef.current : nextRef.current;
      const activatedKey = change.control === "Previous" ? prevKey : nextKey;
      if (
        activated !== null &&
        document.activeElement === activated &&
        hasKey(activatedKey)
      ) {
        revealIfHidden(activated, "whole");
      }
    }
    const active = document.activeElement;
    let other: HTMLAnchorElement | null;
    let otherKey: string | null | undefined;
    if (active !== null && active === previousRef.current) {
      if (hasKey(prevKey)) {
        return;
      }
      other = nextRef.current;
      otherKey = nextKey;
    } else if (active !== null && active === nextRef.current) {
      if (hasKey(nextKey)) {
        return;
      }
      other = previousRef.current;
      otherKey = prevKey;
    } else {
      return;
    }
    if (other !== null && hasKey(otherKey)) {
      other.focus({ preventScroll: true });
      revealIfHidden(other, "whole");
      return;
    }
    const tableElement = tableRef.current;
    if (tableElement !== null) {
      tableElement.focus({ preventScroll: true });
      revealIfHidden(tableElement, "part");
    }
  }, [loading, resultRows, prevKey, nextKey]);

  const isEmpty = data.length === 0;
  // The leaf columns in the order the header shows them, the pinned column
  // last. The leaf count equals columns.length for flat definitions and stays
  // correct when the caller groups columns under a shared header.
  const leafColumns = [
    ...table.getLeftVisibleLeafColumns(),
    ...table.getCenterVisibleLeafColumns(),
    ...table.getRightVisibleLeafColumns(),
  ];
  const columnCount = leafColumns.length;
  const pinned = leafColumns.some((column) => column.getIsPinned() !== false);
  const rowClass = pinned ? PINNED_ROW_CLASS : undefined;

  // The rows of a full page; undefined, reserving nothing, unless the caller
  // gives a positive whole number of them.
  const fullPage =
    pageSize !== undefined && Number.isInteger(pageSize) && pageSize > 0
      ? pageSize
      : undefined;
  const contentHeight = ROW_CONTENT_HEIGHT[rowHeight];
  // The cells of a skeleton row (`bar`) or of an empty page's filler row,
  // one per column, the pinned one last with its pinned class. Each content
  // box has a data row's content height and holds the column's placeholder
  // invisibly: it gives the cell the width of the column's values, so the
  // skeleton and the empty page give the columns one width. A skeleton
  // cell's bar, a skeleton of the table's text-sm line box (h-5), shares the
  // placeholder's grid cell, centred in the content height and stretching
  // across the cell.
  const placeholderCells = (bar: boolean): JSX.Element[] =>
    leafColumns.map((column) => {
      const placeholder = column.columnDef.meta?.placeholder;
      const position = column.getIsPinned();
      return (
        <TableCell
          key={column.id}
          className={cn(
            pinnedCellClass(position),
            !bar && position !== false && RESERVED_PINNED_CELL_CLASS,
          )}
        >
          <div className={cn("grid items-center", contentHeight)}>
            {placeholder === undefined ? null : (
              <div className="invisible col-start-1 row-start-1">
                {placeholder}
              </div>
            )}
            {bar ? <Skeleton className="col-start-1 row-start-1 h-5" /> : null}
          </div>
        </TableCell>
      );
    });
  // Rows that only hold the height of a data row, hidden from assistive
  // technology. A short page's filler rows are a single empty cell whose
  // content box is a data row's, so its rows alone size its columns. An empty
  // page's filler rows are its columns' placeholder cells, so the columns
  // keep the widths the skeleton gave them.
  const fillerRows = (count: number, position: string): JSX.Element[] =>
    Array.from({ length: count }, (_, index) => (
      <TableRow
        key={`filler-${position}-${index}`}
        aria-hidden="true"
        className={RESERVED_ROW_CLASS}
      >
        {isEmpty ? (
          placeholderCells(false)
        ) : (
          <TableCell colSpan={columnCount}>
            <div className={contentHeight} />
          </TableCell>
        )}
      </TableRow>
    ));

  // Skeleton and data rows render every column, the pinned one last with its
  // pinned class, in a row that is the group the pinned cell's tint follows.
  // An empty page's filler rows render every column too, in a row that is no
  // hover group. A short page's filler rows and the empty-page text are one
  // cell spanning every column, so they pin nothing.
  let body: ReactNode;
  if (isEmpty && loading) {
    // Hidden from assistive technology: the rows hold no data, and the
    // table's busy state tells that the page is loading. A full page of them,
    // each as tall as a data row.
    body = Array.from(
      { length: fullPage ?? DEFAULT_SKELETON_ROWS },
      (_, index) => (
        <TableRow
          key={`skeleton-${index}`}
          aria-hidden="true"
          className={rowClass}
        >
          {placeholderCells(true)}
        </TableRow>
      ),
    );
  } else if (isEmpty) {
    const text = emptyText(emptyMessage);
    if (fullPage === undefined) {
      body = (
        <TableRow>
          <TableCell
            colSpan={columnCount}
            className="h-24 text-center whitespace-normal text-muted-foreground"
          >
            {text}
          </TableCell>
        </TableRow>
      );
    } else {
      // The text takes one row of the reserved page, centred among the
      // filler rows, with no border above or below it. The filler rows hold
      // the columns' placeholders, so the columns keep the skeleton's widths.
      const fillers = fullPage - 1;
      const above = Math.floor(fillers / 2);
      body = [
        ...fillerRows(above, "above"),
        <TableRow key="empty" className={RESERVED_ROW_CLASS}>
          <TableCell
            colSpan={columnCount}
            className="text-center whitespace-normal text-muted-foreground"
          >
            <div
              className={cn("flex items-center justify-center", contentHeight)}
            >
              {text}
            </div>
          </TableCell>
        </TableRow>,
        ...fillerRows(fillers - above, "below"),
      ];
    }
  } else {
    // While a page loads, the rows held stay in place, busy, until the
    // answer replaces them; filler rows pad a short page to a full one.
    body = [
      ...table.getRowModel().rows.map((row) => (
        <TableRow
          key={row.id}
          className={cn(rowClass, loading && BUSY_ROW_CLASS)}
        >
          {row.getVisibleCells().map((cell) => (
            <TableCell
              key={cell.id}
              className={pinnedCellClass(cell.column.getIsPinned())}
            >
              {flexRender(cell.column.columnDef.cell, cell.getContext())}
            </TableCell>
          ))}
        </TableRow>
      )),
      ...fillerRows(Math.max(0, (fullPage ?? 0) - data.length), "below"),
    ];
  }

  // min-w-0 lets the card shrink as a flex or grid item. contain-inline-size
  // also keeps the table's width out of the card's intrinsic width, so no
  // ancestor (such as a flex item without min-w-0) is widened past the
  // viewport by a wide table: the card takes its container's width and the
  // table scrolls inside it. A table with a pinned column also clips to the
  // card's rounded corners, which its opaque pinned header cell would
  // otherwise square off; overflow-clip makes no scroll container, so the
  // pinned cells still stick within the table's own.
  return (
    <Card
      className={cn(
        "min-w-0 gap-0 py-0 contain-inline-size",
        pinned && PINNED_CARD_CLASS,
      )}
    >
      {/* The neutral focus target: focusable, outside the Tab order
          (tabIndex -1). The focus outline is drawn inset: the generated
          container scrolls horizontally and would clip an outset one. */}
      <Table
        ref={tableRef}
        tabIndex={-1}
        className="focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        onFocus={onTableFocus}
        onBlur={onTableBlur}
        aria-busy={loading ? true : undefined}
      >
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
            <TableRow key={headerGroup.id} className={rowClass}>
              {headerGroup.headers.map((header) => (
                <TableHead
                  key={header.id}
                  colSpan={header.colSpan}
                  scope={header.colSpan > 1 ? "colgroup" : "col"}
                  className={pinnedCellClass(header.column.getIsPinned())}
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
      {/* Always rendered, an empty page without keys included: both
          controls are then disabled, so every list offers both PF keys.
          The generated CardFooter gives a bordered footer pt-6; pb-6
          matches it, so the controls sit centred between the table and the
          card edge. */}
      <CardFooter className="border-t pb-6">
        <Pagination>
          <PaginationContent>
            <PaginationItem>
              <PaginationPrevious
                ref={previousRef}
                {...pageControl(
                  "Previous",
                  prevKey,
                  requestPage("Previous", onPrevious),
                  loading,
                )}
              />
            </PaginationItem>
            <PaginationItem>
              <PaginationNext
                ref={nextRef}
                {...pageControl(
                  "Next",
                  nextKey,
                  requestPage("Next", onNext),
                  loading,
                )}
              />
            </PaginationItem>
          </PaginationContent>
        </Pagination>
      </CardFooter>
    </Card>
  );
}
