import type { ReactNode } from "react";
import { cn } from "cn";

/**
 * Lines and columns of every map, `DFHMDI SIZE=(24,80)` (for example `app/bms/COSGN00.bms:28`).
 * The `grid-rows-24` and `grid-cols-80` literals of {@link FieldGrid} state the same size.
 */
const SCREEN_ROWS = 24;
const SCREEN_COLUMNS = 80;

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
  /** The field's content, rendered exactly as given. */
  children: ReactNode;
};

/** `value` truncated to an integer and held within `min`..`max`, without branching. */
function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Math.trunc(value)));
}

/**
 * The 24 × 80 terminal grid a screen body places its map fields on.
 *
 * shadcn/ui has no layout primitive, so this is a plain container styled with Tailwind grid
 * utilities and default spacing steps. The 24 row tracks are flexible in a container of auto
 * height, so every line takes the height of the tallest one, and map lines that hold no field
 * keep their place as they do on the terminal.
 */
export function FieldGrid({ children, className }: FieldGridProps): ReactNode {
  return (
    <div className={cn("grid grid-cols-80 grid-rows-24 gap-x-2 gap-y-1", className)}>
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
 * Positions outside the screen are held at its edge, and the span is cut so the cell ends by
 * column 80; a zero-length field still spans one column. The computed class names are generated
 * through the `@source inline` list of `src/index.css`.
 *
 * Children are not formatted, trimmed or rewritten: screens pass card numbers and SSNs already
 * redacted, and the cell shows them as received.
 */
export function GridCell({ row, col, span, children }: GridCellProps): ReactNode {
  const r = clamp(row, 1, SCREEN_ROWS);
  const c = clamp(col, 1, SCREEN_COLUMNS);
  const s = clamp(span, 1, SCREEN_COLUMNS + 1 - c);
  return <div className={`row-start-${r} col-start-${c} col-span-${s}`}>{children}</div>;
}
