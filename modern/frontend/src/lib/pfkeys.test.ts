import { describe, expect, it } from "vitest";
import {
  KEY_ROWS,
  type KeyRowMap,
  type KeyRowSpec,
  PF_KEYS,
  type PfKey,
  type PfKeyEventLike,
  type PfKeyHandlers,
  type PfKeyItem,
  type PfKeyLabel,
  type PfKeyResolution,
  type PfKeyResolveOptions,
  type PfKeyTarget,
  enterHandledNatively,
  keyFromEvent,
  keyRowItems,
  parseKeyRow,
  resolvePfKey,
} from "./pfkeys";
import {
  INVALID_KEY,
  THANK_YOU,
  TITLE01,
  TITLE02,
  formatHeaderDate,
  formatHeaderTime,
} from "./header";

/**
 * Unit tests of the PF-key labels and bindings. Every expected key, label and `DRK` flag is written
 * by hand from the row-24 fields of the BMS maps and the PF-key rows of `modern/audit/screens.md`;
 * none is read from `KEY_ROWS`. Each label row a test proves carries its audit tag.
 *
 * `usePfKeys` needs React and a DOM, which this Node project does not provide; the decisions it
 * delegates to (`resolvePfKey`, `keyFromEvent`, `enterHandledNatively`) are proved here.
 *
 * The header values of `./header` (titles, common messages, date and time formats) are proved at
 * the end of the file; their expected texts are written by hand from `COTTL01Y`, `CSMSG01Y` and
 * `CSDAT01Y`.
 */

/** The 21 maps of the screen inventory in `modern/audit/screens.md`. */
const MAPS: readonly KeyRowMap[] = [
  "COSGN00",
  "COMEN01",
  "COADM01",
  "COACTVW",
  "COACTUP",
  "COCRDLI",
  "COCRDSL",
  "COCRDUP",
  "COTRN00",
  "COTRN01",
  "COTRN02",
  "CORPT00",
  "COBIL00",
  "COUSR00",
  "COUSR01",
  "COUSR02",
  "COUSR03",
  "COPAU00",
  "COPAU01",
  "COTRTLI",
  "COTRTUP",
];

/**
 * Maps whose programs COPY `CSSTRPFY` (COACTVWC, COACTUPC, COCRDLIC, COCRDSLC, COCRDUPC, COTRTLIC,
 * COTRTUPC), which folds PF13–PF24 onto PF1–PF12.
 */
const FOLDING_MAPS: readonly KeyRowMap[] = [
  "COACTVW",
  "COACTUP",
  "COCRDLI",
  "COCRDSL",
  "COCRDUP",
  "COTRTLI",
  "COTRTUP",
];

/** One expected key label: the key, the label text the map shows, and whether its field is DRK. */
type ExpectedLabel = readonly [key: PfKey, label: string, dark: boolean];

/**
 * Row-24 labels of each map in field order, from each field's `INITIAL` (BMS `&&` read as `&`,
 * continued lines joined) and `ATTRB`.
 */
