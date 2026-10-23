import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCotrn00cList } from "@/api/hooks";
import type { components } from "@/api/schema";
import {
  FIELD_INPUT,
  FieldGrid,
  GridCell,
  GridTable,
  LIST_CELL,
  LIST_ROW,
} from "@/components/screen/FieldGrid";
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
// `CCDA-MSG-INVALID-KEY` comes from the shared header module, the one copy every screen shows.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COTRN00_LENGTHS, cotrn00Schema, type Cotrn00Values } from "@/schemas/COTRN00";

/**
 * `COTRN00C`'s message for a selection code other than `S` (`app/cbl/COTRN00C.cbl:196-203`;
 * COTRN00C-010).
 */
const INVALID_SELECTION = "Invalid selection. Valid value is S";

/**
 * The program's message for a failed page read (`app/cbl/COTRN00C.cbl:612-618`; COTRN00C-027),
 * shown when a failure carries no message of its own.
 */
const LOOKUP_FAILED = "Unable to lookup transaction...";

/** Line 21, `POS=(21,12) LENGTH=50`, NEUTRAL BRT (COTRN00-087). */
const INSTRUCTION = "Type 'S' to View Transaction details from the list";

/** The row-24 labels in map order (COTRN00-089, COTRN00-096 to COTRN00-099). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COTRN00);

/**
 * The list's column captions (line 8) and rules (line 9), each its map `INITIAL` with its
 * padding (COTRN00-017 to COTRN00-026), and each column's width as a fraction of the list's 77
 * map columns: from its field's `POS` to the next one's, the selector column from 2 (6 columns),
 * the transaction ID from 8 (19), the date from 27 (11), the description from 38 (29) and the
 * amount from 67 to the list's end at 78 (12).
 */
const COLUMNS = [
  { caption: "Sel", rule: "---", width: "w-6/77" },
  { caption: " Transaction ID ", rule: "----------------", width: "w-19/77" },
  { caption: "  Date  ", rule: "--------", width: "w-11/77" },
  { caption: "     Description          ", rule: "--------------------------", width: "w-29/77" },
  { caption: "   Amount   ", rule: "------------", width: "w-12/77" },
] as const;

/*
 * BLITZY [LAYOUT]: every list line is a `LIST_ROW`, one map line pitch tall whatever it holds,
 * and every cell a `LIST_CELL`, without the cell's `p-2` padding, so each value starts on its
 * map column and the twelve lines keep the terminal's 24-line rhythm. The selector, at column 3,
 * is moved one of its column's six map columns to the right (`left-1/6`).
 */
/**
 * BLUE list values, shown byte for byte with their trailing spaces (COTRN00C-028 to
 * COTRN00C-032), one map line tall.
 */
const VALUE = `${LIST_CELL} whitespace-pre text-muted-foreground`;

/** A caption or rule, shown with its spaces. */
const HEADING = `${LIST_CELL} whitespace-pre`;

/** `SPACES` or `LOW-VALUES`, the COBOL test for an empty selector or transaction ID. */
const BLANK = /^[ \u0000]*$/;

/**
 * The paging state of `CDEMO-CT00-INFO` a menu entry starts from: no keys, page 0, next-page
 * flag `N` (COTRN00C-002). Every later state is the one the last response returned.
 */
const BLANK_PAGING: Pick<
  components["schemas"]["TranListResponse"],
  "first_key" | "last_key" | "page_no" | "has_next_page"
> = { first_key: null, last_key: null, page_no: 0, has_next_page: false };

/** The ten list lines before any page is read: all blank. */
const BLANK_ROWS: components["schemas"]["TranListResponse"]["rows"] = Array.from(
  { length: COTRN00_LENGTHS.rows },
  () => null,
);

/** The transfer keys endpoint 11 accepts on the request that brings the screen up. */
const ENTRY_KEYS: readonly components["schemas"]["TranListEntryKey"][] = ["enter", "pf3", "pf5"];

/**
 * The key a request is sent for: the entry request, ENTER, F7 or F8. It decides the paging state
 * kept after a failure and whether a page read clears the search field.
 */
type ListKey = "entry" | "enter" | "pf7" | "pf8";

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

