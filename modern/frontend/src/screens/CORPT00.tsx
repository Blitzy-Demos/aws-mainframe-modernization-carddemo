import { zodResolver } from "@hookform/resolvers/zod";
import { useId, useMemo, useState, type ReactNode } from "react";
import { useForm, type FieldErrors } from "react-hook-form";
import { useNavigate } from "react-router";
import { useCorpt00cSubmit } from "@/api/hooks";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
// The unserved-key text of CSMSG01Y is shared by every screen through the header module, so it
// is imported rather than restated here.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { CORPT00_LENGTHS, corpt00Schema, type Corpt00Values } from "@/schemas/CORPT00";

/*
 * Transaction reports, map `CORPT0A` of `app/bms/CORPT00.bms`: transaction CR00, program
 * CORPT00C, endpoint 14 `POST /api/v1/reports/transactions`, route `/reports`, menu option 9.
 *
 * Every edit of the request is CORPT00C's and answers through the server (the `server` rows of
 * `modern/audit/online/CORPT00C.md`); the screen repeats only the map's `LENGTH` and `NUM` through
 * `corpt00Schema`, and keeps the date parts as raw text: which custom dates are valid is CSUTLDTC's
 * verdict, halt H5, so no calendar is checked here (Typed Dates and Timestamps, raw-edit-input
 * exception). The response carries the message row and, at most, the name of the job deck file;
 * the map has no field for that name, so it is not shown, and nothing is downloaded or streamed.
 */

/** The ten inputs of the map, in `POS` order, by their form names. */
type Corpt00Field = keyof Corpt00Values;

/**
 * Colour tokens of AAP 0.4.3 for the map's literals. TURQUOISE maps to the foreground and `BRT`
 * to `font-semibold`; BLUE to the muted foreground; NEUTRAL to the foreground. `h-full` with the
 * label's own `flex items-center` centres each literal on its line, level with the inputs.
 */
const TURQUOISE_BRT = "h-full whitespace-pre font-semibold text-foreground";
const TURQUOISE = "h-full whitespace-pre text-foreground";
const BLUE = "h-full whitespace-pre text-muted-foreground";
const HINT_BLUE = "flex h-full items-center whitespace-pre text-muted-foreground";
const HINT_NEUTRAL = "flex h-full items-center whitespace-pre text-foreground";

/*
 * BLITZY [LAYOUT]: the Input's default `w-full` and `px-3` are overridden with `px-0.5` and the
 * width below for each map `LENGTH`. A grid column is narrower than a character, so the default
 * box, at least 26px wide inside a 7px to 52px cell, left no room for a 1- or 2-character value.
 * Each width is a spacing step that holds `LENGTH` monospace `text-sm` characters (0.525rem each)
 * with the padding and border, and a little more for a wider fallback monospace font, so a
 * full-length value shows; the box still ends before the `/` or hint that follows it when the
 * frame is at its minimum width.
 */
const WIDTH_BY_LENGTH = {
  1: "w-4",
  2: "w-6",
  4: "w-11",
} as const satisfies Record<(typeof CORPT00_LENGTHS)[Corpt00Field], string>;

/** One unprotected field of the map with the literal that labels it. */
type InputSpec = {
  readonly name: Corpt00Field;
  /** The input's `POS=(row,col)`; its `LENGTH` is `CORPT00_LENGTHS[name]`. */
  readonly row: number;
  readonly col: number;
  /** The `NUM` attribute: the six date parts. */
  readonly numeric: boolean;
  /**
   * The accessible name of a date part. Its visible label is the row's `Start Date :` or one of
   * the `/` separators the map places before it, which alone would name the day and the year
   * `/`; the name says which part of which date the field holds.
   */
  readonly accessibleName?: string;
  /** The literal bound to the input: its `POS`, `LENGTH`, `INITIAL` and colour classes. */
  readonly label: {
    readonly row: number;
    readonly col: number;
    readonly span: number;
    readonly text: string;
    readonly className: string;
  };
};

/**
 * The map's inputs in field order (`modern/audit/screens.md` rows `CORPT00-012` to `CORPT00-038`),
 * each with the literal `screens.md` binds to it. The three report labels stand after their
 * selectors at column 15; `Start Date :`, `  End Date :` and the confirmation prompt stand before
 * their fields. The leading spaces of `  End Date :` are the map's and keep the colons aligned.
 */