const MAP_LABELS: Readonly<Record<KeyRowMap, readonly ExpectedLabel[]>> = {
  COSGN00: [
    ["ENTER", "ENTER=Sign-on", false], // audit: COSGN00-046
    ["F3", "F3=Exit", false], // audit: COSGN00-047
  ],
  COMEN01: [
    ["ENTER", "ENTER=Continue", false], // audit: COMEN01-035
    ["F3", "F3=Exit", false], // audit: COMEN01-036
  ],
  COADM01: [
    ["ENTER", "ENTER=Continue", false], // audit: COADM01-035
    ["F3", "F3=Exit", false], // audit: COADM01-036
  ],
  COACTVW: [
    ["F3", "F3=Exit", false], // audit: COACTVW-107
  ],
  COACTUP: [
    ["ENTER", "ENTER=Process", false], // audit: COACTUP-135
    ["F3", "F3=Exit", false], // audit: COACTUP-136
    ["F5", "F5=Save", true], // audit: COACTUP-137
    ["F12", "F12=Cancel", true], // audit: COACTUP-138
  ],
  COCRDLI: [
    ["F3", "F3=Exit", false], // audit: COCRDLI-079
    ["F7", "F7=Backward", false], // audit: COCRDLI-080
    ["F8", "F8=Forward", false], // audit: COCRDLI-081
  ],
  COCRDSL: [
    ["ENTER", "ENTER=Search Cards", false], // audit: COCRDSL-038
    ["F3", "F3=Exit", false], // audit: COCRDSL-039
  ],
  COCRDUP: [
    ["ENTER", "ENTER=Process", false], // audit: COCRDUP-041
    ["F3", "F3=Exit", false], // audit: COCRDUP-042
    ["F5", "F5=Save", true], // audit: COCRDUP-043
    ["F12", "F12=Cancel", true], // audit: COCRDUP-044
  ],
  COTRN00: [
    ["ENTER", "ENTER=Continue", false], // audit: COTRN00-096
    ["F3", "F3=Back", false], // audit: COTRN00-097
    ["F7", "F7=Backward", false], // audit: COTRN00-098
    ["F8", "F8=Forward", false], // audit: COTRN00-099
  ],
  COTRN01: [
    ["ENTER", "ENTER=Fetch", false], // audit: COTRN01-063
    ["F3", "F3=Back", false], // audit: COTRN01-064
    ["F4", "F4=Clear", false], // audit: COTRN01-065
    ["F5", "F5=Browse Tran.", false], // audit: COTRN01-066
  ],
  COTRN02: [
    ["ENTER", "ENTER=Continue", false], // audit: COTRN02-068
    ["F3", "F3=Back", false], // audit: COTRN02-069
    ["F4", "F4=Clear", false], // audit: COTRN02-070
    ["F5", "F5=Copy Last Tran.", false], // audit: COTRN02-071
  ],
  CORPT00: [
    ["ENTER", "ENTER=Continue", false], // audit: CORPT00-049
    ["F3", "F3=Back", false], // audit: CORPT00-050
  ],
  COBIL00: [
    ["ENTER", "ENTER=Continue", false], // audit: COBIL00-031
    ["F3", "F3=Back", false], // audit: COBIL00-032
    ["F4", "F4=Clear", false], // audit: COBIL00-033
  ],
  COUSR00: [
    ["ENTER", "ENTER=Continue", false], // audit: COUSR00-096
    ["F3", "F3=Back", false], // audit: COUSR00-097
    ["F7", "F7=Backward", false], // audit: COUSR00-098
    ["F8", "F8=Forward", false], // audit: COUSR00-099
  ],
  COUSR01: [
    ["ENTER", "ENTER=Add User", false], // audit: COUSR01-035
    ["F3", "F3=Back", false], // audit: COUSR01-036
    ["F4", "F4=Clear", false], // audit: COUSR01-037
    ["F12", "F12=Exit", false], // audit: COUSR01-038
  ],
  COUSR02: [
    ["ENTER", "ENTER=Fetch", false], // audit: COUSR02-036
    ["F3", "F3=Save&Exit", false], // audit: COUSR02-037
    ["F4", "F4=Clear", false], // audit: COUSR02-038
    ["F5", "F5=Save", false], // audit: COUSR02-039
    ["F12", "F12=Cancel", false], // audit: COUSR02-040
  ],
  COUSR03: [
    ["ENTER", "ENTER=Fetch", false], // audit: COUSR03-033
    ["F3", "F3=Back", false], // audit: COUSR03-034
    ["F4", "F4=Clear", false], // audit: COUSR03-035
    ["F5", "F5=Delete", false], // audit: COUSR03-036
  ],
  COPAU00: [
    ["ENTER", "ENTER=Continue", false], // audit: COPAU00-111
    ["F3", "F3=Back", false], // audit: COPAU00-112
    ["F7", "F7=Backward", false], // audit: COPAU00-113
    ["F8", "F8=Forward", false], // audit: COPAU00-114
  ],
  COPAU01: [
    ["F3", "F3=Back", false], // audit: COPAU01-061
    ["F5", "F5=Mark/Remove Fraud", false], // audit: COPAU01-062
    ["F8", "F8=Next Auth", false], // audit: COPAU01-063
  ],
  COTRTLI: [
    ["F2", "F2=Add", false], // audit: COTRTLI-088
    ["F3", "F3=Exit", false], // audit: COTRTLI-089
    ["F7", "F7=Page Up", false], // audit: COTRTLI-090
    ["F8", "F8=Page Dn", false], // audit: COTRTLI-091
    ["F10", "F10=Save", false], // audit: COTRTLI-092
  ],
  COTRTUP: [
    ["ENTER", "ENTER=Process", false], // audit: COTRTUP-032
    ["F3", "F3=Exit", false], // audit: COTRTUP-033
    ["F4", "F4=Delete", true], // audit: COTRTUP-034
    ["F5", "F5=Save", true], // audit: COTRTUP-035
    // The map's dark FKEY06 text; COTRTUPC never brightens or serves it (X-SCREENS-12).
    ["F6", "F6=Add", true],
    ["F12", "F12=Cancel", true], // audit: COTRTUP-037
  ],
};

