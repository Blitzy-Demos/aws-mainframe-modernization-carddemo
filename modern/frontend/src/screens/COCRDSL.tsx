import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { cn } from "cn";
import { errorStatus, useCocrdslcView } from "@/api/hooks";
import type { components, paths } from "@/api/schema";
import { FieldControl } from "@/components/screen/FieldControl";
import { FieldGrid, GridCell, fieldBox } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { KEY_ROWS, keyRowItems } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COCRDSL_LENGTHS, cocrdslSchema, type CocrdslValues } from "@/schemas/COCRDSL";

/** Map `CCRDSLA` title, `POS=(4,30)` (COCRDSL-011). */
const TITLE = "View Credit Card Detail";

/**
 * The info row before any request and after every `ScreenError`: COCRDSLC sets
 * `WS-PROMPT-FOR-INPUT` whenever no other information message is set, so no response carries it
 * (COCRDSLC-025, `app/cbl/COCRDSLC.cbl:131-132,489-496`).
 */
const PROMPT = "Please enter Account and Card Number";

/**
 * What an input shows after an ENTER that sent it blank: `1300-SETUP-SCREEN-ATTRS` moves `*` into
 * it and colours it red (COCRDSLC-027, `app/cbl/COCRDSLC.cbl:541-551`). COCRDSLC reads `*` alone
 * as not supplied (`app/cbl/COCRDSLC.cbl:614-627`).
 */
const BLANK_MARK = "*";

/** The `INITIAL` of each literal field of the map, spacing kept (`app/bms/COCRDSL.bms:79-132`). */
const LABELS = {
  acctId: "Account Number    :", // COCRDSL-012
  cardNum: "Card Number       :", // COCRDSL-015
  name: "Name on card      :", // COCRDSL-018
  status: "Card Active Y/N   : ", // COCRDSL-021
  expiry: "Expiry Date       : ", // COCRDSL-024
  separator: "/", // COCRDSL-026
} as const;

/** `LENGTH` of the output fields (COCRDSL-019, COCRDSL-022, COCRDSL-025, COCRDSL-027). */
const OUTPUT_LENGTHS = { name: 50, status: 1, month: 2, year: 4 } as const;

/** Row-24 keys of the map, `ENTER=Search Cards` and `F3=Exit` (COCRDSL-031). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COCRDSL);

/** Every route path, to accept a caller from route state only when it names a screen. */
const KNOWN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/*
 * The literals are TURQUOISE (`text-foreground`, AAP 0.4.3). `h-full` centres each label on its
 * line beside the inputs; `whitespace-pre` keeps the padding that aligns the colons.
 */
const LABEL_CLASS = "h-full whitespace-pre text-foreground";

/**
 * The class of a field box of `length` characters (its map `LENGTH`): the shared field box of
 * `fieldBox`, at the field's `POS` column, which fills its cell and is never narrower than its
 * characters, so the 1-, 2- and 4-column outputs show whole and the month box ends before the `/`
 * at column 28 at every column pitch. Its text is red once the response marks it (COCRDSLC-027).
 */
function inputClass(length: number, marked = false): string {
  return cn(fieldBox(length), marked && "text-destructive");
}

/** A member of the route state, which is whatever the navigating screen passed. */
function stateMember(state: unknown, name: string): unknown {
  if (typeof state !== "object" || state === null || !Object.hasOwn(state, name)) {
    return undefined;
  }
  return Reflect.get(state, name);
}

/**
 * The screen that transferred here, from route state `from`: COCRDLIC passes its own path, the
 * menu passes its path or nothing. Only a path of the route table counts.
 */
function callerPath(state: unknown): string | undefined {
  const from = stateMember(state, "from");
  return typeof from === "string" && KNOWN_PATHS.has(from) ? from : undefined;
}

/**
 * The card a COCRDLI `S` selection names: the row's `acct_id` and `card_ref` (COCRDSLC-002), taken
 * only when `caller` is the card list, as COCRDSLC reads on entry only `WHEN CDEMO-PGM-ENTER AND
 * CDEMO-FROM-PROGRAM EQUAL LIT-CCLISTPGM` (`app/cbl/COCRDSLC.cbl:339-341`). The main menu
 * forwards the route state it received, where an earlier screen may have left an account and a
 * card, so from the menu, from any other caller or from none the screen opens empty
 * (COCRDSLC-001). Route state is not typed, so anything else, a missing member included, is no
 * selection.
 */