const INPUTS: readonly InputSpec[] = [
  {
    name: "monthly",
    row: 7,
    col: 10,
    numeric: false,
    label: { row: 7, col: 15, span: 23, text: "Monthly (Current Month)", className: TURQUOISE_BRT },
  },
  {
    name: "yearly",
    row: 9,
    col: 10,
    numeric: false,
    label: { row: 9, col: 15, span: 23, text: "Yearly (Current Year)", className: TURQUOISE_BRT },
  },
  {
    name: "custom",
    row: 11,
    col: 10,
    numeric: false,
    label: { row: 11, col: 15, span: 23, text: "Custom (Date Range)", className: TURQUOISE_BRT },
  },
  {
    name: "startMonth",
    row: 13,
    col: 29,
    numeric: true,
    accessibleName: "Start Date month",
    label: { row: 13, col: 15, span: 12, text: "Start Date :", className: TURQUOISE },
  },
  {
    name: "startDay",
    row: 13,
    col: 34,
    numeric: true,
    accessibleName: "Start Date day",
    label: { row: 13, col: 32, span: 1, text: "/", className: BLUE },
  },
  {
    name: "startYear",
    row: 13,
    col: 39,
    numeric: true,
    accessibleName: "Start Date year",
    label: { row: 13, col: 37, span: 1, text: "/", className: BLUE },
  },
  {
    name: "endMonth",
    row: 14,
    col: 29,
    numeric: true,
    accessibleName: "End Date month",
    label: { row: 14, col: 15, span: 12, text: "  End Date :", className: TURQUOISE },
  },
  {
    name: "endDay",
    row: 14,
    col: 34,
    numeric: true,
    accessibleName: "End Date day",
    label: { row: 14, col: 32, span: 1, text: "/", className: BLUE },
  },
  {
    name: "endYear",
    row: 14,
    col: 39,
    numeric: true,
    accessibleName: "End Date year",
    label: { row: 14, col: 37, span: 1, text: "/", className: BLUE },
  },
  {
    name: "confirm",
    row: 19,
    col: 66,
    numeric: false,
    label: {
      row: 19,
      col: 6,
      span: 59,
      text: "The Report will be submitted for printing. Please confirm: ",
      className: TURQUOISE,
    },
  },
];

/** The literals bound to no input: the two date-format hints and the `(Y/N)` hint. */
const HINTS: readonly {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly text: string;
  readonly className: string;
}[] = [
  { row: 13, col: 46, span: 12, text: "(MM/DD/YYYY)", className: HINT_BLUE },
  { row: 14, col: 46, span: 12, text: "(MM/DD/YYYY)", className: HINT_BLUE },
  { row: 19, col: 69, span: 5, text: "(Y/N)", className: HINT_NEUTRAL },
];

/**
 * Every input empty: the map as CORPT00C first sends it (`MOVE LOW-VALUES TO CORPT0AO`,
 * CORPT00C-002) and as `INITIALIZE-ALL-FIELDS` leaves it after a 200 (CORPT00C-032, CORPT00C-038).
 */
const BLANK: Corpt00Values = {
  monthly: "",
  yearly: "",
  custom: "",
  startMonth: "",
  startDay: "",
  startYear: "",
  endMonth: "",
  endDay: "",
  endYear: "",
  confirm: "",
};

/**
 * Wire name of a `ScreenError.field` → the input it names. The endpoint names its request members
 * (`start_month`); the form names its fields after the schema (`startMonth`).
 */
const FIELD_OF_WIRE: Readonly<Record<string, Corpt00Field>> = {
  monthly: "monthly",
  yearly: "yearly",
  custom: "custom",
  start_month: "startMonth",
  start_day: "startDay",
  start_year: "startYear",
  end_month: "endMonth",
  end_day: "endDay",
  end_year: "endYear",
  confirm: "confirm",
};

/**
 * The input a response names, or `monthly`, where every CORPT00C send without a named field puts
 * the cursor (CORPT00C-042). An own-property check, so `constructor` and its kin name no input.
 */
function focusTarget(field: string | null | undefined): Corpt00Field {
  return field != null && Object.hasOwn(FIELD_OF_WIRE, field) ? FIELD_OF_WIRE[field] : "monthly";
}

/**
 * The transaction reports screen, CR00 (CORPT00C).
 *
 * - First entry shows every input empty, no message and the cursor on `MONTHLY` (CORPT00C-002).
 * - ENTER sends the ten inputs exactly as typed to endpoint 14 (CORPT00C-042). A 200 empties
 *   every input and focuses `MONTHLY` (CORPT00C-032, CORPT00C-038); its message, the submission
 *   text, is the one message the program colours green, so it shows as information (the `Alert`
 *   default variant; the theme has no green token), not as an error (CORPT00C-040). A
 *   `ScreenError` keeps every input as typed, shows its message in red and focuses the field it
 *   names, or `MONTHLY`.
 * - A value the map would not take (longer than its `LENGTH`, or a date part that is not digits)
 *   is stopped by the schema before any request, with zod's message in the message row; a
 *   message from the server replaces it on the next response.
 * - F3 goes back to the main menu, as CORPT00C transfers to COMEN01C (CORPT00C-004).
 * - Any other function key shows `Invalid key pressed. Please see below...` and focuses
 *   `MONTHLY`, keeping the inputs (CORPT00C-005).
 */