/** The items `keyRowItems` reports for expected labels. */
function items(labels: readonly ExpectedLabel[]): PfKeyItem[] {
  return labels.map(([key, label, dark]) => ({ key, label, dark }));
}

/** A keydown with no modifier held, not repeated and not composing, except as `flags` says. */
function keydown(key: string, flags: Partial<Omit<PfKeyEventLike, "key">> = {}): PfKeyEventLike {
  return {
    key,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    repeat: false,
    isComposing: false,
    ...flags,
  };
}

/** The `KeyboardEvent.key` a browser reports for a key: `Enter` for ENTER, the name otherwise. */
function eventKeyOf(key: PfKey): string {
  return key === "ENTER" ? "Enter" : key;
}

/** Where a keydown landed. */
function targetAt(tagName: string, inForm: boolean, isContentEditable = false): PfKeyTarget {
  return { tagName, inForm, isContentEditable };
}

/** A screen action that counts its runs, so a test can tell whether it ran. */
function countingAction(): { readonly run: () => void; readonly runs: () => number } {
  let runs = 0;
  return {
    run: () => {
      runs += 1;
    },
    runs: () => runs,
  };
}

/** Handlers that serve `key` alone. */
function serving(key: PfKey, action: () => void): PfKeyHandlers {
  const handlers: PfKeyHandlers = {};
  handlers[key] = action;
  return handlers;
}

describe("PF_KEYS", () => {
  it("names ENTER and PF1 to PF12; PF13 to PF24 have no key of their own", () => {
    expect(PF_KEYS).toEqual([
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
    ]);
  });
});

describe("KEY_ROWS", () => {
  it("holds one key row for each of the 21 maps", () => {
    expect(MAPS).toHaveLength(21);
    expect(Object.keys(KEY_ROWS).sort()).toEqual([...MAPS].sort());
  });

  it("folds shifted keys for exactly the seven maps whose programs COPY CSSTRPFY", () => {
    const folding = MAPS.filter((map) => KEY_ROWS[map].foldShifted);
    expect(folding.sort()).toEqual([...FOLDING_MAPS].sort());
  });
});

