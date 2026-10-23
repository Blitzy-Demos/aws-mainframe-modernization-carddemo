import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { useForm } from "react-hook-form";
import { useNavigate } from "react-router";
import { useCoadm01cAdminMenu } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableRow } from "@/components/ui/table";
// The common invalid-key text of `CSMSG01Y` has one home for all 21 screens, beside the header
// values `ScreenShell` takes from the same module; it is imported, never restated here.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems } from "@/lib/pfkeys";
import { PATHS, PROGRAM_ROUTES, SCREENS } from "@/routes";
import { COADM01_LENGTHS, coadm01Schema, type Coadm01Values } from "@/schemas/COADM01";

/**
 * The admin menu, map `COADM1A` of `app/bms/COADM01.bms`: transaction CA00, program COADM01C,
 * route `/admin`, served by endpoint 3 (`GET /api/v1/menus/admin`).
 *
 * The screen holds no authority. The API's admin gate answers 401 or 403 before any menu is
 * built, and that `ScreenError` shows in the message row (COADM01C-001). A 401 also clears the
 * session, so the route guard in front of `/admin` then replaces the screen with sign-on; the
 * guard only hides the screen from a session that is absent or not of user type `A`.
 */

/** `OPTN001` to `OPTN012`: the map's option lines, `POS=(6,20)` to `(17,20)`, `LENGTH=40`. */
const OPTION_LINES = 12;

/**
 * `INITIAL=' '` of every option line (`app/bms/COADM01.bms:80-139`). BMS sends a field's INITIAL
 * where the program moves no text, so the lines past the last option show this one space, which
 * also gives them a text line's height (COADM01-018 to COADM01-023).
 */
const OPTION_INITIAL = " ";

/** Row 24, `ENTER=Continue  F3=Exit` (COADM01-028, COADM01-035, COADM01-036). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COADM01);

/** What ENTER does with a digits-only entry: go to an option's screen, or stay with a message. */
type EnterOutcome =
  | { readonly kind: "transfer"; readonly route: string }
  | { readonly kind: "stay"; readonly message: ShellMessage };

/**
 * The menu as the last key sent it again: that key's message and the number of the send. Every
 * key the screen answers makes a new send, a repeated message included, so the screen renders
 * again with the time of this send in the header (COADM01C-022), the message row announces its
 * text again and the cursor returns to the option input.
 */
type MenuSend = {
  /** The key's message; `undefined` leaves the row to the load failure, if any. */
  readonly message: ShellMessage | undefined;
  /** 0 on first entry, then one more for each send a key produces. */
  readonly number: number;
};

/** First entry: no key has been answered yet (COADM01C-003). */
const FIRST_ENTRY: MenuSend = { message: undefined, number: 0 };

/**
 * The option entry as COADM01C reads it (COADM01C-015): the map right-justifies and zero-fills the
 * two positions (`JUSTIFY=(RIGHT,ZERO)`, COADM01-025), so `1` and `01` both select option 1 and an
 * empty entry is `00`. An entry already two characters long is unchanged.
 */
function justified(entry: string): string {
  return entry.padStart(COADM01_LENGTHS.option, "0");
}

/**
 * The route of an option's program, or `undefined` when no screen serves it. Only the table's own
 * keys count, so a program name such as `constructor` never reads an inherited property.
 */
function programRoute(program: string): string | undefined {
  return Object.hasOwn(PROGRAM_ROUTES, program) ? PROGRAM_ROUTES[program] : undefined;
}

/**
 * `PROCESS-ENTER-KEY` for a justified option, decided from the menu the screen already holds, in
 * the program's order:
 *
 * 1. No option carries that number: `00`, or above the option count (COADM01C-016). The response
 *    lists the options numbered `01` to the count, so a missing number is exactly the program's
 *    "not numeric, above the count or zero" test. Shown as an error.
 * 2. The option is not installed (COADM01C-018), or no screen serves its program, the browser's
 *    counterpart of the `PGMIDERR` handler (COADM01C-019): the option's not-installed text, which
 *    the program colours green, so it shows as information.
 * 3. Otherwise the transfer to the option's screen (COADM01C-017).
 *
 * There is no user-type test: every option of this menu is admin-only, and only an `A` token
 * reaches it (COADM01C-012).
 */
function enterOutcome(menu: components["schemas"]["MenuResponse"], option: string): EnterOutcome {
  const chosen = menu.options.find((entry) => entry.number === option);
  if (chosen === undefined) {
    return { kind: "stay", message: { kind: "error", text: menu.invalid_option_message } };
  }
  const route = chosen.installed ? programRoute(chosen.program) : undefined;
  return route === undefined
    ? { kind: "stay", message: { kind: "info", text: chosen.unavailable_message ?? "" } }
    : { kind: "transfer", route };
}

