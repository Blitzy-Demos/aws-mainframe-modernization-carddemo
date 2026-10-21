import { describe, expect, it } from "vitest";
import {
  type CoactupKeyState,
  type CocrdupKeyState,
  type CotrtupKeyState,
  KEY_ROWS,
  type KeyRowMap,
  type KeyRowSpec,
  PF_KEYS,
  type PfKey,
  type PfKeyAvailability,
  type PfKeyEventLike,
  type PfKeyHandlers,
  type PfKeyItem,
  type PfKeyLabel,
  type PfKeyResolution,
  type PfKeyResolveOptions,
  type PfKeyRole,
  type PfKeyRoleActions,
  type PfKeyTarget,
  type ShownPfKeyItem,
  bindPfKeys,
  coactupKeys,
  cocrdupKeys,
  cotrtupKeys,
  enterHandledNatively,
  keyFromEvent,
  keyRowItems,
  keyRowItemsFor,
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
 * Unit tests of the PF-key labels and bindings and of the header values.
 *
 * Every expected key, label and `DRK` flag is written by hand from the row-24 fields of the BMS
 * maps and the PF-key rows of `modern/audit/screens.md`, and every set of served keys from the AID
 * `EVALUATE` rows of `modern/audit/online/<PROGRAM>.md`; none is read from `KEY_ROWS`. The header
 * texts are written by hand from `COTTL01Y`, `CSMSG01Y` and `CSDAT01Y`.
 *
 * COACTUP, COCRDUP and COTRTUP decide their keys by program state. For every state of each, the
 * role of all 13 keys, the labels shown and the action each key and Shift+key runs are written by
 * hand from the audit rows each case's comment names (among them COACTUPC-003, COACTUPC-006,
 * COACTUPC-082, COCRDUPC-005, COCRDUPC-008 and COTRTUPC-042), never computed from `coactupKeys`,
 * `cocrdupKeys` or `cotrtupKeys`, and checked against those functions, `keyRowItemsFor` and
 * `bindPfKeys`. Sessions then walk each screen from state to state: labels hidden, shown and hidden
 * again, keys refused, and resets that bring back the fresh screen's labels.
 *
 * `usePfKeys` needs React and a DOM, which this Node project does not provide; the decisions it
 * delegates to (`resolvePfKey`, `keyFromEvent`, `enterHandledNatively`) are proved here, and the
 * screens' render tests prove the hook's effect.
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
 * COTRTUPC), whose `YYYY-STORE-PFKEY` folds PF13–PF24 onto PF1–PF12 (`CSSTRPFY.cpy:54-77`).
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

/** Asserts a map's row-24 labels in field order with their DRK flags, by name and by entry. */
function expectLabels(map: KeyRowMap, labels: readonly ExpectedLabel[]): void {
  const expected = labels.map(([key, label, dark]): PfKeyItem => ({ key, label, dark }));
  expect(keyRowItems(map)).toEqual(expected);
  expect(keyRowItems(KEY_ROWS[map])).toEqual(expected);
}

/**
 * Asserts how every key resolves on `map`'s screen while its program serves exactly `served`, with
 * the screen's invalid-key path on: `onInvalidKey`, through which the screen shows `INVALID_KEY`,
 * or submits as ENTER where its program processes an unserved key as ENTER.
 *
 * - Each served key runs its own action, never another key's, and no action runs while deciding.
 * - Where the map folds shifted keys, Shift+F-key resolves as the F-key; where it does not,
 *   Shift+F-key is PF13–PF24, which the program serves under no branch, so it is invalid.
 * - Every unserved F-key is invalid; ENTER, served or not, is never invalid, and Shift+Enter is no
 *   key.
 */
function expectServes(map: KeyRowMap, served: readonly PfKey[]): void {
  const foldShifted = KEY_ROWS[map].foldShifted;
  const options: PfKeyResolveOptions = { foldShifted, hasInvalidKeyHandler: true };
  const ran: PfKey[] = [];
  const handlers: PfKeyHandlers = {};
  for (const key of served) {
    handlers[key] = () => {
      ran.push(key);
    };
  }
  for (const key of PF_KEYS) {
    const handler = handlers[key];
    const plain = resolvePfKey(handlers, keydown(eventKeyOf(key)), options);
    const shifted = resolvePfKey(handlers, keydown(eventKeyOf(key), { shiftKey: true }), options);
    if (key === "ENTER") {
      expect(plain, `${map} ENTER`).toEqual(
        handler === undefined ? undefined : { kind: "handled", key, handler },
      );
      expect(shifted, `${map} Shift+ENTER`).toBeUndefined();
      continue;
    }
    const own: PfKeyResolution =
      handler === undefined ? { kind: "invalid", key } : { kind: "handled", key, handler };
    expect(plain, `${map} ${key}`).toEqual(own);
    expect(shifted, `${map} Shift+${key}`).toEqual(foldShifted ? own : { kind: "invalid", key });
  }
  expect(ran).toEqual([]);
}

/** Resolves Shift+`key` on `map`'s screen with `handlers` and the invalid-key path on. */
function resolveShifted(
  map: KeyRowMap,
  handlers: PfKeyHandlers,
  key: PfKey,
): PfKeyResolution | undefined {
  return resolvePfKey(handlers, keydown(eventKeyOf(key), { shiftKey: true }), {
    foldShifted: KEY_ROWS[map].foldShifted,
    hasInvalidKeyHandler: true,
  });
}

/**
 * The hand-written keys of one state of a state-keyed screen: the keys that run their own action,
 * the keys that start the screen over, the role of every other key, and the keys whose labels
 * show.
 */
interface StateKeys<State extends string> {
  readonly state: State;
  readonly own: readonly PfKey[];
  readonly reset?: readonly PfKey[];
  readonly others: PfKeyRole;
  readonly shown: readonly PfKey[];
}

