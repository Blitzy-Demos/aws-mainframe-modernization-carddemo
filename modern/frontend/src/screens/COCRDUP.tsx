import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldPath } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCocrdupcFetch, useCocrdupcSave } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell, fieldInputClass } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import {
  KEY_ROWS,
  PF_KEYS,
  bindPfKeys,
  cocrdupKeys,
  keyRowItemsFor,
  type CocrdupKeyState,
} from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import {
  COCRDUP_LENGTHS,
  cocrdupKeySchema,
  cocrdupSchema,
  type CocrdupKeyValues,
  type CocrdupValues,
} from "@/schemas/COCRDUP";

/**
 * Map COCRDUP (`CCRDUPA`, `app/bms/COCRDUP.bms`): the card update screen of transaction CCUP,
 * program COCRDUPC, route `/cards/update` (AAP 0.3.5). Rows `COCRDUP-001` to `COCRDUP-044` of
 * `modern/audit/screens.md` place its fields; `modern/audit/online/COCRDUPC.md` decides its states.
 *
 * - Endpoint 9 (`GET /api/v1/cards/edit`) fetches the card: by the typed account and card number
 *   on ENTER, or by the `card_ref` of a COCRDLI row on entry from the list (COCRDUPC-002,
 *   COCRDUPC-016). Endpoint 10 (`PUT /api/v1/cards/{card_ref}`) validates on ENTER and saves on
 *   F5 (COCRDUPC-020 to COCRDUPC-041). F3 and F12 send no request (COCRDUPC-004, COCRDUPC-006).
 * - The screen keeps `CCUP-CHANGE-ACTION` as a `CocrdupKeyState`, which decides the enterable
 *   fields (COCRDUPC-045) and the keys (`cocrdupKeys`), and echoes the snapshot of the last 200
 *   unchanged except for the state an error response leaves (COCRDUPC-044).
 * - The card number is shown only as endpoint 9 sends it, redacted, and the card is addressed by
 *   `card_ref` alone; no typed number is sent once a card is shown (COCRDUPC-042, COCRDUPC-043).
 *   Nothing here logs or stores a payload.
 * - While a request is out every key but F3 is refused, as the terminal keyboard locks until the
 *   program answers; F3 still exits. The lock is taken when a key that may send is accepted,
 *   before the forms' checks, and opens when a check refuses the input or the last request of the
 *   key settles, the fetch that follows a 409 included. While it holds, every key but `F3=Exit` is
 *   disabled, the key row and the form are busy and every input is read-only.
 */

/** The map's screen title, `POS=(4,30)` (COCRDUP-011). */
const TITLE = "Update Credit Card Details";

/** One `GridCell` placement: the field's `POS` row and column and its `LENGTH`. */
type CellSpec = { readonly row: number; readonly col: number; readonly span: number };

/** A map input with the literal that labels it, both as the map places them. */
type MapFieldSpec = {
  readonly label: string;
  readonly labelCell: CellSpec;
  readonly inputCell: CellSpec;
  /**
   * Accessible name where the map's label alone does not tell the field apart: `EXPMON` and
   * `EXPYEAR` share the `Expiry Date` caption, and the year's own literal is `/`.
   */
  readonly accessibleName?: string;
  /**
   * Utilities the literal's box needs beside `LABEL`: the `/` of `EXPYEAR` has a one-column cell,
   * narrower than its glyph at every width, so its box is as wide as the glyph (`min-w-max`). The
   * shared map-field box of `EXPMON` grows past its two-column cell to its two characters and, on
   * the narrowest frame, ends within half a pixel of the `/` cell; the `pl-1` inset keeps the
   * glyph clear of that box, between the month and year boxes as the map's attribute columns do.
   */
  readonly labelClass?: string;
};

/**
 * The six enterable fields with their labels, at the map's `POS` and `LENGTH` (COCRDUP-012 to
 * COCRDUP-027). The label texts are the map's `INITIAL`s, trailing spaces included. `EXPDAY`
 * (COCRDUP-029) is dark and protected and is not rendered: the day travels in the snapshot.
 */
const FIELDS = {
  acctId: {
    label: "Account Number    :",
    labelCell: { row: 7, col: 23, span: 19 },
    inputCell: { row: 7, col: 45, span: COCRDUP_LENGTHS.acctId },
  },
  cardNum: {
    label: "Card Number       :",
    labelCell: { row: 8, col: 23, span: 19 },
    inputCell: { row: 8, col: 45, span: COCRDUP_LENGTHS.cardNum },
  },
  embossedName: {
    label: "Name on card      :",
    labelCell: { row: 11, col: 4, span: 20 },
    inputCell: { row: 11, col: 25, span: COCRDUP_LENGTHS.embossedName },
  },
  activeStatus: {
    label: "Card Active Y/N   : ",
    labelCell: { row: 13, col: 4, span: 20 },
    inputCell: { row: 13, col: 25, span: COCRDUP_LENGTHS.activeStatus },
  },
  expiryMonth: {
    label: "Expiry Date       : ",
    labelCell: { row: 15, col: 4, span: 20 },
    inputCell: { row: 15, col: 25, span: COCRDUP_LENGTHS.expiryMonth },
    accessibleName: "Expiry Date month",
  },
  expiryYear: {
    label: "/",
    labelCell: { row: 15, col: 28, span: 1 },
    inputCell: { row: 15, col: 30, span: COCRDUP_LENGTHS.expiryYear },
    accessibleName: "Expiry Date year",
    labelClass: "min-w-max pl-1",
  },
} as const satisfies Record<FieldPath<CocrdupValues>, MapFieldSpec>;