function isBlank(text: string): boolean {
  return BLANK.test(text);
}

/** A paging key as the state may hold it: text, or nothing when the key is blank. */
function isPagingKey(value: unknown): value is string | null | undefined {
  return value === undefined || value === null || typeof value === "string";
}

/**
 * The transfer that brought the screen up (COTRN00C-003, COTRN00C-004).
 *
 * COTRN01 returns here by F3, when the list transferred to it, and by F5, with route state
 * `{ from: PATHS.COTRN01, entry_key: "pf3" | "pf5", list? }`; `list` is the paging state this
 * screen handed over, passed back unchanged when there was one (COTRN00C-002). An `entry_key`
 * endpoint 11 accepts names the key. Otherwise the origin decides: `from` `/transactions/view`
 * reads as COTRN01's F5, and the menu and every other origin enter as ENTER, which the API also
 * reads F3 as. F5 matters because the program's skip read then drops the lowest transaction
 * (X-COTRN00C-2).
 */
function entryKeyOf(state: unknown): components["schemas"]["TranListEntryKey"] {
  if (!isRecord(state)) {
    return "enter";
  }
  const named = ENTRY_KEYS.find((key) => key === state.entry_key);
  if (named !== undefined) {
    return named;
  }
  return state.from === PATHS.COTRN01 ? "pf5" : "enter";
}

/**
 * The paging state a transfer hands back in route state under `list`, as COTRN01C returns the
 * COMMAREA it received (COTRN00C-002); the blank state when there is none or it is malformed.
 * The state grants nothing: endpoint 11 re-reads every page.
 */
function pagingOf(state: unknown): typeof BLANK_PAGING {
  const list = isRecord(state) ? state.list : undefined;
  if (!isRecord(list)) {
    return BLANK_PAGING;
  }
  const { first_key, last_key, page_no, has_next_page } = list;
  if (
    isPagingKey(first_key) &&
    isPagingKey(last_key) &&
    typeof page_no === "number" &&
    Number.isInteger(page_no) &&
    page_no >= 0 &&
    typeof has_next_page === "boolean"
  ) {
    return { first_key: first_key ?? null, last_key: last_key ?? null, page_no, has_next_page };
  }
  return BLANK_PAGING;
}

/** The state as endpoint 11 takes it back: every field echoed, a blank key left out. */
function echo(paging: typeof BLANK_PAGING) {
  return {
    first_key: paging.first_key ?? undefined,
    last_key: paging.last_key ?? undefined,
    page_no: paging.page_no,
    has_next_page: paging.has_next_page,
  };
}

/**
 * The state after a 400 or a 500, whose `ScreenError` carries none: the echoed state with page
 * number 0 after ENTER or the entry request, which reset it before the read, and the next-page
 * flag `Y` after F7, which sets it before the read (COTRN00C-013, COTRN00C-027).
 */
function afterFailure(paging: typeof BLANK_PAGING, key: ListKey): typeof BLANK_PAGING {
  return {
    ...paging,
    page_no: key === "entry" || key === "enter" ? 0 : paging.page_no,
    has_next_page: key === "pf7" ? true : paging.has_next_page,
  };
}

/** `PAGENUM`, `POS=(4,71) LENGTH=8` (`COTRN00.bms:85-89`): the page number's eight digits. */
const PAGE_DIGITS = 8;

/** `PAGENUM`: the `PIC 9(08)` page number moved to `X(8)`, eight digits (COTRN00C-036). */
function pageNumberText(pageNo: number): string {
  return String(pageNo).padStart(PAGE_DIGITS, "0");
}

/** The failure's message: the `ScreenError` text, or the network or status error's own. */
function failureText(reason: unknown): string {
  return isRecord(reason) && typeof reason.message === "string" ? reason.message : LOOKUP_FAILED;
}

/**
 * The form field a failure's `ScreenError` names by its wire name. Endpoint 11 names only the
 * search field, `start_tran_id` (COTRN00C-013); the selectors are never sent.
 */
function formFieldOf(reason: unknown): "startTranId" | undefined {
  return isRecord(reason) && reason.field === "start_tran_id" ? "startTranId" : undefined;
}

