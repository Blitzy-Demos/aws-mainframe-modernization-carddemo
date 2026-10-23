import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldErrors, type FieldPath } from "react-hook-form";
import { useNavigate } from "react-router";
import { cn } from "cn";
import { useCotrtlicList, useCotrtlicListActions } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldControl } from "@/components/screen/FieldControl";
import {
  FIELD_INPUT,
  FieldGrid,
  GridCell,
  GridTable,
  LIST_CELL,
  LIST_ROW,
  fieldInputClass,
} from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
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
import { KEY_ROWS, keyRowItems, type PfKey, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COTRTLI_LENGTHS, cotrtliSchema, type CotrtliValues } from "@/schemas/COTRTLI";

/** The screen title, the map literal at `POS=(4,28)` (COTRTLI-011). */
const TITLE = "Maintain Transaction Type";

/** `INFOMSGO PIC X(45)`: the width the info message is centred in (COTRTLIC-058). */
const INFO_WIDTH = 45;

/** `PAGENO`, `LENGTH=3` at `POS=(4,76)` (COTRTLI-013). */
const PAGE_NO_LENGTH = 3;

/**
 * The list's columns as fractions of its 71 map columns, each from its line-12 field's `POS` to
 * the next one's: the selector column from 4 (13 columns), the type from 17 (8) and the
 * description from 25 to the list's end at 74 (50).
 */
const COLUMN_WIDTHS = ["w-13/71", "w-8/71", "w-50/71"] as const;

/**
 * A label fills its grid line, so its text, centred by `Label`'s own flex alignment, stands level
 * with the input on the same map line instead of at the line's top.
 */
const LINE_LABEL = "h-full";

/** The 50 dashes under the description caption, `POS=(11,25)` (COTRTLI-025). */
const DESCRIPTION_DASHES = "-".repeat(COTRTLI_LENGTHS.description);

/**
 * The labelled keys refused with ENTER while the keyboard is locked: those that send a request,
 * and F2, whose transfer takes the same lock (COTRTLIC-016). F3 only navigates (COTRTLIC-015),
 * so it stays available.
 */
const LOCKED_KEYS: ReadonlySet<PfKey> = new Set<PfKey>(["F2", "F7", "F8", "F10"]);

/** The form before the first response: both filters and all seven rows blank. */
function blankValues(): CotrtliValues {
  return {
    typeFilter: "",
    descFilter: "",
    rows: Array.from({ length: COTRTLI_LENGTHS.rows }, () => ({ selection: "", description: "" })),
  };
}

/** The seven row positions of a page, top to bottom; `null` is a position that holds no type. */
function screenRows(
  page: components["schemas"]["TranTypeListResponse"] | null,
): (components["schemas"]["TranTypeListShownRow"] | null)[] {
  return Array.from({ length: COTRTLI_LENGTHS.rows }, (_, index) => page?.rows[index] ?? null);
}

/**
 * The form values a page shows: the filters as the response shows them and, per row, the
 * selection and description it returned, blank where the position holds no type.
 */
function pageValues(page: components["schemas"]["TranTypeListResponse"]): CotrtliValues {
  return {
    typeFilter: page.type_filter.text,
    descFilter: page.desc_filter.text,
    rows: screenRows(page).map((row) => ({
      selection: row?.selection ?? "",
      description: row?.description ?? "",
    })),
  };
}

/**
 * The info message centred in its 45 characters, as `2500-SETUP-MESSAGE` places it: from column
 * (45 minus its trimmed length) / 2 + 1, integer part (COTRTLIC-058). Empty when there is none.
 */
function centredInfo(text: string | null | undefined): string {
  const shown = (text ?? "").trim();
  if (shown === "") {
    return "";
  }
  return " ".repeat(Math.max(0, Math.trunc((INFO_WIDTH - shown.length) / 2))) + shown;
}

/**
 * Whether an endpoint 25 request carries an action, `WS-ACTIONS-REQUESTED > 0`: a non-blank
 * selection at a position whose selector the shown page left enterable. The server reads every
 * other selection as blank, as the 3270 sends no protected field (COTRTLIC-018, COTRTLIC-020).
 */
function actionsRequested(
  rows: readonly { readonly selection: string }[],
  shown: components["schemas"]["TranTypeListResponse"],
): boolean {
  const positions = screenRows(shown);
  return rows.some(
    (row, index) => positions[index]?.selector_enterable === true && row.selection.trim() !== "",
  );
}