/** `ACCTSID` and `CARDSID`, the search keys of endpoint 9. */
const KEY_FIELDS = ["acctId", "cardNum"] as const satisfies readonly FieldPath<CocrdupKeyValues>[];

/** `CRDNAME`, `CRDSTCD`, `EXPMON` and `EXPYEAR`, the fields endpoint 10 edits. */
const DETAIL_FIELDS = [
  "embossedName",
  "activeStatus",
  "expiryMonth",
  "expiryYear",
] as const satisfies readonly FieldPath<CocrdupValues>[];

/** Map order, in which a client-side length message names its field. */
const FIELD_ORDER = [...KEY_FIELDS, ...DETAIL_FIELDS];

/** The screen with nothing fetched: every field empty (COCRDUPC-001). */
const BLANK_VALUES: CocrdupValues = {
  acctId: "",
  cardNum: "",
  embossedName: "",
  activeStatus: "",
  expiryMonth: "",
  expiryYear: "",
};

/**
 * TURQUOISE map literals snap to the foreground token (AAP 0.4.3). `h-full` centres each label on
 * its line beside the inputs; `whitespace-pre` keeps their spacing.
 */
const LABEL = "h-full whitespace-pre text-foreground";

/** `KEY_ROWS.COCRDUP.foldShifted`: COCRDUPC folds PF13–PF24 onto PF1–PF12 (`CSSTRPFY`). */
const KEY_OPTIONS = { foldShifted: KEY_ROWS.COCRDUP.foldShifted };

/**
 * `3250-SETUP-INFOMSG` (`app/cbl/COCRDUPC.cbl:1138-1164`): the info line of each state. The screen
 * shows the server's `info_message` after a 200 and this table where no response carries one: on
 * first entry, after F12, after a restart and after an error response (COCRDUPC-001, -005, -006).
 */
const INFO: Readonly<Record<CocrdupKeyState, string>> = {
  "not-fetched": "Please enter Account and Card Number",
  S: "Details of selected card shown above",
  E: "Update card details presented above.",
  N: "Changes validated.Press F5 to save",
  C: "Changes committed to database",
  L: "Changes unsuccessful. Please try again",
  F: "Changes unsuccessful. Please try again",
};

/*
 * COCRDUPC messages whose arrival sets the screen's state. A `ScreenError` carries only the
 * message and the field, not the HTTP status, so the screen derives the state the audit row names
 * from the message itself (`COCRDUPC.md`, "Reading the table").
 */
/** COCRDUPC-020 to COCRDUPC-022: the echoed state stands. */
const NO_CHANGES_DETECTED = "No change detected with respect to values fetched.";
/** COCRDUPC-003, COCRDUPC-018: no card for the key. */
const DID_NOT_FIND_ACCTCARD_COMBO = "Did not find cards for this search condition";
/** COCRDUPC-019: the start of `WS-FILE-ERROR-MESSAGE`, whose code fields vary (H4). */
const FILE_ERROR_PREFIX = "File Error:";
/** COCRDUPC-037: 409, the card changed since it was fetched. */
const DATA_WAS_CHANGED_BEFORE_UPDATE = "Record changed by some one else. Please review";
/** COCRDUPC-036: state `L`. */
const COULD_NOT_LOCK_FOR_UPDATE = "Could not lock record for update";
/** COCRDUPC-040: state `F`. */
const LOCKED_BUT_UPDATE_FAILED = "Update of record failed";

/** Wire state → screen state; the screen names the codes as `cocrdupKeys` does. */
const STATE_OF_WIRE: Readonly<Record<components["schemas"]["CardUpdateState"], CocrdupKeyState>> = {
  not_fetched: "not-fetched",
  show_details: "S",
  changes_not_ok: "E",
  changes_ok_not_confirmed: "N",
  changes_okayed_and_done: "C",
  lock_error: "L",
  update_failed: "F",
};

/** Screen state → wire state, for the snapshot echoed after an error response. */
const WIRE_OF_STATE: Readonly<Record<CocrdupKeyState, components["schemas"]["CardUpdateState"]>> = {
  "not-fetched": "not_fetched",
  S: "show_details",
  E: "changes_not_ok",
  N: "changes_ok_not_confirmed",
  C: "changes_okayed_and_done",
  L: "lock_error",
  F: "update_failed",
};