/**
 * The CA00 admin menu screen (COADM01C-003 to COADM01C-022, COADM01-001 to COADM01-036).
 *
 * - First entry requests endpoint 3 once and shows its options, in table order, on lines 6 to 17;
 *   the lines without an option stay blank (COADM01C-003, COADM01C-005).
 * - ENTER and every other key are served from that response, with no further request. Each key
 *   replaces the message of the one before (COADM01C-004).
 * - ENTER after a failed load, the reload on a return to `/admin` included, requests endpoint 3
 *   again with the keyboard locked as for the first load. Its table decides the entry ENTER
 *   carried, as above, and replaces the failure's message; a new failure, the gate's included,
 *   shows its own message, and the next ENTER tries again. Once F3 or an unmount has left the
 *   screen, the reload's answer decides nothing.
 * - F3 goes to sign-on, which starts with no session (COADM01C-020).
 * - Any other F-key shows the invalid-key message (COADM01C-021).
 *
 * Every send a key produces puts the cursor back on the option input, the map's `IC` field, with
 * the input's text selected so that the next option overtypes it.
 *
 * While endpoint 3 loads or reloads, the keyboard is locked as the terminal's is until the program
 * answers: ENTER is disabled and decides nothing, so it never matches an entry against a cached
 * table the reload is replacing; the key row and the form are busy, the option input is read-only
 * and an unserved F-key shows nothing. F3 stays available.
 */
