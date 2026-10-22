import { zodResolver } from "@hookform/resolvers/zod";
import { useCallback, useId, useMemo, useState, type ReactNode } from "react";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCobil00cPay } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
// The shared `CSMSG01Y` texts of the frontend live in `@/lib/header`, one source for all screens.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COBIL00_LENGTHS, cobil00Schema, type Cobil00Values } from "@/schemas/COBIL00";

/**
 * Bill payment, map `COBIL0A` of `app/bms/COBIL00.bms`: transaction CB00, program COBIL00C,
 * endpoint 15 (`POST /api/v1/bill-payments`), route `/bill-payment`, menu option 10.
 *
 * Placement, texts and colours follow `modern/audit/screens.md` rows `COBIL00-001` to
 * `COBIL00-033`; behaviour follows the `client` rows of `modern/audit/online/COBIL00C.md`.
 * Every edit and every message belongs to the server: a blank confirmation shows the balance,
 * `Y` pays it, `N` clears the screen (COBIL00C-009 to COBIL00C-012). The balance arrives as the
 * 14-character text `PIC +9999999999.99` edits (ARITH-019) and is shown exactly as received; the
 * screen never parses, formats or computes an amount.
 */

const SCREEN = SCREENS.COBIL00;

/** The map's screen title, `POS=(4,35)` (COBIL00-011). */
const TITLE = "Bill Payment";

/* Literal fields of the map, each its `INITIAL` exactly, trailing spaces included. */
/** COBIL00-012, `POS=(6,6) LENGTH=14`, GREEN. */
const ACCT_LABEL = "Enter Acct ID:";
/** COBIL00-015, `POS=(8,6) LENGTH=70`, YELLOW: seventy dashes. */
const RULE = "-".repeat(70);
/** COBIL00-016, `POS=(11,6) LENGTH=25`, TURQUOISE. */
const BALANCE_LABEL = "Your current balance is: ";
/** COBIL00-019, `POS=(15,6) LENGTH=53`, TURQUOISE. */
const CONFIRM_LABEL = "Do you want to pay your balance now. Please confirm: ";
/** COBIL00-022, `POS=(15,63) LENGTH=5`, NEUTRAL. */
const CONFIRM_HINT = "(Y/N)";
/** `CURBAL`, `POS=(11,32) LENGTH=14` (COBIL00-017). */
const BALANCE_LENGTH = 14;

/*
 * Colour tokens of AAP 0.4.3: GREEN, TURQUOISE and NEUTRAL snap to the foreground, the YELLOW
 * rule adds emphasis, the BLUE balance is muted. `h-full` with `items-center` centres each text
 * on its line, which the inputs make taller than one line of text; `whitespace-pre` keeps the
 * literals' spacing.
 */
const LABEL_CLASS = "h-full whitespace-pre text-foreground";
const TEXT_CLASS = "flex h-full items-center whitespace-pre text-foreground";
const RULE_CLASS = `${TEXT_CLASS} font-semibold`;
/*
 * BLITZY [LAYOUT]: each input keeps its map cell (`POS`, `LENGTH`), but on narrow frames the 80
 * columns of `FieldGrid` leave a cell narrower than its `LENGTH` characters plus the Input's
 * padding, which would hide the end of the value. A minimum width on the spacing scale lets the
 * box extend past its cell into map columns that hold nothing:
 * - ACTIDIN, 11 characters (COBIL00-013): `min-w-36`, room for 11 monospace characters (0.6em
 *   each) at the Input's `text-base` below `md` as well as its `text-sm` above, with the default
 *   `px-3` and border; nothing follows it on line 6.
 * - CURBAL, 14 characters (COBIL00-017): `min-w-44`, likewise; nothing follows it on line 11.
 * - CONFIRM, 1 character (COBIL00-020): `min-w-6` with `px-1` (narrowing the Input's `px-3`), so
 *   the box reaches into the empty attribute and stopper columns (COBIL00-021), short of `(Y/N)`.
 */
const ACCT_CLASS = "min-w-36";
const BALANCE_CLASS = "min-w-44 text-muted-foreground";
const CONFIRM_CLASS = "min-w-6 px-1 text-center";