describe("PF-key labels and bindings of each map", () => {
  it.each(MAPS)("%s labels its row-24 keys in field order, with each field's DRK flag", (map) => {
    const expected = items(MAP_LABELS[map]);
    expect(keyRowItems(map)).toEqual(expected);
    expect(keyRowItems(KEY_ROWS[map])).toEqual(expected);
  });

  it.each(MAPS)("%s binds each labelled key to the screen's action for it", (map) => {
    const folds = FOLDING_MAPS.includes(map);
    const options: PfKeyResolveOptions = {
      foldShifted: KEY_ROWS[map].foldShifted,
      hasInvalidKeyHandler: true,
    };
    for (const [key] of MAP_LABELS[map]) {
      const action = countingAction();
      const handlers = serving(key, action.run);
      const handled: PfKeyResolution = { kind: "handled", key, handler: action.run };
      expect(resolvePfKey(handlers, keydown(eventKeyOf(key)), options)).toEqual(handled);
      expect(resolvePfKey(handlers, keydown(eventKeyOf(key), { shiftKey: true }), options)).toEqual(
        folds && key !== "ENTER" ? handled : undefined,
      );
      expect(action.runs()).toBe(0);
    }
  });

  it("shows COUSR01's F12=Exit, which COUSR01C does not serve, as an invalid key", () => {
    // audit: COUSR01-038
    expect(keyRowItems("COUSR01")).toContainEqual({ key: "F12", label: "F12=Exit", dark: false });
    const action = countingAction();
    const handlers: PfKeyHandlers = { ENTER: action.run, F3: action.run, F4: action.run };
    expect(resolvePfKey(handlers, keydown("F12"), { hasInvalidKeyHandler: true })).toEqual({
      kind: "invalid",
      key: "F12",
    });
  });
});

interface ItemsCase {
  readonly title: string;
  readonly spec: KeyRowSpec;
  readonly expected: readonly ExpectedLabel[];
}

interface ItemsErrorCase {
  readonly title: string;
  readonly fields: KeyRowSpec["fields"];
  readonly message: string;
}

describe("keyRowItems", () => {
  it.each<ItemsCase>([
    {
      title: "a spec with no fields has no labels",
      spec: { foldShifted: true, fields: [] },
      expected: [],
    },
    {
      title: "a blank field adds no label",
      spec: {
        foldShifted: false,
        fields: [
          { text: "   ", dark: true },
          { text: "F3=Exit", dark: false },
        ],
      },
      expected: [["F3", "F3=Exit", false]],
    },
    {
      title: "every label of a field takes that field's DRK flag",
      spec: {
        foldShifted: false,
        fields: [
          { text: "ENTER=Process F3=Exit", dark: false },
          { text: "F5=Save F12=Cancel", dark: true },
        ],
      },
      expected: [
        ["ENTER", "ENTER=Process", false],
        ["F3", "F3=Exit", false],
        ["F5", "F5=Save", true],
        ["F12", "F12=Cancel", true],
      ],
    },
  ])("$title", ({ spec, expected }) => {
    expect(keyRowItems(spec)).toEqual(items(expected));
  });

  it.each<ItemsErrorCase>([
    {
      title: "two fields label the same key",
      fields: [
        { text: "ENTER=Process F3=Exit", dark: false },
        { text: "F3=Back", dark: true },
      ],
      message: 'Key row ["ENTER=Process F3=Exit","F3=Back"] labels a key twice',
    },
    {
      title: "a field holds no key label",
      fields: [
        { text: "F3=Exit", dark: false },
        { text: "Continue", dark: false },
      ],
      message: 'Key row "Continue" holds no key label',
    },
    {
      title: "one field labels a key twice",
      fields: [{ text: "F7=Backward  F7=Page Up", dark: false }],
      message: 'Key row "F7=Backward  F7=Page Up" labels F7 twice',
    },
  ])("throws when $title", ({ fields, message }) => {
    expect(() => keyRowItems({ foldShifted: false, fields })).toThrow(new Error(message));
  });
});

interface ParseCase {
  readonly title: string;
  readonly text: string;
  readonly expected: readonly (readonly [key: PfKey, label: string])[];
}

interface ParseErrorCase {
  readonly title: string;
  readonly text: string;
  readonly message: string;
}