/** The role `expected` gives `key`. */
function roleIn(expected: StateKeys<string>, key: PfKey): PfKeyRole {
  if (expected.own.includes(key)) {
    return "own";
  }
  return expected.reset?.includes(key) === true ? "reset" : expected.others;
}

/** What runs for `key` in `role`, as `boundScreen` records it. */
function runOf(key: PfKey, role: PfKeyRole): string {
  switch (role) {
    case "own":
      return key;
    case "enter":
      return "ENTER";
    case "reset":
      return "reset";
    case "invalid":
      return `invalid ${key}`;
  }
}

/**
 * Binds every key of `availability` as a state-keyed screen does, to actions that record what
 * ran: a key's own action records the key, the reset `reset`, the invalid-key message
 * `invalid <key>`. Asserts that binding runs nothing, and returns a press: it resolves the keydown
 * of `key`, with Shift when `shiftKey`, as `usePfKeys` does with `map`'s shifted-key folding and
 * the invalid-key path on, runs the resolved action, and returns what ran.
 */
function boundScreen(
  map: KeyRowMap,
  availability: PfKeyAvailability,
): (key: PfKey, shiftKey?: boolean) => string[] {
  const ran: string[] = [];
  const own: PfKeyHandlers = {};
  for (const key of PF_KEYS) {
    own[key] = () => {
      ran.push(key);
    };
  }
  const handlers = bindPfKeys(availability, {
    own,
    reset: () => {
      ran.push("reset");
    },
    invalid: (key) => {
      ran.push(`invalid ${key}`);
    },
  });
  expect(ran, `${map}: binding runs no action`).toEqual([]);
  const options: PfKeyResolveOptions = {
    foldShifted: KEY_ROWS[map].foldShifted,
    hasInvalidKeyHandler: true,
  };
  return (key, shiftKey = false) => {
    const resolution = resolvePfKey(handlers, keydown(eventKeyOf(key), { shiftKey }), options);
    if (resolution?.kind === "handled") {
      resolution.handler();
    } else if (resolution !== undefined) {
      // Every key is bound; one that is not records an entry no expectation holds.
      ran.push(`unbound ${resolution.key}`);
    }
    return ran.splice(0);
  };
}

/**
 * Asserts one state of `map`'s screen against the hand-written `expected`: the role of every key;
 * the shown keys and the row-24 labels in map order with their DRK flags (`labels`), each shown or
 * hidden; and that each key and Shift+key runs the action of its role (the three state-keyed maps
 * fold shifted keys), Shift+Enter being no key.
 */
