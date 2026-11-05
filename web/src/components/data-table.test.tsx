// Tests of DataTable (components/data-table.tsx), the shared server-paged
// table: loading state (skeleton rows, busy table, paging suppressed), the
// completed page (row order, empty-page text), the Previous/Next cursor
// controls that replace PF7 and PF8, the opt-in pinned end column, and the
// status region that announces each completed page.
import type { ColumnDef } from "@tanstack/react-table";
import { act, render, screen } from "@testing-library/react";
import userEvent, {
  PointerEventsCheckLevel,
} from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DataTable, type DataTableProps } from "./data-table";

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

  it("renders one skeleton row per retained row, hiding the old rows", () => {
    renderTable({ loading: true, data: ROWS });

    const rows = bodyRows();
    expect(rows.length).toBe(ROWS.length);
    for (const row of rows) {
      expect(row.querySelectorAll('[data-slot="skeleton"]').length).toBe(
        columns.length,
      );
    }
    expect(screen.queryByText("FIRST")).toBeNull();
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

  it.each<{ name: string; loading: boolean; shown: boolean }>([
    { name: "keeps the controls while loading", loading: true, shown: true },
    {
      name: "hides the controls for a completed empty page without keys",
      loading: false,
      shown: false,
    },
  ])("$name", ({ loading, shown }) => {
    renderTable({ loading });

    expect(screen.queryByRole("button", { name: "Previous" }) !== null).toBe(
      shown,
    );
    expect(screen.queryByRole("button", { name: "Next" }) !== null).toBe(
      shown,
    );
  });
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

  it.each<{ name: string; loading: boolean; lastCells: string[] }>([
    {
      name: "data rows",
      loading: false,
      lastCells: [
        "Edit 0000000000000003",
        "Edit 0000000000000001",
        "Edit 0000000000000002",
      ],
    },
    { name: "skeleton rows", loading: true, lastCells: ["", "", ""] },
  ])(
    "pins only the named column, rendered last, in the header and $name",
    ({ loading, lastCells }) => {
      render(
        <DataTable
          columns={pinnable}
          data={ROWS}
          onPrevious={vi.fn()}
          onNext={vi.fn()}
          loading={loading}
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
      // Every row is the group whose hover the pinned cell's tint follows.
      const headerRow = headerCells()[0]?.closest("tr");
      expect(
        [headerRow, ...rows].map((row) => row?.classList.contains("group/row")),
      ).toEqual([true, true, true, true]);
      // The opaque pinned header cell does not square off the card's corner.
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
