import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldPath } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCocrdupcFetch, useCocrdupcSave } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
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
 * TURQUOISE map literals snap to the foreground token (AAP 0.4.3); `whitespace-pre` keeps their
 * spacing.
 */
const LABEL = "whitespace-pre text-foreground";

/*
 * BLITZY [LAYOUT]: `Input` pads `px-3` and fills its cell. A grid column is about 0.44rem at the
 * frame's widest, so `CRDSTCD`, `EXPMON` and `EXPYEAR` (1, 2 and 4 columns) would leave no room
 * for their characters. Those fields size to their `LENGTH` in characters (`size`) with the `px-1`
 * spacing step instead; the extra width falls into the empty attribute column to their right.
 */
const NARROW_FIELD = 4;
const NARROW_INPUT = "w-auto px-1";

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
  /** The account of the COCRDLI row, 11 digits. */
  readonly acctId: string | undefined;
  /** The `card_ref` of the COCRDLI row. */
  readonly cardRef: number | undefined;
};

/** Reads the route state; any other shape is a first entry with no caller recorded. */
function routeEntryOf(state: unknown): RouteEntry {
  if (typeof state !== "object" || state === null) {
    return { from: undefined, acctId: undefined, cardRef: undefined };
  }
  const from = "from" in state && typeof state.from === "string" ? state.from : undefined;
  const acctId =
    "acct_id" in state && typeof state.acct_id === "string" ? state.acct_id : undefined;
  const cardRef =
    "card_ref" in state &&
    typeof state.card_ref === "number" &&
    Number.isSafeInteger(state.card_ref)
      ? state.card_ref
      : undefined;
  return { from: from !== undefined && KNOWN_PATHS.has(from) ? from : undefined, acctId, cardRef };
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

/** The `Input` attributes of one map field: its `LENGTH`, protection and accessible name. */
function inputAttributes(name: FieldPath<CocrdupValues>, enterable: boolean) {
  const spec: MapFieldSpec = FIELDS[name];
  const length = COCRDUP_LENGTHS[name];
  const narrow = length <= NARROW_FIELD;
  return {
    maxLength: length,
    readOnly: !enterable,
    autoComplete: "off",
    spellCheck: false,
    "aria-label": spec.accessibleName,
    size: narrow ? length : undefined,
    className: narrow ? NARROW_INPUT : undefined,
  };
}

/**
 * One map field as two grid cells: its literal as the `FormLabel` and its `Input` in the
 * `FormControl`. `FormItem` is `contents`, so both cells sit on the `FieldGrid` itself.
 */
function FieldCells({ spec, children }: { spec: MapFieldSpec; children: ReactNode }): ReactNode {
  return (
    <FormItem className="contents">
      <GridCell {...spec.labelCell}>
        <FormLabel className={LABEL}>{spec.label}</FormLabel>
      </GridCell>
      <GridCell {...spec.inputCell}>
        <FormControl>{children}</FormControl>
      </GridCell>
    </FormItem>
  );
}

/**
 * The card update screen. Entered from COCRDLI with the row's `acct_id` and `card_ref`, it fetches
 * that card at once (COCRDUPC-002); entered from the menu, it waits for the account and card
 * (COCRDUPC-001).
 */
export function COCRDUP(): ReactNode {
  const navigate = useNavigate();
  const entry = routeEntryOf(useLocation().state);
  const formId = useId();
  const formRef = useRef<HTMLFormElement>(null);
  const fetchCard = useCocrdupcFetch();
  const saveCard = useCocrdupcSave();
  const [model, setModel] = useState<ScreenModel>(INITIAL_MODEL);
  /** The field the cursor goes to once the next render has mounted it (COCRDUPC-046). */
  const pendingFocus = useRef<FieldPath<CocrdupValues> | undefined>(undefined);
  const entered = useRef(false);

  const keyForm = useForm<CocrdupKeyValues>({
    resolver: zodResolver(cocrdupKeySchema),
    defaultValues: { acctId: entry.acctId ?? "", cardNum: "" },
  });
  const detailForm = useForm<CocrdupValues>({
    resolver: zodResolver(cocrdupSchema),
    defaultValues: BLANK_VALUES,
  });

  const busy = fetchCard.isPending || saveCard.isPending;
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

  /**
   * Marks the field a server message names: red, `*` when blank, cursor there (COCRDUPC-046).
   * `flag` is false for the no-change answer, which only moves the cursor to the name. Without
   * a field the cursor goes to the account where it is enterable (`WHEN OTHER`).
   */
  function markField(field: string | null | undefined, state: CocrdupKeyState, flag: boolean) {
    const name = formFieldOf(field);
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
      keyForm.setError(name, { type: "server" });
    } else {
      if (detailForm.getValues(name).trim() === "") {
        detailForm.setValue(name, "*");
      }
      detailForm.setError(name, { type: "server" });
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
   * An endpoint 9 error. On the list entry and the 409 refresh, a card that is not found or
   * cannot be read still leaves state `S`, with the details empty and enterable over a blank
   * snapshot (COCRDUPC-003, COCRDUPC-019). Every other error leaves `otherwise`: not fetched for
   * a search (COCRDUPC-015, COCRDUPC-018).
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
      markField(error.field, "S", true);
      return;
    }
    setModel((current) => ({
      ...current,
      state: otherwise,
      message,
      info: INFO[otherwise],
      saveEnabled: false,
    }));
    markField(error.field, otherwise, true);
  }

  /** Endpoint 9 by `card_ref`, as the list entry and the 409 refresh call it (COCRDUPC-002). */
  function fetchByRef(
    listKeys: ListKeys,
    keptMessage: string | undefined,
    otherwise: CocrdupKeyState,
  ) {
    fetchCard.mutate(
      { query: { acct_id: listKeys.acctId, card_ref: listKeys.cardRef } },
      {
        onSuccess: (view) => {
          showFetched(view, keptMessage);
        },
        onError: (error) => {
          fetchFailed(error, listKeys, otherwise);
        },
      },
    );
  }

  /** ENTER before a fetch: endpoint 9 with the keys as typed, never a `card_ref` (COCRDUPC-016). */
  function search(keys: CocrdupKeyValues) {
    fetchCard.mutate(
      { query: { acct_id: keys.acctId, card_num: keys.cardNum } },
      {
        onSuccess: (view) => {
          showFetched(view);
        },
        onError: (error) => {
          fetchFailed(error, undefined, "not-fetched");
        },
      },
    );
  }

  /**
   * A 200 of endpoint 10. The fields keep the text sent, zero fill included; `not_fetched` is
   * the server's restart, the empty search screen (COCRDUPC-034, COCRDUPC-035, COCRDUPC-039).
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
    markField(error.field, state, message !== NO_CHANGES_DETECTED);
  }

  /** ENTER (validate) or F5 (save) with the card shown: endpoint 10 by `card_ref`. */
  function sendChanges(aid: components["schemas"]["CocrdupcAid"], values: CocrdupValues) {
    const { card, snapshot } = model;
    if (card === undefined || snapshot === undefined) {
      return;
    }
    const fields: components["schemas"]["CardUpdateFields"] = {
      embossed_name: values.embossedName,
      status: values.activeStatus,
      expiry_month: zeroFilled(values.expiryMonth, COCRDUP_LENGTHS.expiryMonth),
      expiry_year: zeroFilled(values.expiryYear, COCRDUP_LENGTHS.expiryYear),
    };
    saveCard.mutate(
      { path: { card_ref: card.cardRef }, body: { aid, fields, snapshot } },
      {
        onSuccess: (result) => {
          saved(result, fields, card, snapshot);
        },
        onError: (error) => {
          saveFailed(error, card, snapshot);
        },
      },
    );
  }

  /** A field over its map `LENGTH`: zod's message in the error row, cursor on the field. */
  function showInvalid(errors: Partial<Record<FieldPath<CocrdupValues>, { message?: string }>>) {
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

  // COCRDUPC-002: entered from the list, the row's card is fetched once, before any key.
  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    if (entry.cardRef !== undefined) {
      fetchByRef({ acctId: entry.acctId ?? "", cardRef: entry.cardRef }, undefined, "not-fetched");
    }
  });

  /**
   * COCRDUPC-005: after a save or a failed save, a key other than F3 returns to COCRDLI when that
   * screen transferred here, and otherwise starts over on the empty search screen.
   */
  // `CDEMO-LAST-MAPSET` has no route-state counterpart (X-COCRDUPC-10), so the caller recorded in
  // `from` stands for it: entered from the menu, the next key always starts over.
  function restart() {
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
    if (busy || card === undefined) {
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
    if (busy) {
      return;
    }
    void detailForm.handleSubmit((values) => {
      sendChanges("PF5", values);
    }, showInvalid)();
  }

  /** ENTER, and every key COCRDUPC takes as ENTER: the form's submit (COCRDUPC-008). */
  function submitForm() {
    formRef.current?.requestSubmit();
  }

  /** ENTER: search before a fetch, validate after it; in `C`, `L` and `F` the restart. */
  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (availability.roles.ENTER === "reset") {
      restart();
      return;
    }
    if (keysOnKeyForm) {
      void keyForm.handleSubmit(search, showInvalid)();
      return;
    }
    void detailForm.handleSubmit((values) => {
      sendChanges("ENTER", values);
    }, showInvalid)();
  }

  const handlers = bindPfKeys(availability, {
    own: { ENTER: submitForm, F3: exit, F5: confirmSave, F12: cancel },
    reset: restart,
  });
  const items = keyRowItemsFor(KEY_ROWS.COCRDUP, availability);
  const labelled = new Set(items.map((item) => item.key));
  const keys: PfKeyBarEntry[] = [
    // ENTER keeps no action of its own: its button and the Enter key submit the form natively.
    ...items.map((item) => ({
      ...item,
      hidden: item.key === "F5" || item.key === "F12" ? !model.saveEnabled : !item.shown,
      onPress: item.key === "ENTER" ? undefined : handlers[item.key],
    })),
    // COCRDUPC-008: the keys the map labels nowhere (F1, F2, F4, F6 to F11) still reach the
    // program, which processes them as ENTER; they get hidden entries so `PfKeyBar` binds them.
    ...PF_KEYS.filter((key) => !labelled.has(key)).map((key) => ({
      key,
      label: key,
      dark: true,
      hidden: true,
      onPress: handlers[key],
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
      keys={keys}
      keyOptions={KEY_OPTIONS}
    >
      <form id={formId} ref={formRef} noValidate aria-label={TITLE} onSubmit={onSubmit}>
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
                        {...inputAttributes(name, true)}
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
                      <Input {...field} {...inputAttributes(name, keysEnterable)} />
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
                    <Input {...field} {...inputAttributes(name, detailsEnterable)} />
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
