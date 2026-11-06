// Tests of DataTable (components/data-table.tsx), the shared server-paged
// table: loading state (held rows kept busy, skeleton rows and the column
// placeholders that size them, busy table, paging suppressed), the full page
// the body reserves (filler rows), the completed page (row order, empty-page
// text), the Previous/Next cursor controls that replace PF7 and PF8, the
// opt-in pinned end column, the status region that announces each completed
// page, rows keyed by their record, where focus goes when a page change
// disables the control that holds it or a focused row goes, and when a page
// change scrolls the focus target into view.
import type { ColumnDef } from "@tanstack/react-table";
import { act, render, screen } from "@testing-library/react";
import userEvent, {
  PointerEventsCheckLevel,
} from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Button } from "@/components/ui/button";

import {
  DataTable,
  type DataTableProps,
  type DataTableRowHeight,
} from "./data-table";

interface Row {
  id: string;
  name: string;
}

const columns: ColumnDef<Row>[] = [
  { accessorKey: "id", header: "ID" },
  { accessorKey: "name", header: "Name" },
];

const ROWS: Row[] = [
  { id: "0000000000000003", name: "THIRD" },
  { id: "0000000000000001", name: "FIRST" },
  { id: "0000000000000002", name: "SECOND" },
];

const PREV_KEY = "0000000000000001";
const NEXT_KEY = "0000000000000002";

type TableProps = Omit<
  DataTableProps<Row>,
  "columns" | "onPrevious" | "onNext"
>;

function renderTable(props: Partial<TableProps> = {}) {
  const onPrevious = vi.fn();
  const onNext = vi.fn();
  const element = (next: Partial<TableProps>) => (
    <DataTable
      columns={columns}
      data={[]}
      onPrevious={onPrevious}
      onNext={onNext}
      {...next}
    />
  );
  const view = render(element(props));
  return {
    onPrevious,
    onNext,
    rerender: (next: Partial<TableProps>) => view.rerender(element(next)),
    unmount: view.unmount,
  };
}

function bodyRows(): HTMLTableRowElement[] {
  const body = screen.getByRole("table").querySelector("tbody");
  if (body === null) {
    throw new Error("the table renders no tbody");
  }
  return Array.from(body.rows);
}

type ControlName = "Previous" | "Next";
type Activation = "click" | "Enter" | "Space";

function control(name: ControlName): HTMLElement {
  return screen.getByRole("button", { name });
}

// A disabled control carries pointer-events-none, which jsdom does not apply
// (no stylesheet is loaded); the pointer check is off so a click always
// reaches the handler under test, whatever styles a later setup loads.
async function activate(element: HTMLElement, how: Activation): Promise<void> {
  const user = userEvent.setup({
    pointerEventsCheck: PointerEventsCheckLevel.Never,
  });
  if (how === "click") {
    await user.click(element);
    return;
  }
  element.focus();
  await user.keyboard(how === "Enter" ? "{Enter}" : " ");
}

const CONTROL_ACTIVATIONS: { name: ControlName; how: Activation }[] = [
  { name: "Previous", how: "click" },
  { name: "Previous", how: "Enter" },
  { name: "Previous", how: "Space" },
  { name: "Next", how: "click" },
  { name: "Next", how: "Enter" },
  { name: "Next", how: "Space" },
];

interface EmptyCase {
  name: string;
  emptyMessage: string | null | undefined;
  text: string;
}

function expectDisabled(element: HTMLElement): void {
  expect(element.getAttribute("aria-disabled")).toBe("true");
  expect(element.getAttribute("tabindex")).toBe("-1");
  expect(element.className).toContain("pointer-events-none");
}

