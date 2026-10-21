import { useEffect, useRef } from "react";

/**
 * Function keys of the 3270 key rows: ENTER and PF1–PF12 (AAP 0.3.5).
 *
 * PF13–PF24 have no entry: on a 3270 emulator they are Shift+F1…F12, which `keyFromEvent`
 * folds onto F1–F12 for the programs that COPY `CSSTRPFY` (`app/cpy/CSSTRPFY.cpy:54-77`).
 */
export const PF_KEYS = [
  "ENTER",
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
] as const;

/** One function key, named as the map labels it (`ENTER`, `F3`, `F12`). */
export type PfKey = (typeof PF_KEYS)[number];

/** The action a screen runs for each key it serves; a key with no entry is not served. */
export type PfKeyHandlers = Partial<Record<PfKey, () => void>>;

/** One key label of a map's row 24, exactly as the map shows it (`F3=Exit`, `ENTER=Sign-on`). */
export interface PfKeyLabel {
  readonly key: PfKey;
  readonly label: string;
}

/**
 * One row-24 `DFHMDF` field: its `INITIAL` in source form (BMS `&&` stands for `&`, continued
 * lines joined) and whether its `ATTRB` carries `DRK`.
 */
export interface KeyRowField {
  readonly text: string;
  readonly dark: boolean;
}

/**
 * A key label with its field's `DRK` flag. A dark label stays hidden until the program makes it
 * applicable (AAP 0.4.2); for COACTUP, COCRDUP and COTRTUP, whose programs decide that by state,
 * `keyRowItemsFor` marks each label shown or hidden for the state.
 */
export interface PfKeyItem extends PfKeyLabel {
  readonly dark: boolean;
}

/**
 * The key row of one map: its row-24 fields in `POS` order, and whether the owning program folds
 * PF13–PF24 onto PF1–PF12 through `YYYY-STORE-PFKEY` (`app/cpy/CSSTRPFY.cpy:54-77`).
 */
export interface KeyRowSpec {
  readonly foldShifted: boolean;
  readonly fields: readonly KeyRowField[];
}

/**
 * Row-24 key labels of the 21 maps, from `modern/audit/screens.md` (the rule ID after each map
 * names its key-row field rows) and the maps themselves.
 *
 * `foldShifted` is true for the seven programs that COPY `CSSTRPFY` (COACTVWC, COACTUPC, COCRDLIC,
 * COCRDSLC, COCRDUPC, COTRTLIC, COTRTUPC). The other fourteen treat a shifted key as an unhandled
 * AID (`WHEN OTHER`, for example `app/cbl/COSGN00C.cbl:91-94`), so Shift+F-key is not their key.
 *
 * Keys a program serves without a label on its map (X-SCREENS-10) are the screens' decisions;
 * `coactupKeys`, `cocrdupKeys` and `cotrtupKeys` decide which labels COACTUP, COCRDUP and COTRTUP
 * show in each state (COTRTUP `F6=Add` in none, X-SCREENS-12). This table is the maps' text.
 */
