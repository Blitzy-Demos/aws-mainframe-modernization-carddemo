import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useForm, type FieldErrors, type FieldPath } from "react-hook-form";
import { useNavigate } from "react-router";
import { cn } from "cn";
import { useCocrdlicList } from "@/api/hooks";
import type { components, paths } from "@/api/schema";
import { FieldControl } from "@/components/screen/FieldControl";
import {
  FieldGrid,
  GridCell,
  GridTable,
  LIST_CELL,
  LIST_ROW,
  fieldBox,
} from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
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
import { KEY_ROWS, keyRowItems, type PfKeyHandlers, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COCRDLI_LENGTHS, cocrdliSchema, type CocrdliValues } from "@/schemas/COCRDLI";

/** The map's screen title, `POS=(4,31)` (COCRDLI-011). */
const TITLE = "List Credit Cards";

/** COCRDLIC's selection-edit messages, 88-levels of `WS-ERROR-MSG` (`COCRDLIC.cbl:123-126`). */
const MORE_THAN_ONE_ACTION = "PLEASE SELECT ONLY ONE RECORD TO VIEW OR UPDATE";
const INVALID_ACTION_CODE = "INVALID ACTION CODE";

/** Row positions 0 to 6 of map lines 11 to 17 (`WS-MAX-SCREEN-LINES`, COCRDLIC-017). */
const POSITIONS: readonly number[] = Array.from(
  { length: COCRDLI_LENGTHS.rows },
  (_, position) => position,
);
const NO_SELECTIONS: readonly string[] = POSITIONS.map(() => "");
const NO_MARKS: readonly boolean[] = POSITIONS.map(() => false);

/**
 * ENTER, which COCRDLIC serves although the map labels no ENTER key: a visible button labelled
 * with the bare key name, first in the key row (COCRDLI-082, X-SCREENS-10), so that every action
 * a key performs is also a button to click (AAP 0.4.4). Without `onPress` it submits the form.
 */
const ENTER_KEY: PfKeyBarEntry = { key: "ENTER", label: "ENTER", dark: false };

/** The map's row-24 labels `F3=Exit`, `F7=Backward`, `F8=Forward` (COCRDLI-072). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COCRDLI);

/** The keys that read the list: ENTER (with every key processed as ENTER), F7 and F8. */
type ListKey = "ENTER" | "F7" | "F8";

/** The `direction` each key sends: none for ENTER (COCRDLIC-018, COCRDLIC-023, COCRDLIC-027). */
const DIRECTIONS: Readonly<Record<ListKey, components["schemas"]["PageDirection"] | undefined>> = {
  ENTER: undefined,
  F7: "backward",
  F8: "forward",
};

/** Map colours of the labels (TURQUOISE) and captions (NEUTRAL): the foreground token. */
const LABEL = "whitespace-pre text-foreground";

/**
 * The filter labels keep the foreground when their field is in error: COCRDLIC turns only the
 * field red (`DFHRED` to `ACCTSIDC`, `CARDSIDC`; COCRDLIC-011), not its label.
 */
const FILTER_LABEL = `${LABEL} data-[error=true]:text-foreground`;

/** A field in error shows its text red as well as its border (`DFHRED`, COCRDLIC-011, -035). */
const MARKED = "aria-invalid:text-destructive";

/**
 * A one-character field: the cell is narrow, so the input keeps little padding; as tall as a map
 * field's box (`h-7`, `py-0`), so it fits its `LIST_ROW` line.
 */
const ONE_CHARACTER = `h-7 w-9 px-1 py-0 text-center ${MARKED}`;

/**
 * The list's columns as fractions of its 63 map columns. Each column starts at its data field's
 * `POS`, except the rightmost, which starts at its dash rule's column 65 so that no text of the
 * table's last column is shifted left from beyond the table's right edge: a left shift moves only
 * the text's paint, and its overhang would scroll the table. The selector runs from column 10
 * (12 columns), the account number from 22 (21), the card number from 43 to 64 (22) and the
 * status from 65 to the list's end at 72 (8).
 */
const COLUMN_WIDTHS = ["w-12/63", "w-21/63", "w-22/63", "w-8/63"] as const;

/** A list caption, dash or value, shown with its spaces. */
const LIST_TEXT = `${LIST_CELL} whitespace-pre`;

