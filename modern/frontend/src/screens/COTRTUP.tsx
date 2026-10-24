import { zodResolver } from "@hookform/resolvers/zod";
import { cn } from "cn";
import { useId, useLayoutEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useForm, type ControllerRenderProps } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCotrtupcAction, useCotrtupcFetch } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldControl } from "@/components/screen/FieldControl";
import { FIELD_BOX_HEIGHT, FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  KEY_ROWS,
  bindPfKeys,
  cotrtupKeys,
  keyRowItemsFor,
  type CotrtupKeyState,
} from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import {
  COTRTUP_LENGTHS,
  cotrtupKeySchema,
  cotrtupSchema,
  type CotrtupKeyValues,
  type CotrtupValues,
} from "@/schemas/COTRTUP";

/*
 * Transaction type maintenance: map `CTRTUPA` of `app/app-transaction-type-db2/bms/COTRTUP.bms`,
 * transaction CTTU, program COTRTUPC, endpoints 26 (fetch) and 27 (validate, save, delete), both
 * admin-only (AAP 0.3.4.1, 0.3.5).
 *
 * COTRTUPC is pseudo-conversational: its COMMAREA carries `TTUP-CHANGE-ACTION`, the fetched values
 * (`TTUP-OLD-DETAILS`) and the entered values (`TTUP-NEW-DETAILS`) from one screen to the next.
 * The server keeps none of that, so this screen holds it, as `modern/audit/online/COTRTUPC.md`
 * ("Endpoint mapping") assigns: endpoint 26 serves ENTER in the fresh and X states, endpoint 27
 * serves ENTER in S, E, B and R, F5 in N and the second F4 in 9, and every other step (F3, F12,
 * the first F4, F5 in X, ENTER in N, a refused key, any key after a completed save or delete) is
 * served here without a request. No request carries the held state: the endpoint re-reads the
 * row and re-edits the input, so the state grants nothing (AAP 0.5.1).
 *
 * The state names are `CotrtupKeyState`'s, the `VALUE`s of the program's 88-levels; which keys
 * each state accepts, starts over on or shows is `cotrtupKeys`' (COTRTUPC-042, COTRTUP-021 to
 * COTRTUP-025).
 */

/** `INITIAL` of the title field at `POS=(7,28)` (COTRTUP-011). */
const TITLE = "Maintain Transaction Type";

/** `INITIAL` of the label at `POS=(12,4)` (COTRTUP-012); the double space is the map's. */
const TYPE_CODE_LABEL = "Transaction Type  :";

/** `INITIAL` of the label at `POS=(14,4)` (COTRTUP-015). */
const DESCRIPTION_LABEL = "Description       :";

/** Both labels: `POS` column 4, `LENGTH=19` (COTRTUP-012, COTRTUP-015). */
const LABEL_COLUMN = 4;
const LABEL_LENGTH = 19;

/** Both inputs: `POS` column 26; `TRTYPCD` on line 12, `TRTYDSC` on line 14 (COTRTUP-013, -016). */
const INPUT_COLUMN = 26;
const TYPE_CODE_ROW = 12;
const DESCRIPTION_ROW = 14;

/**
 * TURQUOISE labels snap to the foreground token (AAP 0.4.3); `whitespace-pre` keeps the spaces of
 * the `INITIAL` text, which align the colons. `FieldGrid` centres each cell on its line, so the
 * label stands level with the taller input beside it.
 */
const LABEL_CLASS = "h-full whitespace-pre text-foreground";

/**
 * The width of the `TRTYPCD` input only. Every input carries `size` = its map `LENGTH`, and every
 * input but this one keeps the `Input` default `w-full`, so the 50-character `TRTYDSC` input is
 * exactly its 50 columns at every viewport, as COTRTLI's Description Filter is. A grid column is
 * narrower than a character plus the `Input` padding, so the 2-column `TRTYPCD` input would hide
 * its own text at `w-full`; it takes its 2-character intrinsic width instead (`w-auto`), running
 * into the free columns to its right, as no field follows it on its line. Both inputs take the
 * shared map-field box height (`FIELD_BOX_HEIGHT`) in place of the `Input` default.
 */
const CODE_INPUT_CLASS = "w-auto";

/** `WS-INFO-MSG PIC X(40)`, the width the info row is centred in (COTRTUPC-045). */
const INFO_WIDTH = 40;

/**
 * The info row of each state, `3250-SETUP-INFOMSG` (`COTRTUPC.cbl:142-165,1210-1265`;
 * COTRTUPC-045). A success body carries its own; after a step served without a request, or an
 * error response, which carries only the error row, the screen takes the state's from here.
 */
