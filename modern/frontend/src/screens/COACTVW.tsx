import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type BaseSyntheticEvent,
  type ReactNode,
} from "react";
import { useForm } from "react-hook-form";
import { useNavigate } from "react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { cn } from "cn";
import type { components } from "@/api/schema";
import { useCoactvwcView } from "@/api/hooks";
import { FieldControl } from "@/components/screen/FieldControl";
import { FieldGrid, GridCell, fieldBox } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { KEY_ROWS, keyRowItems, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COACTVW_LENGTHS, coactvwSchema, type CoactvwValues } from "@/schemas/COACTVW";

/*
 * Account view: map COACTVW (CACTVWA), transaction CAVW, program COACTVWC, route /accounts/view
 * (AAP 0.3.5). ENTER sends the account number to endpoint 4 and shows the account and customer
 * blocks of the answer; F3 returns to the main menu. Layout and attributes follow the
 * COACTVW-<nnn> rows of modern/audit/screens.md, behaviour the client rows of
 * modern/audit/online/COACTVWC.md.
 *
 * The server owns every program message: the error row shows the endpoint's message, or, when no
 * answer is current, the schema's MUSTFILL message for a partly filled field. Beside some error
 * messages the endpoint still sends a block, as COACTVWC shows it (COACTVWC-008): the account
 * block when only the customer read did not succeed, both blocks when the account read did not
 * and the customer read found the customer. Values are shown exactly as the endpoint sends them:
 * amounts already edited, the SSN already redacted, dates as their YYYY-MM-DD text. Nothing here
 * logs, stores or copies a response.
 */

/**
 * The information row: the 40-byte `WS-INFO-MSG` of 88 `WS-PROMPT-FOR-INPUT` in the 45-position
 * `INFOMSG`. COACTVWC moves nothing else there, so the screen shows it on every display and no
 * response carries one (COACTVWC-027, COACTVW-096).
 */
const PROMPT_FOR_INPUT = "Enter or update id of account to display".padEnd(45, " ");

/**
 * Endpoint 4's answer to an account number not supplied (COACTVWC-030), after which COACTVWC
 * moves `'*'` into the 11-byte `ACCTSIDO` (COACTVWC-028).
 */
const NO_INPUT_RECEIVED = "No input received";
const BLANK_FILTER_MARK = "*".padEnd(COACTVW_LENGTHS.acctId, " ");

/** The `ScreenError` field that turns the account number red (COACTVWC-028). */
const ACCOUNT_FIELD = "acct_id";

/*
 * Every map literal is TURQUOISE, which snaps to the foreground token (AAP 0.4.3); `whitespace-pre`
 * keeps the padding that aligns the colons, and `h-7` centres the text on the height of the
 * fields beside it.
 */
const LABEL_CLASS = "h-7 whitespace-pre text-foreground";

/** ENTER, served by COACTVWC but unlabelled on the map, as a bare `ENTER` button (COACTVW-108). */
const ENTER_KEY: PfKeyBarEntry = { key: "ENTER", label: "ENTER", dark: false };

/** One literal of the map that captions an output field. */
type MapLabel = {
  readonly text: string;
  readonly row: number;
  readonly col: number;
  readonly length: number;
};

/** One output field of the map, its caption, and the value of its block that it shows. */
type OutputField = {
  /** BMS field name, kept as `data-field`. */
  readonly name: string;
  readonly row: number;
  readonly col: number;
  readonly length: number;
} & (
  | {
      readonly block: "account";
      readonly value: (account: components["schemas"]["AccountViewAccountBlock"]) => string;
    }
  | {
      readonly block: "customer";
      readonly value: (customer: components["schemas"]["AccountViewCustomerBlock"]) => string;
    }
) &
  (
    | { readonly label: MapLabel; readonly ariaLabel?: never }
    | { readonly label?: never; readonly ariaLabel: string }
  );

/** A typed date (`format: date`) as its `YYYY-MM-DD` text; blank for `null` (COACTVWC-014, -019). */
function dateText(value: string | null | undefined): string {
  return value?.slice(0, 10) ?? "";
}

/**
 * The 28 output fields in macro order, each with the literal that captions it and the block it
 * belongs to (COACTVW-015 to COACTVW-094): the ten of the account block (COACTVWC-010 to
 * COACTVWC-015), then the eighteen of the customer block (COACTVWC-016 to COACTVWC-026). The row-14
 * literals caption the three name fields below them. `ACSADL2` has no literal: it is the second
 * line of `Address:`, and its accessible name says so.
 */