export const KEY_ROWS = {
  // COSGN00-037: app/bms/COSGN00.bms:205
  COSGN00: { foldShifted: false, fields: [{ text: "ENTER=Sign-on  F3=Exit", dark: false }] },
  // COMEN01-028: app/bms/COMEN01.bms:162
  COMEN01: { foldShifted: false, fields: [{ text: "ENTER=Continue  F3=Exit", dark: false }] },
  // COADM01-028: app/bms/COADM01.bms:162
  COADM01: { foldShifted: false, fields: [{ text: "ENTER=Continue  F3=Exit", dark: false }] },
  // COACTVW-100: app/bms/COACTVW.bms:373
  COACTVW: { foldShifted: true, fields: [{ text: "  F3=Exit ", dark: false }] },
  // COACTUP-126, COACTUP-127, COACTUP-128: app/bms/COACTUP.bms:497,502,507
  COACTUP: {
    foldShifted: true,
    fields: [
      { text: "ENTER=Process F3=Exit", dark: false },
      { text: "F5=Save", dark: true },
      { text: "F12=Cancel", dark: true },
    ],
  },
  // COCRDLI-072: app/bms/COCRDLI.bms:339
  COCRDLI: {
    foldShifted: true,
    fields: [{ text: "  F3=Exit F7=Backward  F8=Forward", dark: false }],
  },
  // COCRDSL-031: app/bms/COCRDSL.bms:152
  COCRDSL: { foldShifted: true, fields: [{ text: "ENTER=Search Cards  F3=Exit", dark: false }] },
  // COCRDUP-033, COCRDUP-034: app/bms/COCRDUP.bms:162,167
  COCRDUP: {
    foldShifted: true,
    fields: [
      { text: "ENTER=Process F3=Exit", dark: false },
      { text: "F5=Save F12=Cancel", dark: true },
    ],
  },
  // COTRN00-089: app/bms/COTRN00.bms:458-459
  COTRN00: {
    foldShifted: false,
    fields: [{ text: "ENTER=Continue  F3=Back  F7=Backward  F8=Forward", dark: false }],
  },
  // COTRN01-056: app/bms/COTRN01.bms:267-268
  COTRN01: {
    foldShifted: false,
    fields: [{ text: "ENTER=Fetch  F3=Back  F4=Clear  F5=Browse Tran.", dark: false }],
  },
  // COTRN02-061: app/bms/COTRN02.bms:301-302
  COTRN02: {
    foldShifted: false,
    fields: [{ text: "ENTER=Continue  F3=Back  F4=Clear  F5=Copy Last Tran.", dark: false }],
  },
  // CORPT00-042: app/bms/CORPT00.bms:226
  CORPT00: { foldShifted: false, fields: [{ text: "ENTER=Continue  F3=Back", dark: false }] },
  // COBIL00-024: app/bms/COBIL00.bms:135
  COBIL00: {
    foldShifted: false,
    fields: [{ text: "ENTER=Continue  F3=Back  F4=Clear", dark: false }],
  },
  // COUSR00-089: app/bms/COUSR00.bms:457-458
  COUSR00: {
    foldShifted: false,
    fields: [{ text: "ENTER=Continue  F3=Back  F7=Backward  F8=Forward", dark: false }],
  },
  // COUSR01-028: app/bms/COUSR01.bms:159
  COUSR01: {
    foldShifted: false,
    fields: [{ text: "ENTER=Add User  F3=Back  F4=Clear  F12=Exit", dark: false }],
  },
  // COUSR02-029: app/bms/COUSR02.bms:163-164 (source form: `&&` is one `&`)
  COUSR02: {
    foldShifted: false,
    fields: [{ text: "ENTER=Fetch  F3=Save&&Exit  F4=Clear  F5=Save  F12=Cancel", dark: false }],
  },
  // COUSR03-026: app/bms/COUSR03.bms:148
  COUSR03: {
    foldShifted: false,
    fields: [{ text: "ENTER=Fetch  F3=Back  F4=Clear  F5=Delete", dark: false }],
  },
  // COPAU00-104: app/app-authorization-ims-db2-mq/bms/COPAU00.bms:511-512
  COPAU00: {
    foldShifted: false,
    fields: [{ text: "ENTER=Continue  F3=Back  F7=Backward  F8=Forward", dark: false }],
  },
  // COPAU01-054: app/app-authorization-ims-db2-mq/bms/COPAU01.bms:292
  COPAU01: {
    foldShifted: false,
    fields: [{ text: " F3=Back  F5=Mark/Remove Fraud  F8=Next Auth", dark: false }],
  },
  // COTRTLI-077 to COTRTLI-081: app/app-transaction-type-db2/bms/COTRTLI.bms:316,321,326,331,336
  COTRTLI: {
    foldShifted: true,
    fields: [
      { text: "F2=Add", dark: false },
      { text: "F3=Exit", dark: false },
      { text: "F7=Page Up", dark: false },
      { text: "F8=Page Dn", dark: false },
      { text: "F10=Save", dark: false },
    ],
  },
  // COTRTUP-021 to COTRTUP-025: app/app-transaction-type-db2/bms/COTRTUP.bms:115,120,125,130,135
  COTRTUP: {
    foldShifted: true,
    fields: [
      { text: "ENTER=Process F3=Exit", dark: false },
      { text: "F4=Delete", dark: true },
      { text: "F5=Save", dark: true },
      { text: "F6=Add", dark: true },
      { text: "F12=Cancel", dark: true },
    ],
  },
} as const satisfies Record<string, KeyRowSpec>;

/** The member name of one of the 21 maps (`COSGN00`, `COTRTUP`). */
export type KeyRowMap = keyof typeof KEY_ROWS;

/**
 * The fields of a keyboard event the key decision reads; a DOM `KeyboardEvent` satisfies it, so
 * the decision functions run without a DOM.
 */
export interface PfKeyEventLike {
  readonly key: string;
  readonly altKey: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly shiftKey: boolean;
  readonly repeat: boolean;
  readonly isComposing: boolean;
}

/** `KeyboardEvent.key` value → key: `Enter` is ENTER, `F1`…`F12` keep their names. */
const EVENT_KEYS: Readonly<Record<string, PfKey>> = Object.fromEntries(
  PF_KEYS.map((key): [string, PfKey] => [key === "ENTER" ? "Enter" : key, key]),
);

/**
 * The key a keyboard event sends, or `undefined` when it sends none.
 *
 * - Auto-repeat and IME composition send nothing, so a held key acts once and composition is
 *   left alone.
 * - Ctrl, Alt or Meta send nothing, so browser shortcuts are never taken over.
 * - Shift with F1–F12 sends that F-key only when `foldShifted` (the program folds PF13–PF24 onto
 *   PF1–PF12, `app/cpy/CSSTRPFY.cpy:54-77`); Shift+Enter sends nothing.
 */