/** What COCRDLIC's `2250-EDIT-ARRAY` finds in the seven selection codes as received. */
interface SelectionEdit {
  /** The error line the edit sets; `undefined` when every code passes. */
  readonly message: string | undefined;
  /** Each row's error mark, `WS-ROW-CRDSELECT-ERROR`. */
  readonly marks: readonly boolean[];
  /** The 0-based row of the last `S` or `U`, `I-SELECTED`; `undefined` when none holds one. */
  readonly selected: number | undefined;
}

/** The inputs one list request carried, which the screen shows again with its answer. */
interface SentInputs {
  readonly acctId: string;
  readonly cardNum: string;
  readonly selections: readonly string[];
  /** The failed selection edit whose re-read this request is (COCRDLIC-014). */
  readonly failedEdit?: SelectionEdit;
}

/** The inputs of the fresh start: no filters and no selections (COCRDLIC-002). */
const FRESH_START: SentInputs = { acctId: "", cardNum: "", selections: NO_SELECTIONS };

/** `SELECT-OK`: the codes `S` (view) and `U` (update), compared byte for byte. */
function isAction(code: string): boolean {
  return code === "S" || code === "U";
}

/** `SELECT-BLANK`: nothing typed, or a space. */
function isBlank(code: string): boolean {
  return code === "" || code === " ";
}

/**
 * COCRDLIC's selection edit (COCRDLIC-012, COCRDLIC-013): more than one `S` or `U` sets
 * `MORE_THAN_ONE_ACTION` and marks every such row; any other non-blank code, lower-case `s` and
 * `u` included, marks its row and sets `INVALID_ACTION_CODE` unless a message is already set.
 */
function editSelections(codes: readonly string[]): SelectionEdit {
  const moreThanOne = codes.filter(isAction).length > 1;
  let message = moreThanOne ? MORE_THAN_ONE_ACTION : undefined;
  let selected: number | undefined;
  const marks: boolean[] = [];
  for (const [row, code] of codes.entries()) {
    if (isAction(code)) {
      selected = row;
      marks.push(moreThanOne);
    } else if (isBlank(code)) {
      marks.push(false);
    } else {
      message ??= INVALID_ACTION_CODE;
      marks.push(true);
    }
  }
  return { message, marks, selected };
}

/**
 * The card a row position shows. After a backward read that reached the start of the card file,
 * the cards fill the rows from the last one upward, below `blank_rows_above` empty rows
 * (COCRDLIC-029).
 */
function cardAt(
  page: components["schemas"]["CardListResponse"] | undefined,
  position: number,
): components["schemas"]["CardListRow"] | undefined {
  if (page === undefined) {
    return undefined;
  }
  const index = position - page.blank_rows_above;
  return index < 0 ? undefined : page.rows.at(index);
}

/**
 * The paging state of the displayed page, echoed unchanged with every request (X-COCRDLIC-2).
 * With no page displayed nothing is echoed, which the server reads as a fresh start.
 */
function pagingEcho(
  page: components["schemas"]["CardListResponse"] | undefined,
): NonNullable<paths["/api/v1/cards"]["get"]["parameters"]["query"]> {
  if (page === undefined) {
    return {};
  }
  return {
    page_no: page.page_no,
    first_key: page.first_key ?? undefined,
    last_key: page.last_key ?? undefined,
    has_next_page: page.has_next_page,
    last_page_shown: page.last_page_shown,
  };
}

/** A filter as sent: a blank one, which COBOL reads as no filter, is left out. */
function filterParam(value: string): string | undefined {
  return /^ *$/.test(value) ? undefined : value;
}

/**
 * A filter as the screen shows it after a read: as sent, but empty when it was blank or all
 * zeros, which the program shows from the zeroed COMMAREA field (COCRDLIC-036).
 */
function shownFilter(sent: string): string {
  return /^(?: *|0+)$/.test(sent) ? "" : sent;
}

/** The screen field a `ScreenError.field` names, for the two filters the endpoint edits. */
function filterField(field: string | null | undefined): "acctId" | "cardNum" | undefined {
  if (field === "acct_id") {
    return "acctId";
  }
  return field === "card_num" ? "cardNum" : undefined;
}