const OUTPUT_FIELDS: readonly OutputField[] = [
  {
    name: "ACSTTUS",
    block: "account",
    row: 5,
    col: 70,
    length: 1,
    label: { text: "Active Y/N: ", row: 5, col: 57, length: 12 },
    value: (account) => account.active_status,
  },
  {
    name: "ADTOPEN",
    block: "account",
    row: 6,
    col: 17,
    length: 10,
    label: { text: "Opened:", row: 6, col: 8, length: 7 },
    value: (account) => dateText(account.open_date),
  },
  {
    name: "ACRDLIM",
    block: "account",
    row: 6,
    col: 61,
    length: 15,
    label: { text: "Credit Limit        :", row: 6, col: 39, length: 21 },
    value: (account) => account.credit_limit,
  },
  {
    name: "AEXPDT",
    block: "account",
    row: 7,
    col: 17,
    length: 10,
    label: { text: "Expiry:", row: 7, col: 8, length: 7 },
    value: (account) => dateText(account.expiration_date),
  },
  {
    name: "ACSHLIM",
    block: "account",
    row: 7,
    col: 61,
    length: 15,
    label: { text: "Cash credit Limit   :", row: 7, col: 39, length: 21 },
    value: (account) => account.cash_credit_limit,
  },
  {
    name: "AREISDT",
    block: "account",
    row: 8,
    col: 17,
    length: 10,
    label: { text: "Reissue:", row: 8, col: 8, length: 8 },
    value: (account) => dateText(account.reissue_date),
  },
  {
    name: "ACURBAL",
    block: "account",
    row: 8,
    col: 61,
    length: 15,
    label: { text: "Current Balance     :", row: 8, col: 39, length: 21 },
    value: (account) => account.current_balance,
  },
  {
    name: "ACRCYCR",
    block: "account",
    row: 9,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Credit:", row: 9, col: 39, length: 21 },
    value: (account) => account.current_cycle_credit,
  },
  {
    name: "AADDGRP",
    block: "account",
    row: 10,
    col: 23,
    length: 10,
    label: { text: "Account Group:", row: 10, col: 8, length: 14 },
    value: (account) => account.group_id,
  },
  {
    name: "ACRCYDB",
    block: "account",
    row: 10,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Debit :", row: 10, col: 39, length: 21 },
    value: (account) => account.current_cycle_debit,
  },
  {
    name: "ACSTNUM",
    block: "customer",
    row: 12,
    col: 23,
    length: 9,
    label: { text: "Customer id  :", row: 12, col: 8, length: 14 },
    value: (customer) => customer.customer_id,
  },
  {
    name: "ACSTSSN",
    block: "customer",
    row: 12,
    col: 54,
    length: 12,
    label: { text: "SSN:", row: 12, col: 49, length: 4 },
    // Already `***-**-` and the last four digits (COACTVWC-017, E32); shown as received.
    value: (customer) => customer.ssn,
  },
  {
    name: "ACSTDOB",
    block: "customer",
    row: 13,
    col: 23,
    length: 10,
    label: { text: "Date of birth:", row: 13, col: 8, length: 14 },
    value: (customer) => dateText(customer.date_of_birth),
  },
  {
    name: "ACSTFCO",
    block: "customer",
    row: 13,
    col: 61,
    length: 3,
    label: { text: "FICO Score:", row: 13, col: 49, length: 11 },
    value: (customer) => customer.fico_score,
  },
  {
    name: "ACSFNAM",
    block: "customer",
    row: 15,
    col: 1,
    length: 25,
    label: { text: "First Name", row: 14, col: 1, length: 10 },
    value: (customer) => customer.first_name,
  },
  {
    name: "ACSMNAM",
    block: "customer",
    row: 15,
    col: 28,
    length: 25,
    label: { text: "Middle Name: ", row: 14, col: 28, length: 13 },
    value: (customer) => customer.middle_name,
  },
  {
    name: "ACSLNAM",
    block: "customer",
    row: 15,
    col: 55,
    length: 25,
    label: { text: "Last Name : ", row: 14, col: 55, length: 12 },
    value: (customer) => customer.last_name,
  },
  {
    name: "ACSADL1",
    block: "customer",
    row: 16,
    col: 10,
    length: 50,
    label: { text: "Address:", row: 16, col: 1, length: 8 },
    value: (customer) => customer.address_line_1,
  },
  {
    name: "ACSSTTE",
    block: "customer",
    row: 16,
    col: 73,
    length: 2,
    label: { text: "State ", row: 16, col: 63, length: 6 },
    value: (customer) => customer.state,
  },
  {
    name: "ACSADL2",
    block: "customer",
    row: 17,
    col: 10,
    length: 50,
    ariaLabel: "Address line 2",
    value: (customer) => customer.address_line_2,
  },
  {
    name: "ACSZIPC",
    block: "customer",
    row: 17,
    col: 73,
    length: 5,
    label: { text: "Zip", row: 17, col: 63, length: 3 },
    value: (customer) => customer.zip,
  },
  {
    name: "ACSCITY",
    block: "customer",
    row: 18,
    col: 10,
    length: 50,
    label: { text: "City ", row: 18, col: 1, length: 5 },
    value: (customer) => customer.city,
  },
  {
    name: "ACSCTRY",
    block: "customer",
    row: 18,
    col: 73,
    length: 3,
    label: { text: "Country", row: 18, col: 63, length: 7 },
    value: (customer) => customer.country,
  },
  {
    name: "ACSPHN1",
    block: "customer",
    row: 19,
    col: 10,
    length: 13,
    label: { text: "Phone 1:", row: 19, col: 1, length: 8 },
    value: (customer) => customer.phone_1,
  },
  {
    name: "ACSGOVT",
    block: "customer",
    row: 19,
    col: 58,
    length: 20,
    label: { text: "Government Issued Id Ref    : ", row: 19, col: 24, length: 30 },
    value: (customer) => customer.government_id,
  },
  {
    name: "ACSPHN2",
    block: "customer",
    row: 20,
    col: 10,
    length: 13,
    label: { text: "Phone 2:", row: 20, col: 1, length: 8 },
    value: (customer) => customer.phone_2,
  },
  {
    name: "ACSEFTC",
    block: "customer",
    row: 20,
    col: 41,
    length: 10,
    label: { text: "EFT Account Id: ", row: 20, col: 24, length: 16 },
    value: (customer) => customer.eft_account_id,
  },
  {
    name: "ACSPFLG",
    block: "customer",
    row: 20,
    col: 78,
    length: 1,
    label: { text: "Primary Card Holder Y/N:", row: 20, col: 53, length: 24 },
    value: (customer) => customer.primary_card_holder,
  },
];