export function keyFromEvent(event: PfKeyEventLike, foldShifted = false): PfKey | undefined {
  if (event.repeat || event.isComposing || event.ctrlKey || event.altKey || event.metaKey) {
    return undefined;
  }
  const key = namedKey(event.key);
  if (key === undefined || (event.shiftKey && (key === "ENTER" || !foldShifted))) {
    return undefined;
  }
  return key;
}

/** The key a `KeyboardEvent.key` value names, modifiers aside. */
function namedKey(name: string): PfKey | undefined {
  // An own-property check, so inherited names such as `constructor` are no key.
  return Object.hasOwn(EVENT_KEYS, name) ? EVENT_KEYS[name] : undefined;
}

/**
 * The F-key of a Shift+F1…F12 keydown on a screen that does not fold shifted keys, or `undefined`
 * for every other event. The 3270 sends PF13–PF24 for it, which such a program serves under no
 * branch but `WHEN OTHER` (`app/cbl/COSGN00C.cbl:91-94`), so it is an unserved key, not no key.
 */
function unfoldedShiftedFKey(event: PfKeyEventLike, foldShifted: boolean): PfKey | undefined {
  if (
    foldShifted ||
    !event.shiftKey ||
    event.repeat ||
    event.isComposing ||
    event.ctrlKey ||
    event.altKey ||
    event.metaKey
  ) {
    return undefined;
  }
  const key = namedKey(event.key);
  return key === "ENTER" ? undefined : key;
}

/** Where a keydown landed, read from its DOM target. */
export interface PfKeyTarget {
  readonly tagName: string;
  readonly inForm: boolean;
  readonly isContentEditable: boolean;
}

/** Elements on which the browser itself acts on Enter. */
const NATIVE_ENTER_TAGS: ReadonlySet<string> = new Set(["BUTTON", "A", "TEXTAREA", "SELECT"]);

/**
 * Whether the browser already acts on Enter at `target`: inside a `<form>` implicit submission is
 * the screen's ENTER (AAP 0.3.5), and on a button, link, text area, select or editable element
 * Enter belongs to that element (Enter on a focused PF-key `Button` is that button's click).
 * Taking ENTER there as well would send it twice.
 */
export function enterHandledNatively(target: PfKeyTarget | undefined): boolean {
  if (target === undefined) {
    return false;
  }
  return (
    target.inForm || target.isContentEditable || NATIVE_ENTER_TAGS.has(target.tagName.toUpperCase())
  );
}

/**
 * What a keydown means for a screen: a served key with its action, or an unserved F-key. An
 * unserved Shift+F-key on a screen that does not fold shifted keys reports its unshifted F-key,
 * because PF13–PF24 have no key of their own.
 */
export type PfKeyResolution =
  | { readonly kind: "handled"; readonly key: PfKey; readonly handler: () => void }
  | { readonly kind: "invalid"; readonly key: PfKey };

/** How `resolvePfKey` treats shifted keys and unserved F-keys. */
export interface PfKeyResolveOptions {
  readonly foldShifted?: boolean;
  readonly hasInvalidKeyHandler?: boolean;
}

/**
 * Decides a keydown for a screen.
 *
 * Returns `handled` with the action when the screen serves the key, and `invalid` for an F-key it
 * does not serve when the screen shows the invalid-key message for those (the programs'
 * `WHEN OTHER`, `app/cbl/COSGN00C.cbl:91-94`). Without `foldShifted`, Shift+F1…F12 is such an
 * unserved key (PF13–PF24), never the F-key's action. Returns `undefined`, leaving the event to
 * the browser, for every other event: no key, ENTER the browser already handles at `target`,
 * ENTER the screen does not serve (ENTER is never invalid), or an unserved F-key with no
 * invalid-key handling.
 */
export function resolvePfKey(
  handlers: PfKeyHandlers,
  event: PfKeyEventLike,
  options: PfKeyResolveOptions = {},
  target?: PfKeyTarget,
): PfKeyResolution | undefined {
  const foldShifted = options.foldShifted ?? false;
  const key = keyFromEvent(event, foldShifted);
  if (key === undefined) {
    const shifted = unfoldedShiftedFKey(event, foldShifted);
    return shifted !== undefined && options.hasInvalidKeyHandler === true
      ? { kind: "invalid", key: shifted }
      : undefined;
  }
  if (key === "ENTER" && enterHandledNatively(target)) {
    return undefined;
  }
  const handler = handlers[key];
  if (handler !== undefined) {
    return { kind: "handled", key, handler };
  }
  if (key !== "ENTER" && options.hasInvalidKeyHandler === true) {
    return { kind: "invalid", key };
  }
  return undefined;
}