describe("parseKeyRow", () => {
  it.each<ParseCase>([
    { title: "empty text has no labels", text: "", expected: [] },
    { title: "blank text has no labels", text: " \t  ", expected: [] },
    {
      title: "double-spaced labels split at each key",
      text: "ENTER=Sign-on  F3=Exit",
      expected: [
        ["ENTER", "ENTER=Sign-on"],
        ["F3", "F3=Exit"],
      ],
    },
    {
      title: "single-spaced labels split alike",
      text: "ENTER=Process F3=Exit",
      expected: [
        ["ENTER", "ENTER=Process"],
        ["F3", "F3=Exit"],
      ],
    },
    {
      title: "a tab separates labels like a blank",
      text: "ENTER=Fetch\tF3=Back",
      expected: [
        ["ENTER", "ENTER=Fetch"],
        ["F3", "F3=Back"],
      ],
    },
    {
      title: "leading and trailing blanks are trimmed",
      text: "  F3=Exit ",
      expected: [["F3", "F3=Exit"]],
    },
    {
      title: "a label keeps its inner blanks",
      text: "ENTER=Search Cards  F3=Exit",
      expected: [
        ["ENTER", "ENTER=Search Cards"],
        ["F3", "F3=Exit"],
      ],
    },
    {
      title: "BMS && is one ampersand",
      text: "F3=Save&&Exit  F4=Clear",
      expected: [
        ["F3", "F3=Save&Exit"],
        ["F4", "F4=Clear"],
      ],
    },
    {
      title: "F10 to F12 are not read as F1",
      text: "F1=Help F10=Save F11=Print F12=Cancel",
      expected: [
        ["F1", "F1=Help"],
        ["F10", "F10=Save"],
        ["F11", "F11=Print"],
        ["F12", "F12=Cancel"],
      ],
    },
    {
      title: "a key name not after a blank is label text",
      text: "ENTER=Go/F3=Back",
      expected: [["ENTER", "ENTER=Go/F3=Back"]],
    },
    {
      title: "labels keep the order of the text",
      text: "F8=Next Auth  F3=Back  ENTER=Continue",
      expected: [
        ["F8", "F8=Next Auth"],
        ["F3", "F3=Back"],
        ["ENTER", "ENTER=Continue"],
      ],
    },
    {
      title: "a key with nothing after = is still a label",
      text: "F3=",
      expected: [["F3", "F3="]],
    },
  ])("$title", ({ text, expected }) => {
    const labels = expected.map(([key, label]): PfKeyLabel => ({ key, label }));
    expect(parseKeyRow(text)).toEqual(labels);
  });

  it.each<ParseErrorCase>([
    {
      title: "the text holds no key",
      text: "Continue",
      message: 'Key row "Continue" holds no key label',
    },
    {
      title: "the key name is lower case",
      text: "f3=Exit",
      message: 'Key row "f3=Exit" holds no key label',
    },
    {
      title: "the key is PF13 or above",
      text: "F13=Help",
      message: 'Key row "F13=Help" holds no key label',
    },
    {
      title: "the key is not followed by =",
      text: "F3 = Exit",
      message: 'Key row "F3 = Exit" holds no key label',
    },
    {
      title: "the key is joined to the text before it",
      text: "XF3=Exit",
      message: 'Key row "XF3=Exit" holds no key label',
    },
    {
      title: "the text is an escaped ampersand only",
      text: "&&",
      message: 'Key row "&&" holds no key label',
    },
    {
      title: "text precedes the first key",
      text: "Press F3=Exit",
      message: 'Key row "Press F3=Exit" has text before its first key label',
    },
    {
      title: "a key appears twice",
      text: "F3=Exit  F3=Back",
      message: 'Key row "F3=Exit  F3=Back" labels F3 twice',
    },
    {
      title: "a key appears twice in text that the message quotes in source form",
      text: "ENTER=Fetch  F3=Save&&Exit  F3=Back",
      message: 'Key row "ENTER=Fetch  F3=Save&&Exit  F3=Back" labels F3 twice',
    },
  ])("throws when $title", ({ text, message }) => {
    expect(() => parseKeyRow(text)).toThrow(new Error(message));
  });
});