describe("DataTable while loading", () => {
  it.each<{ name: string; emptyMessage: string | null | undefined }>([
    { name: "without a message", emptyMessage: undefined },
    { name: "with a null message", emptyMessage: null },
    {
      name: "with a server message",
      emptyMessage: "NO RECORDS FOUND FOR THIS SEARCH CONDITION.",
    },
  ])(
    "renders hidden skeleton rows, not the empty text, $name",
    ({ emptyMessage }) => {
      renderTable({ loading: true, emptyMessage });

      expect(screen.getByRole("table").getAttribute("aria-busy")).toBe("true");
      const rows = bodyRows();
      expect(rows.length).toBeGreaterThan(0);
      for (const row of rows) {
        expect(row.getAttribute("aria-hidden")).toBe("true");
        expect(row.cells.length).toBe(columns.length);
        expect(row.querySelectorAll('[data-slot="skeleton"]').length).toBe(
          columns.length,
        );
      }
      expect(screen.queryByText("No records")).toBeNull();
      if (emptyMessage) {
        expect(screen.queryByText(emptyMessage)).toBeNull();
      }
    },
  );

  // A column's meta.placeholder sits invisibly in the skeleton bar's grid
  // cell, giving the cell its column's width; a column without one keeps the
  // bar alone.
  it.each<{
    name: string;
    column: number;
    text: string | null;
    element: string | null;
  }>([
    {
      name: "a text placeholder",
      column: 0,
      text: "0000000000000000",
      element: null,
    },
    {
      name: "an element placeholder",
      column: 1,
      text: "SECOND",
      element: "SPAN",
    },
    { name: "no placeholder", column: 2, text: null, element: null },
  ])(
    "renders $name in each skeleton cell of its column",
    ({ column, text, element }) => {
      const placeholderColumns: ColumnDef<Row>[] = [
        {
          accessorKey: "id",
          header: "ID",
          meta: { placeholder: "0000000000000000" },
        },
        {
          accessorKey: "name",
          header: "Name",
          meta: { placeholder: <span className="font-medium">SECOND</span> },
        },
        { id: "actions", header: "Actions" },
      ];
      render(
        <DataTable
          columns={placeholderColumns}
          data={[]}
          onPrevious={vi.fn()}
          onNext={vi.fn()}
          loading
          pageSize={3}
        />,
      );

      const rows = bodyRows();
      expect(rows.length).toBe(3);
      for (const row of rows) {
        expect(row.getAttribute("aria-hidden")).toBe("true");
        const content = row.cells[column]?.firstElementChild ?? null;
        const bar = content?.querySelector('[data-slot="skeleton"]') ?? null;
        expect(bar).not.toBeNull();
        expect(content?.lastElementChild).toBe(bar);
        const hidden = content?.querySelector(".invisible") ?? null;
        if (text === null) {
          expect(hidden).toBeNull();
          expect(content?.children.length).toBe(1);
          expect(row.cells[column]?.textContent).toBe("");
        } else {
          expect(hidden?.textContent).toBe(text);
          expect(hidden?.firstElementChild?.tagName ?? null).toBe(element);
          // Placeholder and bar share the one grid cell.
          for (const shared of [hidden, bar]) {
            expect(shared?.className).toContain("col-start-1");
            expect(shared?.className).toContain("row-start-1");
          }
        }
      }
      // Only the header row is exposed: no placeholder text reaches
      // assistive technology.
      expect(screen.queryAllByRole("row").length).toBe(1);
      expect(screen.queryByRole("cell")).toBeNull();
    },
  );

  it("keeps the rows held in place, busy, until the page loaded", () => {
    const { rerender } = renderTable({ loading: true, data: ROWS });

    const held = bodyRows();
    expect(held.map((row) => row.textContent)).toEqual([
      "0000000000000003THIRD",
      "0000000000000001FIRST",
      "0000000000000002SECOND",
    ]);
    for (const row of held) {
      expect(row.hasAttribute("aria-hidden")).toBe(false);
      expect(row.className).toContain("pointer-events-none");
      expect(row.className).toContain("opacity-50");
    }
    expect(document.querySelector('[data-slot="skeleton"]')).toBeNull();

    rerender({ loading: false, data: ROWS });
    for (const row of bodyRows()) {
      expect(row.className).not.toContain("pointer-events-none");
      expect(row.className).not.toContain("opacity-50");
    }
  });

  it.each(CONTROL_ACTIVATIONS)(
    "disables $name with its key held and ignores $how",
    async ({ name, how }) => {
      const { onPrevious, onNext } = renderTable({
        loading: true,
        data: ROWS,
        prevKey: PREV_KEY,
        nextKey: NEXT_KEY,
      });

      const element = control(name);
      expectDisabled(element);
      await activate(element, how);
      expect(onPrevious).not.toHaveBeenCalled();
      expect(onNext).not.toHaveBeenCalled();
    },
  );

  // An empty page keeps both PF keys on screen, loading or completed.
  it.each<{ name: string; props: Partial<TableProps> }>([
    { name: "while an empty page loads", props: { loading: true } },
    { name: "for a completed empty page without keys", props: {} },
    {
      name: "for a completed empty page with null keys",
      props: { prevKey: null, nextKey: null },
    },
    {
      name: "for a completed empty page with empty keys",
      props: { prevKey: "", nextKey: "" },
    },
  ])("renders Previous and Next, both disabled, $name", ({ props }) => {
    renderTable(props);

    expect(screen.getByRole("navigation", { name: "pagination" })).not.toBeNull();
    expectDisabled(control("Previous"));
    expectDisabled(control("Next"));
  });
});

// What a body row is: a data row (busy while the next page loads), a
// skeleton row, a filler row, or the row of the empty-page text.
type RowKind = "data" | "busy" | "skeleton" | "filler" | "text";

function rowKind(row: HTMLTableRowElement): RowKind {
  if (row.querySelector('[data-slot="skeleton"]') !== null) {
    return "skeleton";
  }
  if (row.getAttribute("aria-hidden") === "true") {
    return "filler";
  }
  if (row.textContent === "No records") {
    return "text";
  }
  return row.className.includes("opacity-50") ? "busy" : "data";
}

// The single cell of a filler or empty-text row, which spans every column.
function spanningCell(row: HTMLTableRowElement): HTMLTableCellElement {
  expect(row.cells.length).toBe(1);
  const cell = row.cells[0];
  if (cell === undefined) {
    throw new Error("the row has no cell");
  }
  expect(cell.colSpan).toBe(columns.length);
  return cell;
}

