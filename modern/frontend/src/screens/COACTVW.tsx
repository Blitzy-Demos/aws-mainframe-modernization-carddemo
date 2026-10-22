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
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Form, FormControl, FormField, FormItem, FormLabel } from "@/components/ui/form";
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
 * answer is current, the schema's MUSTFILL message for a partly filled field. Values are shown
 * exactly as the endpoint sends them: amounts already edited, the SSN already redacted, dates as
 * their YYYY-MM-DD text. Nothing here logs, stores or copies a response.
 */

/** Map rows 1–4 (header rows, a blank row, the title), which `ScreenShell`'s header renders. */
const HEADER_ROWS = 4;

/*
 * BLITZY [LAYOUT]: the body grid holds map rows 5–20, the only rows with body fields, instead of
 * FieldGrid's default 24: its rows are equal flexible tracks, so the header and message rows the
 * shell already renders would otherwise stand as empty lines between the title and row 5 and below
 * row 20.
 */
const BODY_GRID_ROWS = "grid-rows-16";

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
 * keeps the padding that aligns the colons, and `h-full` centres the text on its grid line.
 */
const LABEL_CLASS = "h-full whitespace-pre text-foreground";

/*
 * BLITZY [LAYOUT]: an input spans its cell, and grows to its `size` (the map LENGTH in characters)
 * where the cell is narrower than that, so the 1- to 5-character fields stay readable at every
 * column pitch instead of being clipped by the input's padding.
 */
const FIELD_CLASS = "w-auto min-w-full";

/** ENTER, served by COACTVWC but unlabelled on the map, as a bare `ENTER` button (COACTVW-108). */
const ENTER_KEY: PfKeyBarEntry = { key: "ENTER", label: "ENTER", dark: false };

/** One literal of the map that captions an output field. */
type MapLabel = {
  readonly text: string;
  readonly row: number;
  readonly col: number;
  readonly length: number;
};

/** One output field of the map, its caption, and the response value it shows. */
type OutputField = {
  /** BMS field name, kept as `data-field`. */
  readonly name: string;
  readonly row: number;
  readonly col: number;
  readonly length: number;
  readonly value: (view: components["schemas"]["AccountViewResponse"]) => string;
} & (
  | { readonly label: MapLabel; readonly ariaLabel?: never }
  | { readonly label?: never; readonly ariaLabel: string }
);

/** A typed date (`format: date`) as its `YYYY-MM-DD` text; blank for `null` (COACTVWC-014, -019). */
function dateText(value: string | null | undefined): string {
  return value?.slice(0, 10) ?? "";
}

/**
 * The 28 output fields in macro order, each with the literal that captions it (COACTVW-015 to
 * COACTVW-094). The row-14 literals caption the three name fields below them. `ACSADL2` has no
 * literal: it is the second line of `Address:`, and its accessible name says so.
 */
