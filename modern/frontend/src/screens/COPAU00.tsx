import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCopaus0cSummary } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell, GridTable, LIST_CELL, LIST_ROW } from "@/components/screen/FieldGrid";
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
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COPAU00_LENGTHS, copau00Schema, type Copau00Values } from "@/schemas/COPAU00";

/*
 * Screen COPAU00, map `COPAU0A` of `app/app-authorization-ims-db2-mq/bms/COPAU00.bms`: transaction
 * CPVS, program COPAUS0C, route `/authorizations`. One account's customer, its pending
 * authorization summary and its pending authorizations five to a page, from endpoint 21. Field
 * placement is `modern/audit/screens.md` rows COPAU00-001 to COPAU00-114; behaviour is the
 * `client` rows of `modern/audit/online/COPAUS0C.md`, cited by rule ID below.
 */

/** The map's screen title, `POS=(3,30)` (COPAU00-011). */
const TITLE = "View Authorizations";

/** List rows on the map, lines 16 to 20. */
const ROWS = COPAU00_LENGTHS.rows;

/** Row-24 keys of the map (COPAU00-104): ENTER, F3, F7 and F8. */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COPAU00);

/** COPAUS0C-011, `COPAUS0C.cbl:327-328`. */
const MSG_INVALID_SELECTION = "Invalid selection. Valid value is S";

/** COPAUS0C-045, `COPAUS0C.cbl:381`. */
const MSG_TOP_OF_PAGE = "You are already at the top of the page...";

/** COPAUS0C-040, `COPAUS0C.cbl:409-410`. */
const MSG_BOTTOM_OF_PAGE = "You are already at the bottom of the page...";

/** COBOL `= SPACES OR LOW-VALUES` on a `PIC X(1)` selector: empty, spaces or nulls. */
const BLANK_SELECTOR = /^[ \u0000]*$/;

/** COBOL `CDEMO-ACCT-ID IS NUMERIC` on `PIC 9(11)`: 11 ASCII digits (COPAUS0C-002). */
const NUMERIC_ACCOUNT = /^[0-9]{11}$/;

/** The one field endpoint 21 names in a `ScreenError` (`ScreenError.field`): the account. */
const ACCT_ID_FIELD = "acct_id";

/**
 * Captions of map line 14 and the dashes of line 15, column by column (COPAU00-041 to -056), and
 * each column's width as a fraction of the list's 77 map columns, from its line-16 field's `POS`
 * to the next one's: the selector column from 2, the transaction ID from 8, the date from 27, the
 * time from 38, the type from 49, A/D from 58, STS from 63 and the amount from 67 to the list's
 * end at 78. A caption or dash whose `POS` lies left of its field is moved back by that many of
 * the column's map columns (`captionShift`, `dashShift`; empty when it starts on the field).
 */
const COLUMNS = [
  { caption: "Sel", dashes: "---", width: "w-6/77", captionShift: "", dashShift: "" },
  {
    caption: " Transaction ID ",
    dashes: "----------------",
    width: "w-19/77",
    captionShift: "",
    dashShift: "",
  },
  { caption: "  Date  ", dashes: "--------", width: "w-11/77", captionShift: "", dashShift: "" },
  {
    caption: "  Time  ",
    dashes: "--------",
    width: "w-11/77",
    captionShift: "",
    dashShift: "relative -left-1/11",
  },
  { caption: "Type ", dashes: "----", width: "w-9/77", captionShift: "", dashShift: "" },
  {
    caption: "A/D",
    dashes: "---",
    width: "w-5/77",
    captionShift: "relative -left-2/5",
    dashShift: "relative -left-2/5",
  },
  {
    caption: "STS",
    dashes: "---",
    width: "w-4/77",
    captionShift: "relative -left-2/4",
    dashShift: "relative -left-2/4",
  },
  {
    caption: "   Amount   ",
    dashes: "------------",
    width: "w-12/77",
    captionShift: "",
    dashShift: "",
  },
] as const;