describe("DataTable reserving a full page", () => {
  const D = "data";
  const B = "busy";
  const S = "skeleton";
  const F = "filler";
  const T = "text";

  it.each<{ name: string; props: Partial<TableProps>; rows: RowKind[] }>([
    {
      name: "a load with no rows held, without a page size",
      props: { loading: true },
      rows: [S, S, S, S, S],
    },
    {
      name: "a load with no rows held",
      props: { loading: true, pageSize: 7 },
      rows: [S, S, S, S, S, S, S],
    },
    {
      name: "a load over an empty page",
      props: { loading: true, pageSize: 3, emptyMessage: "No rows." },
      rows: [S, S, S],
    },
    {
      name: "a short page",
      props: { data: ROWS, pageSize: 5 },
      rows: [D, D, D, F, F],
    },
    {
      name: "a full page",
      props: { data: ROWS, pageSize: 3 },
      rows: [D, D, D],
    },
    {
      name: "a page longer than the page size",
      props: { data: ROWS, pageSize: 2 },
      rows: [D, D, D],
    },
    {
      name: "a short page held while the next loads",
      props: { data: ROWS, pageSize: 5, loading: true },
      rows: [B, B, B, F, F],
    },
    {
      name: "rows held while the next page loads, without a page size",
      props: { data: ROWS, loading: true },
      rows: [B, B, B],
    },
    { name: "an empty page", props: { pageSize: 5 }, rows: [F, F, T, F, F] },
    {
      name: "an empty page of an even page size",
      props: { pageSize: 4 },
      rows: [F, T, F, F],
    },
    { name: "an empty page of one row", props: { pageSize: 1 }, rows: [T] },
    {
      name: "a short page with a page size of zero",
      props: { data: ROWS, pageSize: 0 },
      rows: [D, D, D],
    },
    {
      name: "a short page with a negative page size",
      props: { data: ROWS, pageSize: -5 },
      rows: [D, D, D],
    },
    {
      name: "a short page with a fractional page size",
      props: { data: ROWS, pageSize: 4.5 },
      rows: [D, D, D],
    },
  ])("renders $name", ({ props, rows }) => {
    renderTable(props);

    const empty = (props.data ?? []).length === 0;
    const body = bodyRows();
    expect(body.map(rowKind)).toEqual(rows);
    for (const row of body) {
      const kind = rowKind(row);
      if (kind === "skeleton") {
        expect(row.cells.length).toBe(columns.length);
        expect(row.querySelectorAll('[data-slot="skeleton"]').length).toBe(
          columns.length,
        );
      }
      if (kind === "filler" || kind === "text") {
        // No visible border and no hover background, but the 1px a data
        // row's border takes.
        expect(row.className).toContain("border-transparent");
        expect(row.className).toContain("hover:bg-transparent");
        expect(row.className).toContain("border-b");
      }
      if (kind === "filler" && empty) {
        // An empty page's filler rows render every column, each cell blank.
        expect(row.cells.length).toBe(columns.length);
        expect(row.textContent).toBe("");
      } else if (kind === "filler" || kind === "text") {
        expect(spanningCell(row).textContent).toBe(
          kind === "text" ? "No records" : "",
        );
      }
    }
    expect(screen.queryAllByRole("row").length).toBe(
      1 + rows.filter((kind) => kind !== F && kind !== S).length,
    );
  });

  it.each<{ rowHeight: DataTableRowHeight | undefined; height: string }>([
    { rowHeight: undefined, height: "min-h-5" },
    { rowHeight: "text", height: "min-h-5" },
    { rowHeight: "sm", height: "min-h-8" },
    { rowHeight: "default", height: "min-h-9" },
  ])(
    "sizes skeleton, filler and empty-text rows $height for rowHeight $rowHeight",
    ({ rowHeight, height }) => {
      const contentOf = (
        row: HTMLTableRowElement | undefined,
      ): Element | null => row?.cells[0]?.firstElementChild ?? null;
      const { rerender } = renderTable({
        loading: true,
        pageSize: 2,
        rowHeight,
      });
      for (const row of bodyRows()) {
        expect(contentOf(row)?.className).toContain(height);
      }

      rerender({ data: ROWS.slice(0, 1), pageSize: 2, rowHeight });
      expect(contentOf(bodyRows()[1])?.className).toBe(height);

      rerender({ pageSize: 1, rowHeight });
      expect(contentOf(bodyRows()[0])?.className).toContain(height);
    },
  );

  // An empty page's filler rows hold each column's placeholder invisibly in
  // a data row's content box, as the skeleton rows do, so the columns keep
  // the skeleton's widths when the empty page arrives; its text stays one cell
  // spanning every column. A short page's filler rows stay one empty cell
  // spanning every column, so its rows alone size its columns.
  it.each<{
    name: string;
    data: Row[];
    rows: RowKind[];
    span: number;
    placeholders: (string | null)[];
  }>([
    {
      name: "an empty page",
      data: [],
      rows: [F, F, T, F, F],
      span: 1,
      placeholders: ["0000000000000000", "SECOND", null],
    },
    {
      name: "a short page",
      data: ROWS.slice(0, 2),
      rows: [D, D, F, F, F],
      span: 3,
      placeholders: [null],
    },
  ])(
    "sizes the columns from the filler rows of $name",
    ({ data, rows, span, placeholders }) => {
      const placeholderColumns: ColumnDef<Row>[] = [
        {
          accessorKey: "id",
          header: "ID",
          meta: { placeholder: "0000000000000000" },
        },
        {
          accessorKey: "name",
          header: "Name",
          meta: { placeholder: <span className="font-medium">SECOND</span> },
        },
        { id: "actions", header: "Actions" },
      ];
      render(
        <DataTable
          columns={placeholderColumns}
          data={data}
          onPrevious={vi.fn()}
          onNext={vi.fn()}
          pageSize={5}
        />,
      );

      const body = bodyRows();
      // A full page of rows, no more.
      expect(body.map(rowKind)).toEqual(rows);
      for (const row of body.filter((each) => rowKind(each) === F)) {
        // Hidden, blank and borderless, with a data row's content height.
        expect(row.getAttribute("aria-hidden")).toBe("true");
        expect(row.className).toContain("border-transparent");
        expect(row.className).toContain("hover:bg-transparent");
        expect(row.querySelector('[data-slot="skeleton"]')).toBeNull();
        expect(row.cells.length).toBe(placeholders.length);
        const hidden = Array.from(row.cells, (cell) => {
          expect(cell.colSpan).toBe(span);
          expect(cell.firstElementChild?.className).toContain("min-h-5");
          return cell.querySelector(".invisible")?.textContent ?? null;
        });
        expect(hidden).toEqual(placeholders);
      }
      // The empty-page text, when shown, is one exposed cell spanning every
      // column.
      for (const row of body.filter((each) => rowKind(each) === T)) {
        expect(row.hasAttribute("aria-hidden")).toBe(false);
        expect(row.cells.length).toBe(1);
        expect(row.cells[0]?.colSpan).toBe(placeholderColumns.length);
      }
      // No placeholder reaches assistive technology.
      expect(screen.queryAllByRole("row").length).toBe(
        1 + rows.filter((kind) => kind !== F).length,
      );
      expect(
        screen
          .queryAllByRole("cell")
          .filter((cell) => cell.querySelector(".invisible") !== null),
      ).toEqual([]);
    },
  );
});