const INFO_MESSAGES: Readonly<Record<CotrtupKeyState, string>> = {
  fresh: "Enter transaction type to be maintained",
  S: "Enter transaction type to be maintained",
  "8": "Enter transaction type to be maintained",
  "B-blank": "Enter transaction type to be maintained",
  X: "Press F05 to add. F12 to cancel",
  R: "Enter new transaction type details.",
  E: "Update transaction type details shown.",
  B: "Update transaction type details shown.",
  N: "Changes validated.Press F5 to save",
  C: "Changes committed to database",
  L: "Changes unsuccessful",
  "6": "Changes unsuccessful",
  "9": "Delete this record ? Press F4 to confirm",
  "7": "Delete successful.",
};

/** 88 `WS-UPDATE-WAS-CANCELLED`, the error row of F12 in state N (COTRTUPC-039). */
const UPDATE_CANCELLED = "Update was cancelled";

/** 88 `WS-DELETE-WAS-CANCELLED`, the error row of F12 in state 9 (COTRTUPC-040). */
const DELETE_CANCELLED = "Delete was cancelled";

/** 88 `WS-INVALID-KEY-PRESSED`, the error row of a key the state refuses (COTRTUPC-042). */
const INVALID_KEY_PRESSED = "Invalid key pressed";

/*
 * The program texts that name the state an error response leaves. The server sends only the error
 * row; the screen reads which audited branch produced it from the text, and takes the state that
 * branch sets. Store failures are compared by their fixed prefix, the part the program writes;
 * the Db2 text after it is `HALTED(H3)` (COTRTUPC-049) and is never relied on.
 */

/** COTRTUPC-005: the type code was blank or `*`; the field then shows `*` in red (COTRTUPC-047). */
const TYPE_CODE_NOT_SUPPLIED = "Tran Type code must be supplied.";

/** COTRTUPC-018: the description was blank or `*`; the field then shows `*` in red. */
const DESCRIPTION_NOT_SUPPLIED = "Transaction Desc must be supplied.";

/** COTRTUPC-017: S stays S, any other state becomes E. */
const NO_CHANGES_DETECTED = "No change detected with respect to values fetched.";

/** COTRTUPC-011: the fetch failed; state X, as for a code no row holds. */
const FETCH_FAILED = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";

/** COTRTUPC-029: the update could not lock the row; state L. */
const LOCK_FAILED = "Could not lock record for update";

/** COTRTUPC-028 and COTRTUPC-027: the update or the insert failed; state C (X-COTRTUPC-4). */
const SAVE_FAILED = [
  "Error updating: TRANSACTION_TYPE Table. SQLCODE:",
  "Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:",
] as const;

/** COTRTUPC-033: categories refer to the type; state 8 (X-COTRTUPC-6). */
const DELETE_RESTRICTED = "Please delete associated child records first:SQLCODE :";

/** COTRTUPC-034: the delete failed in any other way; state 6. */
const DELETE_FAILED = "Delete failed with message:SQLCODE :";

/** The wire names of the two fields a `ScreenError` can name (`ScreenError.field`). */
const TYPE_CODE_FIELD = "type_code";
const DESCRIPTION_FIELD = "description";

/**
 * `ABEND-DATA` of `CSMSG02Y` as `ABEND-ROUTINE` sends it after the `UNEXPECTED DATA SCENARIO` of
 * `2000-DECIDE-ACTION` (`COTRTUPC.cbl:1073-1080,1675-1698`; `app/cpy/CSMSG02Y.cpy:21-29`;
 * COTRTUPC-037): `ABEND-CODE` `9999` (line 1682 replaces the `0001`), `ABEND-CULPRIT` the program,
 * `ABEND-REASON` blank and `ABEND-MSG`, 4 + 8 + 50 + 72 bytes. `SEND FROM … ERASE` writes them
 * from the top left of the cleared 80-column screen, so they wrap after 80 characters.
 */
const ABEND_DATA = [
  "9999",
  SCREENS.COTRTUP.program.padEnd(8, " "),
  "".padEnd(50, " "),
  "UNEXPECTED DATA SCENARIO".padEnd(72, " "),
].join("");
const SCREEN_COLUMNS = 80;
const ABEND_LINES = [ABEND_DATA.slice(0, SCREEN_COLUMNS), ABEND_DATA.slice(SCREEN_COLUMNS)];

/** A type code and description, as the COMMAREA holds them (`TTUP-OLD-*`, `TTUP-NEW-*`). */
type Values = { readonly code: string; readonly description: string };