/** Map row of the NEUTRAL `Customer Details` caption between the two blocks (COACTVW-045). */
const CUSTOMER_DETAILS_ROW = 11;

/*
 * The two blocks of output fields either side of the caption, each in macro order, so the body
 * reads in map row order: the account block, the caption, then the customer block, whose row-14
 * literals stay with the row-15 name fields they caption.
 */
const ACCOUNT_OUTPUTS: readonly OutputField[] = OUTPUT_FIELDS.filter(
  (output) => output.row < CUSTOMER_DETAILS_ROW,
);
const CUSTOMER_OUTPUTS: readonly OutputField[] = OUTPUT_FIELDS.filter(
  (output) => output.row > CUSTOMER_DETAILS_ROW,
);

/** The answer the screen shows: the last response of endpoint 4, as COACTVWC sent it. */
type Shown = {
  /** The account block: after a view, or beside an error message; `null` otherwise. */
  readonly account: components["schemas"]["AccountViewAccountBlock"] | null;
  /** The customer block: after a view, or beside an error message; `null` otherwise. */
  readonly customer: components["schemas"]["AccountViewCustomerBlock"] | null;
  /** The program's message and the field it names; `null` after a view. */
  readonly error: { readonly message: string; readonly field: string | null } | null;
  /** How many responses have been shown, so focus returns to the field after each one. */
  readonly responses: number;
};

const NOTHING_SHOWN: Shown = { account: null, customer: null, error: null, responses: 0 };

/** Whether a value is an object or `null`, as a block member of an error body may be. */
function isObjectOrNull(value: unknown): value is object | null {
  return typeof value === "object";
}

/**
 * Whether a thrown body is endpoint 4's error: a `ScreenError` whose `account` and `customer`
 * members, when present, are blocks (COACTVWC-008). A failure without a body, such as the
 * browser's network `TypeError`, has a string `message` and neither member, and passes as an
 * error with no block.
 */
function isViewError(body: unknown): body is components["schemas"]["AccountViewError"] {
  return (
    typeof body === "object" &&
    body !== null &&
    "message" in body &&
    typeof body.message === "string" &&
    (!("account" in body) || isObjectOrNull(body.account)) &&
    (!("customer" in body) || isObjectOrNull(body.customer))
  );
}

/** The value `output` shows: its member of its block, blank while that block is not shown. */
function shownValue(output: OutputField, shown: Shown): string {
  if (output.block === "account") {
    return shown.account === null ? "" : output.value(shown.account);
  }
  return shown.customer === null ? "" : output.value(shown.customer);
}