export function CORPT00(): ReactNode {
  const navigate = useNavigate();
  const formId = useId();
  const submit = useCorpt00cSubmit();
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);
  const form = useForm<Corpt00Values>({
    resolver: zodResolver(corpt00Schema),
    defaultValues: BLANK,
  });
  const { reset, setFocus } = form;

  const keys = useMemo<PfKeyBarEntry[]>(
    () =>
      keyRowItems(KEY_ROWS.CORPT00).map((item) =>
        item.key === "F3"
          ? {
              ...item,
              onPress: () => {
                void navigate(PATHS.COMEN01);
              },
            }
          : item,
      ),
    [navigate],
  );

  const keyOptions = useMemo<PfKeyOptions>(
    () => ({
      foldShifted: KEY_ROWS.CORPT00.foldShifted,
      onInvalidKey: () => {
        setMessage({ text: INVALID_KEY, kind: "error" });
        setFocus("monthly");
      },
    }),
    [setFocus],
  );

  function onValid(values: Corpt00Values): void {
    // One request per ENTER: a second ENTER while the first is in flight would write a second
    // job deck for the same screen.
    if (submit.isPending) {
      return;
    }
    submit.mutate(
      {
        body: {
          monthly: values.monthly,
          yearly: values.yearly,
          custom: values.custom,
          start_month: values.startMonth,
          start_day: values.startDay,
          start_year: values.startYear,
          end_month: values.endMonth,
          end_day: values.endDay,
          end_year: values.endYear,
          confirm: values.confirm,
        },
      },
      {
        onSuccess: (result) => {
          // `keepFieldsRef` keeps the registered inputs: a plain reset drops them, and the
          // `setFocus` that follows would then find no `MONTHLY` to focus.
          reset(BLANK, { keepFieldsRef: true });
          setMessage({ text: result.message, kind: "info" });
          setFocus("monthly");
        },
        onError: (error) => {
          setMessage({ text: error.message, kind: "error" });
          setFocus(focusTarget(error.field));
        },
      },
    );
  }

  /** Shows the first schema message in map order; react-hook-form focuses that field. */
  function onInvalid(errors: FieldErrors<Corpt00Values>): void {
    const text = INPUTS.map((input) => errors[input.name]?.message).find(
      (issue) => issue !== undefined,
    );
    setMessage({ text, kind: "error" });
  }

  return (
    <ScreenShell
      transaction={SCREENS.CORPT00.transaction}
      program={SCREENS.CORPT00.program}
      title="Transaction Reports"
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={keyOptions}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          onSubmit={(event) => {
            void form.handleSubmit(onValid, onInvalid)(event);
          }}
        >
          <FieldGrid>
            {INPUTS.map((input) => (
              <FormField
                key={input.name}
                control={form.control}
                name={input.name}
                render={({ field }) => (
                  // `contents` lets the label's cell and the input's cell sit on the map grid
                  // itself while the item still binds the label to the input.
                  <FormItem className="contents">
                    <GridCell row={input.label.row} col={input.label.col} span={input.label.span}>
                      <FormLabel className={input.label.className}>{input.label.text}</FormLabel>
                    </GridCell>
                    <GridCell row={input.row} col={input.col} span={CORPT00_LENGTHS[input.name]}>
                      <FormControl>
                        <Input
                          {...field}
                          className={`px-0.5 ${WIDTH_BY_LENGTH[CORPT00_LENGTHS[input.name]]}`}
                          maxLength={CORPT00_LENGTHS[input.name]}
                          inputMode={input.numeric ? "numeric" : undefined}
                          autoFocus={input.name === "monthly"}
                          autoComplete="off"
                          aria-label={input.accessibleName}
                        />
                      </FormControl>
                    </GridCell>
                  </FormItem>
                )}
              />
            ))}
            {HINTS.map((hint) => (
              <GridCell
                key={`${hint.row}-${hint.col}`}
                row={hint.row}
                col={hint.col}
                span={hint.span}
              >
                <span className={hint.className}>{hint.text}</span>
              </GridCell>
            ))}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