/** Label-start name → key; the token pattern below yields only these names. */
const LABEL_KEYS: Readonly<Record<string, PfKey>> = Object.fromEntries(
  PF_KEYS.map((key): [string, PfKey] => [key, key]),
);

/**
 * A key token, `<key>=`, at the start of the text or after whitespace. The alternation is built
 * from `PF_KEYS`, longest name first, so `F10`–`F12` win over `F1`.
 */
const KEY_TOKEN_SOURCE = `(^|\\s)(${[...PF_KEYS]
  .sort((left, right) => right.length - left.length)
  .join("|")})=`;

/**
 * Splits one row-24 field text into its key labels, in order.
 *
 * BMS `&&` becomes `&` (`app/bms/COUSR02.bms:163`). Each label runs from its key token to the next
 * token, trimmed, so single- and double-spaced rows and labels with inner spaces
 * (`ENTER=Search Cards`, `F5=Copy Last Tran.`) split alike. Blank text has no labels.
 *
 * @throws Error naming the text when non-blank text precedes the first key, when non-blank text
 *   holds no key, or when a key appears twice: each is a data error in the key table.
 */
export function parseKeyRow(text: string): PfKeyLabel[] {
  const row = text.replaceAll("&&", "&");
  if (row.trim() === "") {
    return [];
  }
  const tokens: { readonly key: PfKey; readonly start: number }[] = [];
  const seen = new Set<PfKey>();
  const pattern = new RegExp(KEY_TOKEN_SOURCE, "g");
  for (let match = pattern.exec(row); match !== null; match = pattern.exec(row)) {
    const key = LABEL_KEYS[match[2]];
    if (seen.has(key)) {
      throw new Error(`Key row ${JSON.stringify(text)} labels ${key} twice`);
    }
    seen.add(key);
    tokens.push({ key, start: match.index + match[1].length });
  }
  const first = tokens.at(0);
  if (first === undefined) {
    throw new Error(`Key row ${JSON.stringify(text)} holds no key label`);
  }
  if (row.slice(0, first.start).trim() !== "") {
    throw new Error(`Key row ${JSON.stringify(text)} has text before its first key label`);
  }
  return tokens.map((token, index) => ({
    key: token.key,
    label: row.slice(token.start, tokens.at(index + 1)?.start).trim(),
  }));
}

/**
 * The key labels of a map's row 24, in field order, each with its field's `DRK` flag. COCRDUP, for
 * example, gives `ENTER=Process` and `F3=Exit`, then the dark `F5=Save` and `F12=Cancel`.
 *
 * Takes the map name or its `KEY_ROWS` entry: screens pass `KEY_ROWS.<MAP>`, tests the name.
 *
 * @throws Error when a field text is malformed (`parseKeyRow`) or two fields label the same key,
 *   because `PfKeyBar` keys its buttons by key.
 */
export function keyRowItems(row: KeyRowMap | KeyRowSpec): PfKeyItem[] {
  const spec: KeyRowSpec = typeof row === "string" ? KEY_ROWS[row] : row;
  const items = spec.fields.flatMap((field) =>
    parseKeyRow(field.text).map((label) => ({ ...label, dark: field.dark })),
  );
  const keys = new Set(items.map((item) => item.key));
  if (keys.size !== items.length) {
    throw new Error(
      `Key row ${JSON.stringify(spec.fields.map((field) => field.text))} labels a key twice`,
    );
  }
  return items;
}

/**
 * What a key does in one state of a screen whose program decides its keys by state (COACTUP,
 * COCRDUP, COTRTUP).
 *
 * - `own`: the key's own action (ENTER processes, F3 exits, F4 deletes, F5 saves, F12 cancels).
 * - `enter`: processed as ENTER. COACTUPC and COCRDUPC set `CCARD-AID-ENTER` for a key they do
 *   not accept (`app/cbl/COACTUPC.cbl:898-916`, `app/cbl/COCRDUPC.cbl:406-424`).
 * - `invalid`: refused with the invalid-key message, the state's screen shown again
 *   (COTRTUPC-042, `app/app-transaction-type-db2/cbl/COTRTUPC.cbl:577-608`).
 * - `reset`: starts the screen over with no request (COACTUPC-006, COCRDUPC-005, COTRTUPC-030,
 *   COTRTUPC-039, COTRTUPC-041).
 */
export type PfKeyRole = "own" | "enter" | "invalid" | "reset";

/**
 * The keys of one screen state: the role of each of the 13 keys of `PF_KEYS`, and the keys whose
 * row-24 label the program shows in that state. A hidden label's key can still act (F12 cancels
 * in COACTUP state `S`), and a shown one can start the screen over (F12 in COACTUP state `L`).
 */
