import type { ReactNode } from "react";
import { cn } from "cn";

/**
 * Lines and columns of every map, `DFHMDI SIZE=(24,80)` (for example `app/bms/COSGN00.bms:28`).
 * The `grid-rows-24` and `grid-cols-80` literals of {@link FieldGrid} state the same size, and
 * {@link GridCell} holds a `POS` to it.
 */
const SCREEN_ROWS = 24;
const SCREEN_COLUMNS = 80;

/**
 * The classes a map input sized to its value adds to the `Input`'s own, with `size` = the field's
 * `LENGTH`; {@link fieldBox} is the box sized to the `LENGTH` instead. A grid field of one or two
 * characters takes them through {@link fieldInputClass}, as does code that places fields of any
 * length; a list input's width is set by its list: a selector is a one-character box (`w-9`), any
 * other list input fills its table column.
 *
 * BLITZY [LAYOUT]: on narrow frames a grid column is narrower than a character, so a cell of
 * `LENGTH` columns can be narrower than `LENGTH` characters plus the padding. At the 768px minimum
 * frame a cell of N columns is 9.075N - 8px wide, against 8.4N px of `text-sm` monospace text.
 * - The `Input` default `w-full` stays, so the box fills its cell: the map's field box and width
 *   (AAP 0.4.2).
 * - `min-w-max` (replacing the Input's `min-w-0`) with `field-sizing-content` keeps the box never
 *   narrower than its value plus the inset and border, so no value is clipped. The box is sized to
 *   the value rather than to `size`, because browsers widen a `size` box by a wide-glyph allowance.
 *   A box grown past its cell is therefore 8.4N + 10px at most, and for a field of two or more
 *   characters it still ends before the next field's cell, which starts at least N + 1 columns on.
 *   Where `field-sizing` is unsupported, `size` sets the minimum instead.
 * - `px-1` is the one inset every input taking these classes shares, in place of the Input's
 *   `px-3`.
 * - `text-sm` is the input type size of AAP 0.4.3. It is set here because below the `md`
 *   breakpoint the Input uses `text-base`, whose wider glyphs would widen the box further.
 */
export const FIELD_INPUT = "min-w-max px-1 text-sm field-sizing-content";

/** The longest `LENGTH` whose grid field takes the short-field box of {@link fieldInputClass}. */
const SHORT_FIELD_LENGTH = 2;

/**
 * The `Input` classes of a grid field `length` characters long: {@link FIELD_INPUT}, and for a
 * field of one or two characters a `w-6` box in place of `w-full`.
 *
 * BLITZY [LAYOUT]: a cell of one or two columns is at most about 22px wide, even on the widest
 * frame, and a box sized to an empty value is only its inset and border, about 11px. The short
 * field's box is therefore `w-6` (1.5rem) wide, whether empty or not, and always covers its cell.
 * `min-w-max` still widens it to a two-character value, 26.8px. On the 768px frame it ends before
 * a field that starts three columns on (27.2px).
 */
export function fieldInputClass(length: number): string {
  return length <= SHORT_FIELD_LENGTH ? `${FIELD_INPUT} w-6` : FIELD_INPUT;
}

/** Props of {@link FieldGrid}. */
export type FieldGridProps = {
  /** The map's fields, each placed by a {@link GridCell}. */
  children: ReactNode;
  /** Utilities merged after the grid's own, so a conflicting one given here wins. */
  className?: string;
};

/**
 * Props of {@link GridCell}: one `DFHMDF` field as its `<MAP>-nnn` row of
 * `modern/audit/screens.md` declares it.
 */
export type GridCellProps = {
  /** Line of the field's `POS=(row,col)`, 1 to 24. */
  row: number;
  /** Column of the field's `POS=(row,col)`, 1 to 80. */
  col: number;
  /** The field's `LENGTH`: how many columns the cell spans. */
  span: number;
  /**
   * How many map lines the cell spans from `row`; one when omitted. A list `Table` spans the
   * lines of its captions and rows, so its height is shared by those lines instead of making its
   * first line as tall as the whole table.
   */
  rowSpan?: number;
  /** The field's content, rendered exactly as given. */
  children: ReactNode;
};

/** `value` truncated to an integer and held within `min`..`max`, without branching. */
function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

/**
 * The box of a map field's `Input` sized to its map `LENGTH`, for a field of `length` characters
 * (held within 1 to 80); {@link FIELD_INPUT} is the box sized to the value instead.
 *
 * The box fills its {@link GridCell} and is never narrower than `length` characters of the
 * monospace font plus its padding and border (`field-length-*` of `src/index.css`). A grid column
 * can be narrower than a character, so at such a pitch the box extends into the field's trailing
 * attribute column: every character shows at any width, and the box stays within the field's
 * terminal footprint, `POS` to `POS + LENGTH`, before the next field starts. The text is
 * `text-sm` at every width (AAP 0.4.3), and the height and padding are shared, so the same
 * `LENGTH` gives the same box on every screen that takes it.
 *
 * Callers merge it with `cn`, which lets it replace the `Input`'s own height, minimum width,
 * padding and text size.
 */
export function fieldBox(length: number): string {
  const n = clamp(length, 1, SCREEN_COLUMNS);
  return `h-7 w-full min-w-(--field-width) px-0.5 py-0 text-sm field-length-${n}`;
}

