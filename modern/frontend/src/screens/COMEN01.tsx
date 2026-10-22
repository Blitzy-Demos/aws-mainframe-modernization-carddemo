import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { useForm } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useComen01cMainMenu } from "@/api/hooks";
import type { components } from "@/api/schema";
import { getSession } from "@/auth/session";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableRow } from "@/components/ui/table";
// The invalid-key text every screen shares (COMEN01C-007), kept with the header values.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, PROGRAM_ROUTES, SCREENS } from "@/routes";
import { COMEN01_LENGTHS, type Comen01Values, comen01Schema } from "@/schemas/COMEN01";

/*
 * Main menu for regular users: map `COMEN1A` of `app/bms/COMEN01.bms`, transaction `CM00`,
 * program `COMEN01C`, route `/menu` (AAP 0.3.5).
 *
 * The screen loads endpoint 2 once when it mounts, as COMEN01C sends its option list on first
 * entry (COMEN01C-005), and decides ENTER, F3 and every other key from that response with no
 * further request (COMEN01-035). Every message it shows is a text the response carries, or the
 * shared invalid-key text, so the screen composes none (COMEN01C-011). The menu is a single
 * resource, not a collection, so no page size is sent (Bounded Pagination).
 *
 * The user type the admin-only check compares is the one endpoint 2 states, the verified token's
 * claim (COMEN01C-004, AAP 0.5.1). It only chooses the message; the API's gates are the control.
 */

/** `OPTN001` to `OPTN012`: the map's option rows 6 to 17 (COMEN01-012 to COMEN01-023). */
const OPTION_ROWS = 12;

/** `LENGTH=40` of each option row, the width of the text `BUILD-MENU-OPTIONS` strings. */
const OPTION_TEXT_LENGTH = 40;

/**
 * A row no option fills: 40 spaces, as the map's `INITIAL=' '` field shows on the terminal. Row 17
 * (`OPTN012`) always shows it, because the loop stops after the 11th option (COMEN01C-022), and so
 * does every row while the response is loading. Rendered with its spaces, a blank row keeps the
 * height and width of a filled one.
 */
const BLANK_OPTION = " ".repeat(OPTION_TEXT_LENGTH);

/** `OPTION`, `LENGTH=2`: the width of the option input and of the value the edit produces. */
const OPTION_LENGTH = COMEN01_LENGTHS.option;

/** The row-24 labels `ENTER=Continue` and `F3=Exit` (COMEN01-028, COMEN01-035, COMEN01-036). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COMEN01);

/** The BMS name of the option row at `index` (0-based): `OPTN001` to `OPTN012`. */
function optionField(index: number): string {
  return `OPTN${String(index + 1).padStart(3, "0")}`;
}

/**
 * The option as `PROCESS-ENTER-KEY` reads it from the input (COMEN01C-008,
 * `app/cbl/COMEN01C.cbl:117-124`): the 2-byte field is cut after its last non-space position (or
 * after position 1 when it is all spaces), moved right-justified into `WS-OPTION-X PIC X(02) JUST
 * RIGHT`, and every space becomes `0`. So an empty entry gives `00`, `1` gives `01` and `11` stays
 * `11`. Only spaces count as blank, as for the COBOL loop.
 */
function normaliseOption(typed: string): string {
  const field = typed.padEnd(OPTION_LENGTH, " ").slice(0, OPTION_LENGTH);
  const used = Math.max(1, field.replace(/ +$/, "").length);
  return field.slice(0, used).padStart(OPTION_LENGTH, " ").replaceAll(" ", "0");
}

/**
 * What ENTER leads to: a transfer to the option's screen, or the menu shown again with a message
 * and the option input showing the 2-digit value the edit produced, which COMEN01C moves back to
 * `OPTIONO` (COMEN01C-008).
 */
type EnterOutcome =
  | { readonly kind: "transfer"; readonly route: string }
  | { readonly kind: "stay"; readonly message: string; readonly option: string };