/** The error row for `text`; an absent or empty message leaves the row empty. */
function errorLine(text: string | null | undefined): ShellMessage | undefined {
  return text === null || text === undefined || text === "" ? undefined : { text, kind: "error" };
}

/**
 * Map `COCRDLI` (`CCRDLIA`), the credit card list of transaction CCLI, program COCRDLIC,
 * endpoint 7, route `/cards` (`modern/audit/screens.md` COCRDLI-001 to COCRDLI-082;
 * `modern/audit/online/COCRDLIC.md`).
 *
 * - Entry reads page 1 with no filters (COCRDLIC-002, COCRDLIC-017).
 * - ENTER, F7 and F8 first run the filter edits of `cocrdliSchema`; a filter in error sends
 *   nothing and keeps the page, its selection codes protected (COCRDLIC-011). Then the selection
 *   edit runs (COCRDLIC-012, COCRDLIC-013); when it fails, whatever the key, the list is read
 *   again from its start with the codes and their marks kept (COCRDLIC-014).
 * - ENTER with one `S` or `U` opens the card view or the card update with that row's `acct_id`
 *   and `card_ref`, and sends nothing (COCRDLIC-015); otherwise ENTER re-reads the page and F7
 *   and F8 page, each echoing the paging state (COCRDLIC-018, COCRDLIC-023 to COCRDLIC-027). A
 *   code stays at its row position across a page move (COCRDLIC-016).
 * - F3 opens the main menu and sends nothing (COCRDLIC-004). Every other F-key is ENTER, and
 *   Shift+F1 to F12 (PF13 to PF24) act as F1 to F12 (COCRDLIC-003).
 * - While a request is in flight the keys that read are ignored, as the 3270 keyboard locks
 *   until the program answers; F3 still exits. The lock is taken when ENTER, F7 or F8 is
 *   accepted, before the filter edits, and opens when the edits fail or when the answer arrives;
 *   a selection that opens another screen keeps it until that screen replaces this one. While the
 *   lock is held, ENTER, `F7=Backward` and `F8=Forward` are disabled, the key row and the form
 *   are busy and every input is read-only.
 *
 * Card numbers are shown exactly as the server redacts them, and only `card_ref` travels to the
 * next screen (Cardholder Data Redaction, E32). The page size is never sent, so the server's
 * default of seven rows, the map's, decides every page (Bounded Pagination).
 */
