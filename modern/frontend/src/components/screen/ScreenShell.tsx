import { useId, type ReactNode } from "react";
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { TITLE01, TITLE02, formatHeaderDate, formatHeaderTime } from "@/lib/header";
import { GridCell } from "./FieldGrid";
import { MessageLine, type MessageKind } from "./MessageLine";
import { PfKeyBar, type PfKeyBarProps } from "./PfKeyBar";

/**
 * Which placement of header rows 1 and 2 a map declares.
 *
 * - `standard`: the 20 maps other than COSGN00, for example `app/bms/COMEN01.bms:29-74`: labels
 *   `Tran:`, `Prog:` at column 1 and `Date:`, `Time:` at column 65, five wide; `TRNNAME` and
 *   `PGMNAME` at column 7; `CURTIME` eight wide.
 * - `signon`: COSGN00 only, `app/bms/COSGN00.bms:29-74`: labels `Tran :`, `Prog :` at column 1 and
 *   `Date :`, `Time :` at column 64, six wide; `TRNNAME` and `PGMNAME` at column 8; `CURTIME`
 *   nine wide (`INITIAL='Ahh:mm:ss'`). Its row 3 (`AppID`/`SysID`) is not part of the shell.
 */
export type HeaderLayout = "standard" | "signon";

/** The text of a map's `ERRMSG` row and the colour the program gives it. */
export type ShellMessage = {
  /** The message as received; absent, empty or all spaces leaves the row empty. */
  text?: string;
  /**
   * `error` for an error (RED). `info` for a success text the program moves to `ERRMSG` after
   * recolouring the field, so it shows as information rather than as an error.
   */
  kind: MessageKind;
};

/** Props of {@link ScreenShell}. */
export type ScreenShellProps = {
  /** README transaction ID shown in `TRNNAME` (`CC00`), from the screen's `SCREENS` entry. */
  transaction: string;
  /** COBOL program name shown in `PGMNAME` (`COSGN00C`), from the screen's `SCREENS` entry. */
  program: string;
  /** The map's screen title (`Main Menu`, `View Account`), the screen's single level-1 heading. */
  title: string;
  /** `id` of the screen's `<form>`, which ENTER submits; forwarded to `PfKeyBar`. */
  formId?: string;
  /** The `ERRMSG` row (row 23). Without it the row is empty and stays an error row. */
  message?: ShellMessage;
  /**
   * The `INFOMSG` row, only on the seven maps that declare one (COACTUP, COACTVW, COCRDLI,
   * COCRDSL, COCRDUP, COTRTLI, COTRTUP). Any string, the empty string included, renders the row;
   * `undefined` renders none.
   */
  info?: string;
  /** The map's row-24 keys, forwarded to `PfKeyBar` unchanged. */
  keys: PfKeyBarProps["keys"];
  /** Settings of the keyboard binding, forwarded to `PfKeyBar` as `options`. */
  keyOptions?: PfKeyBarProps["options"];
  /**
   * The instant `CURDATE` and `CURTIME` show; the current time at each render when omitted, as
   * every program refreshes them on each send. Tests pass a fixed instant.
   */
  now?: Date;
  /** Placement of header rows 1 and 2; `standard` unless the map is COSGN00. */
  headerLayout?: HeaderLayout;
  /** The screen body: the screen's `<form>` with its `FieldGrid`. */
  children: ReactNode;
};

/** The six header fields the programs fill in their header paragraph, by their BMS names. */
type HeaderField = "TRNNAME" | "TITLE01" | "CURDATE" | "PGMNAME" | "TITLE02" | "CURTIME";

/** One header-row `DFHMDF` macro: a literal label, or a value field filled at render. */
type HeaderCell = {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly className: string;
} & (
  | { readonly label: string; readonly field?: never }
  | { readonly field: HeaderField; readonly label?: never }
);

/*
 * Colour tokens of AAP 0.4.3. The labels are BLUE, the titles YELLOW, which has no token and is
 * expressed as emphasis on the foreground. The value fields are BLUE on the map, snapped by role
 * to the foreground token of value fields.
 *
 * `whitespace-pre` keeps the padding that centres `TITLE01` and `TITLE02` in their 40 columns
 * (`app/cpy/COTTL01Y.cpy:18-22`). A grid column is wider than a character, so the 40 characters
 * fill only part of their cell; the title text is centred in the cell, which keeps it centred on
 * the screen as the padded field is on the terminal, instead of drifting left of the screen title.
 */
const LABEL = "whitespace-pre text-muted-foreground";
const VALUE = "whitespace-pre text-foreground";
const TITLE = "block whitespace-pre text-center font-semibold text-foreground";

/**
 * Header rows 1 and 2 of each layout, in macro order, as `POS` row, `POS` column and `LENGTH`
 * (`modern/audit/screens.md` rows `<MAP>-001` to `<MAP>-010`). Columns are the map's own, as
 * `GridCell` takes them.
 */
