import { useContext, useId, useMemo, type ReactNode } from "react";
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { TITLE01, TITLE02, formatHeaderDate, formatHeaderTime } from "@/lib/header";
import { TransferPendingContext } from "@/routes";
import { GridCell } from "./FieldGrid";
import { MessageLine, type MessageKind } from "./MessageLine";
import { PfKeyBar, type PfKeyBarProps } from "./PfKeyBar";

/**
 * Which placement of the header rows a map declares.
 *
 * - `standard`: the 20 maps other than COSGN00, for example `app/bms/COMEN01.bms:29-74`: rows 1
 *   and 2, labels `Tran:`, `Prog:` at column 1 and `Date:`, `Time:` at column 65, five wide;
 *   `TRNNAME` and `PGMNAME` at column 7; `CURTIME` eight wide.
 * - `signon`: COSGN00 only, `app/bms/COSGN00.bms:29-93`: rows 1 and 2, labels `Tran :`, `Prog :`
 *   at column 1 and `Date :`, `Time :` at column 64, six wide; `TRNNAME` and `PGMNAME` at column
 *   8; `CURTIME` nine wide (`INITIAL='Ahh:mm:ss'`). Then row 3: `AppID:` at column 1 and `SysID:`
 *   at column 64, six wide, labelling `APPLID` at column 8 and `SYSID` at column 71, eight wide.
 */
export type HeaderLayout = "standard" | "signon";

/** The text of a map's `ERRMSG` row and the colour the program gives it. */
export type ShellMessage = {
  /** The message as received; absent, empty or all spaces leaves the row empty. */
  text?: string;
  /**
   * The colour: `error` for an error (RED). `info` for a success text the program moves to
   * `ERRMSG` after recolouring the field, so it shows in the information colour rather than as an
   * error. Either colour keeps the field's `BRT` emphasis.
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
  /**
   * Whether the `INFOMSG` row shows bright (`font-semibold`); `false` when omitted, as the map
   * declares the field without `BRT`. Set by the three programs that brighten it whenever it holds
   * a message, and every state of theirs has one: COCRDUPC (`DFHBMBRY`, audit row COCRDUPC-046),
   * COACTUPC (`DFHBMASB`, COACTUPC-083) and COTRTUPC (`DFHBMASB`, COTRTUPC-045).
   */
  infoBright?: boolean;
  /** The map's row-24 keys, forwarded to `PfKeyBar` unchanged. */
  keys: PfKeyBarProps["keys"];
  /** Settings of the keyboard binding, forwarded to `PfKeyBar` as `options`. */
  keyOptions?: PfKeyBarProps["options"];
  /**
   * Whether the screen waits for a response; forwarded to `PfKeyBar` as `busy`, which is also
   * true while a transfer to another screen is under way.
   */
  busy?: boolean;
  /**
   * The instant `CURDATE` and `CURTIME` show; the current time at each render when omitted, as
   * every program refreshes them on each send. Tests pass a fixed instant.
   */
  now?: Date;
  /** Placement of the header rows; `standard` unless the map is COSGN00. */
  headerLayout?: HeaderLayout;
  /** The screen body: the screen's `<form>` with its `FieldGrid`. */
  children: ReactNode;
};

/**
 * The header fields the programs fill in their header paragraph, by their BMS names: the six of
 * rows 1 and 2, and the `APPLID` and `SYSID` of COSGN00's row 3.
 */
type HeaderField =
  | "TRNNAME"
  | "TITLE01"
  | "CURDATE"
  | "PGMNAME"
  | "TITLE02"
  | "CURTIME"
  | "APPLID"
  | "SYSID";

/**
 * One header-row `DFHMDF` macro: a literal label and the value field it labels, or a value field
 * filled at render. A value the map labels with no literal, a title, carries its accessible name.
 */
type HeaderCell = {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly className: string;
} & (
  | { readonly label: string; readonly labels: HeaderField; readonly field?: never }
  | { readonly field: HeaderField; readonly name?: string; readonly label?: never }
);