/**
 * How a field is highlighted on the screen a step sends (`3300-SETUP-SCREEN-ATTRS`,
 * `CSSETATY`; COTRTUPC-047): `red` in red, `flag` showing `*` in red.
 */
type Mark = "none" | "red" | "flag";

/** The form field the cursor is on, as the forms name the map's `TRTYPCD` and `TRTYDSC`. */
type Cursor = "typeCode" | "description";

/** One screen as COTRTUPC sends it, with the COMMAREA values the next step needs. */
type ScreenView = {
  readonly state: CotrtupKeyState;
  /** `TTUP-OLD-DETAILS`: the values of the last fetch that found a row. */
  readonly old: Values;
  /** `TTUP-NEW-DETAILS`: the values entered, validated or saved. */
  readonly entered: Values;
  /** `ERRMSG`: the error row, empty when none. */
  readonly message: string;
  /** `INFOMSG` before centring. */
  readonly info: string;
  readonly codeMark: Mark;
  readonly descriptionMark: Mark;
  readonly cursor: Cursor;
  /** Counts the screens sent, so each new one resets the form and places the cursor. */
  readonly version: number;
};

const BLANK: Values = { code: "", description: "" };

/** The fresh screen of COTRTUPC-003: nothing fetched, the code editable, with the cursor. */
const FRESH: ScreenView = {
  state: "fresh",
  old: BLANK,
  entered: BLANK,
  message: "",
  info: INFO_MESSAGES.fresh,
  codeMark: "none",
  descriptionMark: "none",
  cursor: "typeCode",
  version: 0,
};

/**
 * The states whose screen shows the fetched values; the fresh screen shows none and every other
 * state the entered ones (`3200-SETUP-SCREEN-VARS`, `COTRTUPC.cbl:1140-1175`; COTRTUPC-030).
 */
const SHOWS_FETCHED: ReadonlySet<CotrtupKeyState> = new Set([
  "S",
  "9",
  "8",
  "7",
  "6",
  "B",
  "B-blank",
]);

/**
 * Where `3300-SETUP-SCREEN-ATTRS` leaves the code editable (`COTRTUPC.cbl:1269-1325`;
 * COTRTUPC-046).
 */
const CODE_EDITABLE: ReadonlySet<CotrtupKeyState> = new Set(["fresh", "X", "L", "B-blank"]);

/** Where it leaves the description editable (COTRTUPC-046). */
const DESCRIPTION_EDITABLE: ReadonlySet<CotrtupKeyState> = new Set(["S", "E", "R", "B"]);

/** Where the cursor goes on the description when no edit names a field (COTRTUPC-046). */
const CURSOR_ON_DESCRIPTION: ReadonlySet<CotrtupKeyState> = new Set(["S", "R", "E", "N", "L", "B"]);

/**
 * The states served by the search form: the code alone is editable and the description is shown
 * protected, as before any fetch. In `B-blank` every key but F3 starts over (COTRTUPC-039).
 */
const SEARCH_STATES: ReadonlySet<CotrtupKeyState> = new Set(["fresh", "B-blank"]);

/** COBOL `= LOW-VALUES OR = SPACES` on `TTUP-OLD-TTYP-TYPE` (`COTRTUPC.cbl:412-414`). */
function isBlank(values: Values): boolean {
  return values.code.trim() === "";
}

/**
 * The info text placed as `3250-SETUP-INFOMSG` centres it: the text's first
 * `LENGTH(TRIM(WS-INFO-MSG))` characters after floor((40 − length) / 2) spaces (COTRTUPC-045).
 */
function centred(text: string): string {
  const length = text.trim().length;
  return " ".repeat(Math.max(0, Math.floor((INFO_WIDTH - length) / 2))) + text.slice(0, length);
}

/** The code and description the screen of `view` shows (`3200-SETUP-SCREEN-VARS`, `CSSETATY`). */
function shownValues(view: ScreenView): Values {
  const base = SHOWS_FETCHED.has(view.state)
    ? view.old
    : view.state === "fresh"
      ? BLANK
      : view.entered;
  return {
    code: view.codeMark === "flag" ? "*" : base.code,
    description: view.descriptionMark === "flag" ? "*" : base.description,
  };
}

/**
 * The next screen, in `state`: the COMMAREA values carried over, the error row empty, the info row
 * and the cursor the state's own, and no field highlighted but the code in state 6
 * (`COTRTUPC.cbl:1331-1334`), unless `change` says otherwise.
 */
