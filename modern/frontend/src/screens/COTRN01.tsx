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
import { useForm } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCotrn01cView } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldControl } from "@/components/screen/FieldControl";
import { FIELD_INPUT, FieldGrid, GridCell, fieldInputClass } from "@/components/screen/FieldGrid";
import { ScreenShell } from "@/components/screen/ScreenShell";
import {
  Form,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
// `@/lib/header` holds `CCDA-MSG-INVALID-KEY`, the text this screen shows for an unserved F-key
// (COTRN01C-007); it is imported here rather than copied so all 21 screens show one literal.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { cotrn01Schema, type Cotrn01Values } from "@/schemas/COTRN01";

/** One `POS` and `LENGTH` of map `COTRN1A`, from its row of `modern/audit/screens.md`. */
type MapPosition = {
  readonly row: number;
  readonly col: number;
  readonly span: number;
};

/**
 * One output field of `COTRN1A` with the literal that labels it: the BMS field name, the label's
 * `INITIAL` and both positions, and the `TranViewResponse` value the field shows.
 */
type OutputField = {
  readonly name: string;
  readonly label: string;
  readonly labelAt: MapPosition;
  readonly at: MapPosition;
  readonly value: (view: components["schemas"]["TranViewResponse"]) => string;
};

/**
 * The date part of a typed timestamp, the 10 bytes COTRN01C moves from `TRAN-ORIG-TS` or
 * `TRAN-PROC-TS PIC X(26)` into `TORIGDT`/`TPROCDT PIC X(10)`: `YYYY-MM-DD` in every stored layout
 * (COTRN01C-023, COTRN01C-024). The value is cut, never parsed into a `Date`, so no time zone can
 * move the day; `null` (a blank field) shows nothing.
 */
function datePart(timestamp: string | null | undefined): string {
  return timestamp == null ? "" : timestamp.slice(0, 10);
}

/**
 * The 13 detail fields, in map order (`COTRN01-016` to `COTRN01-053`). Every value is shown as the
 * response carries it: the card number already redacted by the server (COTRN01C-017), the amount
 * already edited (COTRN01C-022), codes and IDs with their leading zeros.
 */
const OUTPUT_FIELDS: readonly OutputField[] = [
  {
    name: "TRNID",
    label: "Transaction ID:",
    labelAt: { row: 10, col: 6, span: 15 },
    at: { row: 10, col: 22, span: 16 },
    value: (view) => view.tran_id,
  },
  {
    name: "CARDNUM",
    label: "Card Number:",
    labelAt: { row: 10, col: 45, span: 12 },
    at: { row: 10, col: 58, span: 16 },
    value: (view) => view.card_num,
  },
  {
    name: "TTYPCD",
    label: "Type CD:",
    labelAt: { row: 12, col: 6, span: 8 },
    at: { row: 12, col: 15, span: 2 },
    value: (view) => view.type_cd,
  },
  {
    name: "TCATCD",
    label: "Category CD:",
    labelAt: { row: 12, col: 23, span: 12 },
    at: { row: 12, col: 36, span: 4 },
    value: (view) => view.cat_cd,
  },
  {
    name: "TRNSRC",
    label: "Source:",
    labelAt: { row: 12, col: 46, span: 7 },
    at: { row: 12, col: 54, span: 10 },
    value: (view) => view.source,
  },
  {
    name: "TDESC",
    label: "Description:",
    labelAt: { row: 14, col: 6, span: 12 },
    at: { row: 14, col: 19, span: 60 },
    value: (view) => view.description,
  },
  {
    name: "TRNAMT",
    label: "Amount:",
    labelAt: { row: 16, col: 6, span: 7 },
    at: { row: 16, col: 14, span: 12 },
    value: (view) => view.amount,
  },
  {
    name: "TORIGDT",
    label: "Orig Date:",
    labelAt: { row: 16, col: 31, span: 10 },
    at: { row: 16, col: 42, span: 10 },
    value: (view) => datePart(view.orig_ts),
  },
  {
    name: "TPROCDT",
    label: "Proc Date:",
    labelAt: { row: 16, col: 57, span: 10 },
    at: { row: 16, col: 68, span: 10 },
    value: (view) => datePart(view.proc_ts),
  },
  {
    name: "MID",
    label: "Merchant ID:",
    labelAt: { row: 18, col: 6, span: 12 },
    at: { row: 18, col: 19, span: 9 },
    value: (view) => view.merchant_id,
  },
  {
    name: "MNAME",
    label: "Merchant Name:",
    labelAt: { row: 18, col: 33, span: 14 },
    at: { row: 18, col: 48, span: 30 },
    value: (view) => view.merchant_name,
  },
  {
    name: "MCITY",
    label: "Merchant City:",
    labelAt: { row: 20, col: 6, span: 14 },
    at: { row: 20, col: 21, span: 25 },
    value: (view) => view.merchant_city,
  },
  {
    name: "MZIP",
    label: "Merchant Zip:",
    labelAt: { row: 20, col: 53, span: 13 },
    at: { row: 20, col: 67, span: 10 },
    value: (view) => view.merchant_zip,
  },
];

/** `TRNIDIN` (`COTRN01-013`) and the literal that labels it (`COTRN01-012`). */
const TRAN_ID_LABEL: MapPosition = { row: 6, col: 6, span: 14 };
const TRAN_ID_INPUT: MapPosition = { row: 6, col: 21, span: 16 };

/** The NEUTRAL rule of 70 dashes under the Tran ID input (`COTRN01-015`). */
const SEPARATOR: MapPosition = { row: 8, col: 6, span: 70 };

/** The labels of row 24, `ENTER=Fetch  F3=Back  F4=Clear  F5=Browse Tran.` (`COTRN01-056`). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COTRN01);

/**
 * The key each transfer out of this screen carries as `EIBAID` into the target program. COTRN00C
 * starts differently after PF5 than after PF3 (COTRN00C-003, COTRN00C-004), so the target needs
 * the key as well as the origin; the values are those of the generated `TranListEntryKey`.
 */
const PF3_ENTRY: components["schemas"]["TranListEntryKey"] = "pf3";
const PF5_ENTRY: components["schemas"]["TranListEntryKey"] = "pf5";

/** Every route path of the registry, so only a screen of this application is a valid origin. */
const SCREEN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/**
 * What the transfer into this screen passed in route state, COTRN01C's view of the COMMAREA:
 * `from` is `CDEMO-FROM-PROGRAM` as a route, `tranId` is `CDEMO-CT01-TRN-SELECTED`, and `list` is
 * the paging state of `CDEMO-CT01-INFO` (first and last keys, page number, next-page flag), which
 * this screen never reads and only hands back.
 */
type TransferState = {
  readonly from?: string;
  readonly tranId?: string;
  readonly list?: object;
};

/**
 * Reads the route state a transfer set, `{ from, tran_id, list }` from a COTRN00 `S` selection or
 * `{ from }` from the menu. History state is untyped, so each member is kept only when it has the
 * expected type, `from` only when it is a route of this application, and `list` only when it is a
 * record; COTRN00 checks its fields when it gets the state back.
 */
function transferState(state: unknown): TransferState {
  if (typeof state !== "object" || state === null) {
    return {};
  }
  const from =
    "from" in state && typeof state.from === "string" && SCREEN_PATHS.has(state.from)
      ? state.from
      : undefined;
  const tranId =
    "tran_id" in state && typeof state.tran_id === "string" ? state.tran_id : undefined;
  const list =
    "list" in state &&
    typeof state.list === "object" &&
    state.list !== null &&
    !Array.isArray(state.list)
      ? state.list
      : undefined;
  return { from, tranId, list };
}

/**
 * The route state of a transfer out of this screen by `entryKey` to `target`: this screen as the
 * origin, the key, and, on a return to the transaction list, the paging state the list handed
 * over, unchanged. COTRN01C passes back the COMMAREA extension it received, so the list shows the
 * state it held when it left (COTRN00C-002). Only the list reads that state, and the menu passes
 * on only its 160-byte COMMAREA, so a return anywhere else carries none. Nothing is sent when
 * nothing was received: entered from the menu, F5 starts the list from its blank state.
 */
function transferOut(
  target: string,
  entryKey: components["schemas"]["TranListEntryKey"],
  received: TransferState,
) {
  const { list } = received;
  return {
    from: PATHS.COTRN01,
    entry_key: entryKey,
    ...(target === PATHS.COTRN00 && list !== undefined ? { list } : {}),
  };
}

/**
 * The selected transaction ID to read on entry, or `undefined` when there is none: COTRN01C reads
 * on first entry only when `CDEMO-CT01-TRN-SELECTED` is neither spaces nor low-values
 * (COTRN01C-002, COTRN01C-003), the condition `cotrn01Schema` checks for a typed ID.
 */
function entryTranId(state: TransferState): string | undefined {
  const { tranId } = state;
  return tranId !== undefined && cotrn01Schema.safeParse({ tranId }).success ? tranId : undefined;
}

/**
 * The form input a `ScreenError.field` names, by its wire name: endpoint 12 names `tran_id`, the
 * path parameter the Tran ID input is sent as, in every error it returns (COTRN01C-009,
 * COTRN01C-012, COTRN01C-013). A failure naming no field marks no input.
 */
function formFieldOf(field: string | null | undefined): keyof Cotrn01Values | undefined {
  return field === "tran_id" ? "tranId" : undefined;
}

/**
 * Map `COTRN01` (`COTRN1A`), transaction `CT01`, program `COTRN01C`: views one transaction by its
 * ID through endpoint 12, `GET /api/v1/transactions/{tran_id}`.
 *
 * - Entry: from a COTRN00 `S` selection the selected ID fills the Tran ID input and is read at
 *   once, with no ENTER (COTRN01C-003); from the menu the screen opens empty and sends nothing
 *   (COTRN01C-002).
 * - ENTER sends the Tran ID exactly as typed. A blank one is refused by `cotrn01Schema` with the
 *   program's message before any request (COTRN01C-009); every other message is the server's.
 *   The detail fields are blank while the read runs and after any failure, so a transaction shown
 *   before a failed fetch is no longer shown (COTRN01C-014). The cursor returns to the Tran ID
 *   input after every outcome, as every send of the program puts it there. A failure naming
 *   `tran_id` marks the input invalid until it is edited or the next key is answered; a schema
 *   refusal marks it until the next key is answered. The message row changes only with an answer,
 *   to the entry read or to a key, never while the operator types.
 * - F3 returns to the screen that transferred here, else to the main menu (COTRN01C-004); F4 clears
 *   the screen (COTRN01C-005); F5 goes to the transaction list (COTRN01C-006); any other F-key
 *   shows the invalid-key message, keeps the Tran ID and blanks the details (COTRN01C-007).
 * - While a read is outstanding the keyboard is locked, as the terminal's stays until the program
 *   answers: ENTER and F4 are refused, an unserved F-key shows nothing and the Tran ID input is
 *   read-only, so only one read is ever in flight. F3 and F5 still leave.
 *
 * The origin and the list's paging state stay in the route state for as long as the screen is
 * shown, so F3 returns there after any number of fetches (COTRN01C-008), and F3 or F5 back to the
 * list hands the paging state back unchanged (COTRN00C-002). Nothing is logged or stored: the card
 * number arrives redacted and is only displayed.
 */
export function COTRN01(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const transfer = transferState(location.state);
  const selectedTranId = entryTranId(transfer);

  const formId = useId();
  const fieldIdPrefix = useId();
  // The schema runs when ENTER is answered, never while the operator types: its refusal, and the
  // message row that shows it, change only when a key is answered.
  const form = useForm<Cotrn01Values>({
    resolver: zodResolver(cotrn01Schema),
    defaultValues: { tranId: selectedTranId ?? "" },
    reValidateMode: "onSubmit",
  });
  const view = useCotrn01cView();
  const { mutateAsync, reset: resetView } = view;
  const [keyMessage, setKeyMessage] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  // One read at a time, as the terminal keyboard locks until the program answers. The ref is set
  // before the form's validation starts, so a key pressed before the next render is refused too.
  const inFlight = useRef(false);

  /** Locks the keyboard for one read; `false` when a read already holds it. */
  const lock = useCallback((): boolean => {
    if (inFlight.current) {
      return false;
    }
    inFlight.current = true;
    setBusy(true);
    return true;
  }, []);

  /** Releases the keyboard: the read settled, or validation refused ENTER before any request. */
  const unlock = useCallback((): void => {
    inFlight.current = false;
    setBusy(false);
  }, []);

  /**
   * Reads `tranId` under the lock its caller holds. The read's own promise releases the lock and
   * returns the cursor, so they follow the answer even when `resetView` has detached the view from
   * it; a failure shows through `view.error` and marks the input it names.
   */
  const fetchTransaction = useCallback(
    (tranId: string): void => {
      setKeyMessage(undefined);
      // A new read replaces the last answer, so an input the last one rejected is unmarked, and
      // a read that succeeds leaves it unmarked.
      form.clearErrors("tranId");
      void mutateAsync(
        { path: { tran_id: tranId } },
        {
          // Runs only while the view still follows this read, the read whose error the message
          // row shows: the input the server names is marked invalid with that message, so its
          // field message carries the server's text.
          onError: (error) => {
            const name = formFieldOf(error.field);
            if (name !== undefined) {
              form.setError(name, { type: "server", message: error.message });
            }
          },
        },
      )
        .catch(() => undefined)
        .finally(() => {
          unlock();
          form.setFocus("tranId");
        });
    },
    [mutateAsync, unlock, form],
  );

  // One entry per transfer: the location key changes with every navigation to this route, and
  // the ref keeps a re-run of the effect for the same transfer from reading twice.
  const enteredLocation = useRef<string | null>(null);
  useEffect(() => {
    if (enteredLocation.current === location.key) {
      return;
    }
    enteredLocation.current = location.key;
    form.reset({ tranId: selectedTranId ?? "" }, { keepFieldsRef: true });
    resetView();
    setKeyMessage(undefined);
    if (selectedTranId !== undefined && lock()) {
      fetchTransaction(selectedTranId);
    }
  }, [location.key, selectedTranId, form, resetView, lock, fetchTransaction]);

  const validate = form.handleSubmit(
    (values) => {
      fetchTransaction(values.tranId);
    },
    () => {
      unlock();
      resetView();
      setKeyMessage(undefined);
    },
  );

  /** ENTER: refused while a read is outstanding, else validated under the lock, then read. */
  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    if (!lock()) {
      event.preventDefault();
      return;
    }
    void validate(event);
  }

  function back(): void {
    const target = transfer.from ?? PATHS.COMEN01;
    void navigate(target, { state: transferOut(target, PF3_ENTRY, transfer) });
  }

  function clearScreen(): void {
    if (inFlight.current) {
      return;
    }
    // `keepFieldsRef` keeps the input registered; a plain reset empties the registry until the
    // next render, and `setFocus` below would then find no field to focus.
    form.reset({ tranId: "" }, { keepFieldsRef: true });
    resetView();
    setKeyMessage(undefined);
    form.setFocus("tranId");
  }

  function browse(): void {
    void navigate(PATHS.COTRN00, { state: transferOut(PATHS.COTRN00, PF5_ENTRY, transfer) });
  }

  function showInvalidKey(): void {
    if (inFlight.current) {
      return;
    }
    form.clearErrors();
    resetView();
    setKeyMessage(INVALID_KEY);
  }

  const actions: PfKeyHandlers = { F3: back, F4: clearScreen, F5: browse };
  // While a read is outstanding ENTER and F4 are refused; F3 and F5 only navigate, so they leave.
  const keys = KEY_ITEMS.map((item) => ({
    ...item,
    onPress: actions[item.key],
    disabled: busy && (item.key === "ENTER" || item.key === "F4"),
  }));

  // The server's message wins: a validation message only shows while no response has replaced it.
  const message =
    view.error?.message ??
    keyMessage ??
    form.formState.errors.tranId?.message ??
    view.data?.message ??
    undefined;
  const shown = view.data;

  return (
    <ScreenShell
      transaction={SCREENS.COTRN01.transaction}
      program={SCREENS.COTRN01.program}
      title="View Transaction"
      formId={formId}
      message={{ text: message, kind: "error" }}
      keys={keys}
      keyOptions={{ foldShifted: KEY_ROWS.COTRN01.foldShifted, onInvalidKey: showInvalidKey }}
      busy={busy}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={busy} onSubmit={onSubmit}>
          <FieldGrid>
            <FormField
              control={form.control}
              name="tranId"
              render={({ field }) => (
                // `contents` lets the label and the input each sit in their own grid cell. The
                // message row is the one visible message, so the field message `FormMessage`
                // adds is for assistive technology only, read with the input it rejects. The map
                // gives the field no hint, so its control names no description, and the message
                // only while it shows.
                <FormItem className="contents">
                  <GridCell {...TRAN_ID_LABEL}>
                    <FormLabel className="h-full whitespace-pre text-foreground">
                      Enter Tran ID:
                    </FormLabel>
                  </GridCell>
                  <GridCell {...TRAN_ID_INPUT}>
                    <FieldControl>
                      <Input
                        {...field}
                        onChange={(event) => {
                          // An edit replaces the value the server rejected, so its mark goes
                          // while the message row keeps the server's message. A schema refusal
                          // stays, with its message, until the next key is answered.
                          if (form.getFieldState("tranId").error?.type === "server") {
                            form.clearErrors("tranId");
                          }
                          field.onChange(event);
                        }}
                        data-field="TRNIDIN"
                        readOnly={busy}
                        maxLength={TRAN_ID_INPUT.span}
                        size={TRAN_ID_INPUT.span}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        className={FIELD_INPUT}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />
            <GridCell {...SEPARATOR}>
              <span aria-hidden="true" className="flex h-full items-center whitespace-pre">
                {"-".repeat(SEPARATOR.span)}
              </span>
            </GridCell>
            {OUTPUT_FIELDS.map((output) => {
              const id = `${fieldIdPrefix}-${output.name}`;
              return (
                <OutputCells
                  key={output.name}
                  id={id}
                  field={output}
                  value={shown === undefined ? "" : output.value(shown)}
                />
              );
            })}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}

/** Props of {@link OutputCells}. */
type OutputCellsProps = {
  readonly id: string;
  readonly field: OutputField;
  readonly value: string;
};

/**
 * One `ASKIP` output field and its label, each in its own grid cell. The label is TURQUOISE, the
 * value BLUE (AAP 0.4.3). The box is the shared map-field box (`fieldInputClass` with `size` = its
 * `LENGTH`): it spans its `LENGTH` columns and is never narrower than its value plus the inset,
 * because a grid column is narrower than a character, so the two columns of `TTYPCD` alone could
 * not show its two characters. Like `ASKIP`, the cursor skips it: it takes no tab stop, yet stays
 * readable and selectable.
 */
function OutputCells({ id, field, value }: OutputCellsProps): ReactNode {
  return (
    <>
      <GridCell {...field.labelAt}>
        <Label htmlFor={id} className="h-full whitespace-pre text-foreground">
          {field.label}
        </Label>
      </GridCell>
      <GridCell {...field.at}>
        <Input
          id={id}
          data-field={field.name}
          readOnly
          tabIndex={-1}
          value={value}
          size={field.at.span}
          className={`${fieldInputClass(field.at.span)} text-muted-foreground`}
        />
      </GridCell>
    </>
  );
}