export interface PfKeyAvailability {
  readonly roles: Readonly<Record<PfKey, PfKeyRole>>;
  readonly shown: ReadonlySet<PfKey>;
}

/**
 * A COACTUPC screen state, named by the `VALUE` of the `ACUP-CHANGE-ACTION` 88-level that holds
 * (`app/cbl/COACTUPC.cbl:654-668`): `not-fetched` is `ACUP-DETAILS-NOT-FETCHED` (low-values or
 * spaces), `S` details shown, `E` changes not OK, `N` changes OK and not confirmed, `C` changes
 * done, `L` lock error, `F` update failed.
 *
 * This is the program's screen state, which the screen keeps from the last response it shows;
 * it describes no payload, whose types come only from the generated schema (Generated API Types
 * Only).
 */
export type CoactupKeyState = "not-fetched" | "S" | "E" | "N" | "C" | "L" | "F";

/**
 * A COCRDUPC screen state, named by the `VALUE` of the `CCUP-CHANGE-ACTION` 88-level that holds
 * (`app/cbl/COCRDUPC.cbl:276-290`), with the meanings of `CoactupKeyState`. Like it, a screen
 * state, not a payload type.
 */
export type CocrdupKeyState = "not-fetched" | "S" | "E" | "N" | "C" | "L" | "F";

/**
 * A COTRTUPC screen state, named by the `VALUE` of the `TTUP-CHANGE-ACTION` 88-level that holds
 * (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:296-327`): `fresh` is
 * `TTUP-DETAILS-NOT-FETCHED` (low-values or spaces), `X` not found, `S` details shown, `R` new
 * record, `E` changes not OK, `N` changes OK and not confirmed, `B` changes backed out over fetched
 * values and `B-blank` over blank old values (a new type, lines 412-414), `9` confirm delete, `8`
 * delete started, `7` delete done, `6` delete failed, `C` changes done, `L` lock error.
 *
 * `K` (`TTUP-INVALID-SEARCH-KEYS`, replaced by the fresh state on the next line, COTRTUPC-006),
 * `V` (`TTUP-REVIEW-NEW-RECORD`, never set) and `F` (`TTUP-CHANGES-OKAYED-BUT-FAILED`,
 * unreachable, COTRTUPC-028) have no entry, as `modern/audit/online/COTRTUPC.md` records. Like
 * `CoactupKeyState`, a screen state, not a payload type.
 */
export type CotrtupKeyState =
  | "fresh"
  | "X"
  | "S"
  | "R"
  | "E"
  | "N"
  | "B"
  | "B-blank"
  | "9"
  | "8"
  | "7"
  | "6"
  | "C"
  | "L";

/** The availability whose roles and shown keys the two predicates give, key by key. */
function availabilityFrom(
  role: (key: PfKey) => PfKeyRole,
  shows: (key: PfKey) => boolean,
): PfKeyAvailability {
  return {
    roles: {
      ENTER: role("ENTER"),
      F1: role("F1"),
      F2: role("F2"),
      F3: role("F3"),
      F4: role("F4"),
      F5: role("F5"),
      F6: role("F6"),
      F7: role("F7"),
      F8: role("F8"),
      F9: role("F9"),
      F10: role("F10"),
      F11: role("F11"),
      F12: role("F12"),
    },
    shown: new Set(PF_KEYS.filter((key) => shows(key))),
  };
}

/** The 88-levels of `ACUP-CHANGE-ACTION` and `CCUP-CHANGE-ACTION` that the key rules test. */
interface UpdateConditions {
  readonly detailsNotFetched: boolean;
  readonly changesMade: boolean;
  readonly changesOkNotConfirmed: boolean;
  readonly changesOkayedAndDone: boolean;
  readonly changesFailed: boolean;
}

/**
 * The conditions of a COACTUPC or COCRDUPC state, whose 88-levels carry the same values
 * (`app/cbl/COACTUPC.cbl:656-668`, `app/cbl/COCRDUPC.cbl:278-290`).
 */
function updateConditions(state: CoactupKeyState | CocrdupKeyState): UpdateConditions {
  return {
    detailsNotFetched: state === "not-fetched",
    changesMade: state === "E" || state === "N" || state === "C" || state === "L" || state === "F",
    changesOkNotConfirmed: state === "N",
    changesOkayedAndDone: state === "C",
    changesFailed: state === "L" || state === "F",
  };
}

/**
 * The role of `key` in a COACTUPC or COCRDUPC state; both `0000-MAIN`s decide keys alike.
 *
 * The AID check accepts ENTER, F3, F5 while changes await confirmation and F12 once details are
 * fetched, and sets ENTER for every other key (`app/cbl/COACTUPC.cbl:905-916`,
 * `app/cbl/COCRDUPC.cbl:413-424`; COACTUPC-003, COCRDUPC-008). The `EVALUATE` that follows exits
 * on F3 first, and on the screen of a response that left state `C`, `L` or `F` starts over for
 * every other key (`app/cbl/COACTUPC.cbl:979-989`, `app/cbl/COCRDUPC.cbl:517-528`; COACTUPC-006,
 * COCRDUPC-005).
 */