function listSelection(
  state: unknown,
  caller: string | undefined,
): { acctId: string; cardRef: number } | undefined {
  if (caller !== PATHS.COCRDLI) {
    return undefined;
  }
  const acctId = stateMember(state, "acct_id");
  const cardRef = stateMember(state, "card_ref");
  if (
    typeof acctId !== "string" ||
    acctId.length > COCRDSL_LENGTHS.acctId ||
    typeof cardRef !== "number" ||
    !Number.isSafeInteger(cardRef)
  ) {
    return undefined;
  }
  return { acctId, cardRef };
}

/**
 * The query value of an input as typed, or nothing when it is blank or holds only the `*` marker,
 * both of which COCRDSLC reads as not supplied. Nothing else is changed: the endpoint edits the
 * text as the program edits the field.
 */
function queryText(input: string): string | undefined {
  const text = input.trimEnd();
  return text === "" || text === BLANK_MARK ? undefined : input;
}

/**
 * How COCRDSLC's edit reads a sent input of `width` characters (`online/COCRDSLC.md`, "Reading
 * the table"): blank when absent or all zeros; valid when exactly `width` digits; otherwise
 * invalid. Used only to show the response as `1300-SETUP-SCREEN-ATTRS` does; the endpoint makes
 * every decision and owns every message.
 */
function sentAs(text: string | undefined, width: number): "blank" | "valid" | "invalid" {
  if (text === undefined || text === "0".repeat(width)) {
    return "blank";
  }
  return new RegExp(`^[0-9]{${width}}$`).test(text) ? "valid" : "invalid";
}

/** The input text after an error response: `*` when sent blank, empty when sent invalid. */
function shownAfterError(sent: "blank" | "valid" | "invalid" | "ref"): string | undefined {
  if (sent === "blank") {
    return BLANK_MARK;
  }
  return sent === "invalid" ? "" : undefined;
}

/**
 * `EXPMON` and `EXPYEAR` from the typed expiry date: positions 6-7 and 1-4 of `YYYY-MM-DD`; a
 * `null` date, or text in any other form, leaves both empty (COCRDSLC-024). The date is only
 * sliced, never parsed into a `Date`.
 */
function expiryParts(date: string | null | undefined): { month: string; year: string } {
  if (typeof date !== "string" || !/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(date)) {
    return { month: "", year: "" };
  }
  return { month: date.slice(5, 7), year: date.slice(0, 4) };
}

/**
 * What the screen shows from the last response, kept until the next one arrives as the terminal
 * keeps its screen while a request is out. Screen state, not a payload type: the found card is
 * the generated `CardViewResponse` itself.
 */
type Display = {
  readonly view: components["schemas"]["CardViewResponse"] | null;
  readonly message: string | undefined;
  readonly info: string;
  readonly acctMarked: boolean;
  readonly cardMarked: boolean;
};

/**
 * CCDL / COCRDSLC, map `CCRDSLA`: one credit card's detail, read through endpoint 8
 * (`GET /api/v1/cards/view`).
 *
 * - From the main menu (option 04), or with no caller, the screen opens empty with the account
 *   input focused and sends nothing, whatever account or card the route state still carries
 *   (COCRDSLC-001).
 * - From a COCRDLI `S` selection, whose route state names the card list as `from` and carries
 *   the row's `acct_id` and `card_ref`, the screen reads that card at once, and both inputs stay
 *   read-only and out of the Tab order for the visit, as the program protects them while the card
 *   list is its caller (COCRDSLC-002).
 * - ENTER, and every other key except F3, searches (COCRDSLC-003, COCRDSL-038). The request
 *   carries the account and exactly one of `card_num` (the typed card number) or `card_ref` (the
 *   handle of the card shown, while the card input still shows its number, COCRDSLC-026).
 * - F3 returns to the screen that transferred here, else to the main menu (COCRDSLC-004).
 * - While a request is out every key but F3 is refused, as the terminal keyboard locks until the
 *   program answers: `ENTER=Search Cards` is disabled, the key row and the form are busy and both
 *   inputs are read-only.
 *
 * The card number is shown exactly as the endpoint returns it: all 16 digits only when the
 * server disclosed them to an administrator token, else twelve `*` and the last four digits
 * (COCRDSLC-022, COCRDSLC-023). The screen never decides disclosure and never unmasks. The
 * returned number, an administrator's full number included, lives only in memory: the card
 * input's form value, the `knownCard` ref that lets ENTER re-read the unchanged card by its
 * `card_ref` (COCRDSLC-026), and the shown response in `display.view` and TanStack's in-memory
 * mutation state. It is never logged, written to storage or route state, or sent back: while the
 * card input still shows it, ENTER sends `card_ref` instead.
 */