describe("DataTable with a completed page", () => {
  it.each<{ name: string; props: Partial<TableProps> }>([
    { name: "loading false", props: { loading: false } },
    { name: "loading omitted", props: {} },
  ])("leaves the table without aria-busy ($name)", ({ props }) => {
    renderTable({ data: ROWS, ...props });

    expect(screen.getByRole("table").hasAttribute("aria-busy")).toBe(false);
  });

  it("renders the rows in the order given, with no sorting", () => {
    renderTable({ data: ROWS });

    expect(bodyRows().map((row) => row.textContent)).toEqual([
      "0000000000000003THIRD",
      "0000000000000001FIRST",
      "0000000000000002SECOND",
    ]);
  });

  it.each<EmptyCase>([
    { name: "an absent message", emptyMessage: undefined, text: "No records" },
    { name: "a null message", emptyMessage: null, text: "No records" },
    { name: "an empty message", emptyMessage: "", text: "No records" },
    { name: "a blank message", emptyMessage: "    ", text: "No records" },
    {
      name: "a server message, verbatim and untrimmed",
      emptyMessage: " You are at the top of the page... ",
      text: " You are at the top of the page... ",
    },
  ])("shows $text for an empty page with $name", ({ emptyMessage, text }) => {
    renderTable({ emptyMessage });

    const rows = bodyRows();
    expect(rows.length).toBe(1);
    const cell = rows[0]?.cells[0];
    expect(cell?.textContent).toBe(text);
    expect(cell?.colSpan).toBe(columns.length);
  });

  it.each<{ name: string; key: string | null | undefined }>([
    { name: "null", key: null },
    { name: "absent", key: undefined },
    { name: "empty", key: "" },
  ])(
    "disables Previous for a $name prev_key and ignores a click",
    async ({ key }) => {
      const { onPrevious, onNext } = renderTable({
        data: ROWS,
        prevKey: key,
        nextKey: NEXT_KEY,
      });

      const element = control("Previous");
      expectDisabled(element);
      await activate(element, "click");
      expect(onPrevious).not.toHaveBeenCalled();
      expect(onNext).not.toHaveBeenCalled();
    },
  );

  it.each(CONTROL_ACTIVATIONS)(
    "calls only the $name callback, exactly once, on $how",
    async ({ name, how }) => {
      const { onPrevious, onNext } = renderTable({
        data: ROWS,
        prevKey: PREV_KEY,
        nextKey: NEXT_KEY,
      });

      const element = control(name);
      expect(element.hasAttribute("aria-disabled")).toBe(false);
      await activate(element, how);
      expect(onPrevious).toHaveBeenCalledTimes(name === "Previous" ? 1 : 0);
      expect(onNext).toHaveBeenCalledTimes(name === "Next" ? 1 : 0);
    },
  );
});

