import { describe, expect, it, onTestFinished, vi } from "vitest";
import type { ReactNode } from "react";
import { render, screen, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { keyRowItems } from "@/lib/pfkeys";
import { FieldGrid, GridCell } from "./FieldGrid";
import { ScreenShell, type ScreenShellProps } from "./ScreenShell";

/*
 * Render test of the shared screen frame in jsdom: the row rule of `FieldGrid`, the title line
 * `ScreenShell` draws with the fields a map places beside its title, and the page scrolling the
 * frame does when a screen is shown and when its `ERRMSG` row receives a text.
 *
 * jsdom lays nothing out and has no `document.scrollingElement`, so the tests stand one in whose
 * `scrollTop` records what the frame sets, and give the footer the box a browser would measure.
 */

/** A fixed instant for `CURDATE` and `CURTIME`, so no render depends on the clock. */
const NOW = new Date(2024, 0, 15, 10, 30, 0);

/** The class list of an element, empty for none. */
function classes(element: Element | null | undefined): string[] {
  return element == null ? [] : Array.from(element.classList);
}

/** The frame of a map with the COMEN01 key row, with any prop given here replacing the default. */
function shell(props: Partial<ScreenShellProps> = {}): ReactNode {
  return (
    <ScreenShell
      transaction="CM00"
      program="COMEN01C"
      title="Main Menu"
      keys={keyRowItems("COMEN01")}
      now={NOW}
      {...props}
    >
      {props.children ?? <p>Body</p>}
    </ScreenShell>
  );
}

/**
 * Stands in a page scroll root at `scrollTop`, for the rest of the test, as the browser's
 * `document.scrollingElement`, which jsdom does not define.
 */
function stubScrollRoot(scrollTop: number): { scrollTop: number } {
  const root = { scrollTop };
  Object.defineProperty(document, "scrollingElement", { configurable: true, value: root });
  onTestFinished(() => {
    Reflect.deleteProperty(document, "scrollingElement");
  });
  return root;
}

/** The frame's footer: the message rows and the key row. */
function footer(): HTMLElement {
  const element = document.querySelector<HTMLElement>("[data-slot=card-footer]");
  if (element === null) {
    throw new Error("The frame rendered no footer");
  }
  return element;
}

/**
 * One message the frame receives after its first render with none, with the footer's bottom edge
 * `overflow` pixels below the viewport (negative: above its bottom), and the page's `scrollTop`
 * expected from 40.
 */
type ScrollCase = {
  name: string;
  next: Partial<ScreenShellProps>;
  overflow: number;
  expected: number;
};

/** Makes the footer measure as a browser would lay it out, with its bottom edge at `bottom`. */
function placeFooterBottom(bottom: number): void {
  vi.spyOn(footer(), "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 0, bottom));
}

describe("FieldGrid", () => {
  it("declares the map's 24 lines as row tracks, on which the fields centre", () => {
    const { container } = render(
      <FieldGrid>
        <GridCell row={6} col={5} span={15}>
          Search Tran ID:
        </GridCell>
      </FieldGrid>,
    );
    const grid = container.firstElementChild;

    expect(classes(grid)).toEqual(
      expect.arrayContaining([
        "grid",
        "grid-cols-80",
        "grid-rows-24",
        "items-center",
        "gap-x-2",
        "gap-y-1",
      ]),
    );
    expect(classes(grid)).not.toContain("grid-rows-none");
  });

  it("sizes the 24 line tracks to content, so a line without a field takes no height", () => {
    // Read from disk: Vitest serves every stylesheet import, `?raw` included, as an empty string.
    const indexCss = readFileSync(resolve(import.meta.dirname, "../../index.css"), "utf8");

    expect(indexCss).toMatch(/--grid-template-rows-24:\s*repeat\(24,\s*auto\)/);
  });

  it("keeps the utility a screen passes in place of its own", () => {
    const { container } = render(
      <FieldGrid className="grid-rows-16 gap-y-2">
        <GridCell row={1} col={1} span={1}>
          x
        </GridCell>
      </FieldGrid>,
    );
    const grid = classes(container.firstElementChild);

    expect(grid).toEqual(expect.arrayContaining(["grid-rows-16", "gap-y-2", "items-center"]));
    expect(grid).not.toContain("grid-rows-24");
    expect(grid).not.toContain("gap-y-1");
  });

  it.each([
    { row: 1, col: 1, span: 5, expected: ["row-start-1", "col-start-1", "col-span-5"] },
    { row: 4, col: 70, span: 5, expected: ["row-start-4", "col-start-70", "col-span-5"] },
    { row: 24, col: 1, span: 80, expected: ["row-start-24", "col-start-1", "col-span-80"] },
  ])(
    "places a field at POS ($row,$col) on grid line $row across its LENGTH $span",
    ({ row, col, span, expected }) => {
      render(
        <FieldGrid>
          <GridCell row={row} col={col} span={span}>
            field
          </GridCell>
        </FieldGrid>,
      );

      expect(classes(screen.getByText("field"))).toEqual(expected);
    },
  );
});

describe("ScreenShell title line", () => {
  it("puts the title and the fields beside it on one line, each field at its map column", () => {
    render(
      shell({
        title: "List Credit Cards",
        titleRow: (
          <>
            <GridCell row={1} col={70} span={5}>
              <span>Page</span>
            </GridCell>
            <GridCell row={1} col={76} span={3}>
              <span>1</span>
            </GridCell>
          </>
        ),
      }),
    );
    const heading = screen.getByRole("heading", { level: 1, name: "List Credit Cards" });
    const line = heading.parentElement;
    if (line === null) {
      throw new Error("The title has no line");
    }
    const label = within(line).getByText("Page").parentElement;
    const value = within(line).getByText("1").parentElement;

    expect(line.parentElement?.getAttribute("data-slot")).toBe("card-header");
    expect(classes(line)).toEqual(["grid", "grid-cols-80", "items-center", "gap-x-2"]);
    expect(line.firstElementChild).toBe(heading);
    expect(classes(heading)).toEqual(
      expect.arrayContaining(["col-span-full", "row-start-1", "text-center", "text-base"]),
    );
    expect(label?.parentElement).toBe(line);
    expect(value?.parentElement).toBe(line);
    expect(classes(label)).toEqual(["row-start-1", "col-start-70", "col-span-5"]);
    expect(classes(value)).toEqual(["row-start-1", "col-start-76", "col-span-3"]);
  });

  it("shows the title alone in the header when the map has no other field on its line", () => {
    render(shell());
    const heading = screen.getByRole("heading", { level: 1, name: "Main Menu" });

    expect(heading.parentElement?.getAttribute("data-slot")).toBe("card-header");
    // `text-base` carries its own line height, so the class merge drops the title's `leading-none`.
    expect(classes(heading)).toEqual(["font-semibold", "text-center", "text-base"]);
  });
});

describe("ScreenShell scrolling", () => {
  it("returns the page to its top when a screen is shown, after its first field took focus", () => {
    const root = stubScrollRoot(360);
    let scrolledByFocus = false;
    // A browser scrolls a field it focuses into view; this stands in for that scroll.
    const onFocusIn = (): void => {
      scrolledByFocus = true;
      root.scrollTop = 500;
    };
    document.addEventListener("focusin", onFocusIn);
    onTestFinished(() => {
      document.removeEventListener("focusin", onFocusIn);
    });

    render(shell({ children: <input aria-label="Option" autoFocus /> }));

    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Option" }));
    expect(scrolledByFocus).toBe(true);
    expect(root.scrollTop).toBe(0);
  });

  it.each<ScrollCase>([
    {
      name: "an error whose footer ends below the fold",
      next: { message: { kind: "error", text: "User ID NOT found..." } },
      overflow: 120,
      expected: 160,
    },
    {
      name: "an information text on the ERRMSG row whose footer ends below the fold",
      next: { message: { kind: "info", text: "  User USER0001 has been updated ..." } },
      overflow: 35.5,
      expected: 75.5,
    },
    {
      name: "an error whose footer is in view",
      next: { message: { kind: "error", text: "Please enter User ID ..." } },
      overflow: -10,
      expected: 40,
    },
    {
      name: "an error whose footer ends exactly at the fold",
      next: { message: { kind: "error", text: "Please enter User ID ..." } },
      overflow: 0,
      expected: 40,
    },
    {
      name: "a blank ERRMSG text",
      next: { message: { kind: "error", text: "   " } },
      overflow: 120,
      expected: 40,
    },
    {
      name: "an INFOMSG text alone",
      next: { info: "Enter or update id of account to display" },
      overflow: 120,
      expected: 40,
    },
  ])("after $name, moves the page from 40 to $expected", ({ next, overflow, expected }) => {
    const { rerender } = render(shell());
    const root = stubScrollRoot(40);
    placeFooterBottom(window.innerHeight + overflow);

    rerender(shell(next));

    expect(root.scrollTop).toBe(expected);
  });

  it("scrolls once per message text, not again when a render rebuilds the same message", () => {
    const { rerender } = render(shell());
    const root = stubScrollRoot(0);
    placeFooterBottom(window.innerHeight + 80);

    rerender(shell({ message: { kind: "error", text: "User ID NOT found..." } }));
    expect(root.scrollTop).toBe(80);

    // The user scrolls back up; each later render builds a new message object with the same text,
    // trailing spaces aside.
    root.scrollTop = 0;
    rerender(shell({ message: { kind: "error", text: "User ID NOT found..." } }));
    rerender(shell({ message: { kind: "error", text: "User ID NOT found...   " } }));
    expect(root.scrollTop).toBe(0);
  });

  it("scrolls again when the same message text is sent again", () => {
    const { rerender } = render(shell());
    const root = stubScrollRoot(0);
    placeFooterBottom(window.innerHeight + 80);

    rerender(shell({ message: { kind: "error", text: "Invalid key pressed...", send: 1 } }));
    expect(root.scrollTop).toBe(80);

    // The user scrolls back up and presses the same key again: the same text, a new send.
    root.scrollTop = 0;
    rerender(shell({ message: { kind: "error", text: "Invalid key pressed...", send: 2 } }));
    expect(root.scrollTop).toBe(80);
  });

  it("moves the root element when the document names no scrolling element", () => {
    const page = document.documentElement;
    let scrollTop = 0;
    Object.defineProperty(page, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    onTestFinished(() => {
      Reflect.deleteProperty(page, "scrollTop");
    });
    const { rerender } = render(shell());
    scrollTop = 40;
    placeFooterBottom(window.innerHeight + 25);

    rerender(shell({ message: { kind: "error", text: "User ID NOT found..." } }));

    expect(document.scrollingElement ?? null).toBeNull();
    expect(scrollTop).toBe(65);
  });

  it("brings the footer into view when a screen is first shown with a message", () => {
    const root = stubScrollRoot(200);
    const measure = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockReturnValue(new DOMRect(0, 0, 0, window.innerHeight + 64));
    onTestFinished(() => {
      measure.mockRestore();
    });

    render(shell({ message: { kind: "info", text: "Thank you for using CardDemo" } }));

    expect(root.scrollTop).toBe(64);
  });
});