/** `ScreenError.field` → form field. The DTO's own length rejections name `fields.<member>`. */
const FORM_FIELD_OF_WIRE: Readonly<Record<string, FieldPath<CocrdupValues>>> = {
  acct_id: "acctId",
  card_num: "cardNum",
  embossed_name: "embossedName",
  status: "activeStatus",
  expiry_month: "expiryMonth",
  expiry_year: "expiryYear",
};

/** The fields COCRDUPC's detail edits name; a failing edit leaves state `E` (COCRDUPC-023). */
const DETAIL_WIRE_FIELDS: ReadonlySet<string> = new Set([
  "embossed_name",
  "status",
  "expiry_month",
  "expiry_year",
]);

/** States in which `ACCTSID` and `CARDSID` are enterable (COCRDUPC-045). */
const KEYS_ENTERABLE: ReadonlySet<CocrdupKeyState> = new Set(["not-fetched", "L", "F"]);

/** States in which the name, status, month and year are enterable (COCRDUPC-045). */
const DETAILS_ENTERABLE: ReadonlySet<CocrdupKeyState> = new Set(["S", "E"]);

/** Every screen path; a route-state `from` outside it is no caller. */
const KNOWN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/** What the transferring screen put in the route state (COCRDLI: `from`, `acct_id`, `card_ref`). */
type RouteEntry = {
  /** The caller's path, kept only when it is a screen path. */
  readonly from: string | undefined;
  /** The card a COCRDLI `U` selection names, only when COCRDLI is the caller. */
  readonly selection: ListKeys | undefined;
};

/**
 * Reads the route state; any other shape is a first entry with no caller recorded. The account
 * and `card_ref` are a selection only when the card list is the caller, as COCRDUPC fetches on
 * entry only `WHEN CDEMO-PGM-ENTER AND CDEMO-FROM-PROGRAM EQUAL LIT-CCLISTPGM`
 * (`app/cbl/COCRDUPC.cbl:482-483`, COCRDUPC-002). The main menu forwards the route state it
 * received, where an earlier screen may have left an account and a card, so from the menu, from
 * any other caller or from none the screen opens empty (COCRDUPC-001). An account longer than the
 * map's `ACCTSID` or a `card_ref` that is not a safe integer is no selection either.
 */
function routeEntryOf(state: unknown): RouteEntry {
  if (typeof state !== "object" || state === null) {
    return { from: undefined, selection: undefined };
  }
  const named = "from" in state && typeof state.from === "string" ? state.from : undefined;
  const from = named !== undefined && KNOWN_PATHS.has(named) ? named : undefined;
  if (from !== PATHS.COCRDLI) {
    return { from, selection: undefined };
  }
  const acctId = "acct_id" in state ? state.acct_id : undefined;
  const cardRef = "card_ref" in state ? state.card_ref : undefined;
  if (
    typeof acctId !== "string" ||
    acctId.length > COCRDUP_LENGTHS.acctId ||
    typeof cardRef !== "number" ||
    !Number.isSafeInteger(cardRef)
  ) {
    return { from, selection: undefined };
  }
  return { from, selection: { acctId, cardRef } };
}

/** The card endpoint 9 last showed: its handle, its values as shown (F12) and its snapshot. */
type FetchedCard = {
  readonly cardRef: number;
  readonly values: CocrdupValues;
  readonly snapshot: components["schemas"]["CardUpdateSnapshot"];
};

/** What the screen shows and what its next endpoint 10 request echoes. */
type ScreenModel = {
  /** `CCUP-CHANGE-ACTION`. */
  readonly state: CocrdupKeyState;
  readonly card: FetchedCard | undefined;
  /** The last 200's snapshot, its `state` replaced by the one an error response left. */
  readonly snapshot: components["schemas"]["CardUpdateSnapshot"] | undefined;
  /** The `ERRMSG` row. */
  readonly message: string | undefined;
  /** The `INFOMSG` row. */
  readonly info: string;
  /** The `save_enabled` of the last result: `F5=Save F12=Cancel` shown (COCRDUP-034). */
  readonly saveEnabled: boolean;
};

const INITIAL_MODEL: ScreenModel = {
  state: "not-fetched",
  card: undefined,
  snapshot: undefined,
  message: undefined,
  info: INFO["not-fetched"],
  saveEnabled: false,
};

/** The list-entry keys of endpoint 9: the account and `card_ref` a COCRDLI row carries. */
type ListKeys = { readonly acctId: string; readonly cardRef: number };

/**
 * COCRDUPC-017: the month and year `EXPMON` and `EXPYEAR` show, positions 6–7 and 1–4 of the
 * typed date; the day stays in the snapshot. A blank stored date shows both empty.
 */
function expiryParts(date: string | null | undefined): { month: string; year: string } {
  return date === null || date === undefined
    ? { month: "", year: "" }
    : { month: date.slice(5, 7), year: date.slice(0, 4) };
}