describe("DataTable with a pinned end column", () => {
  // The classes that hold a cell at the scroll container's inline end, opaque
  // and above the cells scrolling under it, with its start border.
  const PINNED_CLASSES = [
    "sticky",
    "end-0",
    "z-10",
    "bg-card",
    "before:border-s",
  ];

  // The row-action column is declared between the others, so pinning it must
  // also move it last.
  const pinnable: ColumnDef<Row>[] = [
    { accessorKey: "id", header: "ID" },
    {
      id: "actions",
      header: "Actions",
      cell: ({ row }) => `Edit ${row.original.id}`,
    },
    { accessorKey: "name", header: "Name" },
  ];

  function isPinned(cell: Element): boolean {
    return PINNED_CLASSES.every((name) => cell.classList.contains(name));
  }

  function headerCells(): HTMLElement[] {
    return screen.getAllByRole("columnheader");
  }

  // Whether the table's card clips its content to its rounded corners.
  function cardClips(): boolean {
    const card = screen.getByRole("table").closest('[data-slot="card"]');
    return card?.classList.contains("overflow-clip") === true;
  }

  const EDIT_CELLS = [
    "Edit 0000000000000003",
    "Edit 0000000000000001",
    "Edit 0000000000000002",
  ];

  it.each<{
    name: string;
    data: Row[];
    loading: boolean;
    busy: boolean;
    lastCells: string[];
  }>([
    {
      name: "data rows",
      data: ROWS,
      loading: false,
      busy: false,
      lastCells: EDIT_CELLS,
    },
    {
      name: "rows held busy while the next page loads",
      data: ROWS,
      loading: true,
      busy: true,
      lastCells: EDIT_CELLS,
    },
    {
      name: "skeleton rows",
      data: [],
      loading: true,
      busy: false,
      lastCells: ["", "", ""],
    },
  ])(
    "pins only the named column, rendered last, in the header and $name",
    ({ data, loading, busy, lastCells }) => {
      render(
        <DataTable
          columns={pinnable}
          data={data}
          onPrevious={vi.fn()}
          onNext={vi.fn()}
          loading={loading}
          pageSize={3}
          pinnedEndColumn="actions"
        />,
      );

      expect(headerCells().map((cell) => cell.textContent)).toEqual([
        "ID",
        "Name",
        "Actions",
      ]);
      expect(headerCells().map(isPinned)).toEqual([false, false, true]);
      const rows = bodyRows();
      expect(rows.map((row) => Array.from(row.cells, isPinned))).toEqual(
        rows.map(() => [false, false, true]),
      );
      expect(rows.map((row) => row.cells[2]?.textContent)).toEqual(lastCells);
      // Every row is the group whose hover the pinned cell's tint follows,
      // and a held row stays busy as well.
      const headerRow = headerCells()[0]?.closest("tr");
      expect(
        [headerRow, ...rows].map((row) => row?.classList.contains("group/row")),
      ).toEqual([true, true, true, true]);
      expect(rows.map((row) => row.classList.contains("opacity-50"))).toEqual(
        rows.map(() => busy),
      );
      // The opaque pinned header cell does not square off the card's corner.
      expect(cardClips()).toBe(true);
    },
  );

  // A short page's filler rows and the empty-page text are one cell spanning
  // every column, the pinned one included, so nothing in them sticks. An
  // empty page's filler rows render every column, the pinned one last and
  // pinned, with a transparent start border, in a row that is no hover
  // group, so the rows stay blank.
  it.each<{ name: string; data: Row[]; spanning: number; columned: number }>([
    { name: "a short page", data: ROWS, spanning: 2, columned: 0 },
    { name: "an empty page", data: [], spanning: 1, columned: 4 },
  ])(
    "pins only the pinned column's own cells in the reserved rows of $name",
    ({ data, spanning, columned }) => {
      render(
        <DataTable
          columns={pinnable}
          data={data}
          onPrevious={vi.fn()}
          onNext={vi.fn()}
          pageSize={5}
          pinnedEndColumn="actions"
        />,
      );

      const rows = bodyRows().filter((row) => row.cells.length === 1);
      expect(rows.length).toBe(spanning);
      for (const row of rows) {
        const cell = row.cells[0];
        expect(cell?.colSpan).toBe(pinnable.length);
        expect(cell?.classList.contains("sticky")).toBe(false);
      }
      const columnedRows = bodyRows().filter(
        (row) =>
          row.getAttribute("aria-hidden") === "true" &&
          row.cells.length === pinnable.length,
      );
      expect(columnedRows.length).toBe(columned);
      for (const row of columnedRows) {
        expect(row.classList.contains("group/row")).toBe(false);
        expect(Array.from(row.cells, isPinned)).toEqual([false, false, true]);
        const border = row.cells[2]?.classList;
        expect(border?.contains("before:border-transparent")).toBe(true);
        expect(border?.contains("before:border-border")).toBe(false);
      }
      expect(cardClips()).toBe(true);
    },
  );

  it.each<{ name: string; pinnedEndColumn: string | undefined }>([
    { name: "without pinnedEndColumn", pinnedEndColumn: undefined },
    { name: "for an id no column has", pinnedEndColumn: "actions" },
  ])("renders every column as before $name", ({ pinnedEndColumn }) => {
    renderTable({ data: ROWS, pinnedEndColumn });

    expect(headerCells().map((cell) => cell.textContent)).toEqual([
      "ID",
      "Name",
    ]);
    const cells = [
      ...headerCells(),
      ...bodyRows().flatMap((row) => Array.from(row.cells)),
    ];
    expect(cells.filter((cell) => cell.classList.contains("sticky"))).toEqual(
      [],
    );
    expect(
      Array.from(screen.getByRole("table").querySelectorAll("tr")).filter(
        (row) => row.classList.contains("group/row"),
      ),
    ).toEqual([]);
    expect(cardClips()).toBe(false);
  });
});