const OUTPUT_FIELDS: readonly OutputField[] = [
  {
    name: "ACSTTUS",
    row: 5,
    col: 70,
    length: 1,
    label: { text: "Active Y/N: ", row: 5, col: 57, length: 12 },
    value: (view) => view.active_status,
  },
  {
    name: "ADTOPEN",
    row: 6,
    col: 17,
    length: 10,
    label: { text: "Opened:", row: 6, col: 8, length: 7 },
    value: (view) => dateText(view.open_date),
  },
  {
    name: "ACRDLIM",
    row: 6,
    col: 61,
    length: 15,
    label: { text: "Credit Limit        :", row: 6, col: 39, length: 21 },
    value: (view) => view.credit_limit,
  },
  {
    name: "AEXPDT",
    row: 7,
    col: 17,
    length: 10,
    label: { text: "Expiry:", row: 7, col: 8, length: 7 },
    value: (view) => dateText(view.expiration_date),
  },
  {
    name: "ACSHLIM",
    row: 7,
    col: 61,
    length: 15,
    label: { text: "Cash credit Limit   :", row: 7, col: 39, length: 21 },
    value: (view) => view.cash_credit_limit,
  },
  {
    name: "AREISDT",
    row: 8,
    col: 17,
    length: 10,
    label: { text: "Reissue:", row: 8, col: 8, length: 8 },
    value: (view) => dateText(view.reissue_date),
  },
  {
    name: "ACURBAL",
    row: 8,
    col: 61,
    length: 15,
    label: { text: "Current Balance     :", row: 8, col: 39, length: 21 },
    value: (view) => view.current_balance,
  },
  {
    name: "ACRCYCR",
    row: 9,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Credit:", row: 9, col: 39, length: 21 },
    value: (view) => view.current_cycle_credit,
  },
  {
    name: "AADDGRP",
    row: 10,
    col: 23,
    length: 10,
    label: { text: "Account Group:", row: 10, col: 8, length: 14 },
    value: (view) => view.group_id,
  },
  {
    name: "ACRCYDB",
    row: 10,
    col: 61,
    length: 15,
    label: { text: "Current Cycle Debit :", row: 10, col: 39, length: 21 },
    value: (view) => view.current_cycle_debit,
  },
  {
    name: "ACSTNUM",
    row: 12,
    col: 23,
    length: 9,
    label: { text: "Customer id  :", row: 12, col: 8, length: 14 },
    value: (view) => view.customer_id,
  },
  {
    name: "ACSTSSN",
    row: 12,
    col: 54,
    length: 12,
    label: { text: "SSN:", row: 12, col: 49, length: 4 },
    // Already `***-**-` and the last four digits (COACTVWC-017, E32); shown as received.
    value: (view) => view.ssn,
  },
  {
    name: "ACSTDOB",
    row: 13,
    col: 23,
    length: 10,
    label: { text: "Date of birth:", row: 13, col: 8, length: 14 },
    value: (view) => dateText(view.date_of_birth),
  },
  {
    name: "ACSTFCO",
    row: 13,
    col: 61,
    length: 3,
    label: { text: "FICO Score:", row: 13, col: 49, length: 11 },
    value: (view) => view.fico_score,
  },
  {
    name: "ACSFNAM",
    row: 15,
    col: 1,
    length: 25,
    label: { text: "First Name", row: 14, col: 1, length: 10 },
    value: (view) => view.first_name,
  },
  {
    name: "ACSMNAM",
    row: 15,
    col: 28,
    length: 25,
    label: { text: "Middle Name: ", row: 14, col: 28, length: 13 },
    value: (view) => view.middle_name,
  },
  {
    name: "ACSLNAM",
    row: 15,
    col: 55,
    length: 25,
    label: { text: "Last Name : ", row: 14, col: 55, length: 12 },
    value: (view) => view.last_name,
  },
  {
    name: "ACSADL1",
    row: 16,
    col: 10,
    length: 50,
    label: { text: "Address:", row: 16, col: 1, length: 8 },
    value: (view) => view.address_line_1,
  },
  {
    name: "ACSSTTE",
    row: 16,
    col: 73,
    length: 2,
    label: { text: "State ", row: 16, col: 63, length: 6 },
    value: (view) => view.state,
  },
  {
    name: "ACSADL2",
    row: 17,
    col: 10,
    length: 50,
    ariaLabel: "Address line 2",
    value: (view) => view.address_line_2,
  },
  {
    name: "ACSZIPC",
    row: 17,
    col: 73,
    length: 5,
    label: { text: "Zip", row: 17, col: 63, length: 3 },
    value: (view) => view.zip,
  },
  {
    name: "ACSCITY",
    row: 18,
    col: 10,
    length: 50,
    label: { text: "City ", row: 18, col: 1, length: 5 },
    value: (view) => view.city,
  },
  {
    name: "ACSCTRY",
    row: 18,
    col: 73,
    length: 3,
    label: { text: "Country", row: 18, col: 63, length: 7 },
    value: (view) => view.country,
  },
  {
    name: "ACSPHN1",
    row: 19,
    col: 10,
    length: 13,
    label: { text: "Phone 1:", row: 19, col: 1, length: 8 },
    value: (view) => view.phone_1,
  },
  {
    name: "ACSGOVT",
    row: 19,
    col: 58,
    length: 20,
    label: { text: "Government Issued Id Ref    : ", row: 19, col: 24, length: 30 },
    value: (view) => view.government_id,
  },
  {
    name: "ACSPHN2",
    row: 20,
    col: 10,
    length: 13,
    label: { text: "Phone 2:", row: 20, col: 1, length: 8 },
    value: (view) => view.phone_2,
  },
  {
    name: "ACSEFTC",
    row: 20,
    col: 41,
    length: 10,
    label: { text: "EFT Account Id: ", row: 20, col: 24, length: 16 },
    value: (view) => view.eft_account_id,
  },
  {
    name: "ACSPFLG",
    row: 20,
    col: 78,
    length: 1,
    label: { text: "Primary Card Holder Y/N:", row: 20, col: 53, length: 24 },
    value: (view) => view.primary_card_holder,
  },
];