/** The cleared map: account and confirmation empty, as first entry and F4 leave them. */
const EMPTY_VALUES: Cobil00Values = { acctId: "", confirm: "" };

/**
 * Clearing keeps the inputs' registered refs; a plain `reset` drops them until the next render,
 * and the cursor could not move to the account in the same handler.
 */
const CLEAR_OPTIONS = { keepFieldsRef: true } as const;

/** `ScreenError.field` wire name → the form field the cursor goes to (`ACTIDINL`, `CONFIRML`). */
const FIELD_BY_WIRE_NAME: Readonly<Record<string, keyof Cobil00Values>> = {
  acct_id: "acctId",
  confirm: "confirm",
};

/** The form field a `ScreenError` names, or `undefined` when it names none of this map's. */
function fieldOf(wireName: string | null | undefined): keyof Cobil00Values | undefined {
  return wireName != null && Object.hasOwn(FIELD_BY_WIRE_NAME, wireName)
    ? FIELD_BY_WIRE_NAME[wireName]
    : undefined;
}

/** The map's row-24 labels: `ENTER=Continue`, `F3=Back`, `F4=Clear` (COBIL00-024). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COBIL00);

/** Route paths of the 21 screens; the only places F3 returns to. */
const SCREEN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/**
 * Where F3 goes: the screen that transferred here, as its route state `from` names it, or the
 * main menu when there is none, as COBIL00C goes to `CDEMO-FROM-PROGRAM` or else to COMEN01C
 * (COBIL00C-004, `app/cbl/COBIL00C.cbl:128-135`). A `from` that is not a screen path is ignored.
 */
function backPath(state: unknown): string {
  if (typeof state === "object" && state !== null && "from" in state) {
    const { from } = state;
    if (typeof from === "string" && SCREEN_PATHS.has(from)) {
      return from;
    }
  }
  return PATHS.COMEN01;
}

/** The message row of a 200: red, except the green payment success (COBIL00C-035). */
function resultMessage(result: components["schemas"]["BillPaymentResult"]): ShellMessage {
  return {
    text: result.message ?? "",
    kind: result.new_tran_id == null ? "error" : "info",
  };
}

/**
 * The bill-payment screen. It opens empty with the cursor on the account and sends nothing until
 * ENTER (COBIL00C-002).
 *
 * - ENTER sends the two inputs as typed. A 200 shows its balance, or none, and its message; when
 *   `cleared` (after `N` and after a payment) the inputs are emptied and the cursor goes to the
 *   account, otherwise the cursor goes to the confirmation of the prompt (COBIL00C-019,
 *   COBIL00C-037). A `ScreenError` shows its message, marks the field it names invalid and moves
 *   the cursor to it; the account, balance and confirmation keep what they showed.
 * - F3 goes back (COBIL00C-004); F4 empties the screen and the message row (COBIL00C-005); any
 *   other function key shows the invalid-key message (COBIL00C-006). None sends a request.
 */