/*
 * Colour tokens of AAP 0.4.3. The labels are BLUE, the titles YELLOW, which has no token and is
 * expressed as emphasis on the foreground. The value fields are BLUE on the map, snapped by role
 * to the foreground token of value fields.
 *
 * Each value is a read-only `Input` as wide as its `LENGTH` (`size`, so `w-auto` replaces the
 * input's full width) at its `POS` column: a grid column is narrower than a character at some
 * widths and wider at others, so a box filling its cell would cut a value or stretch past it.
 * `h-full` centres each label on its line beside the boxes; `whitespace-pre` keeps its spacing.
 *
 * `TITLE01` and `TITLE02` keep the padding that centres them in their 40 characters
 * (`app/cpy/COTTL01Y.cpy:18-22`). Their 40-character box is centred in its 40-column cell
 * (`block mx-auto`), which keeps the titles centred on the screen as the padded fields are on the
 * terminal, instead of drifting left of the screen title.
 */
const LABEL = "h-full whitespace-pre text-muted-foreground";
const VALUE = "w-auto text-foreground";
const TITLE = "mx-auto block w-auto font-semibold text-foreground";

/** The accessible names of the two titles, which no map literal labels. */
const TITLE01_NAME = "Title row 1";
const TITLE02_NAME = "Title row 2";

/**
 * The header rows of each layout, in macro order, as `POS` row, `POS` column and `LENGTH`
 * (`modern/audit/screens.md` rows `<MAP>-001` to `<MAP>-010`, and `COSGN00-011` to
 * `COSGN00-014`). Columns are the map's own, as `GridCell` takes them.
 */
const HEADER_CELLS: Readonly<Record<HeaderLayout, readonly HeaderCell[]>> = {
  standard: [
    { row: 1, col: 1, span: 5, label: "Tran:", labels: "TRNNAME", className: LABEL },
    { row: 1, col: 7, span: 4, field: "TRNNAME", className: VALUE },
    { row: 1, col: 21, span: 40, field: "TITLE01", name: TITLE01_NAME, className: TITLE },
    { row: 1, col: 65, span: 5, label: "Date:", labels: "CURDATE", className: LABEL },
    { row: 1, col: 71, span: 8, field: "CURDATE", className: VALUE },
    { row: 2, col: 1, span: 5, label: "Prog:", labels: "PGMNAME", className: LABEL },
    { row: 2, col: 7, span: 8, field: "PGMNAME", className: VALUE },
    { row: 2, col: 21, span: 40, field: "TITLE02", name: TITLE02_NAME, className: TITLE },
    { row: 2, col: 65, span: 5, label: "Time:", labels: "CURTIME", className: LABEL },
    { row: 2, col: 71, span: 8, field: "CURTIME", className: VALUE },
  ],
  signon: [
    { row: 1, col: 1, span: 6, label: "Tran :", labels: "TRNNAME", className: LABEL },
    { row: 1, col: 8, span: 4, field: "TRNNAME", className: VALUE },
    { row: 1, col: 21, span: 40, field: "TITLE01", name: TITLE01_NAME, className: TITLE },
    { row: 1, col: 64, span: 6, label: "Date :", labels: "CURDATE", className: LABEL },
    { row: 1, col: 71, span: 8, field: "CURDATE", className: VALUE },
    { row: 2, col: 1, span: 6, label: "Prog :", labels: "PGMNAME", className: LABEL },
    { row: 2, col: 8, span: 8, field: "PGMNAME", className: VALUE },
    { row: 2, col: 21, span: 40, field: "TITLE02", name: TITLE02_NAME, className: TITLE },
    { row: 2, col: 64, span: 6, label: "Time :", labels: "CURTIME", className: LABEL },
    { row: 2, col: 71, span: 9, field: "CURTIME", className: VALUE },
    { row: 3, col: 1, span: 6, label: "AppID:", labels: "APPLID", className: LABEL },
    { row: 3, col: 8, span: 8, field: "APPLID", className: VALUE },
    { row: 3, col: 64, span: 6, label: "SysID:", labels: "SYSID", className: LABEL },
    { row: 3, col: 71, span: 8, field: "SYSID", className: VALUE },
  ],
};