export function COCRDSL(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const routeState: unknown = location.state;
  // Route state is fixed for the visit, as the COMMAREA the caller passed is.
  const [entry] = useState(() => {
    const caller = callerPath(routeState);
    return { caller, selection: listSelection(routeState, caller) };
  });
  const fromList = entry.selection !== undefined;

  const formId = useId();
  const outputId = useId();
  const { mutateAsync } = useCocrdslcView();
  // `inFlight` is the lock the key handlers read at once; `pending` renders it while a request
  // is out.
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const form = useForm<CocrdslValues>({
    resolver: zodResolver(cocrdslSchema),
    defaultValues: { acctId: entry.selection?.acctId ?? "", cardNum: "" },
  });
  const [display, setDisplay] = useState<Display>({
    view: null,
    message: undefined,
    info: fromList ? "" : PROMPT,
    acctMarked: false,
    cardMarked: false,
  });

  /*
   * The card the card input names while it shows the text it was given: the list row's handle
   * before the first response (no number known yet), then each found card's handle and returned
   * number. Read only when ENTER builds a request.
   */
  const knownCard = useRef<{ readonly ref: number; readonly shown: string } | null>(
    entry.selection === undefined ? null : { ref: entry.selection.cardRef, shown: "" },
  );

  /** Opens the key lock: the answer has arrived, or the key that took it sends nothing. */
  const unlockKeys = useCallback(() => {
    inFlight.current = false;
    setPending(false);
  }, []);

  /**
   * Sends one endpoint 8 request and shows its outcome as `1000-SEND-MAP` would. The keys stay
   * locked until the request settles, whatever its outcome, after the outcome is shown.
   */
  const search = useCallback(
    (query: NonNullable<paths["/api/v1/cards/view"]["get"]["parameters"]["query"]>) => {
      inFlight.current = true;
      setPending(true);
      const acct = sentAs(query.acct_id, COCRDSL_LENGTHS.acctId);
      const card =
        query.card_ref === undefined ? sentAs(query.card_num, COCRDSL_LENGTHS.cardNum) : "ref";
      void mutateAsync(
        { query },
        {
          onSuccess: (view) => {
            knownCard.current = { ref: view.card_ref, shown: view.card_num };
            form.setValue("acctId", view.acct_id);
            form.setValue("cardNum", view.card_num);
            setDisplay({
              view,
              message: view.message ?? undefined,
              info: view.info_message?.trim() ? view.info_message : PROMPT,
              acctMarked: false,
              cardMarked: false,
            });
            // `1300-SETUP-SCREEN-ATTRS` puts the cursor in the account input (WHEN OTHER).
            form.setFocus("acctId");
          },
          onError: (error) => {
            const field = error.field ?? undefined;
            // A failure without a field (no network, a proxy error) has no COBOL counterpart:
            // the message shows and the inputs stay as sent.
            if (field === undefined) {
              setDisplay({
                view: null,
                message: error.message,
                info: PROMPT,
                acctMarked: false,
                cardMarked: false,
              });
              return;
            }
            const acctText = shownAfterError(acct);
            const cardText = shownAfterError(card);
            if (acctText !== undefined) {
              form.setValue("acctId", acctText);
            }
            if (cardText !== undefined) {
              form.setValue("cardNum", cardText);
            }
            // After edits that passed, the card read failed and the error names `acct_id` either
            // way: COCRDSLC marks both inputs on not-found (404) and only the account on a file
            // error (500), so the status the hook recorded, not the body, tells the two apart.
            const notFound = errorStatus(error) === 404;
            setDisplay({
              view: null,
              message: error.message,
              info: PROMPT,
              acctMarked: acct !== "valid" || field === "acct_id",
              cardMarked: card === "blank" || card === "invalid" || notFound,
            });
            form.setFocus(field === "card_num" ? "cardNum" : "acctId");
          },
        },
      ).then(unlockKeys, unlockKeys);
    },
    [mutateAsync, form, unlockKeys],
  );

  /**
   * ENTER: the account and the card as the inputs hold them. The card goes as `card_ref` while
   * its input still shows exactly the text the screen put there, which may be the redacted
   * number the card edit would reject (COCRDSLC-026). Any other text goes as `card_num`, as
   * typed, an edited redacted number included: the card edit then answers it as the program
   * answers any changed input, rather than the screen silently re-reading the earlier card.
   * Reached only through `submit`, which holds the key lock: one request at a time, as the
   * terminal keyboard locks until the reply.
   */
  const onValid = (values: CocrdslValues): void => {
    const acctId = queryText(values.acctId);
    const known = knownCard.current;
    if (known !== null && values.cardNum === known.shown) {
      search({ acct_id: acctId, card_ref: known.ref });
    } else {
      search({ acct_id: acctId, card_num: queryText(values.cardNum) });
    }
  };

  /**
   * A map `LENGTH` the zod schema rejects: its message in the error row, nothing sent, so the
   * key lock opens again.
   */
  const onInvalid = (errors: FieldErrors<CocrdslValues>): void => {
    unlockKeys();
    const first = errors.acctId ?? errors.cardNum;
    setDisplay((shown) => ({ ...shown, message: first?.message }));
  };

  /**
   * ENTER, by the button, the key, an Enter in a field or a key processed as ENTER. The lock is
   * taken here, before the schema's asynchronous check, so a key that arrives during the check
   * or while the request is out sends nothing and never reaches the browser's own form
   * submission. A check that fails unexpectedly opens it again, as nothing was sent.
   */
  const submit = (event?: FormEvent<HTMLFormElement>): void => {
    event?.preventDefault();
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setPending(true);
    void form
      .handleSubmit(onValid, onInvalid)()
      .catch((error: unknown) => {
        unlockKeys();
        throw error;
      });
  };

  const exit = useCallback(() => {
    void navigate(entry.caller ?? PATHS.COMEN01);
  }, [navigate, entry.caller]);

  // While a request is out only F3 stays enabled.
  const keys = useMemo<PfKeyBarEntry[]>(
    () =>
      KEY_ITEMS.map((item) =>
        item.key === "F3" ? { ...item, onPress: exit } : { ...item, disabled: pending },
      ),
    [exit, pending],
  );

  // COCRDSLC-002: entered from the card list, the program reads the card before its first send.
  // The ref keeps React's development double effect from reading it twice.
  const entryRead = useRef(false);
  useEffect(() => {
    if (entry.selection === undefined || entryRead.current) {
      return;
    }
    entryRead.current = true;
    search({ acct_id: queryText(entry.selection.acctId), card_ref: entry.selection.cardRef });
  }, [entry, search]);

  const view = display.view;
  const expiry = expiryParts(view?.expiration_date);
  // The error row's text, which also describes each input it marks.
  const errorText = display.message ?? "";

  return (
    <ScreenShell
      transaction={SCREENS.COCRDSL.transaction}
      program={SCREENS.COCRDSL.program}
      title={TITLE}
      formId={formId}
      message={{ text: display.message, kind: "error" }}
      info={display.info}
      keys={keys}
      keyOptions={{
        foldShifted: KEY_ROWS.COCRDSL.foldShifted,
        // COCRDSLC-003: any key other than ENTER and F3 is processed as ENTER.
        onInvalidKey: () => {
          submit();
        },
      }}
      busy={pending}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={pending} onSubmit={submit}>
          {/*
           * Rows keep their map numbers. Lines that hold no field (the header and title lines
           * 1-6, which ScreenShell draws above, and the blank lines 9, 10, 12 and 14) take no
           * height of their own, only the row gap, so the body does not repeat the frame's rows
           * as empty space; `gap-y-2` keeps the blank lines visible between the field groups.
           */}
          <FieldGrid className="grid-rows-none gap-y-2">
            {/*
             * Both inputs are protected while the card list is the caller (`DFHBMPRF`,
             * COCRDSLC-002): read-only for the visit and, as the 3270 cursor skips a protected
             * field, out of the Tab order.
             *
             * The error row is the map's one visible message, so an input's own message is for
             * assistive technology only, read with the input it marks: the schema's message, or
             * the row's text for a mark the screen keeps outside react-hook-form. It is the
             * item's direct child, not a cell's: being `sr-only`, it takes no grid cell, and as a
             * grid item it is placed at the grid's origin, inside the screen. The inputs have no
             * hint, so their controls name no description.
             */}
            <FormField
              control={form.control}
              name="acctId"
              render={({ field, fieldState }) => (
                <FormItem className="contents">
                  <GridCell row={7} col={23} span={19}>
                    <FormLabel className={LABEL_CLASS}>{LABELS.acctId}</FormLabel>
                  </GridCell>
                  <GridCell row={7} col={45} span={COCRDSL_LENGTHS.acctId}>
                    <FieldControl messaged={display.acctMarked && errorText !== ""}>
                      <Input
                        {...field}
                        data-field="ACCTSID"
                        maxLength={COCRDSL_LENGTHS.acctId}
                        size={COCRDSL_LENGTHS.acctId}
                        autoFocus
                        readOnly={fromList || pending}
                        tabIndex={fromList ? -1 : undefined}
                        autoComplete="off"
                        spellCheck={false}
                        aria-invalid={display.acctMarked || fieldState.invalid}
                        className={inputClass(COCRDSL_LENGTHS.acctId, display.acctMarked)}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only">
                    {display.acctMarked ? errorText : undefined}
                  </FormMessage>
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="cardNum"
              render={({ field, fieldState }) => (
                <FormItem className="contents">
                  <GridCell row={8} col={23} span={19}>
                    <FormLabel className={LABEL_CLASS}>{LABELS.cardNum}</FormLabel>
                  </GridCell>
                  <GridCell row={8} col={45} span={COCRDSL_LENGTHS.cardNum}>
                    <FieldControl messaged={display.cardMarked && errorText !== ""}>
                      <Input
                        {...field}
                        data-field="CARDSID"
                        maxLength={COCRDSL_LENGTHS.cardNum}
                        size={COCRDSL_LENGTHS.cardNum}
                        readOnly={fromList || pending}
                        tabIndex={fromList ? -1 : undefined}
                        autoComplete="off"
                        spellCheck={false}
                        aria-invalid={display.cardMarked || fieldState.invalid}
                        className={inputClass(COCRDSL_LENGTHS.cardNum, display.cardMarked)}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only">
                    {display.cardMarked ? errorText : undefined}
                  </FormMessage>
                </FormItem>
              )}
            />

            {/*
             * The four outputs are `ASKIP` (`CRDNAME` by the BMS default), which the 3270 cursor
             * skips, so each is read-only and out of the Tab order.
             */}
            <GridCell row={11} col={4} span={20}>
              <Label htmlFor={`${outputId}-name`} className={LABEL_CLASS}>
                {LABELS.name}
              </Label>
            </GridCell>
            <GridCell row={11} col={25} span={OUTPUT_LENGTHS.name}>
              <Input
                id={`${outputId}-name`}
                data-field="CRDNAME"
                readOnly
                tabIndex={-1}
                value={view?.embossed_name ?? ""}
                size={OUTPUT_LENGTHS.name}
                className={inputClass(OUTPUT_LENGTHS.name)}
              />
            </GridCell>

            <GridCell row={13} col={4} span={20}>
              <Label htmlFor={`${outputId}-status`} className={LABEL_CLASS}>
                {LABELS.status}
              </Label>
            </GridCell>
            <GridCell row={13} col={25} span={OUTPUT_LENGTHS.status}>
              <Input
                id={`${outputId}-status`}
                data-field="CRDSTCD"
                readOnly
                tabIndex={-1}
                value={view?.status ?? ""}
                size={OUTPUT_LENGTHS.status}
                className={inputClass(OUTPUT_LENGTHS.status)}
              />
            </GridCell>

            <GridCell row={15} col={4} span={20}>
              <Label htmlFor={`${outputId}-month`} className={LABEL_CLASS}>
                {LABELS.expiry}
              </Label>
            </GridCell>
            <GridCell row={15} col={25} span={OUTPUT_LENGTHS.month}>
              <Input
                id={`${outputId}-month`}
                data-field="EXPMON"
                readOnly
                tabIndex={-1}
                value={expiry.month}
                size={OUTPUT_LENGTHS.month}
                className={inputClass(OUTPUT_LENGTHS.month)}
              />
            </GridCell>
            <GridCell row={15} col={28} span={1}>
              <Label htmlFor={`${outputId}-year`} className={LABEL_CLASS}>
                {LABELS.separator}
              </Label>
            </GridCell>
            <GridCell row={15} col={30} span={OUTPUT_LENGTHS.year}>
              {/* The visible label of the year is the `/` literal; the name says what it holds. */}
              <Input
                id={`${outputId}-year`}
                data-field="EXPYEAR"
                aria-label="Expiry year"
                readOnly
                tabIndex={-1}
                value={expiry.year}
                size={OUTPUT_LENGTHS.year}
                className={inputClass(OUTPUT_LENGTHS.year)}
              />
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