interface EventKeyCase {
  readonly name: string;
  readonly key: PfKey;
}

interface ModifierCase {
  readonly title: string;
  readonly flags: Partial<Omit<PfKeyEventLike, "key">>;
}

interface ShiftCase {
  readonly name: string;
  readonly foldShifted: boolean | undefined;
  readonly expected: PfKey | undefined;
}

describe("keyFromEvent", () => {
  it.each<EventKeyCase>([
    { name: "Enter", key: "ENTER" },
    { name: "F1", key: "F1" },
    { name: "F2", key: "F2" },
    { name: "F3", key: "F3" },
    { name: "F4", key: "F4" },
    { name: "F5", key: "F5" },
    { name: "F6", key: "F6" },
    { name: "F7", key: "F7" },
    { name: "F8", key: "F8" },
    { name: "F9", key: "F9" },
    { name: "F10", key: "F10" },
    { name: "F11", key: "F11" },
    { name: "F12", key: "F12" },
  ])("reads $name as $key, whether or not shifted keys fold", ({ name, key }) => {
    expect(keyFromEvent(keydown(name))).toBe(key);
    expect(keyFromEvent(keydown(name), true)).toBe(key);
  });

  it.each([
    "ENTER",
    "enter",
    "f3",
    "F0",
    "F13",
    "F24",
    "Escape",
    "Tab",
    " ",
    "a",
    "",
    "Unidentified",
    "constructor",
    "toString",
    "__proto__",
  ])("reads %j as no key", (name) => {
    expect(keyFromEvent(keydown(name))).toBeUndefined();
    expect(keyFromEvent(keydown(name), true)).toBeUndefined();
  });

  it.each<ModifierCase>([
    { title: "an auto-repeated keydown", flags: { repeat: true } },
    { title: "a keydown during IME composition", flags: { isComposing: true } },
    { title: "a key with Ctrl held", flags: { ctrlKey: true } },
    { title: "a key with Alt held", flags: { altKey: true } },
    { title: "a key with Meta held", flags: { metaKey: true } },
    { title: "a key with Ctrl and Shift held", flags: { ctrlKey: true, shiftKey: true } },
  ])("sends no key for $title", ({ flags }) => {
    for (const name of ["Enter", "F3", "F12"]) {
      expect(keyFromEvent(keydown(name, flags))).toBeUndefined();
      expect(keyFromEvent(keydown(name, flags), true)).toBeUndefined();
    }
  });

  it.each<ShiftCase>([
    { name: "F1", foldShifted: true, expected: "F1" },
    { name: "F3", foldShifted: true, expected: "F3" },
    { name: "F12", foldShifted: true, expected: "F12" },
    { name: "F3", foldShifted: false, expected: undefined },
    { name: "F12", foldShifted: false, expected: undefined },
    { name: "F3", foldShifted: undefined, expected: undefined },
    { name: "Enter", foldShifted: true, expected: undefined },
    { name: "Enter", foldShifted: false, expected: undefined },
  ])("reads Shift+$name as $expected when foldShifted is $foldShifted", (shift) => {
    expect(keyFromEvent(keydown(shift.name, { shiftKey: true }), shift.foldShifted)).toBe(
      shift.expected,
    );
  });
});

interface TargetCase {
  readonly title: string;
  readonly target: PfKeyTarget | undefined;
  readonly expected: boolean;
}