const HEADER_CELLS: Readonly<Record<HeaderLayout, readonly HeaderCell[]>> = {
  standard: [
    { row: 1, col: 1, span: 5, label: "Tran:", className: LABEL },
    { row: 1, col: 7, span: 4, field: "TRNNAME", className: VALUE },
    { row: 1, col: 21, span: 40, field: "TITLE01", className: TITLE },
    { row: 1, col: 65, span: 5, label: "Date:", className: LABEL },
    { row: 1, col: 71, span: 8, field: "CURDATE", className: VALUE },
    { row: 2, col: 1, span: 5, label: "Prog:", className: LABEL },
    { row: 2, col: 7, span: 8, field: "PGMNAME", className: VALUE },
    { row: 2, col: 21, span: 40, field: "TITLE02", className: TITLE },
    { row: 2, col: 65, span: 5, label: "Time:", className: LABEL },
    { row: 2, col: 71, span: 8, field: "CURTIME", className: VALUE },
  ],
  signon: [
    { row: 1, col: 1, span: 6, label: "Tran :", className: LABEL },
    { row: 1, col: 8, span: 4, field: "TRNNAME", className: VALUE },
    { row: 1, col: 21, span: 40, field: "TITLE01", className: TITLE },
    { row: 1, col: 64, span: 6, label: "Date :", className: LABEL },
    { row: 1, col: 71, span: 8, field: "CURDATE", className: VALUE },
    { row: 2, col: 1, span: 6, label: "Prog :", className: LABEL },
    { row: 2, col: 8, span: 8, field: "PGMNAME", className: VALUE },
    { row: 2, col: 21, span: 40, field: "TITLE02", className: TITLE },
    { row: 2, col: 64, span: 6, label: "Time :", className: LABEL },
    { row: 2, col: 71, span: 9, field: "CURTIME", className: VALUE },
  ],
};

/**
 * The frame of every screen, one BMS map laid out in a shadcn/ui `Card` (AAP 0.3.5, 0.4.2):
 *
 * - `CardHeader`: header rows 1 and 2 on the 80-column grid of `FieldGrid` (the transaction,
 *   the titles of `COTTL01Y`, the date, the program, the time), then the screen title;
 * - `CardContent`: the screen body;
 * - `CardFooter`: the `INFOMSG` row when the map has one, the `ERRMSG` row and the row-24 keys.
 *
 * The header grid has the body grid's columns and gaps inside the same padding, so the header
 * fields stand over the body columns of the same number. The header rows are plain text: they
 * belong to the frame's title rows, not to the map's input or output fields.
 *
 * `PfKeyBar` is the screen's only keyboard binding, so screens never call `usePfKeys`. Every text
 * passed in is shown verbatim and never logged: card numbers and SSNs arrive already redacted.
 *
 * The frame is the page's `main` landmark. The skip link, shown only on keyboard focus, moves past
 * the header rows and the title to the screen body.
 *
 * @throws RangeError when `now` is an invalid `Date` (from `formatHeaderDate`).
 */
export function ScreenShell({
  transaction,
  program,
  title,
  formId,
  message,
  info,
  keys,
  keyOptions,
  now,
  headerLayout = "standard",
  children,
}: ScreenShellProps): ReactNode {
  const bodyId = useId();
  const at = now ?? new Date();
  const values: Readonly<Record<HeaderField, string>> = {
    TRNNAME: transaction,
    TITLE01,
    CURDATE: formatHeaderDate(at),
    PGMNAME: program,
    TITLE02,
    CURTIME: formatHeaderTime(at),
  };

  return (
    // The 80 columns need a minimum width (`min-w-3xl`); below it the page scrolls sideways
    // rather than overlapping fields. Above `max-w-7xl` the frame stops widening and is centred.
    <main className="mx-auto w-full max-w-7xl overflow-x-auto p-4">
      <a href={`#${bodyId}`} className="sr-only underline focus-visible:not-sr-only">
        Skip to main content
      </a>
      {/*
       * `font-mono` and `text-sm` are inherited by every control, so the 80 columns align and
       * labels, values, inputs, table cells, messages and keys share the `text-sm` of AAP 0.4.3;
       * `gap-4` replaces the Card's default `gap-6`.
       */}
      <Card className="min-w-3xl gap-4 font-mono text-sm">
        <CardHeader>
          <div className="grid grid-cols-80 gap-x-2">
            {HEADER_CELLS[headerLayout].map((cell) => (
              <GridCell
                key={`${cell.row}-${cell.col}`}
                row={cell.row}
                col={cell.col}
                span={cell.span}
              >
                <span className={cell.className} data-field={cell.field}>
                  {cell.field === undefined ? cell.label : values[cell.field]}
                </span>
              </GridCell>
            ))}
          </div>
          {/*
           * `CardTitle` renders a `div`, so the heading role and level make it the screen's
           * level-1 heading. `text-base` is the title size of AAP 0.4.3; the map titles sit near
           * column 40, so the text is centred.
           */}
          <CardTitle role="heading" aria-level={1} className="text-center text-base">
            {title}
          </CardTitle>
        </CardHeader>
        {/*
         * `tabIndex={-1}` lets the skip link move focus into the body, not only scroll to it.
         * BLITZY [A11Y]: the body then shows the theme's default focus outline (`outline-ring/50`
         * of `src/index.css`), about 1.5:1 against the card, below the 3:1 of WCAG 1.4.11. Kept as
         * the system token; flagged for designer review.
         */}
        <CardContent id={bodyId} tabIndex={-1}>
          {children}
        </CardContent>
        <CardFooter className="flex-col items-stretch gap-2">
          {info !== undefined && <MessageLine kind="info" text={info} />}
          <MessageLine kind={message?.kind ?? "error"} text={message?.text} />
          <PfKeyBar keys={keys} formId={formId} options={keyOptions} />
        </CardFooter>
      </Card>
    </main>
  );
}