/**
 * The checks of `PROCESS-ENTER-KEY` in program order, applied to the endpoint 2 response
 * (`app/cbl/COMEN01C.cbl:127-188`). `numeric` is the schema's verdict on the entry: the input
 * takes digits only (`NUM`, COMEN01-025).
 *
 * 1. An entry with a non-digit, `00`, or a number no option carries is refused with the invalid
 *    option text (COMEN01C-009, COMEN01C-010). COMEN01C's range test is `1` to the option count,
 *    and the options are numbered `01` to the count, so the lookup by number is that test.
 * 2. An option whose user type is `A` is refused to a `U` user with the admin-only text
 *    (COMEN01C-012); no shipped option has that type.
 * 3. An option whose program is not installed shows its not-installed text, in red
 *    (COMEN01C-016); every shipped option is installed (COMEN01C-013).
 * 4. Otherwise the screen transfers to the option's program (COMEN01C-014, COMEN01C-018). A
 *    program the route table lacks shows the option's unavailable text instead, as no screen
 *    exists to transfer to; every shipped program has one.
 *
 * A text the response omits shows as an empty message row, never as `undefined`.
 */
function enterOutcome(
  menu: components["schemas"]["MenuResponse"],
  typed: string,
  numeric: boolean,
): EnterOutcome {
  const option = normaliseOption(typed);
  const stay = (message: string): EnterOutcome => ({ kind: "stay", message, option });
  const chosen =
    numeric && option !== "00" ? menu.options.find((entry) => entry.number === option) : undefined;
  if (chosen === undefined) {
    return stay(menu.invalid_option_message);
  }
  if (menu.user_type === "U" && chosen.admin_only) {
    return stay(menu.admin_only_message ?? "");
  }
  if (!chosen.installed) {
    return stay(chosen.unavailable_message ?? "");
  }
  // An own-property check, so a program name such as `constructor` finds no inherited entry.
  const route = Object.hasOwn(PROGRAM_ROUTES, chosen.program)
    ? PROGRAM_ROUTES[chosen.program]
    : undefined;
  return route === undefined
    ? stay(chosen.unavailable_message ?? "")
    : { kind: "transfer", route };
}

/**
 * The route state a transfer carries: the state the menu received, which holds the COMMAREA's
 * account, customer and card fields an earlier screen set, with `/menu` as the target's origin.
 * COMEN01C passes its COMMAREA on unchanged apart from the from-program and the context, which
 * starts the target as a first entry (COMEN01C-014, COMEN01C-018).
 */
function transferState(received: unknown): Record<string, unknown> {
  const commarea =
    typeof received === "object" && received !== null && !Array.isArray(received) ? received : {};
  return { ...commarea, from: PATHS.COMEN01 };
}

/**
 * The main-menu screen (CM00 / COMEN01C, endpoint 2).
 *
 * - On mount: endpoint 2 loads the option table; while it loads, the 12 rows are blank, and a
 *   load failure shows its `ScreenError` message in the error row. A failure after which the
 *   session is gone (the API refused the token, and the client cleared the session) sends the
 *   user to sign-on, where the route guard would send a user with no session (COMEN01C-003).
 * - ENTER: `enterOutcome`, decided from the loaded response; while it loads, or reloads, ENTER
 *   decides nothing and is disabled, the key row and the form are busy and the option input is
 *   read-only. The server's texts win over the schema's messages, so none of zod's is shown.
 * - F3: back to sign-on with no request (COMEN01C-006), at any time, a load in progress included.
 *   The session is not cleared here: the sign-on screen's first entry clears it.
 * - Any other F-key: the invalid-key text, the input left as typed (COMEN01C-007); while endpoint
 *   2 loads or reloads, the busy key row takes it with no message.
 *
 * Every key clears the message of the previous one first (COMEN01C-001). Whenever the menu is
 * shown again with a message, the cursor returns to the option input, the map's `IC` field.
 */