describe("enterHandledNatively", () => {
  it.each<TargetCase>([
    { title: "no element target", target: undefined, expected: false },
    { title: "an input outside any form", target: targetAt("INPUT", false), expected: false },
    { title: "a div outside any form", target: targetAt("DIV", false), expected: false },
    { title: "the document body", target: targetAt("BODY", false), expected: false },
    { title: "an input inside a form", target: targetAt("INPUT", true), expected: true },
    { title: "a div inside a form", target: targetAt("DIV", true), expected: true },
    { title: "an editable element", target: targetAt("DIV", false, true), expected: true },
    { title: "a button", target: targetAt("BUTTON", false), expected: true },
    { title: "a lower-case button tag", target: targetAt("button", false), expected: true },
    { title: "a link", target: targetAt("A", false), expected: true },
    { title: "a text area", target: targetAt("TEXTAREA", false), expected: true },
    { title: "a select", target: targetAt("SELECT", false), expected: true },
  ])("is $expected on $title", ({ target, expected }) => {
    expect(enterHandledNatively(target)).toBe(expected);
  });
});

interface ResolveCase {
  readonly title: string;
  readonly handlers: PfKeyHandlers;
  readonly event: PfKeyEventLike;
  readonly options?: PfKeyResolveOptions;
  readonly target?: PfKeyTarget;
  readonly expected: PfKeyResolution | undefined;
}

describe("resolvePfKey", () => {
  const action = countingAction();
  const invalidKeys: PfKeyResolveOptions = { hasInvalidKeyHandler: true };

  it.each<ResolveCase>([
    {
      title: "a served F-key resolves to its action",
      handlers: serving("F5", action.run),
      event: keydown("F5"),
      expected: { kind: "handled", key: "F5", handler: action.run },
    },
    {
      title: "a served F-key in a form field still resolves to its action",
      handlers: serving("F5", action.run),
      event: keydown("F5"),
      target: targetAt("INPUT", true),
      expected: { kind: "handled", key: "F5", handler: action.run },
    },
    {
      title: "an unserved F-key is invalid where the screen shows the invalid-key message",
      handlers: serving("F3", action.run),
      event: keydown("F9"),
      options: invalidKeys,
      expected: { kind: "invalid", key: "F9" },
    },
    {
      title: "an unserved F-key is left to the browser without invalid-key handling",
      handlers: serving("F3", action.run),
      event: keydown("F9"),
      expected: undefined,
    },
    {
      title: "an unserved F-key is left to the browser when invalid-key handling is off",
      handlers: serving("F3", action.run),
      event: keydown("F9"),
      options: { hasInvalidKeyHandler: false },
      expected: undefined,
    },
    {
      title: "a served ENTER with no element target resolves to its action",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter"),
      expected: { kind: "handled", key: "ENTER", handler: action.run },
    },
    {
      title: "a served ENTER on an input outside any form resolves to its action",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter"),
      target: targetAt("INPUT", false),
      expected: { kind: "handled", key: "ENTER", handler: action.run },
    },
    {
      title: "ENTER in a form is left to the form's implicit submission",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter"),
      target: targetAt("INPUT", true),
      expected: undefined,
    },
    {
      title: "ENTER on a focused button is left to that button",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter"),
      target: targetAt("BUTTON", false),
      expected: undefined,
    },
    {
      title: "an unserved ENTER is never invalid",
      handlers: serving("F3", action.run),
      event: keydown("Enter"),
      options: invalidKeys,
      expected: undefined,
    },
    {
      title: "a key that is no PF key is left to the browser",
      handlers: serving("F3", action.run),
      event: keydown("a"),
      options: invalidKeys,
      expected: undefined,
    },
    {
      title: "Shift+F7 is F7 where shifted keys fold",
      handlers: serving("F7", action.run),
      event: keydown("F7", { shiftKey: true }),
      options: { foldShifted: true },
      expected: { kind: "handled", key: "F7", handler: action.run },
    },
    {
      title: "Shift+F7 is no key where shifted keys do not fold",
      handlers: serving("F7", action.run),
      event: keydown("F7", { shiftKey: true }),
      options: invalidKeys,
      expected: undefined,
    },
    {
      title: "an unserved Shift+F7 is invalid where shifted keys fold",
      handlers: serving("F3", action.run),
      event: keydown("F7", { shiftKey: true }),
      options: { foldShifted: true, hasInvalidKeyHandler: true },
      expected: { kind: "invalid", key: "F7" },
    },
    {
      title: "Ctrl+F5 is left to the browser",
      handlers: serving("F5", action.run),
      event: keydown("F5", { ctrlKey: true }),
      options: invalidKeys,
      expected: undefined,
    },
    {
      title: "an auto-repeated served key is left to the browser",
      handlers: serving("F5", action.run),
      event: keydown("F5", { repeat: true }),
      options: invalidKeys,
      expected: undefined,
    },
  ])("$title", ({ handlers, event, options, target, expected }) => {
    expect(resolvePfKey(handlers, event, options, target)).toEqual(expected);
    expect(action.runs()).toBe(0);
  });

  it("decides without running the action, which runs when the caller calls it", () => {
    const save = countingAction();
    const resolution = resolvePfKey(serving("F5", save.run), keydown("F5"));
    expect(save.runs()).toBe(0);
    expect(resolution?.kind).toBe("handled");
    if (resolution?.kind === "handled") {
      resolution.handler();
    }
    expect(save.runs()).toBe(1);
  });
});

