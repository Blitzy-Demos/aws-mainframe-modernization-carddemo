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
 * applicable; `PfKeyBar` and the screen decide that (AAP 0.4.2), this module only reports it.
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
 * Keys a program serves without a label on its map (X-SCREENS-10) and labels a program never
 * shows (COTRTUP `F6=Add`, X-SCREENS-12) are the screens' decisions; this table is the maps' text.
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
 * (AAP 0.3.5). `PfKeyBar` is its one caller per screen.
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