/**
 * The 24 × 80 terminal grid a screen body places its map fields on.
 *
 * shadcn/ui has no layout primitive, so this is a plain container styled with Tailwind grid
 * utilities and default spacing steps. One row rule holds for every screen:
 *
 * - Each field sits on its map line: {@link GridCell} places it at grid line `r` of its
 *   `POS=(r,c)`, so fields of one map line share one track and lines keep their map order.
 * - The grid declares the map's 24 lines as its row tracks (`grid-rows-24`), each sized to its
 *   content by the `--grid-template-rows-24` token of `src/index.css`, so a line is as tall as its
 *   tallest field.
 * - A map line that holds no field takes no height; only the `gap-y-1` row gap remains. These are
 *   the lines `ScreenShell` draws around the body (the header rows, the screen title, the
 *   `INFOMSG` and `ERRMSG` rows and the row-24 keys) and the map's blank lines, so the frame lines
 *   are not reserved a second time as empty bands.
 * - `items-center` centres each field on its line, so a label stands level with the input
 *   beside it.
 *
 * The token, not the utility, sets the track size: Tailwind's own `grid-rows-24` is 24 equal
 * `minmax(0, 1fr)` tracks, which would make every line as tall as the tallest and turn the lines
 * without a field into blank bands.
 */
export function FieldGrid({ children, className }: FieldGridProps): ReactNode {
  return (
    <div className={cn("grid grid-cols-80 grid-rows-24 items-center gap-x-2 gap-y-1", className)}>
      {children}
    </div>
  );
}

/**
 * Places one map field on its {@link FieldGrid} line, from its `POS` column across `LENGTH`
 * columns.
 *
 * The cell starts at the `POS` column itself, not one column to the right where the terminal
 * shows the data after the field's attribute byte. Every field therefore moves left by the same
 * one column: relative placement is unchanged, the attribute-byte column between adjacent fields
 * stays empty, the 80-byte `ERRMSG` of COCRDSL and COCRDUP (X-SCREENS-7) fits on its line, and
 * no cell needs an 81st column.
 *
 * Positions outside the screen are held at its edge, and the spans are cut so the cell ends by
 * column 80 and line 24; a zero-length field still spans one column. A cell of one line carries no
 * row-span class. The computed class names are generated through the `@source inline` list of
 * `src/index.css`.
 *
 * Children are not formatted, trimmed or rewritten: screens pass card numbers and SSNs already
 * redacted, and the cell shows them as received.
 */
export function GridCell({ row, col, span, rowSpan = 1, children }: GridCellProps): ReactNode {
  const r = clamp(row, 1, SCREEN_ROWS);
  const c = clamp(col, 1, SCREEN_COLUMNS);
  const s = clamp(span, 1, SCREEN_COLUMNS + 1 - c);
  const rs = clamp(rowSpan, 1, SCREEN_ROWS + 1 - r);
  const lines = rs > 1 ? ` row-span-${rs}` : "";
  return (
    <div className={`row-start-${r}${lines} col-start-${c} col-span-${s}`}>{children}</div>
  );
}

/**
 * Props of {@link GridTable}: the block of map lines and columns a list `Table` covers, from its
 * first caption to its last row.
 */
export type GridTableProps = {
  /** Line of the list's first caption, 1 to 24. */
  row: number;
  /** Column the list's leftmost field starts at, 1 to 80. */
  col: number;
  /** How many columns the list covers, from `col` to the end of its rightmost field. */
  span: number;
  /** How many map lines the list covers from `row`, one table row per line; one when omitted. */
  rowSpan?: number;
  /** The list `Table`, its columns sized as fractions of `span`. */
  children: ReactNode;
};

/**
 * Places a list `Table` on its {@link FieldGrid} block, so that its columns and rows fall on the
 * map's columns and lines.
 *
 * The block is a {@link GridCell} whose content reaches over the column gap after its last
 * column (`-mr-2`, the grid's `gap-x-2`) and over the line gap after its last line (`-mb-1`, the
 * grid's `gap-y-1`), so it is exactly `span` column pitches wide. A `table-fixed` `Table` whose
 * `col` widths are fractions of `span` starts each column on its map column. The block's lines
 * hold no other field, so their content-sized tracks share the table's height: each of its rows of
 * {@link LIST_ROW} height is one map line.
 */
export function GridTable({ row, col, span, rowSpan = 1, children }: GridTableProps): ReactNode {
  return (
    <GridCell row={row} col={col} span={span} rowSpan={rowSpan}>
      <div className="-mr-2 -mb-1">{children}</div>
    </GridCell>
  );
}

/**
 * The height of one list row in a {@link GridTable}: one map line, as tall as a map field's box
 * ({@link fieldBox}, `h-7`). Every caption, dash and list row has it, whatever it holds, so a page
 * with fewer rows keeps the same lines.
 *
 * BLITZY [LAYOUT]: `h-7` instead of the 40px caption row of the `TableHead` default `h-10`
 * (which {@link LIST_CELL} lifts), and the row's input as compact as a field box, so the list lines
 * stand one after another as the terminal's do and a list screen, up to the twelve list lines of
 * COTRN00, fits a 1024 × 768 viewport with its message row and keys.
 */
export const LIST_ROW = "h-7";

/**
 * The box of every `TableHead` and `TableCell` in a {@link GridTable}: no padding and no height of
 * its own. No inline padding, so the content starts on its column's map column; no vertical
 * padding, and `h-auto` in place of the `TableHead` fixed `h-10`, so a caption row and a row that
 * holds a field's box are both exactly {@link LIST_ROW} tall, with their content centred on the
 * map line, as {@link FieldGrid} centres a field.
 */
export const LIST_CELL = "h-auto px-0 py-0";