export function COCRDLI(): ReactNode {
  const navigate = useNavigate();
  const cards = useCocrdlicList();
  const formId = useId();
  const pageNoId = useId();
  const form = useForm<CocrdliValues>({
    resolver: zodResolver(cocrdliSchema),
    defaultValues: { acctId: "", cardNum: "", selections: [...NO_SELECTIONS] },
  });

  const [page, setPage] = useState<components["schemas"]["CardListResponse"]>();
  const [message, setMessage] = useState<ShellMessage>();
  const [info, setInfo] = useState("");
  const [marks, setMarks] = useState<readonly boolean[]>(NO_MARKS);
  const [selectorsProtected, setSelectorsProtected] = useState(false);
  // A new object per request, so the same field takes the cursor again after every answer.
  const [cursor, setCursor] = useState<{ readonly name: FieldPath<CocrdliValues> }>();
  // `inFlight` is the lock the key handlers read at once; `pending` renders it while a list
  // request is out.
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);

  // Every arrival is the fresh start with no key. COCRDLIC would evaluate the key that left the
  // caller (F7 after COCRDUPC's post-save exit shows `NO PREVIOUS PAGES TO DISPLAY`,
  // COCRDLIC-002), but no route state carries that key here. The lock keeps React's development
  // double effect from reading twice.
  useEffect(() => {
    if (lockKeys()) {
      read({}, FRESH_START);
    }
  }, []);

  useEffect(() => {
    if (cursor !== undefined) {
      form.setFocus(cursor.name);
    }
  }, [cursor, form]);

  /** Takes the lock of the reading keys; false when a key already holds it. */
  function lockKeys(): boolean {
    if (inFlight.current) {
      return false;
    }
    inFlight.current = true;
    setPending(true);
    return true;
  }

  /**
   * Opens the lock: the answer has arrived, or the key that took it sends nothing. A selection's
   * transfer keeps it until the next screen replaces this one.
   */
  function unlockKeys(): void {
    inFlight.current = false;
    setPending(false);
  }

  /**
   * Sends one list request and shows its answer. The lock opens once the request settles,
   * whatever its outcome, after the answer is shown.
   */
  function read(
    query: NonNullable<paths["/api/v1/cards"]["get"]["parameters"]["query"]>,
    sent: SentInputs,
  ): void {
    void cards
      .mutateAsync(
        { query },
        {
          onSuccess: (response) => {
            showPage(response, sent);
          },
          onError: (error) => {
            showFailure(error, sent);
          },
        },
      )
      .then(unlockKeys, unlockKeys);
  }

  /**
   * Shows a page: its rows, page number and both message lines, the filters as COCRDLIC-036
   * shows them, and each selection code at its row position where that row holds a card. After
   * the re-read of a failed selection edit, the marks stay and the edit's message holds the
   * error line unless the server sends one that replaces it (COCRDLIC-014, COCRDLIC-034).
   */
  function showPage(response: components["schemas"]["CardListResponse"], sent: SentInputs): void {
    const holdsCard = POSITIONS.map((position) => cardAt(response, position) !== undefined);
    const nextMarks = POSITIONS.map(
      (position) => holdsCard[position] && (sent.failedEdit?.marks[position] ?? false),
    );
    setPage(response);
    setMessage(errorLine(response.message ?? sent.failedEdit?.message));
    setInfo(response.info_message ?? "");
    setMarks(nextMarks);
    setSelectorsProtected(false);
    form.setValue("acctId", shownFilter(sent.acctId));
    form.setValue("cardNum", shownFilter(sent.cardNum));
    for (const position of POSITIONS) {
      form.setValue(
        `selections.${position}`,
        holdsCard[position] ? (sent.selections.at(position) ?? "") : "",
      );
    }
    // Rows 2 to 7 take the cursor when marked; row 1 never does (COCRDLIC-035).
    const marked = POSITIONS.find((position) => position > 0 && nextMarks[position]);
    setCursor({ name: marked === undefined ? "acctId" : `selections.${marked}` });
  }

  /**
   * Shows a failed request. A filter the server rejects is shown as a filter edit failure
   * (COCRDLIC-011); any other failure keeps the page and shows the message.
   */
  function showFailure(error: components["schemas"]["ScreenError"], sent: SentInputs): void {
    setMessage(errorLine(error.message));
    const field = filterField(error.field);
    if (field === undefined) {
      setMarks(
        POSITIONS.map(
          (position) =>
            cardAt(page, position) !== undefined && (sent.failedEdit?.marks[position] ?? false),
        ),
      );
      return;
    }
    form.setError(field, { type: "server", message: error.message });
    protectSelectors(field);
  }

  /**
   * A filter in error: no page move, every selector protected, no information line and the
   * cursor on the account filter when it is in error, otherwise on the card filter
   * (COCRDLIC-011).
   */
  function protectSelectors(cursorField: "acctId" | "cardNum"): void {
    setInfo("");
    setMarks(NO_MARKS);
    setSelectorsProtected(true);
    setCursor({ name: cursorField });
  }

  /** The filter edits failed: their message holds the error line and nothing is sent. */
  function showFilterErrors(errors: FieldErrors<CocrdliValues>): void {
    setMessage(errorLine(errors.acctId?.message ?? errors.cardNum?.message));
    protectSelectors(errors.acctId === undefined ? "cardNum" : "acctId");
  }

  /** The filters passed: the selection edit decides between a transfer and a read. */
  function proceed(values: CocrdliValues, key: ListKey): void {
    const edit = editSelections(values.selections);
    if (edit.message === undefined && key === "ENTER" && edit.selected !== undefined) {
      const card = cardAt(page, edit.selected);
      if (card !== undefined) {
        const target = values.selections.at(edit.selected) === "S" ? PATHS.COCRDSL : PATHS.COCRDUP;
        // The lock `press` took stays held until the target replaces this screen, so no key
        // reads a page or changes the selection while its module loads.
        void navigate(target, {
          state: { from: PATHS.COCRDLI, acct_id: card.acct_id, card_ref: card.card_ref },
        });
        return;
      }
    }
    const failedEdit = edit.message === undefined ? undefined : edit;
    read(
      {
        acct_id: filterParam(values.acctId),
        card_num: filterParam(values.cardNum),
        direction: DIRECTIONS[key],
        reread: failedEdit === undefined ? undefined : true,
        ...pagingEcho(page),
      },
      {
        acctId: values.acctId,
        cardNum: values.cardNum,
        selections: values.selections,
        failedEdit,
      },
    );
  }

  /**
   * ENTER, F7 or F8: the edits, then a transfer or a read. The lock is taken here, before the
   * asynchronous edits, so a key that arrives during them, while the request is out or while the
   * transfer is under way is refused. Edits that fail, or fail unexpectedly, open it again, as
   * nothing was sent.
   */
  function press(key: ListKey): void {
    if (!lockKeys()) {
      return;
    }
    void form
      .handleSubmit(
        (values) => {
          proceed(values, key);
        },
        (errors) => {
          unlockKeys();
          showFilterErrors(errors);
        },
      )()
      .catch((error: unknown) => {
        unlockKeys();
        throw error;
      });
  }

  const actions: PfKeyHandlers = {
    F3: () => {
      void navigate(PATHS.COMEN01);
    },
    F7: () => {
      press("F7");
    },
    F8: () => {
      press("F8");
    },
  };
  // While a read is out only F3 stays enabled.
  const keys: PfKeyBarEntry[] = [
    { ...ENTER_KEY, disabled: pending },
    ...KEY_ITEMS.map((item) => ({
      ...item,
      onPress: actions[item.key],
      disabled: pending && item.key !== "F3",
    })),
  ];
  // Every F-key the row does not serve is processed as ENTER (COCRDLIC-003).
  const keyOptions: PfKeyOptions = {
    foldShifted: KEY_ROWS.COCRDLI.foldShifted,
    onInvalidKey: () => {
      press("ENTER");
    },
  };
  // The error line's text, which also describes each selector it marks.
  const errorText = message?.text ?? "";

  return (
    <ScreenShell
      transaction={SCREENS.COCRDLI.transaction}
      program={SCREENS.COCRDLI.program}
      title={TITLE}
      titleRow={
        <>
          {/*
           * COCRDLI-012, COCRDLI-013: `Page ` and the page number of the last answer, on the
           * title's line 4 of the map. `PAGENO` is `ASKIP`, which the 3270 cursor skips, so Tab
           * passes over it.
           */}
          <GridCell row={1} col={70} span={5}>
            <Label htmlFor={pageNoId} className={LABEL}>
              {"Page "}
            </Label>
          </GridCell>
          <GridCell row={1} col={76} span={3}>
            <Input
              id={pageNoId}
              readOnly
              tabIndex={-1}
              value={page === undefined ? "" : String(page.page_no)}
              className={fieldBox(3)}
            />
          </GridCell>
        </>
      }
      formId={formId}
      message={message}
      info={info}
      keys={keys}
      keyOptions={keyOptions}
      busy={pending}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={pending}
          onSubmit={(event) => {
            event.preventDefault();
            press("ENTER");
          }}
        >
          <FieldGrid>
            {/*
             * COCRDLI-014, COCRDLI-015: the account filter, the initial cursor (`IC`). The error
             * line is the map's one visible message, so a filter's own message, the schema's or
             * the server's, is for assistive technology only, read with the input it marks. It is
             * the item's direct child, not a cell's: being `sr-only`, it takes no grid cell, and
             * as a grid item it is placed at the grid's origin, inside the screen. The filters
             * have no hint, so their controls name no description.
             */}
            <FormField
              control={form.control}
              name="acctId"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={6} col={22} span={19}>
                    <FormLabel className={FILTER_LABEL}>{"Account Number    :"}</FormLabel>
                  </GridCell>
                  <GridCell row={6} col={44} span={COCRDLI_LENGTHS.acctId}>
                    <FieldControl>
                      <Input
                        {...field}
                        maxLength={COCRDLI_LENGTHS.acctId}
                        readOnly={pending}
                        autoFocus
                        autoComplete="off"
                        className={cn(fieldBox(COCRDLI_LENGTHS.acctId), MARKED)}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            {/* COCRDLI-017, COCRDLI-018: the card filter, request input only; wired as above. */}
            <FormField
              control={form.control}
              name="cardNum"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={7} col={22} span={19}>
                    <FormLabel className={FILTER_LABEL}>{"Credit Card Number:"}</FormLabel>
                  </GridCell>
                  <GridCell row={7} col={44} span={COCRDLI_LENGTHS.cardNum}>
                    <FieldControl>
                      <Input
                        {...field}
                        maxLength={COCRDLI_LENGTHS.cardNum}
                        readOnly={pending}
                        autoComplete="off"
                        className={cn(fieldBox(COCRDLI_LENGTHS.cardNum), MARKED)}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            {/*
             * Map lines 9 to 17, columns 10 to 72: the captions, the dashes and the seven rows
             * (COCRDLI-020 to COCRDLI-068). Each column starts at its data field's `POS`
             * (selector column 10, account 22, card 43), except the rightmost, which starts at
             * its dash rule's column 65 so that no text of the table's last column is shifted
             * left from beyond the table's right edge: a left shift moves only the text's paint,
             * and its overhang would scroll the table. A caption, dash, selector or value whose
             * map column differs is moved by a fraction of its own column's width.
             */}
            <GridTable row={9} col={10} span={63} rowSpan={9}>
              <Table aria-label="Credit cards" className="table-fixed">
                <colgroup>
                  {COLUMN_WIDTHS.map((width) => (
                    <col key={width} className={width} />
                  ))}
                </colgroup>
                <TableHeader>
                  <TableRow className={LIST_ROW}>
                    <TableHead scope="col" className={LIST_TEXT}>
                      {"Select    "}
                    </TableHead>
                    <TableHead scope="col" className={LIST_TEXT}>
                      <span className="relative -left-1/21">{"Account Number"}</span>
                    </TableHead>
                    <TableHead scope="col" className={LIST_TEXT}>
                      <span className="relative left-2/22">{" Card Number "}</span>
                    </TableHead>
                    <TableHead scope="col" className={LIST_TEXT}>
                      <span className="relative left-1/8">{"Active "}</span>
                    </TableHead>
                  </TableRow>
                  {/* The dashes under the captions are decoration, not column titles. */}
                  <TableRow aria-hidden="true" className={LIST_ROW}>
                    <TableCell className={LIST_TEXT}>{"------"}</TableCell>
                    <TableCell className={LIST_TEXT}>
                      <span className="relative -left-2/21">{"---------------"}</span>
                    </TableCell>
                    <TableCell className={LIST_TEXT}>{"---------------"}</TableCell>
                    <TableCell className={LIST_TEXT}>{"--------"}</TableCell>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {POSITIONS.map((position) => {
                    const card = cardAt(page, position);
                    // A code the selection edit marks is invalid, and its hidden field message, in
                    // the code's own item, repeats the error line. The marks are kept outside
                    // react-hook-form, so the control is told the message shows.
                    const marked = marks.at(position) ?? false;
                    return (
                      // A row without a card shows nothing and keeps its line and height
                      // (COCRDLIC-035).
                      <TableRow
                        key={position}
                        aria-hidden={card === undefined ? true : undefined}
                        className={LIST_ROW}
                      >
                        <TableCell className={LIST_CELL}>
                          {card !== undefined && (
                            <FormField
                              control={form.control}
                              name={`selections.${position}`}
                              render={({ field }) => (
                                <FormItem>
                                  <FieldControl messaged={marked && errorText !== ""}>
                                    <Input
                                      {...field}
                                      maxLength={COCRDLI_LENGTHS.selection}
                                      readOnly={selectorsProtected || pending}
                                      autoComplete="off"
                                      aria-label={`Select row ${position + 1}`}
                                      aria-invalid={marked}
                                      className={`relative left-2/12 ${ONE_CHARACTER}`}
                                    />
                                  </FieldControl>
                                  <FormMessage className="sr-only">
                                    {marked ? errorText : undefined}
                                  </FormMessage>
                                </FormItem>
                              )}
                            />
                          )}
                        </TableCell>
                        <TableCell className={LIST_TEXT}>{card?.acct_id}</TableCell>
                        <TableCell className={LIST_TEXT}>{card?.card_num}</TableCell>
                        <TableCell className={LIST_TEXT}>
                          <span className="relative left-2/8">{card?.status}</span>
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