describe("header values", () => {
  it("keeps both titles at the 40 columns of their fields, padding included", () => {
    // audit: COSGN00-039
    expect(TITLE01).toBe(`${" ".repeat(6)}AWS Mainframe Modernization${" ".repeat(7)}`);
    // audit: COSGN00-042
    expect(TITLE02).toBe(`${" ".repeat(14)}CardDemo${" ".repeat(18)}`);
    expect([TITLE01.length, TITLE02.length]).toEqual([40, 40]);
  });

  it.each([
    ["THANK_YOU", THANK_YOU, "Thank you for using CardDemo application...", 7],
    ["INVALID_KEY", INVALID_KEY, "Invalid key pressed. Please see below...", 10],
  ])("holds %s as its PIC X(50) field value", (_name, value, text, padding) => {
    expect(value).toBe(`${text}${" ".repeat(padding)}`);
    expect(value).toHaveLength(50);
  });

  // audit: COSGN00-040
  it.each([
    [new Date(2022, 6, 18), "07/18/22"],
    [new Date(2000, 0, 1), "01/01/00"],
    [new Date(1999, 11, 31, 23, 59, 59), "12/31/99"],
    [new Date(2009, 8, 5), "09/05/09"],
  ])("formats %s as the header date %s", (date, expected) => {
    expect(formatHeaderDate(date)).toBe(expected);
  });

  it("takes the header year without its sign, as the unsigned PIC 9(04) year stores it", () => {
    const date = new Date(2022, 6, 18);
    date.setFullYear(-2022);
    expect(formatHeaderDate(date)).toBe("07/18/22");
  });

  // audit: COSGN00-043
  it.each([
    [new Date(2022, 6, 18, 9, 5, 7), "09:05:07"],
    [new Date(2022, 6, 18, 0, 0, 0), "00:00:00"],
    [new Date(2022, 6, 18, 12, 0, 0), "12:00:00"],
    [new Date(2022, 6, 18, 23, 59, 59), "23:59:59"],
  ])("formats %s as the 8-character header time %s", (date, expected) => {
    expect(formatHeaderTime(date)).toBe(expected);
    expect(formatHeaderTime(date)).toHaveLength(8);
  });

  it.each([
    ["formatHeaderDate", formatHeaderDate],
    ["formatHeaderTime", formatHeaderTime],
  ])("%s rejects an invalid Date instead of rendering NaN", (_name, format) => {
    expect(() => format(new Date(Number.NaN))).toThrow(RangeError);
  });
});