function nextScreen(
  from: ScreenView,
  state: CotrtupKeyState,
  change: Partial<Omit<ScreenView, "state" | "version">> = {},
): ScreenView {
  return {
    old: from.old,
    entered: from.entered,
    message: "",
    info: INFO_MESSAGES[state],
    codeMark: state === "6" ? "red" : "none",
    descriptionMark: "none",
    cursor: CURSOR_ON_DESCRIPTION.has(state) ? "description" : "typeCode",
    ...change,
    state,
    version: from.version + 1,
  };
}

/**
 * The screen state a response names. `K` is replaced by the fresh state on the next statement and
 * `V` is never set (X-COTRTUPC-10), so both are the fresh screen; `F` is unreachable
 * (COTRTUPC-028) and keys, shows and protects as `L`, the other `TTUP-CHANGES-FAILED` value. `B`
 * splits on whether fetched values exist (`COTRTUPC.cbl:412-414`).
 */
function screenState(
  state: components["schemas"]["TranTypeMaintState"],
  old: Values,
): CotrtupKeyState {
  switch (state) {
    case "details_not_fetched":
    case "invalid_search_keys":
    case "review_new_record":
      return "fresh";
    case "details_not_found":
      return "X";
    case "show_details":
      return "S";
    case "create_new_record":
      return "R";
    case "confirm_delete":
      return "9";
    case "start_delete":
      return "8";
    case "delete_done":
      return "7";
    case "delete_failed":
      return "6";
    case "changes_not_ok":
      return "E";
    case "changes_ok_not_confirmed":
      return "N";
    case "changes_okayed_lock_error":
    case "changes_okayed_but_failed":
      return "L";
    case "changes_okayed_and_done":
      return "C";
    case "changes_backed_out":
      return isBlank(old) ? "B-blank" : "B";
  }
}

/**
 * The screen after endpoint 26 answered: a row found is state S with its values as the fetched
 * ones (COTRTUPC-009), the description without its trailing spaces; a code no row holds is state
 * X with the normalised code entered and shown red (COTRTUPC-010, COTRTUPC-014).
 */
function afterFetch(from: ScreenView, result: components["schemas"]["TranTypeView"]): ScreenView {
  const state = screenState(result.state, from.old);
  // A description saved through COTRTUPC comes back space-padded to its 50 characters
  // (COTRTUPC-009, COTRTUPC-023). The 3270 operator types over those spaces of the X(50) field; a
  // browser input bounded by `maxLength` is full and takes no keystroke, so the pad is dropped.
  // Leading spaces are data the map shows and stay. The server sees no difference:
  // `1150-STORE-MAP-IN-NEW` trims the description it receives (`COTRTUPC.cbl:678-684`) and the
  // no-change test compares trimmed values (COTRTUPC-017).
  const values = { code: result.type_code, description: (result.description ?? "").trimEnd() };
  return nextScreen(from, state, {
    old: state === "S" ? values : from.old,
    entered: state === "S" ? BLANK : values,
    message: result.message ?? "",
    info: result.info_message,
    ...(state === "X" ? { codeMark: "red" } : {}),
  });
}

/**
 * The screen after endpoint 27 answered: N with the validated values (COTRTUPC-020), C with the
 * saved ones (COTRTUPC-024, COTRTUPC-026), 7 showing the deleted, fetched values (COTRTUPC-032).
 */
function afterAction(
  from: ScreenView,
  sent: Values,
  result: components["schemas"]["TranTypeActionResult"],
): ScreenView {
  return nextScreen(from, screenState(result.state, from.old), {
    entered: { code: result.type_code, description: result.description ?? sent.description },
    message: result.message ?? "",
    info: result.info_message,
  });
}

/**
 * The screen an error response leaves, from the audited branch its text names, or `undefined` for
 * a failure no branch of COTRTUPC produces (no valid token, the gate, an unreachable server, the
 * blank 500 of COTRTUPC-038), which leaves the screen as it is with the message shown.
 */
function afterFailure(
  from: ScreenView,
  sent: Values,
  error: components["schemas"]["ScreenError"],
): ScreenView | undefined {
  const { message, field } = error;
  if (message.startsWith(FETCH_FAILED)) {
    return nextScreen(from, "X", {
      entered: { code: sent.code, description: "" },
      message,
      codeMark: "red",
    });
  }
  if (message === LOCK_FAILED) {
    return nextScreen(from, "L", { message });
  }
  if (SAVE_FAILED.some((prefix) => message.startsWith(prefix))) {
    return nextScreen(from, "C", { message });
  }
  if (message.startsWith(DELETE_RESTRICTED)) {
    return nextScreen(from, "8", { message });
  }
  if (message.startsWith(DELETE_FAILED)) {
    return nextScreen(from, "6", { message });
  }
  if (field === TYPE_CODE_FIELD) {
    return codeRefused(from, message);
  }
  if (field === DESCRIPTION_FIELD) {
    if (message === NO_CHANGES_DETECTED && from.state === "S") {
      return nextScreen(from, "S", { message });
    }
    return nextScreen(from, "E", {
      entered: sent,
      message,
      descriptionMark:
        message === NO_CHANGES_DETECTED
          ? "none"
          : message === DESCRIPTION_NOT_SUPPLIED
            ? "flag"
            : "red",
      cursor: "description",
    });
  }
  return undefined;
}