/*
 * Colour tokens of AAP 0.4.3: the labels are DEFAULT, TURQUOISE or uncoloured, the output values
 * BLUE, and the line-22 hint NEUTRAL with BRT.
 *
 * BLITZY [LAYOUT]: an input is as wide as its map `LENGTH` in characters (`size`, `w-auto`) and
 * never narrower than its grid cell (`min-w-full`), in place of the `Input` default `w-full`. The
 * grid's columns shrink with the viewport while the characters do not, so a short field such as
 * the 3-character counts would otherwise cut off its last characters; it reaches instead into the
 * empty attribute-byte columns that follow it on the map. Output inputs also use `px-1` instead of
 * the default `px-3`, which keeps that overhang inside those columns.
 */
const LABEL = "h-full whitespace-pre text-foreground";
const FIELD = "w-auto min-w-full";
const OUTPUT = `${FIELD} px-1 text-muted-foreground`;
const CELL = `${LIST_CELL} whitespace-pre text-muted-foreground`;
const HEADING = `${LIST_CELL} whitespace-pre`;

/** Entries of `CDEMO-CPVS-PAUKEY-PREV-PG PIC X(08) OCCURS 20 TIMES` (`COPAUS0C.cbl:120`). */
const FIRST_KEY_PAGES = 20;

/**
 * The paging state COPAUS0C keeps in `CDEMO-CPVS-INFO` of the COMMAREA between interactions
 * (COPAUS0C-039), kept here for as long as the screen is shown. It is program state, not a payload.
 *
 * - `account`: `CDEMO-ACCT-ID`, the account F7 and F8 page, never the field's current text.
 * - `pageNum`: `CDEMO-CPVS-PAGE-NUM`.
 * - `firstKeys`: `CDEMO-CPVS-PAUKEY-PREV-PG`, the first key of each page by page number, for at
 *   most {@link FIRST_KEY_PAGES} pages. A page read that shows a row stores its page `p`'s first
 *   key and drops every later page and every page at or before `p - 20` (COPAUS0C-039). A later
 *   page's key is always stored again before an F7 reads it, as F7 reads the page before the
 *   page number (COPAUS0C-046). So while no page after the 20th has been shown since the page
 *   number last became 0, every page from 1 to `p` is held, as in the COBOL table. From the 21st
 *   page on the COBOL stores past its 20 entries (COPAUS0C-050, HALTED(X-COPAUS0C-H3)); paging
 *   goes on here with the keys of the last 20 pages stored, which is not a parity result. Every
 *   change of the page number to 0 empties it, because only keys stored after that are read.
 * - `lastKey`: `CDEMO-CPVS-PAUKEY-LAST`, the next F8's `page_key`.
 * - `nextPage`: `CDEMO-CPVS-NEXT-PAGE-FLG` = `'Y'`.
 * - `rowKeys`: `CDEMO-CPVS-AUTH-KEYS`, the key a selection on each row passes to COPAUS1C.
 */
type CpvsPagingState = {
  readonly account: string | null;
  readonly pageNum: number;
  readonly firstKeys: ReadonlyMap<number, string>;
  readonly lastKey: string | null;
  readonly nextPage: boolean;
  readonly rowKeys: readonly (string | null)[];
};

/** `INITIALIZE CARDDEMO-COMMAREA` (COPAUS0C-001): no account, page 0, no keys, flag not `Y`. */
const EMPTY_PAGING: CpvsPagingState = {
  account: null,
  pageNum: 0,
  firstKeys: new Map<number, string>(),
  lastKey: null,
  nextPage: false,
  rowKeys: Array.from({ length: ROWS }, () => null),
};

/** The account a first entry carries in its route state when it is numeric, else `null`. */
function accountFromRouteState(state: unknown): string | null {
  if (typeof state !== "object" || state === null || !("acct_id" in state)) {
    return null;
  }
  const account = state.acct_id;
  return typeof account === "string" && NUMERIC_ACCOUNT.test(account) ? account : null;
}

/**
 * `MOVE PA-AUTHORIZATION-KEY TO CDEMO-CPVS-PAUKEY-PREV-PG(CDEMO-CPVS-PAGE-NUM)` (COPAUS0C-039)
 * over the window {@link CpvsPagingState} `firstKeys` describes: `key` becomes page `page`'s first
 * key, and of the keys held only those of the {@link FIRST_KEY_PAGES} − 1 pages before `page` are
 * kept. At most {@link FIRST_KEY_PAGES} entries are read and copied.
 */