describe("DataTable status region", () => {
  // Fake timers drive the delay between clearing the region and filling it.
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function status(): HTMLElement {
    return screen.getByRole("status");
  }

  function flushAnnouncement(): void {
    act(() => {
      vi.runOnlyPendingTimers();
    });
  }

  it("is a visually hidden region outside the busy table", () => {
    renderTable({ loading: true });

    const region = status();
    expect(region.className).toContain("sr-only");
    expect(screen.getByRole("table").contains(region)).toBe(false);
  });

  it.each<{ name: string; props: Partial<TableProps>; text: string }>([
    {
      name: "a one-row page",
      props: { data: ROWS.slice(0, 1) },
      text: "1 row",
    },
    { name: "a three-row page", props: { data: ROWS }, text: "3 rows" },
    { name: "an empty page", props: { data: [] }, text: "No records" },
    {
      name: "an empty page with a blank message",
      props: { data: [], emptyMessage: "   " },
      text: "No records",
    },
    {
      name: "an empty page with a server message",
      props: { data: [], emptyMessage: " You are at the top of the page... " },
      text: " You are at the top of the page... ",
    },
    {
      name: "a captioned page",
      props: { data: ROWS, caption: "Transactions" },
      text: "Transactions: 3 rows",
    },
    {
      name: "a captioned empty page",
      props: { data: [], caption: "Cards" },
      text: "Cards: No records",
    },
    {
      name: "a page with a blank caption",
      props: { data: ROWS, caption: "  " },
      text: "3 rows",
    },
  ])("announces $text for $name after the delay", ({ props, text }) => {
    renderTable(props);

    expect(status().textContent).toBe("");
    flushAnnouncement();
    expect(status().textContent).toBe(text);
  });

  // One result rendered again and again. `props` builds the props of each
  // render: rows are held as one array across renders, while an empty page's
  // fallback is a new array on every render, as `data={page?.items ?? []}`
  // passes.
  const REPEATED_RESULTS: {
    name: string;
    props: () => Partial<TableProps>;
    text: string;
  }[] = [
    {
      name: "the same rows array",
      props: () => ({ data: ROWS }),
      text: "3 rows",
    },
    {
      name: "a new empty array",
      props: () => ({ data: [] }),
      text: "No records",
    },
    {
      name: "a new empty array with a caption and a server message",
      props: () => ({
        data: [],
        caption: "Cards",
        emptyMessage: "NO RECORDS FOUND FOR THIS SEARCH CONDITION.",
      }),
      text: "Cards: NO RECORDS FOUND FOR THIS SEARCH CONDITION.",
    },
  ];

  it.each(REPEATED_RESULTS)(
    "is empty while a page loads and announces when loading ends, with $name",
    ({ props, text }) => {
      const { rerender } = renderTable(props());
      flushAnnouncement();
      expect(status().textContent).toBe(text);

      rerender({ ...props(), loading: true });
      expect(status().textContent).toBe("");
      flushAnnouncement();
      expect(status().textContent).toBe("");

      rerender({ ...props(), loading: false });
      expect(status().textContent).toBe("");
      flushAnnouncement();
      expect(status().textContent).toBe(text);
    },
  );

  it.each(REPEATED_RESULTS)(
    "keeps the announcement when re-rendered with $name",
    ({ props, text }) => {
      const { rerender } = renderTable(props());
      flushAnnouncement();
      expect(status().textContent).toBe(text);

      rerender(props());
      expect(status().textContent).toBe(text);
      expect(vi.getTimerCount()).toBe(0);
    },
  );

  it.each<{
    name: string;
    first: Partial<TableProps>;
    next: Partial<TableProps>;
    before: string;
    after: string;
  }>([
    {
      name: "a same-size page that arrives as a new array",
      first: { data: ROWS },
      next: {
        data: [
          { id: "0000000000000004", name: "FOURTH" },
          { id: "0000000000000005", name: "FIFTH" },
          { id: "0000000000000006", name: "SIXTH" },
        ],
      },
      before: "3 rows",
      after: "3 rows",
    },
    {
      name: "an empty page whose message changes",
      first: { data: [] },
      next: { data: [], emptyMessage: "You are at the top of the page..." },
      before: "No records",
      after: "You are at the top of the page...",
    },
  ])("re-announces $name", ({ first, next, before, after }) => {
    const { rerender } = renderTable(first);
    flushAnnouncement();
    expect(status().textContent).toBe(before);

    rerender(next);
    expect(status().textContent).toBe("");
    flushAnnouncement();
    expect(status().textContent).toBe(after);
  });

  it("cancels a pending announcement on unmount", () => {
    const { unmount } = renderTable({ data: ROWS });
    expect(vi.getTimerCount()).toBe(1);

    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("DataTable focus after a page change", () => {
  const NEW_PAGE: Row[] = [
    { id: "0000000000000004", name: "FOURTH" },
    { id: "0000000000000005", name: "FIFTH" },
  ];

  afterEach(() => {
    vi.restoreAllMocks();
  });

  type Target = ControlName | "table";

  function focusTarget(target: Target): HTMLElement {
    return target === "table" ? screen.getByRole("table") : control(target);
  }

  it.each<{
    name: string;
    activated: ControlName;
    how: Activation;
    // Whether the caller shows a loading state before the new page, or
    // swaps the rows in directly (rows it holds, a cached page).
    loads: boolean;
    page: Partial<TableProps>;
    target: Target;
  }>([
    {
      name: "Next reaching the last page, by Enter",
      activated: "Next",
      how: "Enter",
      loads: true,
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: null },
      target: "Previous",
    },
    {
      name: "Previous reaching the first page, by Space",
      activated: "Previous",
      how: "Space",
      loads: true,
      page: { data: NEW_PAGE, prevKey: null, nextKey: NEXT_KEY },
      target: "Next",
    },
    {
      name: "Next reaching the last page, by click",
      activated: "Next",
      how: "click",
      loads: true,
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: null },
      target: "Previous",
    },
    {
      name: "Next reaching an empty page without keys",
      activated: "Next",
      how: "Enter",
      loads: true,
      page: { data: [], prevKey: null, nextKey: null },
      target: "table",
    },
    {
      name: "Next reaching the last page without a loading state",
      activated: "Next",
      how: "Enter",
      loads: false,
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: undefined },
      target: "Previous",
    },
  ])(
    "moves focus off the disabled control after $name",
    async ({ activated, how, loads, page, target }) => {
      const props = { data: ROWS, prevKey: PREV_KEY, nextKey: NEXT_KEY };
      const { rerender } = renderTable(props);
      await activate(control(activated), how);
      expect(document.activeElement).toBe(control(activated));

      const focus = vi.spyOn(HTMLElement.prototype, "focus");
      if (loads) {
        rerender({ ...props, loading: true });
        expect(document.activeElement).toBe(control(activated));
      }
      rerender(page);

      expectDisabled(control(activated));
      expect(document.activeElement).toBe(focusTarget(target));
      // preventScroll: the target is scrolled only when hidden (below).
      expect(focus).toHaveBeenCalledTimes(1);
      expect(focus).toHaveBeenLastCalledWith({ preventScroll: true });
    },
  );

  it.each<{ activated: ControlName; loads: boolean }>([
    { activated: "Next", loads: true },
    { activated: "Previous", loads: false },
  ])(
    "keeps focus on $activated when the new page leaves it enabled (loads: $loads)",
    async ({ activated, loads }) => {
      const props = { data: ROWS, prevKey: PREV_KEY, nextKey: NEXT_KEY };
      const { rerender } = renderTable(props);
      await activate(control(activated), "Enter");

      if (loads) {
        rerender({ ...props, loading: true });
      }
      rerender({ ...props, data: NEW_PAGE });

      const element = control(activated);
      expect(element.hasAttribute("aria-disabled")).toBe(false);
      expect(document.activeElement).toBe(element);
      // The scroll margin keeps the whole focus ring in view when the
      // control is scrolled into view.
      expect(element.className).toContain("scroll-my-2");
    },
  );

  // Where the focus target lies once the new page renders, in a 1280x800
  // viewport (jsdom performs no layout, so the geometry is stubbed). A
  // paging control is scrolled into view only when part of it is
  // off-screen, so one within its scroll margin of the edge stays under the
  // pointer; the table, only when none of it shows.
  it.each<{
    name: string;
    activated: ControlName;
    page: Partial<TableProps>;
    target: Target;
    top: number;
    height: number;
    scrolls: boolean;
  }>([
    {
      name: "Next keeps focus, wholly in view",
      activated: "Next",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: NEXT_KEY },
      target: "Next",
      top: 600,
      height: 36,
      scrolls: false,
    },
    {
      name: "Next keeps focus, within its scroll margin of the bottom edge",
      activated: "Next",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: NEXT_KEY },
      target: "Next",
      top: 760,
      height: 36,
      scrolls: false,
    },
    {
      name: "Next keeps focus, below the viewport",
      activated: "Next",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: NEXT_KEY },
      target: "Next",
      top: 874,
      height: 36,
      scrolls: true,
    },
    {
      name: "Previous keeps focus, partly above the viewport",
      activated: "Previous",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: NEXT_KEY },
      target: "Previous",
      top: -20,
      height: 36,
      scrolls: true,
    },
    {
      name: "focus moves to Previous, wholly in view",
      activated: "Next",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: null },
      target: "Previous",
      top: 760,
      height: 36,
      scrolls: false,
    },
    {
      name: "focus moves to Previous, partly below the viewport",
      activated: "Next",
      page: { data: NEW_PAGE, prevKey: PREV_KEY, nextKey: null },
      target: "Previous",
      top: 780,
      height: 36,
      scrolls: true,
    },
    {
      name: "focus moves to the table, taller than the viewport, partly in view",
      activated: "Next",
      page: { data: [], prevKey: null, nextKey: null },
      target: "table",
      top: -400,
      height: 1600,
      scrolls: false,
    },
    {
      name: "focus moves to the table, wholly above the viewport",
      activated: "Next",
      page: { data: [], prevKey: null, nextKey: null },
      target: "table",
      top: -900,
      height: 600,
      scrolls: true,
    },
  ])(
    "scrolls the focus target into view only when hidden: $name",
    async ({ activated, page, target, top, height, scrolls }) => {
      const viewport = document.documentElement;
      vi.spyOn(viewport, "clientWidth", "get").mockReturnValue(1280);
      vi.spyOn(viewport, "clientHeight", "get").mockReturnValue(800);
      const props = { data: ROWS, prevKey: PREV_KEY, nextKey: NEXT_KEY };
      const { rerender } = renderTable(props);
      await activate(control(activated), "Enter");
      rerender({ ...props, loading: true });
      const element = focusTarget(target);
      vi.spyOn(element, "getBoundingClientRect").mockReturnValue(
        DOMRect.fromRect({ x: 600, y: top, width: 80, height }),
      );
      const scroll = vi.spyOn(Element.prototype, "scrollIntoView");

      rerender(page);

      expect(document.activeElement).toBe(element);
      if (scrolls) {
        expect(scroll).toHaveBeenCalledTimes(1);
        expect(scroll.mock.contexts[0]).toBe(element);
        expect(scroll).toHaveBeenLastCalledWith({ block: "nearest" });
      } else {
        expect(scroll).not.toHaveBeenCalled();
      }
    },
  );

  // A page shown from the cache, then read again: the read's answer takes
  // the key of the control that kept focus, with no loading state between.
  it.each<{ later: Partial<TableProps>; target: Target }>([
    { later: { prevKey: PREV_KEY, nextKey: null }, target: "Previous" },
    { later: { prevKey: null, nextKey: null }, target: "table" },
  ])(
    "moves focus off Next when a later answer disables it, to the $target",
    async ({ later, target }) => {
      const props = { data: ROWS, prevKey: PREV_KEY, nextKey: NEXT_KEY };
      const { rerender } = renderTable(props);
      await activate(control("Next"), "Enter");
      rerender({ ...props, data: NEW_PAGE });
      expect(document.activeElement).toBe(control("Next"));

      rerender({ data: NEW_PAGE, ...later });

      expectDisabled(control("Next"));
      expect(document.activeElement).toBe(focusTarget(target));
    },
  );

  it("leaves focus the user moved elsewhere during the load", async () => {
    const props = { data: ROWS, prevKey: PREV_KEY, nextKey: NEXT_KEY };
    const outside = document.createElement("button");
    document.body.append(outside);
    try {
      const { rerender } = renderTable(props);
      await activate(control("Next"), "Enter");
      rerender({ ...props, loading: true });
      outside.focus();

      rerender({ data: NEW_PAGE, prevKey: PREV_KEY, nextKey: null });

      expect(document.activeElement).toBe(outside);
    } finally {
      outside.remove();
    }
  });

  it("is a focus target outside the Tab order with an inset focus outline", () => {
    renderTable({ data: ROWS, caption: "Transactions" });

    const element = screen.getByRole("table", { name: "Transactions" });
    expect(element.getAttribute("tabindex")).toBe("-1");
    expect(element.className).toContain("focus-visible:-outline-offset-2");
  });
});