/**
 * A refused type code: the fresh state, the code emptied and red, or `*` in red when it was not
 * supplied, with the cursor (COTRTUPC-005 to COTRTUPC-007, COTRTUPC-016).
 */
function codeRefused(from: ScreenView, message: string): ScreenView {
  return nextScreen(from, "fresh", {
    message,
    codeMark: message === TYPE_CODE_NOT_SUPPLIED ? "flag" : "red",
    cursor: "typeCode",
  });
}

/** A key the state refuses: the state's screen again, nothing typed kept (COTRTUPC-042). */
function keyRefused(from: ScreenView): ScreenView {
  return nextScreen(from, from.state, { message: INVALID_KEY_PRESSED });
}

/**
 * F3's destination: the screen that transferred here, when it names one of the routes, else the
 * admin menu, COADM01C, also when no caller is recorded (`COTRTUPC.cbl:429-460`; COTRTUPC-044).
 */
function callerPath(locationState: unknown): string {
  if (
    typeof locationState === "object" &&
    locationState !== null &&
    "from" in locationState &&
    typeof locationState.from === "string" &&
    Object.values<string>(PATHS).includes(locationState.from)
  ) {
    return locationState.from;
  }
  return PATHS.COADM01;
}

/** The input a `FormField` of either form binds, both fields holding text. */
type BoundInput =
  | ControllerRenderProps<CotrtupKeyValues, "typeCode">
  | ControllerRenderProps<CotrtupValues, "typeCode" | "description">;

/** Props of {@link MapInput}. */
type MapInputProps = {
  row: number;
  label: string;
  length: number;
  input: BoundInput;
  invalid: boolean;
  /**
   * Whether the screen state leaves the input enterable (COTRTUPC-046). A protected input is
   * read-only and out of the Tab order, as the 3270 Tab key skips protected fields; it still takes
   * the cursor the program places on it.
   */
  editable: boolean;
  /**
   * A request holds the keyboard: the input is read-only, and an enterable one keeps its Tab stop.
   */
  locked: boolean;
  mark: Mark;
  /** `ERRMSG`, the error row's text, which describes the input while the screen marks it. */
  message: string;
  autoFocus?: boolean;
};

/** One map input with its label on their map line, inside a `FormField`. */
function MapInput({ row, label, ...cell }: MapInputProps): ReactNode {
  return (
    // `contents` lets the label and input cells sit on the `FieldGrid` itself.
    <FormItem className="contents">
      <GridCell row={row} col={LABEL_COLUMN} span={LABEL_LENGTH}>
        <FormLabel className={LABEL_CLASS}>{label}</FormLabel>
      </GridCell>
      <MapInputCell row={row} {...cell} />
    </FormItem>
  );
}

/**
 * The input cell of a {@link MapInput}, inside its `FormItem`. An input the step's screen marks
 * (`Mark`) or the form edit refuses is invalid and described by an `sr-only` `FormMessage`, which
 * holds the form edit's message, else the error row's text: the error row stays the screen's one
 * visible message, and the marks stay the screen's. The control names that message while it
 * shows text, and nothing else: the map gives neither input a hint.
 */
function MapInputCell({
  row,
  length,
  input,
  invalid,
  editable,
  locked,
  mark,
  message,
  autoFocus = false,
}: Omit<MapInputProps, "label">): ReactNode {
  const red = mark !== "none";
  return (
    <GridCell row={row} col={INPUT_COLUMN} span={length}>
      {/* A screen mark's text is the `FormMessage` children below; a form edit's error wins. */}
      <FieldControl messaged={red && message !== ""}>
        <Input
          {...input}
          maxLength={length}
          size={length}
          autoFocus={autoFocus}
          autoComplete="off"
          readOnly={!editable || locked}
          tabIndex={editable ? undefined : -1}
          aria-invalid={red || invalid}
          className={cn(
            FIELD_BOX_HEIGHT,
            length === COTRTUP_LENGTHS.typeCode && CODE_INPUT_CLASS,
            red && "text-destructive",
          )}
        />
      </FieldControl>
      <FormMessage className="sr-only">{red ? message : undefined}</FormMessage>
    </GridCell>
  );
}