export function COBIL00(): ReactNode {
  const formId = useId();
  const balanceId = useId();
  const navigate = useNavigate();
  const { state } = useLocation();
  const pay = useCobil00cPay();
  // A field the server rejects stays marked invalid until it is retyped, which re-validates it
  // on change; or until the next ENTER, whose schema pass replaces every error; or until a clear,
  // whose `reset` keeps none.
  const form = useForm<Cobil00Values>({
    resolver: zodResolver(cobil00Schema),
    defaultValues: EMPTY_VALUES,
    reValidateMode: "onChange",
  });
  const { reset, setError, setFocus, setValue } = form;
  const [balance, setBalance] = useState("");
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);

  // The terminal keyboard stays locked until the program's reply arrives, so ENTER and F4 do
  // nothing while a request is in flight: a second ENTER cannot pay twice, and no reply lands on
  // a screen cleared after it was sent.
  const pending = pay.isPending;

  const clear = useCallback(() => {
    if (pending) {
      return;
    }
    reset(EMPTY_VALUES, CLEAR_OPTIONS);
    setBalance("");
    setMessage(undefined);
    setFocus("acctId");
  }, [pending, reset, setFocus]);

  const back = useCallback(() => {
    // COBIL00C names itself the from-program before its XCTL (`RETURN-TO-PREV-SCREEN`).
    void navigate(backPath(state), { state: { from: PATHS.COBIL00 } });
  }, [navigate, state]);

  const keys = useMemo<PfKeyBarEntry[]>(() => {
    const actions: PfKeyHandlers = { F3: back, F4: clear };
    return KEY_ITEMS.map((item) => ({ ...item, onPress: actions[item.key] }));
  }, [back, clear]);

  const keyOptions = useMemo<PfKeyOptions>(
    () => ({
      onInvalidKey: () => {
        setMessage({ text: INVALID_KEY, kind: "error" });
      },
    }),
    [],
  );

  function submit(values: Cobil00Values): void {
    if (pending) {
      return;
    }
    pay.mutate(
      { body: { acct_id: values.acctId, confirm: values.confirm } },
      {
        onSuccess: (result) => {
          if (result.cleared) {
            reset(EMPTY_VALUES, CLEAR_OPTIONS);
          } else {
            setValue("acctId", result.acct_id);
          }
          setBalance(result.current_balance ?? "");
          setMessage(resultMessage(result));
          setFocus(result.cleared ? "acctId" : "confirm");
        },
        onError: (error) => {
          setMessage({ text: error.message, kind: "error" });
          const field = fieldOf(error.field);
          if (field !== undefined) {
            // The named input is marked invalid; its text stays in the map's one message row.
            setError(field, { type: "server", message: error.message });
            setFocus(field);
          }
        },
      },
    );
  }

  // The schema checks only the map lengths, which `maxLength` already holds; should one fail, its
  // message takes the message row until the server's next message replaces it, and the form
  // moves the cursor to the field.
  function reject(errors: FieldErrors<Cobil00Values>): void {
    setMessage({
      text: errors.acctId?.message ?? errors.confirm?.message ?? "",
      kind: "error",
    });
  }

  return (
    <ScreenShell
      transaction={SCREEN.transaction}
      program={SCREEN.program}
      title={TITLE}
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={keyOptions}
    >
      <Form {...form}>
        <form id={formId} noValidate onSubmit={form.handleSubmit(submit, reject)}>
          <FieldGrid>
            <FormField
              control={form.control}
              name="acctId"
              render={({ field }) => (
                // `contents` lets the label and the input sit in their own grid cells.
                <FormItem className="contents">
                  <GridCell row={6} col={6} span={ACCT_LABEL.length}>
                    <FormLabel className={LABEL_CLASS}>{ACCT_LABEL}</FormLabel>
                  </GridCell>
                  <GridCell row={6} col={21} span={COBIL00_LENGTHS.acctId}>
                    <FormControl>
                      <Input
                        {...field}
                        maxLength={COBIL00_LENGTHS.acctId}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        className={ACCT_CLASS}
                      />
                    </FormControl>
                  </GridCell>
                </FormItem>
              )}
            />
            <GridCell row={8} col={6} span={RULE.length}>
              <span aria-hidden="true" className={RULE_CLASS}>
                {RULE}
              </span>
            </GridCell>
            <GridCell row={11} col={6} span={BALANCE_LABEL.length}>
              <Label htmlFor={balanceId} className={LABEL_CLASS}>
                {BALANCE_LABEL}
              </Label>
            </GridCell>
            <GridCell row={11} col={32} span={BALANCE_LENGTH}>
              <Input id={balanceId} readOnly value={balance} className={BALANCE_CLASS} />
            </GridCell>
            <FormField
              control={form.control}
              name="confirm"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={15} col={6} span={CONFIRM_LABEL.length}>
                    <FormLabel className={LABEL_CLASS}>{CONFIRM_LABEL}</FormLabel>
                  </GridCell>
                  <GridCell row={15} col={60} span={COBIL00_LENGTHS.confirm}>
                    <FormControl>
                      <Input
                        {...field}
                        maxLength={COBIL00_LENGTHS.confirm}
                        autoComplete="off"
                        spellCheck={false}
                        className={CONFIRM_CLASS}
                      />
                    </FormControl>
                  </GridCell>
                </FormItem>
              )}
            />
            <GridCell row={15} col={63} span={CONFIRM_HINT.length}>
              <span className={TEXT_CLASS}>{CONFIRM_HINT}</span>
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