/**
 * The field the cursor goes to after a response, the first in map order that
 * `2200-SETUP-ARRAY-ATTRIBS` and `2400-SETUP-SCREEN-ATTRS` mark (COTRTLIC-056, COTRTLIC-057): a
 * filter in error, then per row a flagged field or the field of the highlighted row (its
 * description while its update is pending, otherwise its selector), else the type filter.
 */
function focusTarget(
  page: components["schemas"]["TranTypeListResponse"],
  rowErrors: readonly components["schemas"]["TranTypeListRowError"][],
): FieldPath<CotrtliValues> {
  if (page.type_filter.status === "in_error") {
    return "typeFilter";
  }
  if (page.desc_filter.status === "in_error") {
    return "descFilter";
  }
  const positions = screenRows(page);
  for (let index = 0; index < positions.length; index += 1) {
    const flagged = rowErrors.filter((error) => error.row === index + 1);
    if (flagged.some((error) => error.field === "selection")) {
      return `rows.${index}.selection`;
    }
    if (flagged.some((error) => error.field === "description")) {
      return `rows.${index}.description`;
    }
    const row = positions[index];
    if (row?.highlighted === true) {
      return row.description_enterable ? `rows.${index}.description` : `rows.${index}.selection`;
    }
  }
  return "typeFilter";
}

/** The form field a `ScreenError.field` wire name addresses, such as `rows[2].selection`. */
function fieldOfWireName(wire: string | null | undefined): FieldPath<CotrtliValues> | undefined {
  if (wire === "type_filter") {
    return "typeFilter";
  }
  if (wire === "desc_filter") {
    return "descFilter";
  }
  for (let index = 0; index < COTRTLI_LENGTHS.rows; index += 1) {
    if (wire === `rows[${index}].selection`) {
      return `rows.${index}.selection`;
    }
    if (wire === `rows[${index}].description`) {
      return `rows.${index}.description`;
    }
  }
  return undefined;
}

/**
 * Whether a thrown error body is an endpoint 24 page. The API answers a filter in error (400) and
 * a failed read (500) with the page and its message rather than a `ScreenError`, and the hooks
 * throw every non-2xx body unchanged.
 */
function isListPage(body: unknown): body is components["schemas"]["TranTypeListResponse"] {
  return (
    typeof body === "object" &&
    body !== null &&
    "state" in body &&
    "page_key" in body &&
    "type_filter" in body &&
    "desc_filter" in body &&
    "rows" in body &&
    Array.isArray(body.rows)
  );
}

/**
 * Whether a thrown error body is an endpoint 25 result: the page, its row errors and its message,
 * which the API sends with an input error (400), a row deleted by another user (404) and a failed
 * statement (500).
 */
function isActionResult(body: unknown): body is components["schemas"]["TranTypeListActionResult"] {
  return isListPage(body) && "row_errors" in body && Array.isArray(body.row_errors);
}

/** The first client-side validation message, in map order. */
function firstMessage(errors: FieldErrors<CotrtliValues>): string | undefined {
  const rowMessages = Array.from({ length: COTRTLI_LENGTHS.rows }, (_, index) => [
    errors.rows?.[index]?.selection?.message,
    errors.rows?.[index]?.description?.message,
  ]).flat();
  return [
    errors.typeFilter?.message,
    errors.descFilter?.message,
    ...rowMessages,
    errors.rows?.message,
  ].find((text) => text !== undefined && text !== "");
}

/**
 * CTLI / COTRTLIC, map `CTRTLIA`: the admin list of the Db2 data version of the transaction
 * types, seven rows a page, with a single-row update (`U`) or delete (`D`) confirmed by F10
 * (AAP 0.3.4.1, 0.3.5; `modern/audit/screens.md` COTRTLI-001 to COTRTLI-093,
 * `modern/audit/online/COTRTLIC.md`).
 *
 * - Mount: endpoint 24 with no paging, the first entry (COTRTLIC-002).
 * - ENTER and F10: endpoint 25 with the filters, the seven selections and descriptions as typed
 *   and the `state` of the last response, echoed unchanged; the server decides whether F10
 *   commits (COTRTLIC-036). Every unlabelled F-key is ENTER (COTRTLIC-014).
 * - F7 and F8: endpoint 24 with the filters on the screen and the last response's `page_key`.
 * - F2 and F3: navigation only (COTRTLIC-015, COTRTLIC-016).
 *
 * While a request is out the keyboard is locked, as the 3270's is until the program answers. The
 * lock is taken when a key is accepted, before the form's asynchronous check, and opens when the
 * check refuses the input or the reply settles. F2 takes the same lock for its transfer and holds
 * it until COTRTUP replaces this screen. Meanwhile ENTER, F2, F7, F8 and F10 are disabled and
 * refused, the key row and the form are busy and every input is read-only; F3 still navigates.
 *
 * The state is the server's: it carries no authority, and the server re-reads and re-validates
 * every commit. The error line shows the server's message; the zod schema enforces only the map
 * lengths.
 */