/** The first message the form schema reports, in field order. */
function firstFormError(errors: FieldErrors<Cotrn00Values>): string | undefined {
  const selectors = Array.from(
    { length: COTRN00_LENGTHS.rows },
    (_, index) => errors.selections?.[index]?.message,
  );
  return (
    errors.startTranId?.message ??
    errors.selections?.message ??
    errors.selections?.root?.message ??
    selectors.find((text) => text !== undefined)
  );
}

/**
 * List Transactions, map `COTRN0A` of transaction CT00 (COTRN00C), at `/transactions`: ten
 * transactions to a page from endpoint 11 (`modern/audit/screens.md` COTRN00-001 to COTRN00-099;
 * `modern/audit/online/COTRN00C.md`).
 *
 * - Entry reads the first page with a blank search, as the program's first entry runs ENTER
 *   (COTRN00C-003, COTRN00C-004).
 * - ENTER: the first non-blank selector in row order decides (COTRN00C-007). `S` or `s` on a row
 *   that holds a transaction opens it at `/transactions/view` with no request (COTRN00C-008); any
 *   other code on such a row sends the search with `Invalid selection. Valid value is S` pending,
 *   which a message of the response or of a `ScreenError` replaces (COTRN00C-010, COTRN00C-035);
 *   a code on a blank row is ignored (COTRN00C-009). Otherwise ENTER searches from the typed ID.
 * - F7 and F8 page backward and forward from the echoed paging state (COTRN00C-015 to
 *   COTRN00C-026); F3 returns to the main menu (COTRN00C-005); any other F-key shows the
 *   invalid-key message (COTRN00C-006).
 * - The rows, `PAGENUM` and the search field change only when a page was read, the search field
 *   clearing after a forward page (COTRN00C-036, COTRN00C-037). Selectors are never cleared
 *   (COTRN00C-038), and the cursor returns to the search field after every answer.
 * - While a request is outstanding the keyboard is locked, as the terminal's stays until the
 *   program answers: ENTER, F7 and F8 are refused before any selection is read, an unserved F-key
 *   shows nothing, and the search field and selectors are read-only. F3 still leaves. An `S`
 *   selection's transfer keeps the keyboard locked the same way until COTRN01 replaces this
 *   screen.
 *
 * The page size is never sent, so the server's ten rows, the map's, decide each page.
 */