function withFirstKey(
  firstKeys: ReadonlyMap<number, string>,
  page: number,
  key: string,
): ReadonlyMap<number, string> {
  const kept = new Map<number, string>();
  for (const [heldPage, heldKey] of firstKeys) {
    if (heldPage < page && heldPage > page - FIRST_KEY_PAGES) {
      kept.set(heldPage, heldKey);
    }
  }
  kept.set(page, key);
  return kept;
}

/**
 * `PROCESS-PAGE-FORWARD` applied to the paging state (COPAUS0C-036, COPAUS0C-039): the last key
 * becomes none and then each row's key; at the first row the page number grows by 1 and that row's
 * key becomes the page's first key. Only a full page sets the next-page flag (`has_next_page` is
 * `null` otherwise), and rows the page does not fill keep their earlier keys, as the COBOL never
 * clears `CDEMO-CPVS-AUTH-KEYS`.
 */
function afterPageRead(
  state: CpvsPagingState,
  page: components["schemas"]["AuthSummaryResponse"],
): CpvsPagingState {
  const shown = page.rows.slice(0, ROWS);
  const first = shown.at(0);
  const pageNum = first === undefined ? state.pageNum : state.pageNum + 1;
  return {
    account: state.account,
    pageNum,
    firstKeys:
      first === undefined
        ? state.firstKeys
        : withFirstKey(state.firstKeys, pageNum, first.auth_key),
    lastKey: shown.at(-1)?.auth_key ?? null,
    nextPage: typeof page.has_next_page === "boolean" ? page.has_next_page : state.nextPage,
    rowKeys: state.rowKeys.map((key, index) => shown.at(index)?.auth_key ?? key),
  };
}

/** The first message of a rejected ENTER, in field order. */
function firstMessage(errors: FieldErrors<Copau00Values>): string | undefined {
  if (errors.acctId?.message !== undefined) {
    return errors.acctId.message;
  }
  const selections = errors.selections;
  if (selections === undefined) {
    return undefined;
  }
  for (let index = 0; index < ROWS; index += 1) {
    const text = selections[index]?.message;
    if (text !== undefined) {
      return text;
    }
  }
  return selections.message;
}

/** Props of {@link OutputCell}. */
type OutputCellProps = {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly value: string;
  /** `id` of the input, which a `Label` names it by. */
  readonly id: string;
  /** Accessible name of a field the map gives no label. */
  readonly name?: string;
};

/**
 * One protected output field (`ATTRB=ASKIP`): a read-only `Input` at its `POS`, `LENGTH` columns
 * wide, out of the tab order as the terminal skips protected fields. The value is shown verbatim.
 */
function OutputCell({ row, col, span, value, id, name }: OutputCellProps): ReactNode {
  return (
    <GridCell row={row} col={col} span={span}>
      <Input
        id={id}
        aria-label={name}
        readOnly
        tabIndex={-1}
        size={span}
        value={value}
        className={OUTPUT}
      />
    </GridCell>
  );
}

/** Props of {@link LabelCell}. */
type LabelCellProps = {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly text: string;
  readonly htmlFor: string;
};

/** One literal label field at its `POS`, bound to the output field it names. */
function LabelCell({ row, col, span, text, htmlFor }: LabelCellProps): ReactNode {
  return (
    <GridCell row={row} col={col} span={span}>
      <Label htmlFor={htmlFor} className={LABEL}>
        {text}
      </Label>
    </GridCell>
  );
}