export function COADM01(): ReactNode {
  const formId = useId();
  const navigate = useNavigate();
  const menuQuery = useCoadm01cAdminMenu();
  const menu = menuQuery.data;
  // Whether endpoint 3 is loading or reloading. Read here, during render, so the screen renders
  // again when the fetch ends: TanStack Query re-renders only for the result fields a render reads.
  // ENTER's guard and its `disabled` flag both follow it.
  const pending = menuQuery.isFetching;
  // Set while the reload an ENTER started is out, from before the render that marks the query
  // fetching, so a second ENTER in between neither sends a request nor decides anything.
  const reloading = useRef(false);
  // Set once F3 or an unmount has left the screen, so a reload that answers later neither
  // transfers nor sends the menu again. Cleared on every mount, as StrictMode mounts twice.
  const left = useRef(false);
  useEffect(() => {
    left.current = false;
    return () => {
      left.current = true;
    };
  }, []);
  const [sent, setSent] = useState<MenuSend>(FIRST_ENTRY);
  const form = useForm<Coadm01Values>({
    resolver: zodResolver(coadm01Schema),
    defaultValues: { option: "" },
  });

  const keys = useMemo(
    () =>
      KEY_ITEMS.map((item) =>
        item.key === "F3"
          ? {
              ...item,
              onPress: () => {
                // COSGN00C is entered without a COMMAREA, so the sign-on screen clears the
                // stored session on arrival (COSGN00C-002); no route state is passed.
                left.current = true;
                void navigate(PATHS.COSGN00);
              },
            }
          : { ...item, disabled: pending },
      ),
    [navigate, pending],
  );

  // Each send puts the cursor back on the option input, the map's `IC` field (COADM01-025), with
  // its text selected, so the next option overtypes the value shown from the first position, as on
  // the terminal; at `maxLength` an unselected value would refuse the digit. The effect follows
  // the render that wrote that value into the input, so the selection outlasts it.
  const { setFocus } = form;
  useEffect(() => {
    if (sent.number > 0) {
      setFocus("option", { shouldSelect: true });
    }
  }, [sent.number, setFocus]);

  /**
   * The menu sent again with `message`, the cursor back on the option input (`IC`): a new send
   * even when the message repeats the last one.
   */
  function sendAgain(message: ShellMessage | undefined): void {
    setSent((previous) => ({ message, number: previous.number + 1 }));
  }

  /**
   * ENTER served from the option table: `serve` decides from the table the screen holds, or, after
   * a failed load, from the table endpoint 3 answers when it is requested again. A new failure
   * shows its own message, the gate's included. `refetch` settles with the query's result and
   * never rejects. Nothing is decided while a load is out.
   */
  function onTable(serve: (table: components["schemas"]["MenuResponse"]) => void): void {
    if (pending || reloading.current) {
      return;
    }
    if (menuQuery.isError) {
      reloading.current = true;
      void menuQuery.refetch().then((result) => {
        reloading.current = false;
        if (left.current) {
          return;
        }
        if (result.isSuccess) {
          serve(result.data);
        } else {
          sendAgain(undefined);
        }
      });
      return;
    }
    if (menu !== undefined) {
      serve(menu);
    }
  }

  /** ENTER with an entry the schema accepts: empty, or one or two digits. */
  function onEnter(values: Coadm01Values): void {
    onTable((table) => {
      const option = justified(values.option);
      const outcome = enterOutcome(table, option);
      if (outcome.kind === "transfer") {
        // The target's F3 or F12 returns here; identity travels only in the token.
        void navigate(outcome.route, { state: { from: PATHS.COADM01 } });
        return;
      }
      // `MOVE WS-OPTION TO OPTIONO`: the input shows the two-digit option it was read as.
      form.setValue("option", option);
      sendAgain(outcome.message);
    });
  }

  /**
   * ENTER with an entry the numeric field refuses. The map's option edit owns the message row, so
   * the program's invalid-option text is shown rather than the schema's own issue text.
   */
  function onRefusedEntry(): void {
    onTable((table) => {
      sendAgain({ kind: "error", text: table.invalid_option_message });
    });
  }

  /** An F-key the program does not serve: the entry is sent back unexamined, as received. */
  function onInvalidKey(): void {
    const entry = form.getValues("option");
    if (entry !== "") {
      form.setValue("option", justified(entry));
    }
    sendAgain({ kind: "error", text: INVALID_KEY });
  }

  const loadError = menuQuery.error?.message;
  const shown: ShellMessage | undefined =
    sent.message ?? (loadError === undefined ? undefined : { kind: "error", text: loadError });
  const message: ShellMessage | undefined =
    shown === undefined ? undefined : { ...shown, send: sent.number };

  return (
    <ScreenShell
      transaction={SCREENS.COADM01.transaction}
      program={SCREENS.COADM01.program}
      title="Admin Menu"
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={{ onInvalidKey }}
      busy={pending}
    >
      <Form {...form}>
        <form
          id={formId}
          aria-busy={pending}
          onSubmit={form.handleSubmit(onEnter, onRefusedEntry)}
        >
          <FieldGrid>
            {/*
             * The option table holds the twelve OPTNnnn fields of lines 6 to 17, so it spans those
             * lines; a single-line GridCell would make every line of the 24-line grid as tall as
             * the table. Departs from GridCell placement for this one element only.
             */}
            <div className="col-span-40 col-start-20 row-span-12 row-start-6">
              <Table aria-label="Admin Menu options">
                <TableBody>
                  {Array.from({ length: OPTION_LINES }, (_, index) => {
                    const field = `OPTN${String(index + 1).padStart(3, "0")}`;
                    const text = menu?.options.at(index)?.text;
                    return (
                      // A line without an option carries nothing to read, so assistive technology
                      // skips it; it still shows the field's INITIAL and keeps its line. Its row
                      // rule is transparent, so the blank map field draws no line, yet keeps the
                      // rule's width and with it the height of a filled line; nor does it take the
                      // table's hover band.
                      <TableRow
                        key={field}
                        aria-hidden={text === undefined ? true : undefined}
                        className={
                          text === undefined ? "border-transparent hover:bg-transparent" : undefined
                        }
                      >
                        <TableCell
                          data-field={field}
                          className="whitespace-pre text-muted-foreground"
                        >
                          {text ?? OPTION_INITIAL}
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
            <FormField
              control={form.control}
              name="option"
              render={({ field }) => (
                // `contents` lets the label and the input each take their own map position.
                <FormItem className="contents">
                  <GridCell row={20} col={15} span={25}>
                    {/* `h-full` centres the text on the line, level with the input's text. */}
                    <FormLabel className="h-full whitespace-pre font-semibold text-foreground">
                      Please select an option :
                    </FormLabel>
                  </GridCell>
                  <GridCell row={20} col={41} span={COADM01_LENGTHS.option}>
                    <FormControl>
                      {/*
                       * Two grid columns are narrower than two digits and the input's padding, so
                       * the box is sized by its map LENGTH (`size`) and extends over the empty
                       * columns after the field.
                       */}
                      <Input
                        {...field}
                        readOnly={pending}
                        data-field="OPTION"
                        maxLength={COADM01_LENGTHS.option}
                        size={COADM01_LENGTHS.option}
                        inputMode="numeric"
                        autoComplete="off"
                        autoFocus
                        className="w-auto"
                      />
                    </FormControl>
                  </GridCell>
                </FormItem>
              )}
            />
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