/** COACTVW: the account view of transaction CAVW (COACTVWC). */
export function COACTVW(): ReactNode {
  const formId = useId();
  const fieldId = useId();
  const navigate = useNavigate();
  const view = useCoactvwcView();
  const [shown, setShown] = useState<Shown>(NOTHING_SHOWN);
  // The terminal inhibits input until the answer arrives. `locked` is the lock the key handlers
  // read at once; `pending` renders it: ENTER disabled, the key row and the form busy, the
  // account number read-only.
  const locked = useRef(false);
  const [pending, setPending] = useState(false);

  // The error row reflects the last ENTER only, so the schema re-validates on submit, not while
  // the operator types.
  const form = useForm<CoactvwValues>({
    resolver: zodResolver(coactvwSchema),
    defaultValues: { acctId: "" },
    reValidateMode: "onSubmit",
  });
  const { setFocus, setValue } = form;

  // The cursor stays in the account number after every response (COACTVWC-028). Selecting its
  // text lets the next number overtype it from the first position, as on the terminal.
  useEffect(() => {
    if (shown.responses > 0) {
      setFocus("acctId", { shouldSelect: true });
    }
  }, [shown.responses, setFocus]);

  /** Opens the lock ENTER took: the answer has been shown, or nothing was sent. */
  function unlock(): void {
    locked.current = false;
    setPending(false);
  }

  function onValid({ acctId }: CoactvwValues): void {
    const request = view.mutateAsync(
      // The field's 11 positions, as COACTVWC moves them into `CC-ACCT-ID PIC X(11)`: endpoint 4
      // edits the text exactly as typed (COACTVWC-005, COACTVWC-029), and an empty field would
      // leave an empty path segment, which addresses no endpoint.
      { path: { acct_id: acctId.padEnd(COACTVW_LENGTHS.acctId, " ") } },
      {
        onSuccess: (data) => {
          setShown((previous) => ({
            account: data,
            customer: data,
            error: null,
            responses: previous.responses + 1,
          }));
        },
        onError: (error) => {
          if (error.message === NO_INPUT_RECEIVED) {
            setValue("acctId", BLANK_FILTER_MARK);
          }
          // The blocks COACTVWC shows beside the message, as served; every other error clears
          // both (COACTVWC-008).
          const blocks = isViewError(error) ? error : { account: null, customer: null };
          setShown((previous) => ({
            account: blocks.account ?? null,
            customer: blocks.customer ?? null,
            error: { message: error.message, field: error.field ?? null },
            responses: previous.responses + 1,
          }));
        },
      },
    );
    // The lock opens once the answer has been shown, whichever way the request settles.
    void request.then(unlock, unlock);
  }

  // A partly filled field is not transmitted (MUSTFILL, COACTVW-013): nothing is sent, the blocks
  // stay, and the schema's message replaces the last answer's. No request is out, so the lock
  // ENTER took opens again.
  function onInvalid(): void {
    setShown((previous) => ({ ...previous, error: null }));
    unlock();
  }

  /**
   * ENTER, by the button, the Enter key or a key COACTVWC processes as ENTER. The lock is taken
   * here, before the schema's asynchronous check, so a key that arrives during the check or while
   * the view is out sends nothing and never reaches the browser's own form submission. A check
   * that fails unexpectedly opens the lock again, as no request was sent, and still reports its
   * error.
   */
  function submit(event?: BaseSyntheticEvent): void {
    if (locked.current) {
      event?.preventDefault();
      return;
    }
    locked.current = true;
    setPending(true);
    void form
      .handleSubmit(onValid, onInvalid)(event)
      .catch((error: unknown) => {
        unlock();
        throw error;
      });
  }

  const exit = useCallback(() => {
    // COMEN01C is the only program that transfers here, and PF3 returns to it on every visit
    // (COACTVWC-004).
    void navigate(PATHS.COMEN01);
  }, [navigate]);

  // Stable while `exit` and `pending` are, so PfKeyBar keeps its key binding across renders.
  // ENTER is refused while the view is out; F3 sends no request and stays available.
  const keys = useMemo<readonly PfKeyBarEntry[]>(
    () => [
      { ...ENTER_KEY, disabled: pending },
      ...keyRowItems(KEY_ROWS.COACTVW).map((item) =>
        item.key === "F3" ? { ...item, onPress: exit } : item,
      ),
    ],
    [exit, pending],
  );

  // Only ENTER and PF3 are valid: COACTVWC processes every other key as ENTER, and folds
  // PF13–PF24 onto PF1–PF12 (COACTVWC-003).
  const keyOptions: PfKeyOptions = {
    foldShifted: KEY_ROWS.COACTVW.foldShifted,
    onInvalidKey: () => {
      submit();
    },
  };

  // The program turns the field red when its message names it (COACTVWC-028); a MUSTFILL
  // rejection shows the same way. The message that names the field also describes it.
  const accountInError = shown.error?.field === ACCOUNT_FIELD;
  const accountMessage = accountInError ? (shown.error?.message ?? "") : "";
  const messageText = shown.error?.message ?? form.formState.errors.acctId?.message;

  return (
    <ScreenShell
      transaction={SCREENS.COACTVW.transaction}
      program={SCREENS.COACTVW.program}
      title="View Account"
      formId={formId}
      message={{ kind: "error", text: messageText }}
      info={PROMPT_FOR_INPUT}
      keys={keys}
      keyOptions={keyOptions}
      busy={pending}
    >
      <Form {...form}>
        <form id={formId} onSubmit={submit} noValidate aria-busy={pending}>
          <FieldGrid>
            <FormField
              control={form.control}
              name="acctId"
              render={({ field, fieldState }) => (
                // `contents` puts the label and the input on the grid themselves. The error row is
                // the map's one visible message, so the field's own message is for assistive
                // technology only, read with the input it marks: the schema's MUSTFILL message, or
                // the server's message that names the field, which the screen keeps outside
                // react-hook-form. It is the item's direct child, not a cell's: being `sr-only`,
                // it takes no grid cell, and as a grid item it is placed at the grid's origin,
                // inside the screen. The account number has no hint, so its control names no
                // description.
                <FormItem className="contents">
                  <GridCell row={5} col={19} span={16}>
                    {/*
                     * COACTVWC recolours the field, never its literal (COACTVWC-028), so the
                     * label keeps its colour when the field is in error.
                     */}
                    <FormLabel className={cn(LABEL_CLASS, "data-[error=true]:text-foreground")}>
                      Account Number :
                    </FormLabel>
                  </GridCell>
                  <GridCell row={5} col={38} span={COACTVW_LENGTHS.acctId}>
                    <FieldControl messaged={accountMessage !== ""}>
                      <Input
                        {...field}
                        readOnly={pending}
                        data-field="ACCTSID"
                        maxLength={COACTVW_LENGTHS.acctId}
                        size={COACTVW_LENGTHS.acctId}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        aria-invalid={accountInError || fieldState.invalid}
                        className={cn(
                          fieldBox(COACTVW_LENGTHS.acctId),
                          (accountInError || fieldState.invalid) && "text-destructive",
                        )}
                      />
                    </FieldControl>
                  </GridCell>
                  <FormMessage className="sr-only">{accountMessage}</FormMessage>
                </FormItem>
              )}
            />
            {ACCOUNT_OUTPUTS.map((output) => (
              <OutputCell
                key={output.name}
                output={output}
                id={`${fieldId}-${output.name}`}
                value={shownValue(output, shown)}
              />
            ))}
            <GridCell row={CUSTOMER_DETAILS_ROW} col={32} span={16}>
              <span
                role="heading"
                aria-level={2}
                className="flex h-7 items-center whitespace-pre text-foreground"
              >
                Customer Details
              </span>
            </GridCell>
            {CUSTOMER_OUTPUTS.map((output) => (
              <OutputCell
                key={output.name}
                output={output}
                id={`${fieldId}-${output.name}`}
                value={shownValue(output, shown)}
              />
            ))}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}

/**
 * One output field and its caption. The field is `ASKIP` on the map, so it is read-only and the
 * Tab key passes over it, as the terminal's cursor skips it.
 *
 * BLITZY [LAYOUT]: every input of the screen, this one and the account number, has the shared
 * field box of its map LENGTH (`fieldBox`): it spans its cell and is never narrower than its
 * characters, so the 1- to 5-character fields stay readable at every column pitch, and each box
 * ends within its field's columns, the column-78 `ACSPFLG` inside the frame.
 */
function OutputCell({
  output,
  id,
  value,
}: {
  output: OutputField;
  id: string;
  value: string;
}): ReactNode {
  return (
    <>
      {output.label !== undefined && (
        <GridCell row={output.label.row} col={output.label.col} span={output.label.length}>
          <Label htmlFor={id} className={LABEL_CLASS}>
            {output.label.text}
          </Label>
        </GridCell>
      )}
      <GridCell row={output.row} col={output.col} span={output.length}>
        <Input
          id={id}
          data-field={output.name}
          readOnly
          tabIndex={-1}
          size={output.length}
          value={value}
          aria-label={output.ariaLabel}
          className={fieldBox(output.length)}
        />
      </GridCell>
    </>
  );
}