function expectStateKeys(
  map: KeyRowMap,
  labels: readonly ExpectedLabel[],
  availability: PfKeyAvailability,
  expected: StateKeys<string>,
): void {
  const at = `${map} state ${expected.state}`;
  expect(availability.roles, at).toEqual(
    Object.fromEntries(PF_KEYS.map((key): [PfKey, PfKeyRole] => [key, roleIn(expected, key)])),
  );
  expect(availability.shown, at).toEqual(new Set(expected.shown));
  const items = labels.map(([key, label, dark]): ShownPfKeyItem => ({
    key,
    label,
    dark,
    shown: expected.shown.includes(key),
  }));
  expect(keyRowItemsFor(map, availability), at).toEqual(items);
  expect(keyRowItemsFor(KEY_ROWS[map], availability), at).toEqual(items);
  const press = boundScreen(map, availability);
  for (const key of PF_KEYS) {
    const runs = [runOf(key, roleIn(expected, key))];
    expect(press(key), `${at} ${key}`).toEqual(runs);
    expect(press(key, true), `${at} Shift+${key}`).toEqual(key === "ENTER" ? [] : runs);
  }
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

describe("row-24 labels and key bindings of the screens whose programs fold no shifted key", () => {
  // audit: COSGN00-046
  // audit: COSGN00-047
  // audit: COSGN00C-005
  it("COSGN00 labels ENTER=Sign-on and F3=Exit; COSGN00C serves ENTER and F3 alone", () => {
    expectLabels("COSGN00", [
      ["ENTER", "ENTER=Sign-on", false],
      ["F3", "F3=Exit", false],
    ]);
    expectServes("COSGN00", ["ENTER", "F3"]);
    // Shift+F3 is PF15, which `WHEN OTHER` takes: the invalid-key message, never the F3 exit.
    const exit = countingAction();
    expect(resolveShifted("COSGN00", serving("F3", exit.run), "F3")).toEqual({
      kind: "invalid",
      key: "F3",
    });
    expect(exit.runs()).toBe(0);
  });

  // audit: COMEN01-035
  // audit: COMEN01-036
  // audit: COMEN01C-007
  it("COMEN01 labels ENTER=Continue and F3=Exit; COMEN01C serves ENTER and F3 alone", () => {
    expectLabels("COMEN01", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Exit", false],
    ]);
    expectServes("COMEN01", ["ENTER", "F3"]);
  });

  // audit: COADM01-035
  // audit: COADM01-036
  // audit: COADM01C-021
  it("COADM01 labels ENTER=Continue and F3=Exit; COADM01C serves ENTER and F3 alone", () => {
    expectLabels("COADM01", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Exit", false],
    ]);
    expectServes("COADM01", ["ENTER", "F3"]);
  });

  // audit: COTRN00-096
  // audit: COTRN00-097
  // audit: COTRN00-098
  // audit: COTRN00-099
  // audit: COTRN00C-006
  it("COTRN00 labels ENTER, F3, F7 and F8; COTRN00C serves those four alone", () => {
    expectLabels("COTRN00", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
      ["F7", "F7=Backward", false],
      ["F8", "F8=Forward", false],
    ]);
    expectServes("COTRN00", ["ENTER", "F3", "F7", "F8"]);
  });

  // audit: COTRN01-063
  // audit: COTRN01-064
  // audit: COTRN01-065
  // audit: COTRN01-066
  // audit: COTRN01C-007
  it("COTRN01 labels ENTER, F3, F4 and F5; COTRN01C serves those four alone", () => {
    expectLabels("COTRN01", [
      ["ENTER", "ENTER=Fetch", false],
      ["F3", "F3=Back", false],
      ["F4", "F4=Clear", false],
      ["F5", "F5=Browse Tran.", false],
    ]);
    expectServes("COTRN01", ["ENTER", "F3", "F4", "F5"]);
  });

  // audit: COTRN02-068
  // audit: COTRN02-069
  // audit: COTRN02-070
  // audit: COTRN02-071
  // audit: COTRN02C-006
  it("COTRN02 labels ENTER, F3, F4 and F5; COTRN02C serves those four alone", () => {
    expectLabels("COTRN02", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
      ["F4", "F4=Clear", false],
      ["F5", "F5=Copy Last Tran.", false],
    ]);
    expectServes("COTRN02", ["ENTER", "F3", "F4", "F5"]);
  });

  // audit: CORPT00-049
  // audit: CORPT00-050
  // audit: CORPT00C-005
  it("CORPT00 labels ENTER=Continue and F3=Back; CORPT00C serves ENTER and F3 alone", () => {
    expectLabels("CORPT00", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
    ]);
    expectServes("CORPT00", ["ENTER", "F3"]);
  });

  // audit: COBIL00-031
  // audit: COBIL00-032
  // audit: COBIL00-033
  // audit: COBIL00C-006
  it("COBIL00 labels ENTER, F3 and F4; COBIL00C serves those three alone", () => {
    expectLabels("COBIL00", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
      ["F4", "F4=Clear", false],
    ]);
    expectServes("COBIL00", ["ENTER", "F3", "F4"]);
  });

  // audit: COUSR00-096
  // audit: COUSR00-097
  // audit: COUSR00-098
  // audit: COUSR00-099
  // audit: COUSR00C-005
  it("COUSR00 labels ENTER, F3, F7 and F8; COUSR00C serves those four alone", () => {
    expectLabels("COUSR00", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
      ["F7", "F7=Backward", false],
      ["F8", "F8=Forward", false],
    ]);
    expectServes("COUSR00", ["ENTER", "F3", "F7", "F8"]);
  });

  // audit: COUSR01-035
  // audit: COUSR01-036
  // audit: COUSR01-037
  // audit: COUSR01-038
  // audit: COUSR01C-009
  it("COUSR01 labels F12=Exit, which COUSR01C does not serve, so F12 is an invalid key", () => {
    expectLabels("COUSR01", [
      ["ENTER", "ENTER=Add User", false],
      ["F3", "F3=Back", false],
      ["F4", "F4=Clear", false],
      ["F12", "F12=Exit", false],
    ]);
    expectServes("COUSR01", ["ENTER", "F3", "F4"]);
  });

  // audit: COUSR02-036
  // audit: COUSR02-037
  // audit: COUSR02-038
  // audit: COUSR02-039
  // audit: COUSR02-040
  // audit: COUSR02C-009
  it("COUSR02 labels ENTER, F3=Save&Exit, F4, F5 and F12; COUSR02C serves those five alone", () => {
    expectLabels("COUSR02", [
      ["ENTER", "ENTER=Fetch", false],
      ["F3", "F3=Save&Exit", false],
      ["F4", "F4=Clear", false],
      ["F5", "F5=Save", false],
      ["F12", "F12=Cancel", false],
    ]);
    expectServes("COUSR02", ["ENTER", "F3", "F4", "F5", "F12"]);
  });

  // audit: COUSR03-033
  // audit: COUSR03-034
  // audit: COUSR03-035
  // audit: COUSR03-036
  // audit: COUSR03C-009
  it("COUSR03 labels ENTER, F3, F4 and F5; COUSR03C also serves the unlabelled F12", () => {
    expectLabels("COUSR03", [
      ["ENTER", "ENTER=Fetch", false],
      ["F3", "F3=Back", false],
      ["F4", "F4=Clear", false],
      ["F5", "F5=Delete", false],
    ]);
    expectServes("COUSR03", ["ENTER", "F3", "F4", "F5", "F12"]);
  });

  // audit: COPAU00-111
  // audit: COPAU00-112
  // audit: COPAU00-113
  // audit: COPAU00-114
  // audit: COPAUS0C-004
  it("COPAU00 labels ENTER, F3, F7 and F8; COPAUS0C serves those four alone", () => {
    expectLabels("COPAU00", [
      ["ENTER", "ENTER=Continue", false],
      ["F3", "F3=Back", false],
      ["F7", "F7=Backward", false],
      ["F8", "F8=Forward", false],
    ]);
    expectServes("COPAU00", ["ENTER", "F3", "F7", "F8"]);
  });

  // audit: COPAU01-061
  // audit: COPAU01-062
  // audit: COPAU01-063
  // audit: COPAUS1C-005
  it("COPAU01 labels F3, F5 and F8; COPAUS1C serves those and the unlabelled ENTER", () => {
    expectLabels("COPAU01", [
      ["F3", "F3=Back", false],
      ["F5", "F5=Mark/Remove Fraud", false],
      ["F8", "F8=Next Auth", false],
    ]);
    expectServes("COPAU01", ["ENTER", "F3", "F5", "F8"]);
  });
});

/** COACTUP's row 24 (`COACTUP.bms:493-507`): `FKEYS`, then the dark `FKEY05` and `FKEY12`. */
const COACTUP_LABELS: readonly ExpectedLabel[] = [
  ["ENTER", "ENTER=Process", false],
  ["F3", "F3=Exit", false],
  ["F5", "F5=Save", true],
  ["F12", "F12=Cancel", true],
];

