import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCopaus0cSummary } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
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

/** Captions of map line 14 and the dashes of line 15, column by column (COPAU00-041 to -056). */
const COLUMNS = [
  { caption: "Sel", dashes: "---" },
  { caption: " Transaction ID ", dashes: "----------------" },
  { caption: "  Date  ", dashes: "--------" },
  { caption: "  Time  ", dashes: "--------" },
  { caption: "Type ", dashes: "----" },
  { caption: "A/D", dashes: "---" },
  { caption: "STS", dashes: "---" },
  { caption: "   Amount   ", dashes: "------------" },
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
const CELL = "py-0 whitespace-pre text-muted-foreground";

/**
 * The paging state COPAUS0C keeps in `CDEMO-CPVS-INFO` of the COMMAREA between interactions
 * (COPAUS0C-039), kept here for as long as the screen is shown. It is program state, not a payload.
 *
 * - `account`: `CDEMO-ACCT-ID`, the account F7 and F8 page, never the field's current text.
 * - `pageNum`: `CDEMO-CPVS-PAGE-NUM`.
 * - `firstKeys`: `CDEMO-CPVS-PAUKEY-PREV-PG`, index `p - 1` holding page `p`'s first key. The
 *   COBOL table stops at 20 pages (COPAUS0C-050, HALTED(X-COPAUS0C-H3)); paging goes on here.
 * - `lastKey`: `CDEMO-CPVS-PAUKEY-LAST`, the next F8's `page_key`.
 * - `nextPage`: `CDEMO-CPVS-NEXT-PAGE-FLG` = `'Y'`.
 * - `rowKeys`: `CDEMO-CPVS-AUTH-KEYS`, the key a selection on each row passes to COPAUS1C.
 */
type CpvsPagingState = {
  readonly account: string | null;
  readonly pageNum: number;
  readonly firstKeys: readonly string[];
  readonly lastKey: string | null;
  readonly nextPage: boolean;
  readonly rowKeys: readonly (string | null)[];
};

/** `INITIALIZE CARDDEMO-COMMAREA` (COPAUS0C-001): no account, page 0, no keys, flag not `Y`. */
const EMPTY_PAGING: CpvsPagingState = {
  account: null,
  pageNum: 0,
  firstKeys: [],
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
  const firstKeys = [...state.firstKeys];
  if (first !== undefined) {
    firstKeys[pageNum - 1] = first.auth_key;
  }
  return {
    account: state.account,
    pageNum,
    firstKeys,
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
 * is applied to the state it was sent from. The cursor returns to the account field after every
 * response (COPAUS0C-053). Nothing here logs or stores a response; the screen shows no card
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

  /** Shows `text` in the message row (none when `undefined`) and puts the cursor on the account. */
  function settle(text: string | undefined): void {
    setMessage(text);
    form.setFocus("acctId");
  }

  /** A failed read: its message alone, as the first failure's send shows it (X-COPAUS0C-1). */
  function fail(error: components["schemas"]["ScreenError"]): void {
    setErased(true);
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
   * invalid-selection text, which shows only when the read sets no message (COPAUS0C-011).
   */
  function readAccount(account: string, selectionMessage: string | undefined): void {
    setPaging((state) => ({ ...state, account, pageNum: 0 }));
    summary.mutate(
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
    );
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
    summary.mutate(
      { path: { acct_id: account }, query: { page_key: pageKey, direction } },
      {
        onSuccess: (page) => {
          showRows(page);
          setPaging((state) => afterPageRead(state, page));
          settle(undefined);
        },
        onError: fail,
      },
    );
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
        // COPAUS0C-010: XCTL COPAUS1C with the account typed on this ENTER and the row's key.
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
   */
  function onRejected(errors: FieldErrors<Copau00Values>): void {
    if (errors.acctId !== undefined) {
      form.setValue("acctId", "");
      setPaging((state) => ({ ...state, pageNum: 0 }));
      setErased(true);
    }
    settle(firstMessage(errors));
  }

  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    if (summary.isPending) {
      event.preventDefault();
      return;
    }
    void form.handleSubmit(onEnter, onRejected)(event);
  }

  /** F3, COPAUS0C-003: XCTL COMEN01C with no request; the paging state is dropped. */
  function back(): void {
    if (summary.isPending) {
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
    if (summary.isPending) {
      return;
    }
    if (paging.pageNum <= 1 || paging.account === null) {
      settle(MSG_TOP_OF_PAGE);
      return;
    }
    const pageNum = paging.pageNum - 1;
    setPaging((state) => ({ ...state, pageNum, nextPage: true }));
    readPage(paging.account, paging.firstKeys[pageNum - 1], "backward");
  }

  /**
   * F8, COPAUS0C-040 to -042: with the flag not `Y`, the bottom-of-page message; otherwise the page
   * after the last key. With the flag `Y` and no last key the COBOL issues a DL/I call with no PSB
   * scheduled: HALTED(X-COPAUS0C-H4) (COPAUS0C-043). The screen shows the bottom-of-page message
   * without a request there, which is not a parity result.
   */
  function pageForward(): void {
    if (summary.isPending) {
      return;
    }
    if (!paging.nextPage || paging.lastKey === null || paging.account === null) {
      settle(MSG_BOTTOM_OF_PAGE);
      return;
    }
    readPage(paging.account, paging.lastKey, "forward");
  }

  /** COPAUS0C-004: an unserved key is refused; the map goes with `ERASE` and nothing is read. */
  function invalidKey(): void {
    if (summary.isPending) {
      return;
    }
    setErased(true);
    settle(INVALID_KEY);
  }

  const actions: PfKeyHandlers = { F3: back, F7: pageBackward, F8: pageForward };
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({ ...item, onPress: actions[item.key] }));

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
      keyOptions={{ foldShifted: KEY_ROWS.COPAU00.foldShifted, onInvalidKey: invalidKey }}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={summary.isPending} onSubmit={onSubmit}>
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
                        maxLength={COPAU00_LENGTHS.acctId}
                        size={COPAU00_LENGTHS.acctId}
                        autoComplete="off"
                        autoFocus
                        className={FIELD}
                      />
                    </FormControl>
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
             * from column 2 to column 78. `GridCell` places a field on one line, and a Table on one
             * line of FieldGrid's equal-height rows would stretch every line to the Table's height,
             * so the Table's grid item spans the seven lines it holds on the map.
             */}
            <div className="col-start-2 col-span-78 row-start-14 row-span-7">
              <Table>
                <TableHeader>
                  <TableRow>
                    {COLUMNS.map((column) => (
                      <TableHead key={column.caption} scope="col" className="whitespace-pre">
                        {column.caption}
                      </TableHead>
                    ))}
                  </TableRow>
                  <TableRow aria-hidden="true">
                    {COLUMNS.map((column) => (
                      <TableHead key={column.caption} className="whitespace-pre">
                        {column.dashes}
                      </TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: ROWS }, (_, index) => {
                    const row = rows.at(index);
                    const isProtected = pageRead && row === undefined;
                    return (
                      // BLITZY [LAYOUT]: cells drop the default vertical padding (`py-0`) so a
                      // list row keeps the height of one map line.
                      <TableRow key={index}>
                        <TableCell className="py-0">
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
                                    readOnly={isProtected}
                                    tabIndex={isProtected ? -1 : undefined}
                                    className="w-9"
                                  />
                                </FormControl>
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
            </div>

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