export function COTRN00(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const list = useCotrn00cList();
  const formId = useId();
  const pageId = useId();
  const form = useForm<Cotrn00Values>({
    resolver: zodResolver(cotrn00Schema),
    defaultValues: {
      startTranId: "",
      selections: Array.from({ length: COTRN00_LENGTHS.rows }, () => ""),
    },
  });
  const [rows, setRows] = useState(BLANK_ROWS);
  const [pageLabel, setPageLabel] = useState("");
  const [paging, setPaging] = useState(() => pagingOf(location.state));
  const [message, setMessage] = useState<string>();
  const [busy, setBusy] = useState(false);
  // One request at a time, as the terminal keyboard locks until the program answers. ENTER takes
  // the lock before the form's validation starts, so no selection is acted on while it is held.
  const inFlight = useRef(false);
  // The entry request runs once, also when StrictMode runs the mount effect twice.
  const entered = useRef(false);

  /** Locks the keyboard for one key; `false` while another key holds it. */
  function lock(): boolean {
    if (inFlight.current) {
      return false;
    }
    inFlight.current = true;
    setBusy(true);
    return true;
  }

  /**
   * Releases the keyboard: the answer arrived, or ENTER ended without a request. A selection's
   * transfer keeps it until COTRN01 replaces this screen.
   */
  function unlock(): void {
    inFlight.current = false;
    setBusy(false);
  }

  /**
   * Sends endpoint 11 for `key` under the lock its caller holds, with the current paging state
   * echoed, then shows the answer and releases the lock. The promise of `mutateAsync` carries the
   * answer, so it lands even when the mutation observer is detached, as StrictMode's simulated
   * unmount does to the entry request.
   */
  function send(
    key: ListKey,
    query: NonNullable<Parameters<typeof list.mutateAsync>[0]["query"]>,
    selectionMessage?: string,
  ): void {
    // A server error marks the field only until the next answer, so each request drops it. A
    // correction drops it sooner: the server rejects the field only on ENTER, after which the
    // form revalidates a field on every change (its default `reValidateMode`).
    if (form.getFieldState("startTranId").error?.type === "server") {
      form.clearErrors("startTranId");
    }
    const sent = paging;
    void list
      .mutateAsync({ query: { ...query, ...echo(sent) } })
      .then(
        (page) => {
          setPaging({
            first_key: page.first_key ?? null,
            last_key: page.last_key ?? null,
            page_no: page.page_no,
            has_next_page: page.has_next_page,
          });
          if (!page.rows_kept) {
            setRows(page.rows);
            setPageLabel(pageNumberText(page.page_no));
            if (key !== "pf7") {
              form.setValue("startTranId", "");
            }
          }
          setMessage(page.message ?? selectionMessage);
        },
        (reason: unknown) => {
          setPaging(afterFailure(sent, key));
          const text = failureText(reason);
          setMessage(text);
          // The field named turns invalid with the server's text, which replaces the schema's
          // verdict; the error row stays the one place it shows.
          const named = formFieldOf(reason);
          if (named !== undefined) {
            form.setError(named, { type: "server", message: text });
          }
        },
      )
      .finally(() => {
        unlock();
        form.setFocus("startTranId");
      });
  }

  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    if (lock()) {
      send("entry", { entry_key: entryKeyOf(location.state) });
    }
    // Runs on mount only: later requests come from the keys.
  }, []);

  /** ENTER after validation, under the lock {@link onSubmit} took. */
  function onEnter(values: Cotrn00Values): void {
    const selected = values.selections.findIndex((code) => !isBlank(code));
    let selectionMessage: string | undefined;
    if (selected >= 0) {
      const code = values.selections[selected];
      const tranId = rows[selected]?.tran_id ?? "";
      if (!isBlank(tranId)) {
        if (code === "S" || code === "s") {
          // The lock ENTER took stays held until COTRN01 replaces this screen, so no key pages
          // the list or changes the selection while its module loads.
          navigate(PATHS.COTRN01, {
            state: { from: PATHS.COTRN00, tran_id: tranId, list: paging },
          });
          return;
        }
        selectionMessage = INVALID_SELECTION;
      }
    }
    send(
      "enter",
      { start_tran_id: values.startTranId === "" ? undefined : values.startTranId },
      selectionMessage,
    );
  }

  function onInvalid(errors: FieldErrors<Cotrn00Values>): void {
    unlock();
    setMessage(firstFormError(errors));
  }

  const validate = form.handleSubmit(onEnter, onInvalid);

  /** ENTER: refused while a request is outstanding, else validated and acted on under the lock. */
  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    if (!lock()) {
      event.preventDefault();
      return;
    }
    void validate(event);
  }

  const handlers: PfKeyHandlers = {
    F3: () => {
      navigate(PATHS.COMEN01);
    },
    F7: () => {
      if (lock()) {
        send("pf7", { direction: "backward" });
      }
    },
    F8: () => {
      if (lock()) {
        send("pf8", { direction: "forward" });
      }
    },
  };
  // ENTER has no action of its own, so its button and the Enter key submit the form. While a
  // request is outstanding ENTER, F7 and F8 are refused; F3 still leaves.
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({
    ...item,
    onPress: handlers[item.key],
    disabled: busy && (item.key === "ENTER" || item.key === "F7" || item.key === "F8"),
  }));

  return (
    <ScreenShell
      transaction={SCREENS.COTRN00.transaction}
      program={SCREENS.COTRN00.program}
      title="List Transactions"
      formId={formId}
      message={{ kind: "error", text: message }}
      keys={keys}
      keyOptions={{
        foldShifted: KEY_ROWS.COTRN00.foldShifted,
        onInvalidKey: () => {
          if (inFlight.current) {
            return;
          }
          setMessage(INVALID_KEY);
          form.setFocus("startTranId");
        },
      }}
      busy={busy}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={busy} onSubmit={onSubmit}>
          {/*
           * Auto-height lines: with the grid's equal lines, the ten-row table on line 8 would
           * make every one of the 24 lines as tall as the table.
           */}
          <FieldGrid className="grid-rows-none">
            <GridCell row={4} col={65} span={5}>
              <Label htmlFor={pageId} className="h-9 whitespace-pre font-semibold text-foreground">
                Page:
              </Label>
            </GridCell>
            <GridCell row={4} col={71} span={PAGE_DIGITS}>
              {/*
               * ASKIP output: the cursor skips it, as on the terminal. Like every map input it
               * takes the shared map-field box, `FIELD_INPUT` with `size` = its `LENGTH`, so all
               * eight digits show on the narrowest frame and a page change stays visible.
               */}
              <Input
                id={pageId}
                readOnly
                tabIndex={-1}
                value={pageLabel}
                size={PAGE_DIGITS}
                className={`${FIELD_INPUT} text-muted-foreground`}
              />
            </GridCell>
            <FormField
              control={form.control}
              name="startTranId"
              render={({ field }) => (
                // `contents` makes the label and input cells items of the screen grid.
                <FormItem className="contents">
                  <GridCell row={6} col={5} span={15}>
                    <FormLabel className="h-9 whitespace-pre text-foreground">
                      Search Tran ID:
                    </FormLabel>
                  </GridCell>
                  <GridCell row={6} col={21} span={16}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={COTRN00_LENGTHS.startTranId}
                        size={COTRN00_LENGTHS.startTranId}
                        autoComplete="off"
                        autoFocus
                        className={FIELD_INPUT}
                      />
                    </FormControl>
                  </GridCell>
                  {/*
                   * The error row is the map's only visible message, so the field's message is
                   * for assistive technology only, read with the input it rejects; out of flow,
                   * it takes no grid cell.
                   */}
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />
            <GridTable row={8} col={2} span={77}>
              {/*
               * BLITZY [A11Y]: the selectors keep the input's default `h-9` (36px) square, below
               * the 44px touch-target guidance, and its focus ring (`ring-ring/50`) is about 1.5:1
               * against the card, below 3:1. Both are the system's defaults; flagged for designer
               * review.
               */}
              <Table aria-label="Transactions" className="table-fixed">
                <colgroup>
                  {COLUMNS.map((column) => (
                    <col key={column.caption} className={column.width} />
                  ))}
                </colgroup>
                <TableHeader>
                  <TableRow className={LIST_ROW}>
                    {COLUMNS.map((column) => (
                      <TableHead key={column.caption} scope="col" className={HEADING}>
                        {column.caption}
                      </TableHead>
                    ))}
                  </TableRow>
                  {/* Line 9's rules are decoration under the captions, not column titles. */}
                  <TableRow aria-hidden="true" className={LIST_ROW}>
                    {COLUMNS.map((column) => (
                      <TableCell key={column.caption} className={HEADING}>
                        {column.rule}
                      </TableCell>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Array.from({ length: COTRN00_LENGTHS.rows }, (_, index) => {
                    const row = rows[index] ?? null;
                    return (
                      <TableRow key={index} className={LIST_ROW}>
                        <TableCell className={LIST_CELL}>
                          <FormField
                            control={form.control}
                            name={`selections.${index}`}
                            render={({ field }) => (
                              <FormItem>
                                <FormLabel className="sr-only">
                                  {`Select row ${index + 1}`}
                                </FormLabel>
                                <FormControl>
                                  <Input
                                    {...field}
                                    readOnly={busy}
                                    maxLength={COTRN00_LENGTHS.selection}
                                    size={COTRN00_LENGTHS.selection}
                                    autoComplete="off"
                                    className={`${FIELD_INPUT} relative left-1/6 w-9 text-center`}
                                  />
                                </FormControl>
                                <FormMessage className="sr-only" />
                              </FormItem>
                            )}
                          />
                        </TableCell>
                        <TableCell className={VALUE}>{row?.tran_id ?? ""}</TableCell>
                        <TableCell className={VALUE}>{row?.display_date ?? ""}</TableCell>
                        <TableCell className={VALUE}>{row?.description ?? ""}</TableCell>
                        <TableCell className={VALUE}>{row?.amount ?? ""}</TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </GridTable>
            <GridCell row={21} col={12} span={50}>
              <p className="whitespace-pre font-semibold text-foreground">{INSTRUCTION}</p>
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