/**
 * COCRDUPC-033: `JUSTIFY=(RIGHT)` names no fill, so BMS right-justifies a shorter entry and fills
 * it with `0` on the left (`3` → `03`); an entry left empty is sent empty.
 */
function zeroFilled(text: string, length: number): string {
  return text === "" ? text : text.padStart(length, "0");
}

/** The form field a `ScreenError.field` names, if any. */
function formFieldOf(field: string | null | undefined): FieldPath<CocrdupValues> | undefined {
  if (field === null || field === undefined) {
    return undefined;
  }
  const member = field.startsWith("fields.") ? field.slice("fields.".length) : field;
  return Object.hasOwn(FORM_FIELD_OF_WIRE, member) ? FORM_FIELD_OF_WIRE[member] : undefined;
}

/** Whether `name` is one of the two search keys. */
function isKeyField(name: FieldPath<CocrdupValues>): name is (typeof KEY_FIELDS)[number] {
  return name === "acctId" || name === "cardNum";
}

/** COCRDUPC-003, COCRDUPC-019: the card read of the list entry found nothing or failed. */
function cardReadFailed(message: string): boolean {
  return message === DID_NOT_FIND_ACCTCARD_COMBO || message.startsWith(FILE_ERROR_PREFIX);
}

/**
 * The state an endpoint 10 error leaves, from the state the request echoed: `L` and `F` for the
 * store failures (COCRDUPC-036, COCRDUPC-040); the echoed state for no change, except that a PF5
 * from `N` answers as ENTER from `S` (COCRDUPC-020, COCRDUPC-021, COCRDUPC-041); `E` for a failing
 * detail edit (COCRDUPC-023 to COCRDUPC-032). Any other failure, such as a lost connection or an
 * expired token, leaves the state as it was.
 */
function stateAfterSaveError(
  message: string,
  field: string | null | undefined,
  echoed: CocrdupKeyState,
): CocrdupKeyState {
  if (message === COULD_NOT_LOCK_FOR_UPDATE) {
    return "L";
  }
  if (message === LOCKED_BUT_UPDATE_FAILED) {
    return "F";
  }
  if (message === NO_CHANGES_DETECTED) {
    return echoed === "N" ? "S" : echoed;
  }
  if (field !== null && field !== undefined && DETAIL_WIRE_FIELDS.has(field)) {
    return "E";
  }
  return echoed;
}

/**
 * The `Input` attributes of one map field: its `LENGTH`, protection and accessible name. A field
 * the state protects (`enterable` false) is read-only and out of the Tab order, as the 3270 Tab
 * key skips protected fields; it still takes the cursor the program places on it. While `busy`, a
 * request holds the keyboard: every field is read-only, and an enterable one keeps its Tab stop.
 * Every field takes the shared map-field box (`fieldInputClass` with `size` = the `LENGTH`), so
 * each one, the read-only redacted card number included, shows a full-length value on the
 * narrowest frame.
 */
function inputAttributes(name: FieldPath<CocrdupValues>, enterable: boolean, busy: boolean) {
  const spec: MapFieldSpec = FIELDS[name];
  const length = COCRDUP_LENGTHS[name];
  return {
    maxLength: length,
    readOnly: !enterable || busy,
    tabIndex: enterable ? undefined : -1,
    autoComplete: "off",
    spellCheck: false,
    "aria-label": spec.accessibleName,
    size: length,
    className: fieldInputClass(length),
  };
}

/**
 * One map field as two grid cells: its literal as the `FormLabel` and its `Input` in the
 * `FormControl`. `FormItem` is `contents`, so both cells sit on the `FieldGrid` itself. The
 * field's error is its `FormMessage`, which the input's `aria-describedby` names while the field
 * is invalid. The `ERRMSG` row is the map's one visible message, so the `FormMessage` is for
 * assistive technology only: `sr-only` takes it out of the grid flow, and it fills no cell.
 */
function FieldCells({ spec, children }: { spec: MapFieldSpec; children: ReactNode }): ReactNode {
  const labelClass = spec.labelClass === undefined ? LABEL : `${LABEL} ${spec.labelClass}`;
  return (
    <FormItem className="contents">
      <GridCell {...spec.labelCell}>
        <FormLabel className={labelClass}>{spec.label}</FormLabel>
      </GridCell>
      <GridCell {...spec.inputCell}>
        <FormControl>{children}</FormControl>
      </GridCell>
      <FormMessage className="sr-only" />
    </FormItem>
  );
}

/**
 * The card update screen. Entered from COCRDLI with the row's `acct_id` and `card_ref`, it fetches
 * that card at once (COCRDUPC-002); entered from the menu, from any other caller or with none, it
 * sends nothing and waits for the account and card, whatever account or card the route state
 * still carries (COCRDUPC-001).
 */