/**
 * The pending-authorization summary screen (CPVS / COPAUS0C, endpoint 21).
 *
 * - First entry (COPAUS0C-001, COPAUS0C-002): a numeric `acct_id` in the route state, as F3 on
 *   COPAU01 passes it, fills the account field and loads it as ENTER would, without the edits and
 *   the selection; otherwise every field is blank and nothing is requested.
 * - ENTER: `copau00Schema` repeats the account edits (COPAUS0C-005, -006); then the first non-blank
 *   selector decides (COPAUS0C-009 to -012): `S` or `s` on a row holding an authorization goes to
 *   COPAU01 with no request, any other code is refused with its message while the account is read
 *   as without a selection (COPAUS0C-008).
 * - F7 and F8 page the account of the paging state from the keys it holds (COPAUS0C-039 to -049);
 *   the top- and bottom-of-page decisions are the screen's, from that state (X-COPAUS0C-6).
 * - F3 returns to the main menu (COPAUS0C-003); any other function key shows the invalid-key
 *   message (COPAUS0C-004).
 *
 * While a request is outstanding the keyboard is locked, as the terminal's is, so every response
 * is applied to the state it was sent from: every key is disabled, F3 included, and the account
 * and the selectors are read-only. A selection's transfer to COPAU01 keeps the keyboard locked
 * the same way until COPAU01 replaces this screen. The cursor returns to the account field after
 * every response (COPAUS0C-053). Nothing here logs or stores a response; the screen shows no card
 * number, and endpoint 21 returns none (COPAUS0C-055).
 */