describe("DataTable rows with controls", () => {
  // A row action per record, named after it, as a list's Delete is.
  const actionColumns: ColumnDef<Row>[] = [
    { accessorKey: "id", header: "ID" },
    {
      id: "actions",
      header: "Actions",
      cell: ({ row }) => (
        <Button type="button" variant="link">
          {`Delete ${row.original.name}`}
        </Button>
      ),
    },
  ];

  function rowId(row: Row): string {
    return row.id;
  }

  function renderRows(data: Row[]) {
    const element = (rows: Row[]) => (
      <DataTable
        columns={actionColumns}
        data={rows}
        getRowId={rowId}
        onPrevious={vi.fn()}
        onNext={vi.fn()}
      />
    );
    const view = render(element(data));
    return { rerender: (rows: Row[]) => view.rerender(element(rows)) };
  }

  function rowAction(name: string): HTMLElement {
    return screen.getByRole("button", { name: `Delete ${name}` });
  }

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("keys rows by getRowId, so a row's elements stay with their record", () => {
    const { rerender } = renderRows(ROWS);
    const first = rowAction("FIRST");
    const second = rowAction("SECOND");

    // THIRD, the first row, is deleted: FIRST and SECOND move up a row.
    rerender(ROWS.slice(1));

    expect(rowAction("FIRST")).toBe(first);
    expect(rowAction("SECOND")).toBe(second);
    expect(bodyRows().map((row) => row.textContent)).toEqual([
      "0000000000000001Delete FIRST",
      "0000000000000002Delete SECOND",
    ]);
  });

  it.each<{
    name: string;
    // How focus leaves the row control before its row goes, if it does.
    moved: "not" | "to another element" | "to nothing";
    next: Row[];
    target: "table" | "outside" | "body";
  }>([
    {
      name: "its record is deleted",
      moved: "not",
      next: [ROWS[0], ROWS[2]].filter((row) => row !== undefined),
      target: "table",
    },
    {
      name: "another page replaces the rows",
      moved: "not",
      next: [{ id: "0000000000000004", name: "FOURTH" }],
      target: "table",
    },
    {
      name: "its record is deleted after focus moved to another element",
      moved: "to another element",
      next: [ROWS[0], ROWS[2]].filter((row) => row !== undefined),
      target: "outside",
    },
    {
      name: "its record is deleted after focus moved to nothing",
      moved: "to nothing",
      next: [ROWS[0], ROWS[2]].filter((row) => row !== undefined),
      target: "body",
    },
  ])(
    "settles focus on the $target when the focused row goes because $name",
    async ({ moved, next, target }) => {
      const outside = document.createElement("button");
      document.body.append(outside);
      try {
        const { rerender } = renderRows(ROWS);
        const focused = rowAction("FIRST");
        focused.focus();
        if (moved === "to another element") {
          outside.focus();
        } else if (moved === "to nothing") {
          focused.blur();
        }
        // The blur onto nothing is judged once the current task's
        // microtasks have run.
        await Promise.resolve();
        const focus = vi.spyOn(HTMLElement.prototype, "focus");

        rerender(next);

        expect(focused.isConnected).toBe(false);
        const expected = {
          table: screen.getByRole("table"),
          outside,
          body: document.body,
        }[target];
        expect(document.activeElement).toBe(expected);
        if (target === "table") {
          // In place: the user goes on from where the row was.
          expect(focus).toHaveBeenCalledTimes(1);
          expect(focus).toHaveBeenLastCalledWith({ preventScroll: true });
        } else {
          expect(focus).not.toHaveBeenCalled();
        }
      } finally {
        outside.remove();
      }
    },
  );
});