function updateKeyRole(key: PfKey, is: UpdateConditions): PfKeyRole {
  if (key !== "F3" && (is.changesOkayedAndDone || is.changesFailed)) {
    return "reset";
  }
  const accepted =
    key === "ENTER" ||
    key === "F3" ||
    (key === "F5" && is.changesOkNotConfirmed) ||
    (key === "F12" && !is.detailsNotFetched);
  return accepted ? "own" : "enter";
}

/**
 * The keys of a COACTUP state (COACTUPC-003, COACTUPC-006, COACTUPC-069; COACTUP-127,
 * COACTUP-128).
 *
 * Roles as `updateKeyRole` gives them: F5 saves only in state `N`, F12 cancels in `S`, `E` and
 * `N`, every key not accepted is ENTER, and in `C`, `L` and `F` every key but F3 starts over.
 * Shown as `3390-SETUP-INFOMSG-ATTRS` brightens the dark fields (`app/cbl/COACTUPC.cbl:3566-3586`;
 * COACTUPC-082, COACTUPC-083): the `FKEYS` labels ENTER and F3 always; `F12=Cancel` while changes
 * are made and not done; `F5=Save` and `F12=Cancel` with the info row `PROMPT-FOR-CONFIRMATION`,
 * which `3250-SETUP-INFOMSG` sets in state `N` (lines 2966-2967). So F5 shows only in `N`, F12 in
 * `E`, `N`, `L` and `F`.
 */
export function coactupKeys(state: CoactupKeyState): PfKeyAvailability {
  const is = updateConditions(state);
  const promptForConfirmation = is.changesOkNotConfirmed;
  return availabilityFrom(
    (key) => updateKeyRole(key, is),
    (key) => {
      switch (key) {
        case "ENTER":
        case "F3":
          return true;
        case "F5":
          return promptForConfirmation;
        case "F12":
          return (is.changesMade && !is.changesOkayedAndDone) || promptForConfirmation;
        default:
          return false;
      }
    },
  );
}

/**
 * The keys of a COCRDUP state (COCRDUPC-005, COCRDUPC-006, COCRDUPC-008; COCRDUP-034).
 *
 * Roles as COACTUP's (`updateKeyRole`). In `C`, `L` and `F` the reset is COCRDUPC-005's, which
 * opens the caller instead of the empty search screen when the last map set is COCRDLI
 * (`app/cbl/COCRDUPC.cbl:435-440`); the screen's reset action makes that choice. Shown as
 * `3300-SETUP-SCREEN-ATTRS` brightens the one dark field `F5=Save F12=Cancel`
 * (`app/cbl/COCRDUPC.cbl:1314-1316`): with the info row `PROMPT-FOR-CONFIRMATION`, which
 * `3250-SETUP-INFOMSG` sets in state `N` (lines 1149-1150). ENTER and F3 always show.
 */
export function cocrdupKeys(state: CocrdupKeyState): PfKeyAvailability {
  const is = updateConditions(state);
  const promptForConfirmation = is.changesOkNotConfirmed;
  return availabilityFrom(
    (key) => updateKeyRole(key, is),
    (key) =>
      key === "ENTER" || key === "F3" || ((key === "F5" || key === "F12") && promptForConfirmation),
  );
}

/** The 88-levels of `TTUP-CHANGE-ACTION` that the key rules test. */
interface TtupConditions {
  readonly detailsNotFound: boolean;
  readonly showDetails: boolean;
  readonly createNewRecord: boolean;
  readonly deleteInProgress: boolean;
  readonly confirmDelete: boolean;
  readonly deleteDone: boolean;
  readonly deleteFailed: boolean;
  readonly changesOkNotConfirmed: boolean;
  readonly changesFailed: boolean;
  readonly changesOkayedAndDone: boolean;
  readonly changesBackedOut: boolean;
  /** `TTUP-OLD-DETAILS` low-values or spaces; recorded for state `B`, the one state it decides. */
  readonly oldDetailsBlank: boolean;
}

/**
 * The conditions of a COTRTUPC state (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:298-327`).
 * `TTUP-CHANGES-FAILED` holds only in `L` here, its `F` being unreachable.
 */
function ttupConditions(state: CotrtupKeyState): TtupConditions {
  return {
    detailsNotFound: state === "X",
    showDetails: state === "S",
    createNewRecord: state === "R",
    deleteInProgress: state === "9" || state === "8" || state === "7" || state === "6",
    confirmDelete: state === "9",
    deleteDone: state === "7",
    deleteFailed: state === "6",
    changesOkNotConfirmed: state === "N",
    changesFailed: state === "L",
    changesOkayedAndDone: state === "C",
    changesBackedOut: state === "B" || state === "B-blank",
    oldDetailsBlank: state === "B-blank",
  };
}