/**
 * The frame of every screen, one BMS map laid out in a shadcn/ui `Card` (AAP 0.3.5, 0.4.2):
 *
 * - `CardHeader`: header rows 1 and 2 on the 80-column grid of `FieldGrid` (the transaction,
 *   the titles of `COTTL01Y`, the date, the program, the time), with COSGN00's row 3 (the
 *   application and system IDs), then the screen title;
 * - `CardContent`: the screen body;
 * - `CardFooter`: the `INFOMSG` row when the map has one, the `ERRMSG` row and the row-24 keys.
 *
 * The header grid has the body grid's columns and gaps inside the same padding, so the header
 * fields stand over the body columns of the same number. Each header value is an output field of
 * the map, so it is a read-only `Input` keeping the field's box and width; each header literal is
 * the `Label` of the value after it, bound by `htmlFor`, and the titles, which no literal labels,
 * are named by `aria-label`. The header fields are `ASKIP` or `PROT`, which the 3270 cursor skips,
 * so each value is out of the tab order (`tabIndex={-1}`) and Tab moves past the header rows to
 * the body. No CICS region assigns `APPLID` and `SYSID`, so they stay empty (X-SCREENS-1).
 *
 * `PfKeyBar` is the screen's only keyboard binding, so screens never call `usePfKeys`; a screen
 * locks its keys through the keys' `disabled` and `busy`, which `PfKeyBar` receives. While a
 * transfer to another screen is under way (`TransferPendingContext`: the router is loading the
 * target screen and this one stays mounted), the frame locks them all itself: every key it
 * forwards is `disabled` and the row is `busy`, whatever the screen passes, so no key acts on the
 * screen being left and an unserved F-key shows no invalid-key message. Every text passed in is
 * shown verbatim and never logged: card numbers and SSNs arrive already redacted.
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
  infoBright = false,
  keys,
  keyOptions,
  busy,
  now,
  headerLayout = "standard",
  children,
}: ScreenShellProps): ReactNode {
  const bodyId = useId();
  const headerId = useId();
  const transferPending = useContext(TransferPendingContext);
  // Memoized, so the keys of a screen that memoizes them reach `PfKeyBar` as stable as they left.
  const shownKeys = useMemo(
    () => (transferPending ? keys.map((entry) => ({ ...entry, disabled: true })) : keys),
    [keys, transferPending],
  );
  const at = now ?? new Date();
  const values: Readonly<Record<HeaderField, string>> = {
    TRNNAME: transaction,
    TITLE01,
    CURDATE: formatHeaderDate(at),
    PGMNAME: program,
    TITLE02,
    CURTIME: formatHeaderTime(at),
    APPLID: "",
    SYSID: "",
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
                {cell.field === undefined ? (
                  <Label htmlFor={`${headerId}-${cell.labels}`} className={cell.className}>
                    {cell.label}
                  </Label>
                ) : (
                  <Input
                    id={`${headerId}-${cell.field}`}
                    data-field={cell.field}
                    aria-label={cell.name}
                    readOnly
                    tabIndex={-1}
                    value={values[cell.field]}
                    size={cell.span}
                    className={cell.className}
                  />
                )}
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
          {info !== undefined && <MessageLine kind="info" bright={infoBright} text={info} />}
          {/*
           * Every map declares `ERRMSG` `ATTRB=(ASKIP,BRT,FSET)` and no program changes its
           * attribute byte, so the row is bright whatever colour its message takes.
           */}
          <MessageLine kind={message?.kind ?? "error"} bright text={message?.text} />
          <PfKeyBar
            keys={shownKeys}
            formId={formId}
            options={keyOptions}
            busy={transferPending || busy === true}
          />
        </CardFooter>
      </Card>
    </main>
  );
}