export function COTRTLI(): ReactNode {
  const navigate = useNavigate();
  const formId = useId();
  const pageNoId = useId();
  const { mutate: readPage } = useCotrtlicList();
  const { mutate: sendActions } = useCotrtlicListActions();
  const form = useForm<CotrtliValues>({
    resolver: zodResolver(cotrtliSchema),
    defaultValues: blankValues(),
  });
  const { reset, setError, clearErrors, getValues, handleSubmit, setFocus } = form;

  const [page, setPage] = useState<components["schemas"]["TranTypeListResponse"] | null>(null);
  const [message, setMessage] = useState<string | undefined>(undefined);
  // `2400-SETUP-SCREEN-ATTRS`: both filters are shown protected, in blue, while the request that
  // produced the page carried an action (COTRTLIC-057). The response does not say so, so the
  // screen keeps it from the request it sent.
  const [filtersLocked, setFiltersLocked] = useState(false);
  const [cursor, setCursor] = useState<{ readonly name: FieldPath<CotrtliValues> } | null>(null);
  // `inFlight` is the lock the key handlers read at once, so a second key pressed before the next
  // render is refused too; `busy` renders it while a request is out.
  const inFlight = useRef(false);
  const [busy, setBusy] = useState(false);

  /** Locks the keyboard for a request; `false`, refusing the key, while one is outstanding. */
  const lock = useCallback((): boolean => {
    if (inFlight.current) {
      return false;
    }
    inFlight.current = true;
    setBusy(true);
    return true;
  }, []);

  /** Opens the keyboard again: the reply settled, or the form's check sent nothing. */
  const unlock = useCallback(() => {
    inFlight.current = false;
    setBusy(false);
  }, []);

  /** Shows a page, its row and filter errors, its messages and its cursor. */
  const show = useCallback(
    (
      next: components["schemas"]["TranTypeListResponse"],
      rowErrors: readonly components["schemas"]["TranTypeListRowError"][],
      locked: boolean,
    ) => {
      setPage(next);
      setMessage(next.message ?? undefined);
      setFiltersLocked(locked);
      reset(pageValues(next));
      for (const error of rowErrors) {
        const index = error.row - 1;
        if (index >= 0 && index < COTRTLI_LENGTHS.rows) {
          setError(
            error.field === "description" ? `rows.${index}.description` : `rows.${index}.selection`,
            { type: "server", message: error.message },
          );
        }
      }
      if (next.type_filter.status === "in_error") {
        setError("typeFilter", { type: "server", message: next.message ?? "" });
      }
      if (next.desc_filter.status === "in_error") {
        setError("descFilter", { type: "server", message: next.message ?? "" });
      }
      setCursor({ name: focusTarget(next, rowErrors) });
    },
    [reset, setError],
  );

  /** Shows a `ScreenError` (or a network failure) over the page already shown. */
  const showFailure = useCallback(
    (error: components["schemas"]["ScreenError"]) => {
      setMessage(error.message);
      const field = fieldOfWireName(error.field);
      if (field !== undefined) {
        setError(field, { type: "server", message: error.message });
        setCursor({ name: field });
      }
    },
    [setError],
  );

  /**
   * Endpoint 24 without paging: the first entry, page 1 under no filter (COTRTLIC-002). Its caller
   * has locked the keyboard; the reply opens it.
   */
  const load = useCallback(() => {
    readPage(
      { query: {} },
      {
        onSuccess: (next) => {
          show(next, [], false);
        },
        onError: (error) => {
          const body: unknown = error;
          if (isListPage(body)) {
            show(body, [], false);
          } else {
            showFailure(error);
          }
        },
        onSettled: unlock,
      },
    );
  }, [readPage, show, showFailure, unlock]);

  // One first entry per mount, also under StrictMode's repeated effects.
  const entered = useRef(false);
  useEffect(() => {
    if (!entered.current) {
      entered.current = true;
      lock();
      load();
    }
  }, [load, lock]);

  useEffect(() => {
    if (cursor !== null) {
      setFocus(cursor.name);
    }
  }, [cursor, setFocus]);

  /** F7 (`backward`) and F8 (`forward`): endpoint 24 under the filters on the screen. */
  function movePage(direction: components["schemas"]["PageDirection"]): void {
    if (!lock()) {
      return;
    }
    if (page === null) {
      load();
      return;
    }
    const values = getValues();
    clearErrors();
    readPage(
      {
        query: {
          type_filter: values.typeFilter,
          desc_filter: values.descFilter,
          page_key: page.page_key,
          direction,
        },
      },
      {
        onSuccess: (next) => {
          show(next, [], false);
        },
        onError: (error) => {
          const body: unknown = error;
          if (isListPage(body)) {
            show(body, [], false);
          } else {
            showFailure(error);
          }
        },
        onSettled: unlock,
      },
    );
  }

  /**
   * ENTER and F10: endpoint 25 with the screen's input and the last `state`, unchanged. Reached
   * once the form's check passed, with the keyboard locked by `lockedSubmit`; the reply opens it.
   */
  function sendKey(aid: components["schemas"]["CotrtlicAid"], values: CotrtliValues): void {
    if (page === null) {
      load();
      return;
    }
    const rows = values.rows.map((row) => ({
      selection: row.selection,
      description: row.description,
    }));
    const locked = actionsRequested(rows, page);
    sendActions(
      {
        body: {
          aid,
          type_filter: values.typeFilter,
          desc_filter: values.descFilter,
          rows,
          state: page.state,
        },
      },
      {
        onSuccess: (result) => {
          show(result, result.row_errors, locked);
        },
        onError: (error) => {
          const body: unknown = error;
          if (isActionResult(body)) {
            show(body, body.row_errors, locked);
          } else {
            showFailure(error);
          }
        },
        onSettled: unlock,
      },
    );
  }

  /**
   * The client-side length check failed: its message stands until the server answers, and the
   * keyboard opens again, as nothing was sent.
   */
  function showInvalid(errors: FieldErrors<CotrtliValues>): void {
    unlock();
    setMessage(firstMessage(errors));
  }

  const submitEnter = handleSubmit((values) => {
    sendKey("ENTER", values);
  }, showInvalid);
  const submitSave = handleSubmit((values) => {
    sendKey("PF10", values);
  }, showInvalid);

  /**
   * ENTER or F10 by its button, its key or an Enter in a field: `submit` (one of the two above)
   * once the keyboard is locked. The lock is taken before the form's check, which settles later,
   * so a key that arrives during the check or while the request is out sends nothing and never
   * reaches the browser's own form submission. A check that fails unexpectedly sent nothing, so it
   * opens the lock again and still reports its error.
   */
  function lockedSubmit(submit: typeof submitEnter, event?: FormEvent<HTMLFormElement>): void {
    if (!lock()) {
      event?.preventDefault();
      return;
    }
    void submit(event).catch((error: unknown) => {
      unlock();
      throw error;
    });
  }

  const handlers: PfKeyHandlers = {
    // COTRTLIC-016: to CTTU to add a type; the list state is discarded. `from` lets CTTU's F3
    // return here (COTRTUPC-044). F2 is refused while a request is out, and its lock stays held
    // until COTRTUP replaces this screen, so no key sends a request while its module loads.
    F2: () => {
      if (!lock()) {
        return;
      }
      void navigate(PATHS.COTRTUP, { state: { from: PATHS.COTRTLI } });
    },
    // COTRTLIC-015: always the admin menu. Every display records COTRTLIC itself as the calling
    // program, so the COBOL's test on its caller always picks COADM01C; the screen therefore does
    // not return to whichever screen opened it.
    F3: () => {
      void navigate(PATHS.COADM01, { state: { from: PATHS.COTRTLI } });
    },
    F7: () => {
      movePage("backward");
    },
    F8: () => {
      movePage("forward");
    },
    F10: () => {
      lockedSubmit(submitSave);
    },
  };

  // The map labels no ENTER key although COTRTLIC serves it, so ENTER gets a visible button named
  // after the bare key, first in the row (COTRTLI-093, X-SCREENS-10): every PF action exists as a
  // button (AAP 0.4.4), and a hidden one could not be clicked.
  const keys: PfKeyBarEntry[] = [
    { key: "ENTER", label: "ENTER", dark: false, disabled: busy },
    ...keyRowItems(KEY_ROWS.COTRTLI).map((item) => ({
      ...item,
      onPress: handlers[item.key],
      disabled: busy && LOCKED_KEYS.has(item.key),
    })),
  ];

  const positions = screenRows(page);

  return (
    <ScreenShell
      transaction={SCREENS.COTRTLI.transaction}
      program={SCREENS.COTRTLI.program}
      title={TITLE}
      titleRow={
        <>
          {/* COTRTLI-012, COTRTLI-013: `Page ` and `PAGENO`, on the title's line 4 of the map. */}
          <GridCell row={1} col={70} span={5}>
            <Label htmlFor={pageNoId} className={LINE_LABEL}>
              Page
            </Label>
          </GridCell>
          <GridCell row={1} col={76} span={3}>
            {/*
             * Every input of the screen carries `size` = its map `LENGTH` and takes the shared
             * map-field box (`FIELD_INPUT`, through `fieldInputClass` for the two-character type
             * filter): a grid field covers its `LENGTH` columns, a list description fills its
             * table column, a list selector is a one-character box (`w-9`), and none is
             * narrower than its value plus the inset.
             */}
            <Input
              id={pageNoId}
              readOnly
              tabIndex={-1}
              size={PAGE_NO_LENGTH}
              className={FIELD_INPUT}
              value={page === null ? "" : String(page.page_no)}
            />
          </GridCell>
        </>
      }
      formId={formId}
      message={{ text: message, kind: "error" }}
      info={centredInfo(page?.info_message)}
      keys={keys}
      keyOptions={{
        foldShifted: KEY_ROWS.COTRTLI.foldShifted,
        // COTRTLIC-014: COTRTLIC treats every key it does not serve as ENTER.
        onInvalidKey: () => {
          lockedSubmit(submitEnter);
        },
      }}
      busy={busy}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={busy}
          onSubmit={(event) => {
            lockedSubmit(submitEnter, event);
          }}
        >
          <FieldGrid>
            <FormField
              control={form.control}
              name="typeFilter"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={6} col={30} span={12}>
                    <FormLabel className={LINE_LABEL}>Type Filter:</FormLabel>
                  </GridCell>
                  <GridCell row={6} col={44} span={COTRTLI_LENGTHS.typeFilter}>
                    {/*
                     * No input of the map has a hint, so each control names only its field's
                     * message, while that shows text: a filter the page puts in error with no
                     * message is marked invalid and described by nothing.
                     */}
                    <FieldControl>
                      <Input
                        {...field}
                        maxLength={COTRTLI_LENGTHS.typeFilter}
                        size={COTRTLI_LENGTHS.typeFilter}
                        autoFocus
                        autoComplete="off"
                        readOnly={filtersLocked || busy}
                        tabIndex={filtersLocked ? -1 : undefined}
                        className={cn(
                          fieldInputClass(COTRTLI_LENGTHS.typeFilter),
                          "aria-invalid:text-destructive",
                          filtersLocked && "text-muted-foreground",
                        )}
                      />
                    </FieldControl>
                  </GridCell>
                  {/*
                   * For assistive technology only: the error line is the visible message. Each
                   * filter's message is its item's direct child rather than a cell's: an `sr-only`
                   * grid item takes no cell and sits at the grid's origin, while one in a cell
                   * past the edge of a narrow viewport would widen the page.
                   */}
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name="descFilter"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={8} col={4} span={19}>
                    <FormLabel className={LINE_LABEL}>Description Filter:</FormLabel>
                  </GridCell>
                  <GridCell row={8} col={25} span={COTRTLI_LENGTHS.descFilter}>
                    <FieldControl>
                      <Input
                        {...field}
                        maxLength={COTRTLI_LENGTHS.descFilter}
                        size={COTRTLI_LENGTHS.descFilter}
                        autoComplete="off"
                        readOnly={filtersLocked || busy}
                        tabIndex={filtersLocked ? -1 : undefined}
                        className={cn(
                          FIELD_INPUT,
                          "aria-invalid:text-destructive",
                          filtersLocked && "text-muted-foreground",
                        )}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            {/* Map lines 10 to 18, columns 4 to 74: the captions, the dashes and seven rows of
                the map's eight; COTRTLIC never writes the eighth, line 19 (X-SCREENS-13). One
                `LIST_ROW` per map line, so the table stands on those nine lines without making
                any line of the grid taller. Each column starts at its line-12 field's `POS`
                (selector column 4, type 17, description 25); a caption, dash or selector whose
                map column differs is moved by a fraction of its own column's width. */}
            <GridTable row={10} col={4} span={71} rowSpan={9}>
              <Table aria-label="Transaction types" className="table-fixed">
                <colgroup>
                  {COLUMN_WIDTHS.map((width) => (
                    <col key={width} className={width} />
                  ))}
                </colgroup>
                <TableHeader>
                  <TableRow className={LIST_ROW}>
                    <TableHead scope="col" className={LIST_CELL}>
                      Select
                    </TableHead>
                    <TableHead scope="col" className={LIST_CELL}>
                      <span className="relative -left-1/8">Type</span>
                    </TableHead>
                    <TableHead scope="col" className={LIST_CELL}>
                      <span className="relative left-17/50">Description</span>
                    </TableHead>
                  </TableRow>
                  {/* The dashes under the captions are decoration, not column titles. */}
                  <TableRow aria-hidden="true" className={LIST_ROW}>
                    <TableCell className={LIST_CELL}>------</TableCell>
                    <TableCell className={LIST_CELL}>
                      <span className="relative -left-2/8">-----</span>
                    </TableCell>
                    <TableCell className={LIST_CELL}>{DESCRIPTION_DASHES}</TableCell>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {positions.map((row, index) => {
                    const name = row === null ? `row ${index + 1}` : `type ${row.type_code}`;
                    const selectorOpen = row?.selector_enterable === true;
                    const descriptionOpen = row?.description_enterable === true;
                    const highlighted = row?.highlighted === true;
                    return (
                      // The seven positions are fixed map rows, so the position is the key.
                      // COTRTLIC marks the pending, deleted or updated row by colour alone
                      // (`DFHNEUTR` to its type and description, COTRTLIC.cbl:1348-1370), and
                      // NEUTRAL and DEFAULT both snap to the foreground token. Emphasis, as for
                      // BRT (`font-semibold`), and `aria-current` make the "HIGHLIGHTED row" the
                      // messages name perceivable without colour (WCAG 1.4.1).
                      <TableRow
                        key={index}
                        data-state={highlighted ? "selected" : undefined}
                        aria-current={highlighted ? "true" : undefined}
                        className={cn(LIST_ROW, highlighted && "font-semibold")}
                      >
                        <TableCell className={LIST_CELL}>
                          <FormField
                            control={form.control}
                            name={`rows.${index}.selection`}
                            render={({ field }) => (
                              <FormItem>
                                <FieldControl>
                                  <Input
                                    {...field}
                                    aria-label={`Select ${name}`}
                                    maxLength={COTRTLI_LENGTHS.selection}
                                    size={COTRTLI_LENGTHS.selection}
                                    autoComplete="off"
                                    readOnly={!selectorOpen || busy}
                                    tabIndex={selectorOpen ? undefined : -1}
                                    className={cn(
                                      FIELD_INPUT,
                                      "relative left-2/13 w-9 text-center",
                                      "aria-invalid:text-destructive",
                                    )}
                                  />
                                </FieldControl>
                                <FormMessage className="sr-only" />
                              </FormItem>
                            )}
                          />
                        </TableCell>
                        <TableCell className={LIST_CELL}>{row?.type_code ?? ""}</TableCell>
                        {/* `pr-2` leaves the column gap after column 74 free, so the description
                            is the 50 columns of the Description Filter above it. */}
                        <TableCell className={cn(LIST_CELL, "pr-2")}>
                          <FormField
                            control={form.control}
                            name={`rows.${index}.description`}
                            render={({ field }) => (
                              <FormItem>
                                <FieldControl>
                                  <Input
                                    {...field}
                                    aria-label={`Description ${name}`}
                                    maxLength={COTRTLI_LENGTHS.description}
                                    size={COTRTLI_LENGTHS.description}
                                    autoComplete="off"
                                    readOnly={!descriptionOpen || busy}
                                    tabIndex={descriptionOpen ? undefined : -1}
                                    className={cn(FIELD_INPUT, "aria-invalid:text-destructive")}
                                  />
                                </FieldControl>
                                <FormMessage className="sr-only" />
                              </FormItem>
                            )}
                          />
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </GridTable>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