/**
 * `0001-CHECK-PFKEYS` (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:577-608`; COTRTUPC-042):
 * F3 always; ENTER unless a delete awaits confirmation; F4 with details shown or a delete to
 * confirm; F5 while changes await confirmation, after a fetch that found nothing, or while a
 * delete is in progress; F12 while changes await confirmation, with details shown, after a fetch
 * that found nothing, with a delete to confirm or a new record. No other key is accepted.
 */
function cotrtupAccepts(key: PfKey, is: TtupConditions): boolean {
  switch (key) {
    case "F3":
      return true;
    case "ENTER":
      return !is.confirmDelete;
    case "F4":
      return is.showDetails || is.confirmDelete;
    case "F5":
      return is.changesOkNotConfirmed || is.detailsNotFound || is.deleteInProgress;
    case "F12":
      return (
        is.changesOkNotConfirmed ||
        is.showDetails ||
        is.detailsNotFound ||
        is.confirmDelete ||
        is.createNewRecord
      );
    default:
      return false;
  }
}

/**
 * The `EVALUATE` after `0001-CHECK-PFKEYS`, which simulates a fresh entry
 * (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:405-419`): F12 with details shown, a new
 * record or a fetch that found nothing (COTRTUPC-041), and every key once changes are done or
 * failed, after a cancel over blank old values, or once a delete is done or failed (COTRTUPC-030,
 * COTRTUPC-039), an invalid key included.
 */
function cotrtupStartsOver(key: PfKey, is: TtupConditions): boolean {
  return (
    (key === "F12" && (is.showDetails || is.createNewRecord || is.detailsNotFound)) ||
    is.changesOkayedAndDone ||
    is.changesFailed ||
    (is.changesBackedOut && is.oldDetailsBlank) ||
    is.deleteDone ||
    is.deleteFailed
  );
}

/**
 * `3391-SETUP-PFKEY-ATTRS` (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:1397-1422`;
 * COTRTUP-021 to COTRTUP-025): the `FKEYS` labels ENTER and F3 unless a delete awaits
 * confirmation; `F4=Delete` with details shown or a delete to confirm; `F5=Save` while changes
 * await confirmation or after a fetch that found nothing; `F12=Cancel` in the states where
 * `0001-CHECK-PFKEYS` accepts F12. No paragraph brightens `F6=Add` (X-SCREENS-12).
 */
function cotrtupShows(key: PfKey, is: TtupConditions): boolean {
  switch (key) {
    case "ENTER":
    case "F3":
      return !is.confirmDelete;
    case "F4":
      return is.showDetails || is.confirmDelete;
    case "F5":
      return is.changesOkNotConfirmed || is.detailsNotFound;
    case "F12":
      return (
        is.changesOkNotConfirmed ||
        is.showDetails ||
        is.detailsNotFound ||
        is.confirmDelete ||
        is.createNewRecord
      );
    default:
      return false;
  }
}

/**
 * The keys of a COTRTUP state (COTRTUPC-003, COTRTUPC-013 to COTRTUPC-015, COTRTUPC-022,
 * COTRTUPC-030, COTRTUPC-031, COTRTUPC-035, COTRTUPC-036, COTRTUPC-039, COTRTUPC-041,
 * COTRTUPC-042; COTRTUP-021 to COTRTUP-025).
 *
 * F3 exits in every state, because the exit `WHEN` comes first
 * (`app/app-transaction-type-db2/cbl/COTRTUPC.cbl:429-460`). Any other key starts over where
 * `cotrtupStartsOver` says, is its own action where `0001-CHECK-PFKEYS` accepts it, and is
 * invalid otherwise. Shown as `3391-SETUP-PFKEY-ATTRS` gives (`cotrtupShows`), so in state `9`
 * the ENTER and F3 labels hide although F3 still exits.
 */
export function cotrtupKeys(state: CotrtupKeyState): PfKeyAvailability {
  const is = ttupConditions(state);
  return availabilityFrom(
    (key) => {
      if (key !== "F3" && cotrtupStartsOver(key, is)) {
        return "reset";
      }
      return cotrtupAccepts(key, is) ? "own" : "invalid";
    },
    (key) => cotrtupShows(key, is),
  );
}

/** A key label with whether the program shows it in the screen's current state. */
export interface ShownPfKeyItem extends PfKeyItem {
  readonly shown: boolean;
}

/**
 * The key labels of a map's row 24, as `keyRowItems` gives them and in that order, each marked
 * shown when `availability` shows its key. A label that is not dark can be hidden too: COTRTUP's
 * `ENTER=Process` and `F3=Exit` while a delete awaits confirmation.
 *
 * @throws Error as `keyRowItems` does.
 */