export function COMEN01(): ReactNode {
  const navigate = useNavigate();
  const { state: received } = useLocation();
  const menu = useComen01cMainMenu();
  const formId = useId();
  const form = useForm<Comen01Values>({
    resolver: zodResolver(comen01Schema),
    defaultValues: { option: "" },
  });
  // The message the last key produced; `undefined` until a key has decided one, so until then the
  // row shows the load failure, if any.
  const [message, setMessage] = useState<string | undefined>(undefined);
  // Whether endpoint 2 is loading or reloading, when ENTER decides nothing. Read here, during
  // render, so the screen renders again when the fetch ends: TanStack Query re-renders only for
  // the result fields a render reads, and a reload on a return to `/menu` brings identical data,
  // so a flag read only in the handler would keep the value of the render that started the reload.
  // The same flag disables ENTER, so the button and the key refuse what the handler would ignore.
  const busy = menu.isFetching;

  useEffect(() => {
    if (menu.isError && getSession() === null) {
      void navigate(PATHS.COSGN00, { replace: true });
    }
  }, [menu.isError, navigate]);

  function enter(typed: string, numeric: boolean): void {
    const loaded = menu.data;
    if (loaded === undefined || busy) {
      return;
    }
    const outcome = enterOutcome(loaded, typed, numeric);
    if (outcome.kind === "transfer") {
      void navigate(outcome.route, { state: transferState(received) });
      return;
    }
    setMessage(outcome.message);
    form.setValue("option", outcome.option);
    // The map is sent again, and its `IC` field takes the cursor (COMEN01-025).
    form.setFocus("option");
  }

  const keys = useMemo<readonly PfKeyBarEntry[]>(
    () =>
      KEY_ITEMS.map((item) =>
        item.key === "F3"
          ? {
              ...item,
              onPress: () => {
                void navigate(PATHS.COSGN00);
              },
            }
          : { ...item, disabled: busy },
      ),
    [navigate, busy],
  );

  const keyOptions = useMemo<PfKeyOptions>(
    () => ({
      foldShifted: KEY_ROWS.COMEN01.foldShifted,
      onInvalidKey: () => {
        setMessage(INVALID_KEY);
        form.setFocus("option");
      },
    }),
    [form],
  );

  const options = menu.data?.options ?? [];
  const rows = Array.from(
    { length: OPTION_ROWS },
    (_, index) => options.at(index)?.text ?? BLANK_OPTION,
  );

  return (
    <ScreenShell
      transaction={SCREENS.COMEN01.transaction}
      program={SCREENS.COMEN01.program}
      title="Main Menu"
      formId={formId}
      message={{ kind: "error", text: message ?? menu.error?.message ?? "" }}
      keys={keys}
      keyOptions={keyOptions}
      busy={busy}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={busy}
          onSubmit={form.handleSubmit(
            (values) => {
              enter(values.option, true);
            },
            () => {
              enter(form.getValues("option"), false);
            },
          )}
        >
          <FieldGrid>
            {/*
             * The option rows, `POS=(6,20)` to `POS=(17,20)`, `LENGTH=40`, as one read-only table
             * (COMEN01-012 to COMEN01-023). `GridCell` places a field on one line; a 12-row table
             * in a one-line cell would stretch every flexible line of the grid to its height, so
             * this cell spans the 12 lines the rows occupy on the map.
             */}
            <div className="row-start-6 row-span-12 col-start-20 col-span-40">
              <Table aria-label="Menu options">
                <TableBody>
                  {rows.map((text, index) => (
                    // Read-only rows: the table's hover highlight would mark them as interactive.
                    <TableRow key={optionField(index)} className="hover:bg-transparent">
                      {/* BLUE output field; `whitespace-pre` shows the 40 bytes as sent. */}
                      <TableCell
                        data-field={optionField(index)}
                        className="whitespace-pre text-muted-foreground"
                      >
                        {text}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            <FormField
              control={form.control}
              name="option"
              render={({ field }) => (
                // `contents` keeps the label's and the input's cells on the map's grid.
                <FormItem className="contents">
                  {/*
                   * `POS=(20,15) LENGTH=25`, TURQUOISE and BRT (COMEN01-024). `h-full` lets the
                   * label's own centring align its text with the input's on the same line.
                   */}
                  <GridCell row={20} col={15} span={25}>
                    <FormLabel className="h-full whitespace-pre font-semibold text-foreground">
                      Please select an option :
                    </FormLabel>
                  </GridCell>
                  {/*
                   * `POS=(20,41) LENGTH=2`, `NUM`, `IC`, underlined (COMEN01-025). Two grid
                   * columns are narrower than the input's padding, so the input takes the width
                   * of its two characters (`size`) rather than its cell's.
                   */}
                  <GridCell row={20} col={41} span={OPTION_LENGTH}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={OPTION_LENGTH}
                        size={OPTION_LENGTH}
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