/** COCRDUP's row 24 (`COCRDUP.bms:158-167`): `FKEYS`, then the one dark field `FKEYSC`. */
const COCRDUP_LABELS: readonly ExpectedLabel[] = [
  ["ENTER", "ENTER=Process", false],
  ["F3", "F3=Exit", false],
  ["F5", "F5=Save", true],
  ["F12", "F12=Cancel", true],
];

/**
 * COTRTUP's row 24 (`COTRTUP.bms:111-135`): `FKEYS`, then the dark `FKEY04`, `FKEY05`, `FKEY06`
 * and `FKEY12`.
 */
const COTRTUP_LABELS: readonly ExpectedLabel[] = [
  ["ENTER", "ENTER=Process", false],
  ["F3", "F3=Exit", false],
  ["F4", "F4=Delete", true],
  ["F5", "F5=Save", true],
  // The map's dark FKEY06 text; COTRTUPC never shows or accepts it (COTRTUP-036, X-SCREENS-12):
  // no state below shows F6 or gives it its own action.
  ["F6", "F6=Add", true],
  ["F12", "F12=Cancel", true],
];

describe("row-24 labels and key bindings of the screens whose programs COPY CSSTRPFY", () => {
  // audit: COACTVW-107
  // audit: COACTVWC-003
  it("COACTVW labels F3=Exit; COACTVWC serves ENTER and F3, and PF15 acts as PF3", () => {
    expectLabels("COACTVW", [["F3", "F3=Exit", false]]);
    expectServes("COACTVW", ["ENTER", "F3"]);
    const exit = countingAction();
    expect(resolveShifted("COACTVW", serving("F3", exit.run), "F3")).toEqual({
      kind: "handled",
      key: "F3",
      handler: exit.run,
    });
  });

  // audit: COACTUP-127
  // audit: COACTUP-128
  // audit: COACTUP-135
  // audit: COACTUP-136
  // audit: COACTUP-137
  // audit: COACTUP-138
  // audit: COACTUPC-003
  // audit: COACTUPC-006
  // audit: COACTUPC-069
  // audit: COACTUPC-083
  it.each<StateKeys<CoactupKeyState>>([
    // COACTUPC-001, COACTUPC-003: before a fetch F5 and F12 are ENTER, and neither label shows.
    { state: "not-fetched", own: ["ENTER", "F3"], others: "enter", shown: ["ENTER", "F3"] },
    // COACTUPC-069: F12 cancels once details show; its label shows only after a change
    // (COACTUPC-082).
    { state: "S", own: ["ENTER", "F3", "F12"], others: "enter", shown: ["ENTER", "F3"] },
    { state: "E", own: ["ENTER", "F3", "F12"], others: "enter", shown: ["ENTER", "F3", "F12"] },
    // COACTUPC-003, COACTUPC-082: F5 saves, and shows, only while changes await confirmation.
    {
      state: "N",
      own: ["ENTER", "F3", "F5", "F12"],
      others: "enter",
      shown: ["ENTER", "F3", "F5", "F12"],
    },
    // COACTUPC-006, COACTUPC-083: every key but F3 starts over; F12=Cancel shows in L and F.
    { state: "C", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "L", own: ["F3"], others: "reset", shown: ["ENTER", "F3", "F12"] },
    { state: "F", own: ["F3"], others: "reset", shown: ["ENTER", "F3", "F12"] },
  ])(
    "COACTUP in state $state labels ENTER and F3, then the dark F5=Save and F12=Cancel, by state",
    (expected) => {
      expectLabels("COACTUP", COACTUP_LABELS);
      expectStateKeys("COACTUP", COACTUP_LABELS, coactupKeys(expected.state), expected);
    },
  );

  // audit: COCRDLI-079
  // audit: COCRDLI-080
  // audit: COCRDLI-081
  // audit: COCRDLIC-003
  it("COCRDLI labels F3, F7 and F8; COCRDLIC serves those and ENTER, and folds F13 to F24", () => {
    expectLabels("COCRDLI", [
      ["F3", "F3=Exit", false],
      ["F7", "F7=Backward", false],
      ["F8", "F8=Forward", false],
    ]);
    expectServes("COCRDLI", ["ENTER", "F3", "F7", "F8"]);
    const exit = countingAction();
    const backward = countingAction();
    const forward = countingAction();
    const handlers: PfKeyHandlers = { F3: exit.run, F7: backward.run, F8: forward.run };
    expect(resolveShifted("COCRDLI", handlers, "F3")).toEqual({
      kind: "handled",
      key: "F3",
      handler: exit.run,
    });
    expect(resolveShifted("COCRDLI", handlers, "F7")).toEqual({
      kind: "handled",
      key: "F7",
      handler: backward.run,
    });
    expect(resolveShifted("COCRDLI", handlers, "F8")).toEqual({
      kind: "handled",
      key: "F8",
      handler: forward.run,
    });
  });

  // audit: COCRDSL-038
  // audit: COCRDSL-039
  // audit: COCRDSLC-003
  it("COCRDSL labels ENTER=Search Cards and F3=Exit; COCRDSLC serves those, F15 as F3", () => {
    expectLabels("COCRDSL", [
      ["ENTER", "ENTER=Search Cards", false],
      ["F3", "F3=Exit", false],
    ]);
    expectServes("COCRDSL", ["ENTER", "F3"]);
  });

  // audit: COCRDUP-034
  // audit: COCRDUP-041
  // audit: COCRDUP-042
  // audit: COCRDUP-043
  // audit: COCRDUP-044
  // audit: COCRDUPC-005
  // audit: COCRDUPC-006
  // audit: COCRDUPC-008
  it.each<StateKeys<CocrdupKeyState>>([
    // COCRDUPC-001, COCRDUPC-008: before a fetch F5 and F12 are submitted as ENTER, unlabelled.
    { state: "not-fetched", own: ["ENTER", "F3"], others: "enter", shown: ["ENTER", "F3"] },
    // COCRDUPC-006: F12 cancels once details show; the one dark field shows only in N
    // (COCRDUP-034).
    { state: "S", own: ["ENTER", "F3", "F12"], others: "enter", shown: ["ENTER", "F3"] },
    { state: "E", own: ["ENTER", "F3", "F12"], others: "enter", shown: ["ENTER", "F3"] },
    {
      state: "N",
      own: ["ENTER", "F3", "F5", "F12"],
      others: "enter",
      shown: ["ENTER", "F3", "F5", "F12"],
    },
    // COCRDUPC-005: on the result of C, L or F every key but F3 starts over (COCRDUPC-039: no
    // F5 or F12 label).
    { state: "C", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "L", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "F", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
  ])(
    "COCRDUP in state $state labels ENTER and F3, then the dark F5=Save F12=Cancel field, by state",
    (expected) => {
      expectLabels("COCRDUP", COCRDUP_LABELS);
      expectStateKeys("COCRDUP", COCRDUP_LABELS, cocrdupKeys(expected.state), expected);
    },
  );

  // audit: COTRTLI-088
  // audit: COTRTLI-089
  // audit: COTRTLI-090
  // audit: COTRTLI-091
  // audit: COTRTLI-092
  // audit: COTRTLIC-014
  it("COTRTLI labels F2, F3, F7, F8 and F10; COTRTLIC serves those and ENTER", () => {
    expectLabels("COTRTLI", [
      ["F2", "F2=Add", false],
      ["F3", "F3=Exit", false],
      ["F7", "F7=Page Up", false],
      ["F8", "F8=Page Dn", false],
      ["F10", "F10=Save", false],
    ]);
    expectServes("COTRTLI", ["ENTER", "F2", "F3", "F7", "F8", "F10"]);
  });

  // audit: COTRTUP-021
  // audit: COTRTUP-022
  // audit: COTRTUP-023
  // audit: COTRTUP-025
  // audit: COTRTUP-032
  // audit: COTRTUP-033
  // audit: COTRTUP-034
  // audit: COTRTUP-035
  // audit: COTRTUP-037
  // audit: COTRTUPC-003
  // audit: COTRTUPC-013
  // audit: COTRTUPC-014
  // audit: COTRTUPC-015
  // audit: COTRTUPC-022
  // audit: COTRTUPC-030
  // audit: COTRTUPC-031
  // audit: COTRTUPC-035
  // audit: COTRTUPC-036
  // audit: COTRTUPC-039
  // audit: COTRTUPC-040
  // audit: COTRTUPC-041
  // audit: COTRTUPC-042
  it.each<StateKeys<CotrtupKeyState>>([
    // COTRTUPC-003: the fresh screen shows ENTER=Process and F3=Exit alone.
    { state: "fresh", own: ["ENTER", "F3"], others: "invalid", shown: ["ENTER", "F3"] },
    // COTRTUPC-014, COTRTUPC-015, COTRTUPC-041: not found: F5 adds, F12 starts over.
    {
      state: "X",
      own: ["ENTER", "F3", "F5"],
      reset: ["F12"],
      others: "invalid",
      shown: ["ENTER", "F3", "F5", "F12"],
    },
    // COTRTUPC-013, COTRTUPC-031, COTRTUPC-041: details shown: F4 asks to delete, F12 starts over.
    {
      state: "S",
      own: ["ENTER", "F3", "F4"],
      reset: ["F12"],
      others: "invalid",
      shown: ["ENTER", "F3", "F4", "F12"],
    },
    // COTRTUPC-015, COTRTUPC-041: a new type: F12 starts over.
    {
      state: "R",
      own: ["ENTER", "F3"],
      reset: ["F12"],
      others: "invalid",
      shown: ["ENTER", "F3", "F12"],
    },
    // COTRTUPC-018, COTRTUPC-042: an edit error: ENTER and F3 alone.
    { state: "E", own: ["ENTER", "F3"], others: "invalid", shown: ["ENTER", "F3"] },
    // COTRTUPC-022, COTRTUPC-039: changes await confirmation: F5 saves, F12 cancels.
    {
      state: "N",
      own: ["ENTER", "F3", "F5", "F12"],
      others: "invalid",
      shown: ["ENTER", "F3", "F5", "F12"],
    },
    // COTRTUPC-039: cancelled over fetched values, with no F4, F5 or F12 label.
    { state: "B", own: ["ENTER", "F3"], others: "invalid", shown: ["ENTER", "F3"] },
    // COTRTUPC-039, COTRTUPC-042: cancelled over blank old values: every key but F3 starts over.
    { state: "B-blank", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    // COTRTUPC-031, COTRTUPC-036, COTRTUPC-040, COTRTUPC-042: a delete to confirm: ENTER is
    // invalid, its label dark with F3's; F5 is accepted although dark.
    { state: "9", own: ["F3", "F4", "F5", "F12"], others: "invalid", shown: ["F4", "F12"] },
    // COTRTUPC-035: a delete refused for child records: ENTER and F5 accepted, F4 and F12 invalid.
    { state: "8", own: ["ENTER", "F3", "F5"], others: "invalid", shown: ["ENTER", "F3"] },
    // COTRTUPC-030: after a delete done or failed, changes done or a lock error, every key but F3
    // starts over.
    { state: "7", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "6", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "C", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
    { state: "L", own: ["F3"], others: "reset", shown: ["ENTER", "F3"] },
  ])(
    "COTRTUP in state $state labels ENTER, F3 and dark F4, F5, F6, F12; COTRTUPC accepts by state",
    (expected) => {
      expectLabels("COTRTUP", COTRTUP_LABELS);
      expectStateKeys("COTRTUP", COTRTUP_LABELS, cotrtupKeys(expected.state), expected);
    },
  );
});

/** One step of a screen session: the state, the labels it shows, a key pressed and what ran. */
interface SessionStep<State extends string> {
  readonly state: State;
  readonly shows: readonly string[];
  readonly press: PfKey;
  readonly runs: string;
}

/** A session on one of the three state-keyed screens, its steps in order. */
type SessionCase =
  | {
      readonly title: string;
      readonly map: "COACTUP";
      readonly steps: readonly SessionStep<CoactupKeyState>[];
    }
  | {
      readonly title: string;
      readonly map: "COCRDUP";
      readonly steps: readonly SessionStep<CocrdupKeyState>[];
    }
  | {
      readonly title: string;
      readonly map: "COTRTUP";
      readonly steps: readonly SessionStep<CotrtupKeyState>[];
    };

/** One step of a session with the availability its state has. */
type StepKeys = readonly [step: SessionStep<string>, availability: PfKeyAvailability];

/** Each step of `session` with the availability of its state, from the map's state policy. */
function sessionKeys(session: SessionCase): StepKeys[] {
  switch (session.map) {
    case "COACTUP":
      return session.steps.map((step): StepKeys => [step, coactupKeys(step.state)]);
    case "COCRDUP":
      return session.steps.map((step): StepKeys => [step, cocrdupKeys(step.state)]);
    case "COTRTUP":
      return session.steps.map((step): StepKeys => [step, cotrtupKeys(step.state)]);
  }
}

describe("sessions on the screens whose programs decide keys by state", () => {
  // audit: COACTUPC-003
  // audit: COACTUPC-006
  // audit: COACTUPC-083
  // audit: COCRDUPC-005
  // audit: COCRDUPC-006
  // audit: COCRDUPC-008
  // audit: COTRTUPC-015
  // audit: COTRTUPC-030
  // audit: COTRTUPC-031
  // audit: COTRTUPC-035
  // audit: COTRTUPC-039
  // audit: COTRTUPC-040
  // audit: COTRTUPC-041
  // audit: COTRTUPC-042
  it.each<SessionCase>([
    {
      title: "COACTUP: F5 and F12 show, then reset",
      map: "COACTUP",
      steps: [
        // COACTUPC-011: ENTER fetches; the details show in state S.
        {
          state: "not-fetched",
          shows: ["ENTER=Process", "F3=Exit"],
          press: "ENTER",
          runs: "ENTER",
        },
        // COACTUPC-003: F5 before confirmation validates as ENTER; an edit fails (state E).
        { state: "S", shows: ["ENTER=Process", "F3=Exit"], press: "F5", runs: "ENTER" },
        {
          state: "E",
          shows: ["ENTER=Process", "F3=Exit", "F12=Cancel"],
          press: "ENTER",
          runs: "ENTER",
        },
        // The changes validate (state N); F5 saves them (state C).
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        // COACTUPC-006: any key but F3 starts over, back to the first labels.
        { state: "C", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "reset" },
        { state: "not-fetched", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "ENTER" },
      ],
    },
    {
      title: "COCRDUP: cancel, save, then reset",
      map: "COCRDUP",
      steps: [
        {
          state: "not-fetched",
          shows: ["ENTER=Process", "F3=Exit"],
          press: "ENTER",
          runs: "ENTER",
        },
        { state: "S", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        // COCRDUPC-006: F12 discards the validated changes (state S again).
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F12",
          runs: "F12",
        },
        { state: "S", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        // COCRDUPC-005: after the save any key but F3 starts over.
        { state: "C", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "reset" },
        { state: "not-fetched", shows: ["ENTER=Process", "F3=Exit"], press: "F5", runs: "ENTER" },
      ],
    },
    {
      title: "COTRTUP: add a type not found, reset",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-010: the fetch finds nothing (state X).
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        {
          state: "X",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F4",
          runs: "invalid F4",
        },
        // COTRTUPC-015: F5 starts a new type (state R).
        {
          state: "X",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        // COTRTUPC-020, COTRTUPC-021: ENTER validates it (state N); F5 inserts it (state C).
        {
          state: "R",
          shows: ["ENTER=Process", "F3=Exit", "F12=Cancel"],
          press: "ENTER",
          runs: "ENTER",
        },
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        // COTRTUPC-030: any key but F3 gives the fresh screen.
        { state: "C", shows: ["ENTER=Process", "F3=Exit"], press: "F5", runs: "reset" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "F5", runs: "invalid F5" },
      ],
    },
    {
      title: "COTRTUP: F12 on not found starts over",
      map: "COTRTUP",
      steps: [
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        // COTRTUPC-041: F12 in state X gives the fresh screen.
        {
          state: "X",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F12",
          runs: "reset",
        },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "invalid F12" },
      ],
    },
    {
      title: "COTRTUP: delete asked, ENTER invalid",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-009: the fetch finds the type (state S); COTRTUPC-031: F4 asks to delete it.
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        {
          state: "S",
          shows: ["ENTER=Process", "F3=Exit", "F4=Delete", "F12=Cancel"],
          press: "F4",
          runs: "F4",
        },
        // COTRTUPC-042: ENTER is no key of state 9; COTRTUPC-040: F12 cancels the delete.
        { state: "9", shows: ["F4=Delete", "F12=Cancel"], press: "ENTER", runs: "invalid ENTER" },
        { state: "9", shows: ["F4=Delete", "F12=Cancel"], press: "F12", runs: "F12" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
      ],
    },
    {
      title: "COTRTUP: edit error, then cancel to B",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-018: the edited description fails its edit (state E).
        {
          state: "S",
          shows: ["ENTER=Process", "F3=Exit", "F4=Delete", "F12=Cancel"],
          press: "ENTER",
          runs: "ENTER",
        },
        { state: "E", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "invalid F12" },
        // COTRTUPC-020: corrected, it validates (state N); COTRTUPC-039: F12 backs it out (B).
        { state: "E", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F12",
          runs: "F12",
        },
        { state: "B", shows: ["ENTER=Process", "F3=Exit"], press: "F4", runs: "invalid F4" },
        { state: "B", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
      ],
    },
    {
      title: "COTRTUP: delete fails (6), then reset",
      map: "COTRTUP",
      steps: [
        {
          state: "S",
          shows: ["ENTER=Process", "F3=Exit", "F4=Delete", "F12=Cancel"],
          press: "F4",
          runs: "F4",
        },
        // COTRTUPC-034: the delete fails (state 6); COTRTUPC-030: any key but F3 starts over.
        { state: "9", shows: ["F4=Delete", "F12=Cancel"], press: "F4", runs: "F4" },
        { state: "6", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "reset" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "ENTER", runs: "ENTER" },
      ],
    },
    {
      title: "COTRTUP: delete refused (8)",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-033: child records refuse the delete (state 8); COTRTUPC-035: F4 and F12 are
        // invalid there, F5 is accepted.
        { state: "9", shows: ["F4=Delete", "F12=Cancel"], press: "F4", runs: "F4" },
        { state: "8", shows: ["ENTER=Process", "F3=Exit"], press: "F4", runs: "invalid F4" },
        { state: "8", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "invalid F12" },
        { state: "8", shows: ["ENTER=Process", "F3=Exit"], press: "F5", runs: "F5" },
      ],
    },
    {
      title: "COTRTUP: delete done (7), then reset",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-032: the delete succeeds (state 7); COTRTUPC-030: any key but F3 starts over.
        { state: "9", shows: ["F4=Delete", "F12=Cancel"], press: "F4", runs: "F4" },
        { state: "7", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "reset" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "F12", runs: "invalid F12" },
      ],
    },
    {
      title: "COTRTUP: lock error (L), then reset",
      map: "COTRTUP",
      steps: [
        // COTRTUPC-029: the save cannot lock the row (state L); COTRTUPC-030: any key but F3
        // starts over.
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        { state: "L", shows: ["ENTER=Process", "F3=Exit"], press: "F4", runs: "reset" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "F4", runs: "invalid F4" },
      ],
    },
    {
      title: "COTRTUP: new type cancelled (B-blank)",
      map: "COTRTUP",
      steps: [
        {
          state: "X",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F5",
          runs: "F5",
        },
        {
          state: "R",
          shows: ["ENTER=Process", "F3=Exit", "F12=Cancel"],
          press: "ENTER",
          runs: "ENTER",
        },
        // COTRTUPC-039: F12 backs out a new type over blank old values; COTRTUPC-042: every key
        // but F3 then gives the fresh screen.
        {
          state: "N",
          shows: ["ENTER=Process", "F3=Exit", "F5=Save", "F12=Cancel"],
          press: "F12",
          runs: "F12",
        },
        { state: "B-blank", shows: ["ENTER=Process", "F3=Exit"], press: "F6", runs: "reset" },
        { state: "fresh", shows: ["ENTER=Process", "F3=Exit"], press: "F6", runs: "invalid F6" },
      ],
    },
  ])("$title", (session) => {
    for (const [step, availability] of sessionKeys(session)) {
      const at = `${session.map} state ${step.state}`;
      const shows = keyRowItemsFor(session.map, availability)
        .filter((item) => item.shown)
        .map((item) => item.label);
      expect(shows, at).toEqual(step.shows);
      expect(boundScreen(session.map, availability)(step.press), `${at} ${step.press}`).toEqual([
        step.runs,
      ]);
    }
  });
});

/** Roles that give every key `role`. */
function everyKey(role: PfKeyRole): Record<PfKey, PfKeyRole> {
  return {
    ENTER: role,
    F1: role,
    F2: role,
    F3: role,
    F4: role,
    F5: role,
    F6: role,
    F7: role,
    F8: role,
    F9: role,
    F10: role,
    F11: role,
    F12: role,
  };
}

interface BindErrorCase {
  readonly title: string;
  readonly availability: () => PfKeyAvailability;
  readonly actions: PfKeyRoleActions;
  readonly message: string;
}

describe("bindPfKeys", () => {
  const action = countingAction();

  it.each<BindErrorCase>([
    {
      title: "an own key has no action",
      availability: () => coactupKeys("N"),
      actions: { own: { ENTER: action.run, F3: action.run, F12: action.run } },
      message: 'PF key F5 has role "own", but the screen passes no F5 action',
    },
    {
      title: "a key taken as ENTER lacks ENTER",
      availability: () => ({ roles: { ...everyKey("enter"), F3: "own" }, shown: new Set<PfKey>() }),
      actions: { own: { F3: action.run } },
      message: 'PF key ENTER has role "enter", but the screen passes no ENTER action',
    },
    {
      title: "a reset key has no reset action",
      availability: () => coactupKeys("C"),
      actions: { own: { ENTER: action.run, F3: action.run } },
      message: 'PF key ENTER has role "reset", but the screen passes no reset action',
    },
    {
      title: "an invalid key has no invalid action",
      availability: () => cotrtupKeys("fresh"),
      actions: { own: { ENTER: action.run, F3: action.run }, reset: action.run },
      message: 'PF key F1 has role "invalid", but the screen passes no invalid-key action',
    },
  ])("throws when $title", ({ availability, actions, message }) => {
    expect(() => bindPfKeys(availability(), actions)).toThrow(new Error(message));
    expect(action.runs()).toBe(0);
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
    const items = expected.map(([key, label, dark]): PfKeyItem => ({ key, label, dark }));
    expect(keyRowItems(spec)).toEqual(items);
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
      title: "mixed spacing with leading and trailing blanks splits alike",
      text: "  F3=Exit F7=Backward  F8=Forward",
      expected: [
        ["F3", "F3=Exit"],
        ["F7", "F7=Backward"],
        ["F8", "F8=Forward"],
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
      title: "a single ampersand is kept as it is",
      text: "F3=Save&Exit",
      expected: [["F3", "F3=Save&Exit"]],
    },
    {
      title: "F12 is not read as F1",
      text: "F5=Save F12=Cancel",
      expected: [
        ["F5", "F5=Save"],
        ["F12", "F12=Cancel"],
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
      text: "xx F3=Exit",
      message: 'Key row "xx F3=Exit" has text before its first key label',
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

  it("reads the browser name of every key of PF_KEYS as that key", () => {
    for (const key of PF_KEYS) {
      expect(keyFromEvent(keydown(eventKeyOf(key)))).toBe(key);
    }
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
  const foldingInvalidKeys: PfKeyResolveOptions = { foldShifted: true, hasInvalidKeyHandler: true };

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
      title: "a served ENTER on a div outside any form resolves to its action",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter"),
      target: targetAt("DIV", false),
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
      title: "an unserved Shift+F7 is invalid where shifted keys fold",
      handlers: serving("F3", action.run),
      event: keydown("F7", { shiftKey: true }),
      options: foldingInvalidKeys,
      expected: { kind: "invalid", key: "F7" },
    },
    {
      title: "Shift+F7 is PF19, an invalid key, where shifted keys do not fold, F7 served or not",
      handlers: serving("F7", action.run),
      event: keydown("F7", { shiftKey: true }),
      options: invalidKeys,
      expected: { kind: "invalid", key: "F7" },
    },
    {
      title: "Shift+F7 is left to the browser where shifted keys do not fold, without invalid keys",
      handlers: serving("F7", action.run),
      event: keydown("F7", { shiftKey: true }),
      expected: undefined,
    },
    {
      title: "Shift+Enter is no key where shifted keys do not fold",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter", { shiftKey: true }),
      options: invalidKeys,
      expected: undefined,
    },
    {
      title: "Shift+Enter is no key where shifted keys fold",
      handlers: serving("ENTER", action.run),
      event: keydown("Enter", { shiftKey: true }),
      options: foldingInvalidKeys,
      expected: undefined,
    },
    {
      title: "Shift with a key that is no PF key is left to the browser",
      handlers: serving("F3", action.run),
      event: keydown("a", { shiftKey: true }),
      options: invalidKeys,
      expected: undefined,
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

  it.each<ModifierCase>([
    { title: "Ctrl", flags: { ctrlKey: true } },
    { title: "Alt", flags: { altKey: true } },
    { title: "Meta", flags: { metaKey: true } },
    { title: "auto-repeat", flags: { repeat: true } },
    { title: "IME composition", flags: { isComposing: true } },
  ])("leaves Shift+F7 with $title to the browser where shifted keys do not fold", ({ flags }) => {
    const event = keydown("F7", { ...flags, shiftKey: true });
    expect(resolvePfKey(serving("F7", action.run), event, invalidKeys)).toBeUndefined();
    expect(resolvePfKey({}, event, invalidKeys)).toBeUndefined();
  });

  it("hands back the very handler the screen registered for the key", () => {
    const exit = countingAction();
    const resolution = resolvePfKey({ F3: exit.run }, keydown("F3"));
    expect(resolution).toEqual({ kind: "handled", key: "F3", handler: exit.run });
    expect(resolution?.kind === "handled" ? resolution.handler : undefined).toBe(exit.run);
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
  // audit: COSGN00-039
  // audit: COSGN00-042
  // audit: COSGN00C-012
  it("keeps both titles at the 40 columns of their fields, padding included", () => {
    expect(TITLE01).toBe("      AWS Mainframe Modernization       ");
    expect(TITLE02).toBe("              CardDemo                  ");
    expect([TITLE01.length, TITLE02.length]).toEqual([40, 40]);
  });

  // The CSMSG01Y `VALUE` literals are 49 bytes (6 and 9 trailing spaces); the `PIC X(50)` field
  // pads them with one more space, so the field values end in 7 and 10 spaces.

  // audit: COSGN00C-004
  it("holds the session-end text F3 shows on COSGN00 as its PIC X(50) field value", () => {
    expect(THANK_YOU).toBe(`Thank you for using CardDemo application...${" ".repeat(7)}`);
    expect(THANK_YOU).toHaveLength(50);
  });

  // audit: COSGN00C-005
  it("holds the invalid-key message as its PIC X(50) field value", () => {
    expect(INVALID_KEY).toBe(`Invalid key pressed. Please see below...${" ".repeat(10)}`);
    expect(INVALID_KEY).toHaveLength(50);
  });

  // audit: COSGN00-040
  // audit: COSGN00C-012
  it.each([
    [new Date(2022, 6, 18, 9, 5, 7), "07/18/22"],
    [new Date(2000, 0, 1, 0, 0, 0), "01/01/00"],
    [new Date(2024, 11, 31, 23, 59, 59), "12/31/24"],
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
  // audit: COSGN00C-012
  it.each([
    [new Date(2022, 6, 18, 9, 5, 7), "09:05:07"],
    [new Date(2000, 0, 1, 0, 0, 0), "00:00:00"],
    [new Date(2022, 6, 18, 12, 0, 0), "12:00:00"],
    [new Date(2024, 11, 31, 23, 59, 59), "23:59:59"],
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