export function COPAU00(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const formId = useId();
  const fieldId = useId();
  const [entryAccount] = useState(() => accountFromRouteState(location.state));
  const form = useForm<Copau00Values>({
    resolver: zodResolver(copau00Schema),
    defaultValues: {
      acctId: entryAccount ?? "",
      selections: Array.from({ length: ROWS }, () => ""),
    },
  });
  const summary = useCopaus0cSummary();
  const [paging, setPaging] = useState<CpvsPagingState>(() => ({
    ...EMPTY_PAGING,
    account: entryAccount,
  }));
  /** The customer and summary block of the last account read (COPAUS0C-020 to -029). */
  const [block, setBlock] = useState<components["schemas"]["AuthSummaryBlock"] | null>(null);
  /**
   * Whether a send with `ERASE` has followed the last block read without moving the block again:
   * the block fields the map does not declare `FSET` then show blank (COPAUS0C-004, -007).
   */
  const [erased, setErased] = useState(false);
  const [rows, setRows] = useState<components["schemas"]["AuthSummaryRow"][]>([]);
  /**
   * Whether a page has been read since entry. Until then every selector is enterable, as the map
   * declares; afterwards a row holding no authorization has a protected selector (COPAUS0C-012,
   * COPAUS0C-035).
   */
  const [pageRead, setPageRead] = useState(false);
  const [message, setMessage] = useState<string>();
  const entered = useRef(false);
  // Whether a read is outstanding, from the key that sends it to its answer. The ref decides at
  // once; the state renders the locked keys and fields, which a key pressed before that render
  // would pass.
  const [pending, setPending] = useState(false);
  const pendingRef = useRef(false);

  /** Locks the keyboard for one read, from before its edits run. */
  function lock(): void {
    pendingRef.current = true;
    setPending(true);
  }

  /**
   * Unlocks the keyboard: the edits refused ENTER or a read answered. A selection's transfer keeps
   * the lock, which goes with this screen once the next one replaces it.
   */
  function unlock(): void {
    pendingRef.current = false;
    setPending(false);
  }

  /** Shows `text` in the message row (none when `undefined`) and puts the cursor on the account. */
  function settle(text: string | undefined): void {
    setMessage(text);
    form.setFocus("acctId");
  }

  /**
   * A failed read: its message alone, as the first failure's send shows it (X-COPAUS0C-1). An
   * error naming the account (`acct_id`, COPAUS0C-005, -006, -014) marks the account field
   * invalid with the same text until the next key; the message row stays its only visible place.
   */
  function fail(error: components["schemas"]["ScreenError"]): void {
    setErased(true);
    if (error.field === ACCT_ID_FIELD) {
      form.setError("acctId", { type: "server", message: error.message });
    }
    settle(error.message);
  }

  /**
   * `INITIALIZE-AUTH-DATA`, then the rows of `page` (COPAUS0C-035). Every selector is left blank:
   * the program sends each selector's attribute without the modified flag, so no earlier entry in
   * it is received again.
   */
  function showRows(page: components["schemas"]["AuthSummaryResponse"]): void {
    setRows(page.rows.slice(0, ROWS));
    setPageRead(true);
    for (let index = 0; index < ROWS; index += 1) {
      form.setValue(`selections.${index}`, "");
    }
  }

  /**
   * `GATHER-DETAILS` for `account`, on ENTER and on first entry (COPAUS0C-008): the account and
   * page number 0 enter the state at once, then endpoint 21 is read from its first page. The page
   * is accounted for only when the account has a summary (COPAUS0C-029). `selectionMessage` is the
   * invalid-selection text, which shows only when the read sets no message (COPAUS0C-011). With
   * the page number 0 no first key held is read again before a page read stores it anew
   * (COPAUS0C-039, COPAUS0C-046), so the earlier account's first keys are dropped.
   */
  function readAccount(account: string, selectionMessage: string | undefined): void {
    setPaging((state) => ({ ...state, account, pageNum: 0, firstKeys: EMPTY_PAGING.firstKeys }));
    lock();
    void summary
      .mutateAsync(
        { path: { acct_id: account }, query: {} },
        {
          onSuccess: (page) => {
            form.setValue("acctId", page.acct_id);
            setBlock(page.summary);
            setErased(false);
            showRows(page);
            if (page.summary.summary_found) {
              setPaging((state) => afterPageRead(state, page));
            }
            settle(selectionMessage);
          },
          onError: fail,
        },
      )
      .then(unlock, unlock);
  }

  /**
   * An F7 or F8 page read from `pageKey` (COPAUS0C-041, -047): the rows change and the block stays
   * as shown, because the map then goes without `ERASE` (COPAUS0C-048).
   */
  function readPage(
    account: string,
    pageKey: string,
    direction: components["schemas"]["PageDirection"],
  ): void {
    lock();
    void summary
      .mutateAsync(
        { path: { acct_id: account }, query: { page_key: pageKey, direction } },
        {
          onSuccess: (page) => {
            showRows(page);
            setPaging((state) => afterPageRead(state, page));
            settle(undefined);
          },
          onError: fail,
        },
      )
      .then(unlock, unlock);
  }

  useEffect(() => {
    // Once per mount: a first entry reads its account a single time, also when development mode
    // runs effects twice.
    if (entered.current) {
      return;
    }
    entered.current = true;
    if (entryAccount !== null) {
      readAccount(entryAccount, undefined);
    }
  });

  /** ENTER with an account `copau00Schema` accepts: the selection rule, then the read. */
  function onEnter(values: Copau00Values): void {
    const row = values.selections.findIndex((value) => !BLANK_SELECTOR.test(value));
    let selectionMessage: string | undefined;
    const key = row < 0 ? null : (paging.rowKeys[row] ?? null);
    if (key !== null) {
      const code = values.selections[row];
      if (code === "S" || code === "s") {
        // COPAUS0C-010: XCTL COPAUS1C with the account typed on this ENTER and the row's key. The
        // lock ENTER took stays held until COPAU01 replaces this screen, so no key acts here
        // while its module loads.
        navigate(PATHS.COPAU01, {
          state: { from: PATHS.COPAU00, acct_id: values.acctId, auth_key: key },
        });
        return;
      }
      selectionMessage = MSG_INVALID_SELECTION;
    }
    readAccount(values.acctId, selectionMessage);
  }

  /**
   * ENTER refused by the account edits (COPAUS0C-007): the account field is blanked, the map goes
   * with `ERASE` and the page number becomes 0; nothing is read, and the selectors are not read.
   * The COBOL keeps its first keys there, but with the page number 0 none is read again before a
   * page read stores it anew (COPAUS0C-039, COPAUS0C-046), so they are dropped here.
   */
  function onRejected(errors: FieldErrors<Copau00Values>): void {
    unlock();
    if (errors.acctId !== undefined) {
      form.setValue("acctId", "");
      setPaging((state) => ({ ...state, pageNum: 0, firstKeys: EMPTY_PAGING.firstKeys }));
      setErased(true);
    }
    settle(firstMessage(errors));
  }

  /**
   * ENTER, refused while a read or a transfer is outstanding. The lock is taken before the edits,
   * which `handleSubmit` runs asynchronously, and released at their refusal or failure, or at the
   * read's answer; a transfer holds it until this screen is replaced.
   */
  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    if (pendingRef.current) {
      event.preventDefault();
      return;
    }
    lock();
    void form
      .handleSubmit(onEnter, onRejected)(event)
      .catch((error: unknown) => {
        unlock();
        throw error;
      });
  }

  /** F3, COPAUS0C-003: XCTL COMEN01C with no request; the paging state is dropped. */
  function back(): void {
    if (pendingRef.current) {
      return;
    }
    navigate(PATHS.COMEN01, { state: { from: PATHS.COPAU00 } });
  }

  /**
   * F7, COPAUS0C-045 and -046: at page 1 or before, the top-of-page message; otherwise the page
   * number drops by 1, the flag becomes `Y` and the page is read again from the first key stored
   * for that page number. The page read adds the 1 back (X-COPAUS0C-2, preserved).
   */
  function pageBackward(): void {
    if (pendingRef.current) {
      return;
    }
    form.clearErrors();
    const pageNum = paging.pageNum - 1;
    // While no page after the 20th has been shown since the page number last became 0, the key of
    // every page before the page number is held (CpvsPagingState). One is missing only after such
    // a page, once backward reads that found no row have taken the page number down to the
    // earliest page still held: paging that HALTED(X-COPAUS0C-H3) covers (COPAUS0C-050). There
    // the screen shows the top-of-page message without a request, which is not a parity result.
    const pageKey = paging.firstKeys.get(pageNum);
    if (paging.pageNum <= 1 || paging.account === null || pageKey === undefined) {
      settle(MSG_TOP_OF_PAGE);
      return;
    }
    setPaging((state) => ({ ...state, pageNum, nextPage: true }));
    readPage(paging.account, pageKey, "backward");
  }

  /**
   * F8, COPAUS0C-040 to -042: with the flag not `Y`, the bottom-of-page message; otherwise the page
   * after the last key. With the flag `Y` and no last key the COBOL issues a DL/I call with no PSB
   * scheduled: HALTED(X-COPAUS0C-H4) (COPAUS0C-043). The screen shows the bottom-of-page message
   * without a request there, which is not a parity result.
   */
  function pageForward(): void {
    if (pendingRef.current) {
      return;
    }
    form.clearErrors();
    if (!paging.nextPage || paging.lastKey === null || paging.account === null) {
      settle(MSG_BOTTOM_OF_PAGE);
      return;
    }
    readPage(paging.account, paging.lastKey, "forward");
  }

  /** COPAUS0C-004: an unserved key is refused; the map goes with `ERASE` and nothing is read. */
  function invalidKey(): void {
    if (pendingRef.current) {
      return;
    }
    form.clearErrors();
    setErased(true);
    settle(INVALID_KEY);
  }

  const actions: PfKeyHandlers = { F3: back, F7: pageBackward, F8: pageForward };
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({
    ...item,
    onPress: actions[item.key],
    disabled: pending,
  }));

  /** A block field the map does not declare `FSET`: blank after an erasing send. */
  const unsent = (value: string | undefined): string => (erased ? "" : (value ?? ""));
  const ids = {
    cname: `${fieldId}-cname`,
    custid: `${fieldId}-custid`,
    addr001: `${fieldId}-addr001`,
    accstat: `${fieldId}-accstat`,
    addr002: `${fieldId}-addr002`,
    phone1: `${fieldId}-phone1`,
    apprcnt: `${fieldId}-apprcnt`,
    declcnt: `${fieldId}-declcnt`,
    credlim: `${fieldId}-credlim`,
    cashlim: `${fieldId}-cashlim`,
    appramt: `${fieldId}-appramt`,
    credbal: `${fieldId}-credbal`,
    cashbal: `${fieldId}-cashbal`,
    declamt: `${fieldId}-declamt`,
  };

  return (
    <ScreenShell
      transaction={SCREENS.COPAU00.transaction}
      program={SCREENS.COPAU00.program}
      title={TITLE}
      formId={formId}
      message={{ text: message, kind: "error" }}
      keys={keys}
      busy={pending}
      keyOptions={{ foldShifted: KEY_ROWS.COPAU00.foldShifted, onInvalidKey: invalidKey }}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={pending} onSubmit={onSubmit}>
          <FieldGrid>
            <FormField
              control={form.control}
              name="acctId"
              render={({ field }) => (
                // `contents`: the label and the input sit in their own grid cells of FieldGrid.
                <FormItem className="contents">
                  <GridCell row={5} col={3} span={15}>
                    <FormLabel className={LABEL}>Search Acct Id:</FormLabel>
                  </GridCell>
                  <GridCell row={5} col={19} span={COPAU00_LENGTHS.acctId}>
                    <FormControl>
                      {/* COPAUS0C-053: the cursor starts, and stays, on the account field. */}
                      <Input
                        {...field}
                        readOnly={pending}
                        maxLength={COPAU00_LENGTHS.acctId}
                        size={COPAU00_LENGTHS.acctId}
                        autoComplete="off"
                        autoFocus
                        className={FIELD}
                      />
                    </FormControl>
                    {/* For assistive technology only: the message row is the visible message. */}
                    <FormMessage className="sr-only" />
                  </GridCell>
                </FormItem>
              )}
            />

            <LabelCell row={6} col={3} span={6} text="Name: " htmlFor={ids.cname} />
            <OutputCell
              row={6}
              col={10}
              span={25}
              id={ids.cname}
              value={unsent(block?.customer_name)}
            />
            <LabelCell row={6} col={44} span={13} text="Customer Id: " htmlFor={ids.custid} />
            <OutputCell
              row={6}
              col={58}
              span={9}
              id={ids.custid}
              value={unsent(block?.customer_id)}
            />

            <OutputCell
              row={7}
              col={10}
              span={25}
              id={ids.addr001}
              name="Address line 1"
              value={unsent(block?.address_line_1)}
            />
            <LabelCell row={7} col={44} span={13} text="Acct Status: " htmlFor={ids.accstat} />
            {/* COPAUS0C-026: no statement of the program fills ACCSTAT, so it is always blank. */}
            <OutputCell row={7} col={58} span={1} id={ids.accstat} value="" />

            <OutputCell
              row={8}
              col={10}
              span={25}
              id={ids.addr002}
              name="Address line 2"
              value={unsent(block?.address_line_2)}
            />

            <LabelCell row={9} col={10} span={3} text="PH:" htmlFor={ids.phone1} />
            <OutputCell row={9} col={15} span={13} id={ids.phone1} value={unsent(block?.phone)} />
            <LabelCell row={9} col={44} span={13} text="Approval # : " htmlFor={ids.apprcnt} />
            <OutputCell
              row={9}
              col={58}
              span={3}
              id={ids.apprcnt}
              value={unsent(block?.approved_count)}
            />
            <LabelCell row={9} col={64} span={10} text="Decline #:" htmlFor={ids.declcnt} />
            <OutputCell
              row={9}
              col={76}
              span={3}
              id={ids.declcnt}
              value={unsent(block?.declined_count)}
            />

            {/* The six amounts are FSET, so they keep what they show across every send. */}
            <LabelCell row={11} col={6} span={11} text="Credit Lim:" htmlFor={ids.credlim} />
            <OutputCell
              row={11}
              col={19}
              span={12}
              id={ids.credlim}
              value={block?.credit_limit ?? ""}
            />
            <LabelCell row={11} col={35} span={9} text="Cash Lim:" htmlFor={ids.cashlim} />
            <OutputCell
              row={11}
              col={46}
              span={9}
              id={ids.cashlim}
              value={block?.cash_limit ?? ""}
            />
            <LabelCell row={11} col={58} span={9} text="Appr Amt:" htmlFor={ids.appramt} />
            <OutputCell
              row={11}
              col={69}
              span={10}
              id={ids.appramt}
              value={block?.approved_amount ?? ""}
            />

            <LabelCell row={12} col={6} span={11} text="Credit Bal:" htmlFor={ids.credbal} />
            <OutputCell
              row={12}
              col={19}
              span={12}
              id={ids.credbal}
              value={block?.credit_balance ?? ""}
            />
            <LabelCell row={12} col={35} span={9} text="Cash Bal:" htmlFor={ids.cashbal} />
            <OutputCell
              row={12}
              col={46}
              span={9}
              id={ids.cashbal}
              value={block?.cash_balance ?? ""}
            />
            <LabelCell row={12} col={58} span={9} text="Decl Amt:" htmlFor={ids.declamt} />
            <OutputCell
              row={12}
              col={69}
              span={10}
              id={ids.declamt}
              value={block?.declined_amount ?? ""}
            />

            {/*
             * BLITZY [LAYOUT]: the list occupies map lines 14 to 20 (captions, dashes, five rows)
             * from column 2 to column 78. A Table on one line of FieldGrid's equal-height rows
             * would stretch every line to the Table's height, so its block spans the seven lines
             * it holds on the map, one `LIST_ROW` per line, and its fixed columns start at the
             * line-16 fields' `POS`.
             */}
            <GridTable row={14} col={2} span={77} rowSpan={7}>
              <Table aria-label="Authorizations" className="table-fixed">
                <colgroup>
                  {COLUMNS.map((column) => (
                    <col key={column.caption} className={column.width} />
                  ))}
                </colgroup>
                <TableHeader>
                  <TableRow className={LIST_ROW}>
                    {COLUMNS.map((column) => (
                      <TableHead key={column.caption} scope="col" className={HEADING}>
                        {column.captionShift === "" ? (
                          column.caption
                        ) : (
                          <span className={column.captionShift}>{column.caption}</span>
                        )}
                      </TableHead>
                    ))}
                  </TableRow>
                  {/* Line 15's dashes are decoration under the captions, not column titles. */}
                  <TableRow aria-hidden="true" className={LIST_ROW}>
                    {COLUMNS.map((column) => (
                      <TableCell key={column.caption} className={HEADING}>
                        {column.dashShift === "" ? (
                          column.dashes
                        ) : (
                          <span className={column.dashShift}>{column.dashes}</span>
                        )}
                      </TableCell>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: ROWS }, (_, index) => {
                    const row = rows.at(index);
                    const isProtected = pageRead && row === undefined;
                    return (
                      // BLITZY [LAYOUT]: cells drop the default padding (`LIST_CELL`) so a list
                      // row keeps the height of one map line and each value starts on its column.
                      <TableRow key={index} className={LIST_ROW}>
                        <TableCell className={LIST_CELL}>
                          <FormField
                            control={form.control}
                            name={`selections.${index}`}
                            render={({ field }) => (
                              <FormItem>
                                <FormControl>
                                  <Input
                                    {...field}
                                    aria-label={`Select row ${index + 1}`}
                                    maxLength={COPAU00_LENGTHS.selection}
                                    autoComplete="off"
                                    readOnly={isProtected || pending}
                                    tabIndex={isProtected ? -1 : undefined}
                                    className="relative left-1/6 w-9"
                                  />
                                </FormControl>
                                <FormMessage className="sr-only" />
                              </FormItem>
                            )}
                          />
                        </TableCell>
                        <TableCell className={CELL}>{row?.transaction_id ?? ""}</TableCell>
                        {/* `date` and `time` arrive as the program's MM/DD/YY and HH:MM:SS. */}
                        <TableCell className={CELL}>{row?.date ?? ""}</TableCell>
                        <TableCell className={CELL}>{row?.time ?? ""}</TableCell>
                        <TableCell className={CELL}>{row?.auth_type ?? ""}</TableCell>
                        <TableCell className={CELL}>{row?.approval ?? ""}</TableCell>
                        <TableCell className={CELL}>{row?.match_status ?? ""}</TableCell>
                        <TableCell className={CELL}>{row?.amount ?? ""}</TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </GridTable>

            <GridCell row={22} col={12} span={52}>
              <p className="flex h-full items-center whitespace-pre font-semibold text-foreground">
                Type &apos;S&apos; to View Authorization details from the list
              </p>
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