export function keyRowItemsFor(
  row: KeyRowMap | KeyRowSpec,
  availability: PfKeyAvailability,
): ShownPfKeyItem[] {
  return keyRowItems(row).map((item) => ({ ...item, shown: availability.shown.has(item.key) }));
}

/** The actions a state-keyed screen binds its keys to, by role. */
export interface PfKeyRoleActions {
  /** Each key's own action; a key whose role is `enter` runs the `ENTER` entry. */
  readonly own: PfKeyHandlers;
  /** Starts the screen over with no request; needed where a state has a `reset` key. */
  readonly reset?: () => void;
  /** Shows the invalid-key message for `key`; needed where a state has an `invalid` key. */
  readonly invalid?: (key: PfKey) => void;
}

/** The error for a key whose role needs an action the screen does not pass. */
function missingAction(key: PfKey, role: PfKeyRole, action: string): Error {
  return new Error(`PF key ${key} has role "${role}", but the screen passes no ${action} action`);
}

/** The action `key` runs in its role. */
function roleAction(key: PfKey, role: PfKeyRole, actions: PfKeyRoleActions): () => void {
  switch (role) {
    case "own": {
      const own = actions.own[key];
      if (own === undefined) {
        throw missingAction(key, role, key);
      }
      return own;
    }
    case "enter": {
      const enter = actions.own.ENTER;
      if (enter === undefined) {
        throw missingAction(key, role, "ENTER");
      }
      return enter;
    }
    case "reset": {
      const reset = actions.reset;
      if (reset === undefined) {
        throw missingAction(key, role, "reset");
      }
      return reset;
    }
    case "invalid": {
      const invalid = actions.invalid;
      if (invalid === undefined) {
        throw missingAction(key, role, "invalid-key");
      }
      return () => {
        invalid(key);
      };
    }
  }
}

/**
 * The handlers of a state-keyed screen: one for each of the 13 keys of `PF_KEYS`, running the
 * action of the key's role in `availability`: its own action, the ENTER action, the reset, or
 * the invalid-key message for that key. Binding runs no action.
 *
 * @throws Error naming the key and its role when the action that role needs is missing: a wiring
 *   error of the screen.
 */
export function bindPfKeys(
  availability: PfKeyAvailability,
  actions: PfKeyRoleActions,
): PfKeyHandlers {
  const handlers: PfKeyHandlers = {};
  for (const key of PF_KEYS) {
    handlers[key] = roleAction(key, availability.roles[key], actions);
  }
  return handlers;
}

/** Settings of `usePfKeys`. */
export interface PfKeyOptions {
  /** The map's `KEY_ROWS` `foldShifted`: Shift+F-key acts as the F-key. */
  readonly foldShifted?: boolean;
  /**
   * Called for an F-key the screen does not serve, where the screen shows the invalid-key message
   * (`INVALID_KEY` of `@/lib/header`). Without it such keys are left to the browser.
   */
  readonly onInvalidKey?: (key: PfKey) => void;
  /** `false` stops the binding without unmounting the screen. */
  readonly enabled?: boolean;
}

/**
 * Binds the screen's function keys to the window while the calling component is mounted
 * (AAP 0.3.5). `PfKeyBar` is its one caller per screen. COACTUP, COCRDUP and COTRTUP pass the
 * `bindPfKeys(...)` handlers of their current state, and their `PfKeyBar` shows the
 * `keyRowItemsFor(...)` items of that state.
 *
 * The browser decides whether `preventDefault` takes effect (F12 may still open developer tools,
 * AAP 0.4.4), so every action also exists as a `Button`; the NAV suite clicks buttons, never keys.
 * An event the screen does not take is left untouched. All decisions are `resolvePfKey`'s.
 */
export function usePfKeys(handlers: PfKeyHandlers, options: PfKeyOptions = {}): void {
  // The latest arguments, read at keydown time: a re-render never re-binds the listener, and
  // the listener never runs a stale handler.
  const latest = useRef({ handlers, options });
  useEffect(() => {
    latest.current = { handlers, options };
  });

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      const { handlers: current, options: settings } = latest.current;
      if (settings.enabled === false || event.defaultPrevented) {
        return;
      }
      const element = event.target;
      const target =
        element instanceof HTMLElement
          ? {
              tagName: element.tagName,
              inForm: element.closest("form") !== null,
              isContentEditable: element.isContentEditable,
            }
          : undefined;
      const resolution = resolvePfKey(
        current,
        event,
        {
          foldShifted: settings.foldShifted,
          hasInvalidKeyHandler: settings.onInvalidKey !== undefined,
        },
        target,
      );
      if (resolution === undefined) {
        return;
      }
      event.preventDefault();
      if (resolution.kind === "handled") {
        resolution.handler();
      } else {
        settings.onInvalidKey?.(resolution.key);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
}