/**
 * CTTU: shows one type of the Db2 data version, `db2.transaction_type`, for update, add or delete
 * (COTRTUPC-002), served step by step as COTRTUPC serves it.
 */
export function COTRTUP(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const formId = useId();
  const descriptionId = useId();
  const fetchType = useCotrtupcFetch();
  const actOnType = useCotrtupcAction();
  const [view, setView] = useState<ScreenView>(FRESH);
  const [abended, setAbended] = useState(false);
  const searchForm = useForm<CotrtupKeyValues>({
    resolver: zodResolver(cotrtupKeySchema),
    defaultValues: { typeCode: "" },
  });
  const detailsForm = useForm<CotrtupValues>({
    resolver: zodResolver(cotrtupSchema),
    defaultValues: { typeCode: "", description: "" },
  });
  // Set from the key that starts a request until its reply, as the 3270 keyboard locks until the
  // program answers; a ref, so a second key pressed before the next render is refused too, and
  // `pending`, which renders it: every key disabled, the key row and the form busy and both
  // inputs read-only.
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const searching = SEARCH_STATES.has(view.state);

  /** Locks (`true`) or opens (`false`) the keyboard, for the key handlers and for the screen. */
  function setInFlight(locked: boolean): void {
    inFlight.current = locked;
    setPending(locked);
  }

  /** A form check that failed unexpectedly sent nothing: the keyboard opens, the error stands. */
  function unlockAndRethrow(error: unknown): never {
    setInFlight(false);
    throw error;
  }

  // Each screen sent places the cursor where `3300-SETUP-SCREEN-ATTRS` puts it (COTRTUPC-046).
  // A layout effect, so it runs after the `autoFocus` of a newly mounted form and before paint.
  useLayoutEffect(() => {
    if (searching) {
      searchForm.setFocus("typeCode");
    } else {
      detailsForm.setFocus(view.cursor);
    }
  }, [view.version, view.cursor, searching, searchForm, detailsForm]);

  /** Sends `next`: the forms take the values it shows, discarding anything typed. */
  function show(next: ScreenView): void {
    const shown = shownValues(next);
    searchForm.reset({ typeCode: shown.code });
    detailsForm.reset({ typeCode: shown.code, description: shown.description });
    setView(next);
  }

  /** Applies an error response from the screen `from`, which sent `sent`. */
  function failed(
    from: ScreenView,
    sent: Values,
    error: components["schemas"]["ScreenError"],
  ): void {
    const next = afterFailure(from, sent, error);
    if (next === undefined) {
      setView({ ...from, message: error.message });
    } else {
      show(next);
    }
  }

  /** Endpoint 26 for the code typed (COTRTUPC-009 to COTRTUPC-012). */
  function fetchCode(from: ScreenView, typeCode: string): void {
    const sent = { code: typeCode, description: "" };
    setInFlight(true);
    fetchType.mutate(
      { path: { type_code: typeCode } },
      {
        onSuccess: (result) => {
          show(afterFetch(from, result));
        },
        onError: (error) => {
          failed(from, sent, error);
        },
        onSettled: () => {
          setInFlight(false);
        },
      },
    );
  }

  /** Endpoint 27 with `aid` for the code shown (COTRTUPC-016 to COTRTUPC-034). */
  function actOn(
    from: ScreenView,
    aid: components["schemas"]["CotrtupcAid"],
    sent: Values,
  ): void {
    setInFlight(true);
    actOnType.mutate(
      {
        path: { type_code: sent.code },
        // The second F4 receives nothing typed; the delete takes the code alone (COTRTUPC-032).
        body: aid === "PF4" ? { aid } : { aid, description: sent.description },
      },
      {
        onSuccess: (result) => {
          show(afterAction(from, sent, result));
        },
        onError: (error) => {
          failed(from, sent, error);
        },
        onSettled: () => {
          setInFlight(false);
        },
      },
    );
  }

  /**
   * A zod refusal before any request. The search form repeats only COTRTUPC-005, whose screen is
   * the fresh one with `*` in red; the length checks cannot fail behind `maxLength`, and show
   * their message where they do.
   */
  function refusedHere(from: ScreenView, codeMessage?: string, descriptionMessage?: string): void {
    setInFlight(false);
    if (codeMessage !== undefined) {
      show(codeRefused(from, codeMessage));
    } else if (descriptionMessage !== undefined) {
      setView({ ...from, message: descriptionMessage });
    }
  }

  /**
   * ENTER where `0001-CHECK-PFKEYS` accepts it (COTRTUPC-012, -016 to -022, -035). The keyboard
   * locks while the form validates, so the request it may send cannot be sent twice.
   */
  function enter(): void {
    const from = view;
    switch (from.state) {
      case "fresh":
        setInFlight(true);
        void searchForm
          .handleSubmit(
            (values) => {
              fetchCode(from, values.typeCode);
            },
            (errors) => {
              refusedHere(from, errors.typeCode?.message);
            },
          )()
          .catch(unlockAndRethrow);
        return;
      case "X":
        setInFlight(true);
        void detailsForm
          .handleSubmit(
            (values) => {
              fetchCode(from, values.typeCode);
            },
            (errors) => {
              refusedHere(from, errors.typeCode?.message, errors.description?.message);
            },
          )()
          .catch(unlockAndRethrow);
        return;
      case "S":
      case "E":
      case "B":
      case "R":
        setInFlight(true);
        void detailsForm
          .handleSubmit(
            (values) => {
              actOn(from, "ENTER", { code: values.typeCode, description: values.description });
            },
            (errors) => {
              refusedHere(from, errors.typeCode?.message, errors.description?.message);
            },
          )()
          .catch(unlockAndRethrow);
        return;
      case "N":
        // No edit runs once changes are validated: the same screen again (COTRTUPC-022).
        show(nextScreen(from, "N"));
        return;
      case "8":
        // No `WHEN` of `2000-DECIDE-ACTION` serves state 8 (COTRTUPC-035, COTRTUPC-037).
        setAbended(true);
        return;
      default:
        // `cotrtupKeys` gives ENTER no own action in the other states.
        show(keyRefused(from));
    }
  }

  /** F4: the first press asks for confirmation, the second deletes (COTRTUPC-031, COTRTUPC-032). */
  function deleteKey(): void {
    const from = view;
    if (from.state === "S") {
      show(nextScreen(from, "9"));
    } else if (from.state === "9") {
      actOn(from, "PF4", from.old);
    } else {
      show(keyRefused(from));
    }
  }

  /**
   * F5: saves validated changes (COTRTUPC-022 to COTRTUPC-029); after a fetch that found nothing,
   * starts the add with the not-found code, nothing typed received (COTRTUPC-015); while a delete
   * is in progress, ends in the abend of COTRTUPC-035 to COTRTUPC-037.
   */
  function saveKey(): void {
    const from = view;
    switch (from.state) {
      case "N":
        actOn(from, "PF5", from.entered);
        return;
      case "X":
        show(nextScreen(from, "R", { entered: { code: from.entered.code, description: "" } }));
        return;
      case "9":
      case "8":
        setAbended(true);
        return;
      default:
        show(keyRefused(from));
    }
  }

  /**
   * F12 where it cancels rather than starting over, reading nothing (X-COTRTUPC-8): validated
   * changes are backed out (COTRTUPC-039), a pending delete is dropped (COTRTUPC-040).
   */
  function cancelKey(): void {
    const from = view;
    if (from.state === "N") {
      show(
        nextScreen(from, isBlank(from.old) ? "B-blank" : "B", {
          entered: BLANK,
          message: UPDATE_CANCELLED,
        }),
      );
    } else if (from.state === "9") {
      show(nextScreen(from, "fresh", { message: DELETE_CANCELLED }));
    } else {
      show(keyRefused(from));
    }
  }

  /** Runs `action` unless a request is in flight. */
  function unlessPending(action: () => void): () => void {
    return () => {
      if (!inFlight.current) {
        action();
      }
    };
  }

  const availability = cotrtupKeys(view.state);
  const handlers = bindPfKeys(availability, {
    own: {
      ENTER: unlessPending(enter),
      F3: unlessPending(() => {
        void navigate(callerPath(location.state));
      }),
      F4: unlessPending(deleteKey),
      F5: unlessPending(saveKey),
      F12: unlessPending(cancelKey),
    },
    // The fresh screen of COTRTUPC-003, as `INITIALIZE WS-THIS-PROGCOMMAREA` leaves it
    // (COTRTUPC-030, COTRTUPC-039, COTRTUPC-041).
    reset: unlessPending(() => {
      show(nextScreen(view, "fresh", { old: BLANK, entered: BLANK }));
    }),
    invalid: unlessPending(() => {
      show(keyRefused(view));
    }),
  });
  // `F6=Add` is never brightened and PF6 never accepted, so it gets no button (COTRTUP-036); like
  // every unlabelled key, it reaches `onInvalidKey` below.
  const keys = keyRowItemsFor(KEY_ROWS.COTRTUP, availability)
    .filter((item) => item.key !== "F6")
    .map((item) => ({
      key: item.key,
      label: item.label,
      dark: item.dark,
      hidden: !item.shown,
      onPress: handlers[item.key],
      // `unlessPending` refuses every key while a request is out, F3 included.
      disabled: pending,
    }));

  /** A submit the ENTER button did not intercept is still the screen's ENTER. */
  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    handlers.ENTER?.();
  }

  if (abended) {
    // The map is erased and the conversation ends; the next visit opens the fresh screen.
    return (
      <main
        aria-label={`${SCREENS.COTRTUP.transaction} ended abnormally`}
        className="p-4 font-mono text-sm text-foreground"
      >
        {ABEND_LINES.map((line, index) => (
          <p key={index} className="whitespace-pre">
            {line}
          </p>
        ))}
      </main>
    );
  }

  const shown = shownValues(view);

  return (
    <ScreenShell
      transaction={SCREENS.COTRTUP.transaction}
      program={SCREENS.COTRTUP.program}
      title={TITLE}
      formId={formId}
      message={{ text: view.message, kind: "error" }}
      info={centred(view.info)}
      // COTRTUPC-045: the info row is shown bright.
      infoBright
      keys={keys}
      keyOptions={{
        foldShifted: KEY_ROWS.COTRTUP.foldShifted,
        onInvalidKey: (key) => {
          handlers[key]?.();
        },
      }}
      busy={pending}
    >
      {searching ? (
        <Form {...searchForm}>
          <form key="search" id={formId} noValidate aria-busy={pending} onSubmit={submit}>
            <FieldGrid>
              <FormField
                control={searchForm.control}
                name="typeCode"
                render={({ field, fieldState }) => (
                  <MapInput
                    row={TYPE_CODE_ROW}
                    label={TYPE_CODE_LABEL}
                    length={COTRTUP_LENGTHS.typeCode}
                    input={field}
                    invalid={fieldState.invalid}
                    editable={CODE_EDITABLE.has(view.state)}
                    locked={pending}
                    mark={view.codeMark}
                    message={view.message}
                    autoFocus
                  />
                )}
              />
              <GridCell row={DESCRIPTION_ROW} col={LABEL_COLUMN} span={LABEL_LENGTH}>
                <Label htmlFor={descriptionId} className={LABEL_CLASS}>
                  {DESCRIPTION_LABEL}
                </Label>
              </GridCell>
              <GridCell
                row={DESCRIPTION_ROW}
                col={INPUT_COLUMN}
                span={COTRTUP_LENGTHS.description}
              >
                {/* Protected on the search form: the Tab key skips it, as on the terminal. */}
                <Input
                  id={descriptionId}
                  value={shown.description}
                  maxLength={COTRTUP_LENGTHS.description}
                  size={COTRTUP_LENGTHS.description}
                  autoComplete="off"
                  readOnly
                  tabIndex={-1}
                  className={FIELD_BOX_HEIGHT}
                />
              </GridCell>
            </FieldGrid>
          </form>
        </Form>
      ) : (
        <Form {...detailsForm}>
          <form key="details" id={formId} noValidate aria-busy={pending} onSubmit={submit}>
            <FieldGrid>
              <FormField
                control={detailsForm.control}
                name="typeCode"
                render={({ field, fieldState }) => (
                  <MapInput
                    row={TYPE_CODE_ROW}
                    label={TYPE_CODE_LABEL}
                    length={COTRTUP_LENGTHS.typeCode}
                    input={field}
                    invalid={fieldState.invalid}
                    editable={CODE_EDITABLE.has(view.state)}
                    locked={pending}
                    mark={view.codeMark}
                    message={view.message}
                    autoFocus
                  />
                )}
              />
              <FormField
                control={detailsForm.control}
                name="description"
                render={({ field, fieldState }) => (
                  <MapInput
                    row={DESCRIPTION_ROW}
                    label={DESCRIPTION_LABEL}
                    length={COTRTUP_LENGTHS.description}
                    input={field}
                    invalid={fieldState.invalid}
                    editable={DESCRIPTION_EDITABLE.has(view.state)}
                    locked={pending}
                    mark={view.descriptionMark}
                    message={view.message}
                  />
                )}
              />
            </FieldGrid>
          </form>
        </Form>
      )}
    </ScreenShell>
  );
}