/** A map field at its `POS` and `LENGTH`, on the body grid that starts at map row 5. */
function MapCell({
  row,
  col,
  span,
  children,
}: {
  row: number;
  col: number;
  span: number;
  children: ReactNode;
}): ReactNode {
  return (
    <GridCell row={row - HEADER_ROWS} col={col} span={span}>
      {children}
    </GridCell>
  );
}

/** The answer the screen shows: the last response of endpoint 4, as COACTVWC sent it. */
type Shown = {
  /** The account and customer blocks; `null` before any view and after every error. */
  readonly view: components["schemas"]["AccountViewResponse"] | null;
  /** The program's message and the field it names; `null` after a view. */
  readonly error: { readonly message: string; readonly field: string | null } | null;
  /** How many responses have been shown, so focus returns to the field after each one. */
  readonly responses: number;
};

const NOTHING_SHOWN: Shown = { view: null, error: null, responses: 0 };

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
            view: data,
            error: null,
            responses: previous.responses + 1,
          }));
        },
        onError: (error) => {
          if (error.message === NO_INPUT_RECEIVED) {
            setValue("acctId", BLANK_FILTER_MARK);
          }
          setShown((previous) => ({
            view: null,
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
  // rejection shows the same way.
  const accountInError = shown.error?.field === ACCOUNT_FIELD;
  const messageText = shown.error?.message ?? form.formState.errors.acctId?.message;
  const { view: account } = shown;

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
          <FieldGrid className={BODY_GRID_ROWS}>
            <FormField
              control={form.control}
              name="acctId"
              render={({ field, fieldState }) => (
                // `contents` puts the label and the input on the grid themselves.
                <FormItem className="contents">
                  <MapCell row={5} col={19} span={16}>
                    {/*
                     * COACTVWC recolours the field, never its literal (COACTVWC-028), so the
                     * label keeps its colour when the field is in error.
                     */}
                    <FormLabel className={cn(LABEL_CLASS, "data-[error=true]:text-foreground")}>
                      Account Number :
                    </FormLabel>
                  </MapCell>
                  <MapCell row={5} col={38} span={COACTVW_LENGTHS.acctId}>
                    <FormControl>
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
                          FIELD_CLASS,
                          (accountInError || fieldState.invalid) && "text-destructive",
                        )}
                      />
                    </FormControl>
                  </MapCell>
                </FormItem>
              )}
            />
            <MapCell row={11} col={32} span={16}>
              <span
                role="heading"
                aria-level={2}
                className="flex h-full items-center whitespace-pre text-foreground"
              >
                Customer Details
              </span>
            </MapCell>
            {OUTPUT_FIELDS.map((output) => (
              <OutputCell
                key={output.name}
                output={output}
                id={`${fieldId}-${output.name}`}
                value={account === null ? "" : output.value(account)}
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
        <MapCell row={output.label.row} col={output.label.col} span={output.label.length}>
          <Label htmlFor={id} className={LABEL_CLASS}>
            {output.label.text}
          </Label>
        </MapCell>
      )}
      <MapCell row={output.row} col={output.col} span={output.length}>
        <Input
          id={id}
          data-field={output.name}
          readOnly
          tabIndex={-1}
          size={output.length}
          value={value}
          aria-label={output.ariaLabel}
          className={FIELD_CLASS}
        />
      </MapCell>
    </>
  );
}