export function COCRDUP(): ReactNode {
  const navigate = useNavigate();
  const entry = routeEntryOf(useLocation().state);
  const formId = useId();
  const formRef = useRef<HTMLFormElement>(null);
  const fetchCard = useCocrdupcFetch();
  const saveCard = useCocrdupcSave();
  const [model, setModel] = useState<ScreenModel>(INITIAL_MODEL);
  // `inFlight` is the lock the key handlers read at once; `busy` renders it while a request is
  // out. Only the settlement of the latest request, numbered by `latestRequest`, opens it.
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);
  const latestRequest = useRef(0);
  /** The field the cursor goes to once the next render has mounted it (COCRDUPC-046). */
  const pendingFocus = useRef<FieldPath<CocrdupValues> | undefined>(undefined);
  const entered = useRef(false);

  const keyForm = useForm<CocrdupKeyValues>({
    resolver: zodResolver(cocrdupKeySchema),
    defaultValues: { acctId: entry.selection?.acctId ?? "", cardNum: "" },
  });
  const detailForm = useForm<CocrdupValues>({
    resolver: zodResolver(cocrdupSchema),
    defaultValues: BLANK_VALUES,
  });

  const availability = cocrdupKeys(model.state);
  const keysOnKeyForm = model.state === "not-fetched";
  // COCRDUPC-004: back to the screen that transferred here, else the main menu.
  const exitPath = entry.from ?? PATHS.COMEN01;

  useEffect(() => {
    const name = pendingFocus.current;
    if (name === undefined) {
      return;
    }
    pendingFocus.current = undefined;
    if (keysOnKeyForm && isKeyField(name)) {
      keyForm.setFocus(name);
    } else {
      detailForm.setFocus(name);
    }
  });

  /** Takes the key lock for a key that may send a request. */
  function lockKeys() {
    inFlight.current = true;
    setBusy(true);
  }

  /** Opens the key lock: the last request has settled, or the key that took it sends nothing. */
  function unlockKeys() {
    inFlight.current = false;
    setBusy(false);
  }

  /**
   * Holds the key lock until `request` settles, whatever its outcome, after its outcome is shown.
   * A request sent while that outcome is shown, the fetch after a 409, holds it in turn.
   */
  function holdUntilSettled(request: Promise<unknown>) {
    lockKeys();
    latestRequest.current += 1;
    const number = latestRequest.current;
    const settled = () => {
      if (latestRequest.current === number) {
        unlockKeys();
      }
    };
    void request.then(settled, settled);
  }

  /**
   * Runs a form check the key lock was taken for. A check that fails unexpectedly opens the lock
   * again, as nothing was sent, and still reports its error.
   */
  function checkLocked(check: Promise<void>) {
    void check.catch((error: unknown) => {
      unlockKeys();
      throw error;
    });
  }

  /**
   * Marks the field a server message names: red, `*` when blank, cursor there (COCRDUPC-046).
   * The field error carries the message the error row shows, so the field's `FormMessage` reads
   * it with the input. `flag` is false for the no-change answer, which only moves the cursor to
   * the name. Without a field the cursor goes to the account where it is enterable (`WHEN OTHER`).
   * Only the one field a `ScreenError` names is marked here; an endpoint 9 not-found marks the
   * card beside it (`markCardNotFound`).
   */
  function markField(
    error: components["schemas"]["ScreenError"],
    state: CocrdupKeyState,
    flag: boolean,
  ) {
    const name = formFieldOf(error.field);
    if (name === undefined) {
      pendingFocus.current = KEYS_ENTERABLE.has(state) ? "acctId" : undefined;
      return;
    }
    pendingFocus.current = name;
    if (!flag) {
      return;
    }
    if (state === "not-fetched" && isKeyField(name)) {
      if (keyForm.getValues(name).trim() === "") {
        keyForm.setValue(name, "*");
      }
      keyForm.setError(name, { type: "server", message: error.message });
    } else {
      if (detailForm.getValues(name).trim() === "") {
        detailForm.setValue(name, "*");
      }
      detailForm.setError(name, { type: "server", message: error.message });
    }
  }

  /**
   * A 200 of endpoint 9: the card in state `S` with the name focused (COCRDUPC-016, COCRDUPC-017).
   * The name and status are shown without their trailing spaces, which the server's fixed-width
   * fields restore; the snapshot is kept byte for byte. `message` keeps a 409's text over the
   * refreshed values (COCRDUPC-037).
   */
  function showFetched(view: components["schemas"]["CardUpdateView"], message?: string) {
    const { month, year } = expiryParts(view.expiration_date);
    const values: CocrdupValues = {
      acctId: view.acct_id,
      cardNum: view.card_num,
      embossedName: view.embossed_name.trimEnd(),
      activeStatus: view.status.trimEnd(),
      expiryMonth: month,
      expiryYear: year,
    };
    detailForm.reset(values);
    const card: FetchedCard = { cardRef: view.card_ref, values, snapshot: view.snapshot };
    setModel({
      state: STATE_OF_WIRE[view.snapshot.state],
      card,
      snapshot: view.snapshot,
      message: message ?? view.message ?? undefined,
      info: view.info_message ?? INFO.S,
      saveEnabled: false,
    });
    pendingFocus.current = "embossedName";
  }

  /**
   * COCRDUPC-003, COCRDUPC-018: on `NOTFND`, `9100-GETCARD-BYACCTCARD` sets
   * `FLG-CARDFILTER-NOT-OK` beside `FLG-ACCTFILTER-NOT-OK`, so the card is marked in error with
   * the account the `ScreenError` names, its field error carrying the same message. The not-found
   * answer is told apart by its message, `DID-NOT-FIND-ACCTCARD-COMBO`, which COCRDUPC moves only
   * on `NOTFND`, as the screen tells its other answers apart by theirs; the file error of
   * COCRDUPC-019 names the account too but marks it alone.
   * NOT-OK is red only: the `*` goes into a blank filter, never here, so the card keeps its text
   * (blank on the list entry, which knows only the `card_ref`), and the cursor stays where
   * `markField` put it. `state` picks the form that shows the keys, as in `markField`.
   */
  function markCardNotFound(error: components["schemas"]["ScreenError"], state: CocrdupKeyState) {
    if (error.message !== DID_NOT_FIND_ACCTCARD_COMBO) {
      return;
    }
    const cardError = { type: "server", message: error.message };
    if (state === "not-fetched") {
      keyForm.setError("cardNum", cardError);
    } else {
      detailForm.setError("cardNum", cardError);
    }
  }

  /**
   * An endpoint 9 error. On the list entry and the 409 refresh, a card that is not found or
   * cannot be read still leaves state `S`, with the details empty and enterable over a blank
   * snapshot (COCRDUPC-003, COCRDUPC-019). Every other error leaves `otherwise`: not fetched for
   * a search (COCRDUPC-015, COCRDUPC-018). Either way the error marks the field it names, and a
   * card that is not found marks the card as well (`markCardNotFound`).
   */
  function fetchFailed(
    error: components["schemas"]["ScreenError"],
    listKeys: ListKeys | undefined,
    otherwise: CocrdupKeyState,
  ) {
    const message = error.message;
    if (listKeys !== undefined && cardReadFailed(message)) {
      const values: CocrdupValues = { ...BLANK_VALUES, acctId: listKeys.acctId };
      const snapshot: components["schemas"]["CardUpdateSnapshot"] = {
        state: "show_details",
        acct_id: listKeys.acctId,
        embossed_name: "",
        expiration_date: null,
        status: "",
      };
      detailForm.reset(values);
      setModel({
        state: "S",
        card: { cardRef: listKeys.cardRef, values, snapshot },
        snapshot,
        message,
        info: INFO.S,
        saveEnabled: false,
      });
      markField(error, "S", true);
      markCardNotFound(error, "S");
      return;
    }
    setModel((current) => ({
      ...current,
      state: otherwise,
      message,
      info: INFO[otherwise],
      saveEnabled: false,
    }));
    markField(error, otherwise, true);
    markCardNotFound(error, otherwise);
  }

  /** Endpoint 9 by `card_ref`, as the list entry and the 409 refresh call it (COCRDUPC-002). */
  function fetchByRef(
    listKeys: ListKeys,
    keptMessage: string | undefined,
    otherwise: CocrdupKeyState,
  ) {
    holdUntilSettled(
      fetchCard.mutateAsync(
        { query: { acct_id: listKeys.acctId, card_ref: listKeys.cardRef } },
        {
          onSuccess: (view) => {
            showFetched(view, keptMessage);
          },
          onError: (error) => {
            fetchFailed(error, listKeys, otherwise);
          },
        },
      ),
    );
  }

  /** ENTER before a fetch: endpoint 9 with the keys as typed, never a `card_ref` (COCRDUPC-016). */
  function search(keys: CocrdupKeyValues) {
    holdUntilSettled(
      fetchCard.mutateAsync(
        { query: { acct_id: keys.acctId, card_num: keys.cardNum } },
        {
          onSuccess: (view) => {
            showFetched(view);
          },
          onError: (error) => {
            fetchFailed(error, undefined, "not-fetched");
          },
        },
      ),
    );
  }

  /**
   * A 200 of endpoint 10. The fields keep the text sent, zero fill included; `not_fetched` is
   * the server's restart, the empty search screen (COCRDUPC-034, COCRDUPC-035, COCRDUPC-039).
   * Every other 200 stays on the card with the cursor on the account, read-only as it is there:
   * its info line is the confirmation prompt or the commit, no error and no failed edit, so
   * `POSITION CURSOR` takes `WHEN OTHER` (COCRDUPC-046).
   */
  function saved(
    result: components["schemas"]["CardUpdateResult"],
    fields: components["schemas"]["CardUpdateFields"],
    card: FetchedCard,
    echoed: components["schemas"]["CardUpdateSnapshot"],
  ) {
    const state = STATE_OF_WIRE[result.state];
    if (state === "not-fetched") {
      showSearch(result.info_message);
      return;
    }
    detailForm.reset({
      ...detailForm.getValues(),
      embossedName: fields.embossed_name,
      activeStatus: fields.status,
      expiryMonth: fields.expiry_month,
      expiryYear: fields.expiry_year,
    });
    setModel({
      state,
      card,
      snapshot: result.snapshot ?? { ...echoed, state: result.state },
      message: undefined,
      info: result.info_message,
      saveEnabled: result.save_enabled,
    });
    pendingFocus.current = "acctId";
  }

  /**
   * An endpoint 10 error. A 409 shows state `S` and takes the card's current values from
   * endpoint 9 by the same `card_ref`, keeping the 409's message (COCRDUPC-037); every other
   * error sets the state its message names and echoes it in the snapshot (COCRDUPC-044).
   */
  function saveFailed(
    error: components["schemas"]["ScreenError"],
    card: FetchedCard,
    echoed: components["schemas"]["CardUpdateSnapshot"],
  ) {
    const message = error.message;
    if (message === DATA_WAS_CHANGED_BEFORE_UPDATE) {
      setModel({
        state: "S",
        card,
        snapshot: card.snapshot,
        message,
        info: INFO.S,
        saveEnabled: false,
      });
      fetchByRef({ acctId: echoed.acct_id, cardRef: card.cardRef }, message, "S");
      return;
    }
    const state = stateAfterSaveError(message, error.field, STATE_OF_WIRE[echoed.state]);
    setModel({
      state,
      card,
      snapshot: { ...echoed, state: WIRE_OF_STATE[state] },
      message,
      info: INFO[state],
      saveEnabled: false,
    });
    markField(error, state, message !== NO_CHANGES_DETECTED);
  }

  /**
   * ENTER (validate) or F5 (save) with the card shown: endpoint 10 by `card_ref`. With no card
   * shown nothing is sent, so the key lock opens again.
   */
  function sendChanges(aid: components["schemas"]["CocrdupcAid"], values: CocrdupValues) {
    const { card, snapshot } = model;
    if (card === undefined || snapshot === undefined) {
      unlockKeys();
      return;
    }
    const fields: components["schemas"]["CardUpdateFields"] = {
      embossed_name: values.embossedName,
      status: values.activeStatus,
      expiry_month: zeroFilled(values.expiryMonth, COCRDUP_LENGTHS.expiryMonth),
      expiry_year: zeroFilled(values.expiryYear, COCRDUP_LENGTHS.expiryYear),
    };
    holdUntilSettled(
      saveCard.mutateAsync(
        { path: { card_ref: card.cardRef }, body: { aid, fields, snapshot } },
        {
          onSuccess: (result) => {
            saved(result, fields, card, snapshot);
          },
          onError: (error) => {
            saveFailed(error, card, snapshot);
          },
        },
      ),
    );
  }

  /**
   * A field over its map `LENGTH`: zod's message in the error row, cursor on the field. Nothing
   * is sent, so the key lock opens again.
   */
  function showInvalid(errors: Partial<Record<FieldPath<CocrdupValues>, { message?: string }>>) {
    unlockKeys();
    const name = FIELD_ORDER.find((key) => errors[key]?.message !== undefined);
    if (name === undefined) {
      return;
    }
    const message = errors[name]?.message;
    setModel((current) => ({ ...current, message }));
    pendingFocus.current = name;
  }

  /** The empty search screen of COCRDUPC-001, with `info` as its info line. */
  function showSearch(info: string) {
    keyForm.reset({ acctId: "", cardNum: "" });
    detailForm.reset(BLANK_VALUES);
    setModel({ ...INITIAL_MODEL, info });
    pendingFocus.current = "acctId";
  }

  // COCRDUPC-002: entered from the list, the row's card is fetched once, before any key. Only a
  // COCRDLI selection fetches; any other entry sends nothing (COCRDUPC-001).
  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    if (entry.selection !== undefined) {
      fetchByRef(entry.selection, undefined, "not-fetched");
    }
  });

  /**
   * COCRDUPC-005: after a save or a failed save, a key other than F3 returns to COCRDLI when that
   * screen transferred here, and otherwise starts over on the empty search screen.
   */
  // `CDEMO-LAST-MAPSET` has no route-state counterpart (X-COCRDUPC-10), so the caller recorded in
  // `from` stands for it: entered from the menu, the next key always starts over.
  function restart() {
    if (inFlight.current) {
      return;
    }
    if (entry.from === PATHS.COCRDLI) {
      navigate(PATHS.COCRDLI);
      return;
    }
    showSearch(INFO["not-fetched"]);
  }

  /** F3: no request; pending edits are discarded (COCRDUPC-004). */
  function exit() {
    navigate(exitPath);
  }

  /** F12 in `S`, `E` or `N`: the last endpoint 9 values again, state `S` (COCRDUPC-006). */
  function cancel() {
    const { card } = model;
    if (inFlight.current || card === undefined) {
      return;
    }
    detailForm.reset(card.values);
    setModel({
      state: "S",
      card,
      snapshot: card.snapshot,
      message: undefined,
      info: INFO.S,
      saveEnabled: false,
    });
    pendingFocus.current = "embossedName";
  }

  /** F5 in `N`: endpoint 10 with `aid` PF5 (COCRDUPC-036 to COCRDUPC-041). */
  function confirmSave() {
    if (inFlight.current) {
      return;
    }
    lockKeys();
    checkLocked(
      detailForm.handleSubmit((values) => {
        sendChanges("PF5", values);
      }, showInvalid)(),
    );
  }

  /** ENTER, and every key COCRDUPC takes as ENTER: the form's submit (COCRDUPC-008). */
  function submitForm() {
    formRef.current?.requestSubmit();
  }

  /**
   * ENTER: search before a fetch, validate after it; in `C`, `L` and `F` the restart. The key lock
   * is taken before the asynchronous checks, so a key that arrives during them or while the
   * request is out sends nothing.
   */
  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (inFlight.current) {
      return;
    }
    if (availability.roles.ENTER === "reset") {
      restart();
      return;
    }
    lockKeys();
    if (keysOnKeyForm) {
      checkLocked(keyForm.handleSubmit(search, showInvalid)());
      return;
    }
    checkLocked(
      detailForm.handleSubmit((values) => {
        sendChanges("ENTER", values);
      }, showInvalid)(),
    );
  }

  const handlers = bindPfKeys(availability, {
    own: { ENTER: submitForm, F3: exit, F5: confirmSave, F12: cancel },
    reset: restart,
  });
  const items = keyRowItemsFor(KEY_ROWS.COCRDUP, availability);
  const labelled = new Set(items.map((item) => item.key));
  // While a request is out only F3 stays enabled, the hidden keys included.
  const keys: PfKeyBarEntry[] = [
    // ENTER keeps no action of its own: its button and the Enter key submit the form natively.
    ...items.map((item) => ({
      ...item,
      hidden: item.key === "F5" || item.key === "F12" ? !model.saveEnabled : !item.shown,
      onPress: item.key === "ENTER" ? undefined : handlers[item.key],
      disabled: busy && item.key !== "F3",
    })),
    // COCRDUPC-008: the keys the map labels nowhere (F1, F2, F4, F6 to F11) still reach the
    // program, which processes them as ENTER; they get hidden entries so `PfKeyBar` binds them.
    ...PF_KEYS.filter((key) => !labelled.has(key)).map((key) => ({
      key,
      label: key,
      dark: true,
      hidden: true,
      onPress: handlers[key],
      disabled: busy,
    })),
  ];

  const keysEnterable = KEYS_ENTERABLE.has(model.state);
  const detailsEnterable = DETAILS_ENTERABLE.has(model.state);

  return (
    <ScreenShell
      transaction={SCREENS.COCRDUP.transaction}
      program={SCREENS.COCRDUP.program}
      title={TITLE}
      formId={formId}
      message={{ kind: "error", text: model.message }}
      info={model.info}
      // COCRDUPC-046: the info line is always shown, bright.
      infoBright
      keys={keys}
      keyOptions={KEY_OPTIONS}
      busy={busy}
    >
      <form
        id={formId}
        ref={formRef}
        noValidate
        aria-label={TITLE}
        aria-busy={busy}
        onSubmit={onSubmit}
      >
        <FieldGrid>
          {keysOnKeyForm && (
            <Form {...keyForm}>
              {KEY_FIELDS.map((name) => (
                <FormField
                  key={`search-${name}`}
                  control={keyForm.control}
                  name={name}
                  render={({ field }) => (
                    <FieldCells spec={FIELDS[name]}>
                      {/* `IC` on ACCTSID: the cursor starts on the account (COCRDUP-013). */}
                      <Input
                        {...field}
                        {...inputAttributes(name, true, busy)}
                        autoFocus={name === "acctId"}
                      />
                    </FieldCells>
                  )}
                />
              ))}
            </Form>
          )}
          <Form {...detailForm}>
            {!keysOnKeyForm &&
              KEY_FIELDS.map((name) => (
                <FormField
                  key={`shown-${name}`}
                  control={detailForm.control}
                  name={name}
                  render={({ field }) => (
                    <FieldCells spec={FIELDS[name]}>
                      <Input {...field} {...inputAttributes(name, keysEnterable, busy)} />
                    </FieldCells>
                  )}
                />
              ))}
            {DETAIL_FIELDS.map((name) => (
              <FormField
                key={name}
                control={detailForm.control}
                name={name}
                render={({ field }) => (
                  <FieldCells spec={FIELDS[name]}>
                    <Input {...field} {...inputAttributes(name, detailsEnterable, busy)} />
                  </FieldCells>
                )}
              />
            ))}
          </Form>
        </FieldGrid>
      </form>
    </ScreenShell>
  );
}
