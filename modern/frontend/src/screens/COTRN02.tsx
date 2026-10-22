import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useForm, type Control, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCotrn02cAdd } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
// `@/lib/header` is the one home of the CSMSG01Y messages every screen shows (AAP 0.5.4), so the
// invalid-key text is taken from there rather than restated here.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import {
  COTRN02_LENGTHS,
  cotrn02KeySchema,
  cotrn02Schema,
  type Cotrn02Values,
} from "@/schemas/COTRN02";

/*
 * Add Transaction, map `COTRN2A` of `app/bms/COTRN02.bms`: transaction CT02, program COTRN02C,
 * route `/transactions/add`, endpoint 13 `POST /api/v1/transactions` (AAP 0.3.4.1, 0.3.5).
 *
 * The screen sends what it shows and shows what the program answers; every edit is the server's
 * (`modern/audit/online/COTRN02C.md`), and the zod schema repeats only the map lengths.
 */

/** One input of the form, by its `Cotrn02Values` member. */
type FieldName = keyof Cotrn02Values;

/** A `DFHMDF` literal as `modern/audit/screens.md` places it: `POS=(row,col)` and `LENGTH`. */
interface MapText {
  readonly row: number;
  readonly col: number;
  readonly span: number;
  readonly text: string;
}

/**
 * One unprotected map field with its label, and the format hint the map prints for it. The
 * input's `LENGTH` is `COTRN02_LENGTHS[name]`; its row is the label's.
 */
interface MapInput {
  readonly name: FieldName;
  readonly col: number;
  readonly label: MapText;
  readonly hint?: MapText & { readonly neutral?: boolean };
}

/*
 * The fourteen inputs in map order, which is also the tab order: `COTRN02-012` to `COTRN02-059`
 * of `modern/audit/screens.md` (`app/bms/COTRN02.bms:80-292`). Each label is TURQUOISE, the amount
 * and date hints BLUE, and `(Y/N)` NEUTRAL. The `(or)` literal stands between the two keys.
 */
const ACCOUNT_INPUT: MapInput = {
  name: "acctId",
  col: 21,
  label: { row: 6, col: 6, span: 13, text: "Enter Acct #:" },
};
const CARD_INPUT: MapInput = {
  name: "cardNum",
  col: 55,
  label: { row: 6, col: 46, span: 7, text: "Card #:" },
};
const DATA_INPUTS: readonly MapInput[] = [
  { name: "typeCode", col: 15, label: { row: 10, col: 6, span: 8, text: "Type CD:" } },
  { name: "categoryCode", col: 36, label: { row: 10, col: 23, span: 12, text: "Category CD:" } },
  { name: "source", col: 54, label: { row: 10, col: 46, span: 7, text: "Source:" } },
  { name: "description", col: 19, label: { row: 12, col: 6, span: 12, text: "Description:" } },
  {
    name: "amount",
    col: 14,
    label: { row: 14, col: 6, span: 7, text: "Amount:" },
    hint: { row: 15, col: 13, span: 14, text: "(-99999999.99)" },
  },
  {
    name: "origDate",
    col: 42,
    label: { row: 14, col: 31, span: 10, text: "Orig Date:" },
    hint: { row: 15, col: 41, span: 12, text: "(YYYY-MM-DD)" },
  },
  {
    name: "procDate",
    col: 68,
    label: { row: 14, col: 57, span: 10, text: "Proc Date:" },
    hint: { row: 15, col: 67, span: 12, text: "(YYYY-MM-DD)" },
  },
  { name: "merchantId", col: 19, label: { row: 16, col: 6, span: 12, text: "Merchant ID:" } },
  { name: "merchantName", col: 48, label: { row: 16, col: 33, span: 14, text: "Merchant Name:" } },
  { name: "merchantCity", col: 21, label: { row: 18, col: 6, span: 14, text: "Merchant City:" } },
  { name: "merchantZip", col: 67, label: { row: 18, col: 53, span: 13, text: "Merchant Zip:" } },
  {
    name: "confirm",
    col: 63,
    label: {
      row: 21,
      col: 6,
      span: 55,
      text: "You are about to add this transaction. Please confirm :",
    },
    hint: { row: 21, col: 66, span: 5, text: "(Y/N)", neutral: true },
  },
];
const INPUTS: readonly MapInput[] = [ACCOUNT_INPUT, CARD_INPUT, ...DATA_INPUTS];

/** `(or)` between the two keys, NEUTRAL (`COTRN02-015`). */
const OR_TEXT: MapText = { row: 6, col: 37, span: 4, text: "(or)" };

/** The NEUTRAL rule under the keys (`COTRN02-019`), decorative. */
const RULE_TEXT: MapText = { row: 8, col: 6, span: 70, text: "-".repeat(70) };

/** Every input empty: the first send (`COTRN02C-002`), F4 (`COTRN02C-005`) and after the add. */
const EMPTY: Cotrn02Values = {
  acctId: "",
  cardNum: "",
  typeCode: "",
  categoryCode: "",
  source: "",
  description: "",
  amount: "",
  origDate: "",
  procDate: "",
  merchantId: "",
  merchantName: "",
  merchantCity: "",
  merchantZip: "",
  confirm: "",
};

/**
 * Wire name of a request member, as `TranAddResult.field` and `ScreenError.field` name it, → the
 * input the program's cursor goes to. A length rejection names a data field as `fields.<name>`.
 */
const INPUT_OF_MEMBER: Readonly<Record<string, FieldName>> = {
  acct_id: "acctId",
  card_num: "cardNum",
  type_cd: "typeCode",
  cat_cd: "categoryCode",
  source: "source",
  description: "description",
  amount: "amount",
  orig_date: "origDate",
  proc_date: "procDate",
  merchant_id: "merchantId",
  merchant_name: "merchantName",
  merchant_city: "merchantCity",
  merchant_zip: "merchantZip",
  confirm: "confirm",
};

/** Row 24, `ENTER=Continue  F3=Back  F4=Clear  F5=Copy Last Tran.` (`COTRN02-061`). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COTRN02);

/** Every route a screen can have come from; any other `from` falls back to the main menu. */
const SCREEN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/*
 * TURQUOISE labels and NEUTRAL literals take the foreground token, BLUE hints keep the muted
 * foreground of `FormDescription` (AAP 0.4.3). `h-full` with `items-center` centres each text on
 * its grid line beside the inputs; `whitespace-pre` keeps a literal on one line, as on the map.
 */
const LABEL = "h-full whitespace-pre text-foreground";
const HINT = "flex h-full items-center whitespace-pre";
const STATIC = "flex h-full items-center whitespace-pre text-foreground";

/*
 * BLITZY [LAYOUT]: inputs use `px-1` instead of the Input default `px-3`, so a field shows its
 * map LENGTH of characters inside the cell of that many grid columns. The 1-, 2- and 4-character
 * fields (CONFIRM, TTYPCD, TCATCD) also get a minimum width on the spacing scale, because their
 * cell is narrower than their characters plus padding; the extra width falls on columns the map
 * leaves empty after each of them.
 */
const INPUT = "px-1";
const MIN_WIDTH: Readonly<Partial<Record<number, string>>> = {
  1: "min-w-6",
  2: "min-w-8",
  4: "min-w-12",
};

/** The Input classes of a field `length` characters long. */
function inputClass(length: number): string {
  const minWidth = MIN_WIDTH[length];
  return minWidth === undefined ? INPUT : `${INPUT} ${minWidth}`;
}

/** Blank as COBOL tests it: empty or only spaces (`SPACES` or `LOW-VALUES` on the map). */
function isBlank(value: string): boolean {
  return value.replaceAll(" ", "") === "";
}

/** The input a response or error names, or `undefined` for none or an unknown member. */
function inputOfMember(member: string | null | undefined): FieldName | undefined {
  if (member === undefined || member === null) {
    return undefined;
  }
  const name = member.startsWith("fields.") ? member.slice("fields.".length) : member;
  return Object.hasOwn(INPUT_OF_MEMBER, name) ? INPUT_OF_MEMBER[name] : undefined;
}

/** A typed date of the response as the 10-character `YYYY-MM-DD` text of its map field. */
function dateText(date: string | null | undefined): string {
  return date === undefined || date === null ? "" : date.slice(0, 10);
}

/** Where F3 goes: the route that transferred here, else the main menu (`COTRN02C-004`). */
function backPath(state: unknown): string {
  if (typeof state === "object" && state !== null && "from" in state) {
    const { from } = state;
    if (typeof from === "string" && SCREEN_PATHS.has(from)) {
      return from;
    }
  }
  return PATHS.COMEN01;
}

/**
 * The endpoint 13 body for `aid`: the account and card only when not blank, the eleven data
 * fields and the confirmation exactly as typed (`COTRN02C-060`). The dates and the amount are the
 * raw text COTRN02C edits (the raw-edit-input exception of Typed Dates and Timestamps).
 *
 * Cardholder Data Redaction: a card number the program filled in arrives redacted and is never
 * sent back; only a card number the user typed is request input. While the card field still holds
 * the filled-in value, the key is the account the program filled beside it.
 */
function requestBody(
  aid: components["schemas"]["Cotrn02cAid"],
  values: Cotrn02Values,
  filledCard: string | undefined,
): components["schemas"]["TranAddRequest"] {
  const typedCard = !isBlank(values.cardNum) && values.cardNum !== filledCard;
  return {
    aid,
    acct_id: isBlank(values.acctId) ? undefined : values.acctId,
    card_num: typedCard ? values.cardNum : undefined,
    fields: {
      type_cd: values.typeCode,
      cat_cd: values.categoryCode,
      source: values.source,
      description: values.description,
      amount: values.amount,
      orig_date: values.origDate,
      proc_date: values.procDate,
      merchant_id: values.merchantId,
      merchant_name: values.merchantName,
      merchant_city: values.merchantCity,
      merchant_zip: values.merchantZip,
    },
    confirm: values.confirm,
  };
}

/**
 * What the screen shows after the confirmation prompt (`COTRN02C-041`, `COTRN02C-056`,
 * `COTRN02C-057`): the account and the redacted card the program resolved, and the data fields
 * as edited or copied, trailing spaces and the re-edited amount included; the dates are typed and
 * shown as their `YYYY-MM-DD` text. The confirmation keeps what was sent.
 */
function promptValues(
  result: components["schemas"]["TranAddResult"],
  sent: Cotrn02Values,
): Cotrn02Values {
  const copied = result.copied ?? undefined;
  return {
    acctId: result.acct_id ?? sent.acctId,
    cardNum: result.card_num ?? sent.cardNum,
    typeCode: copied?.type_cd ?? sent.typeCode,
    categoryCode: copied?.cat_cd ?? sent.categoryCode,
    source: copied?.source ?? sent.source,
    description: copied?.description ?? sent.description,
    amount: copied?.amount ?? sent.amount,
    origDate: copied === undefined ? sent.origDate : dateText(copied.orig_date),
    procDate: copied === undefined ? sent.procDate : dateText(copied.proc_date),
    merchantId: copied?.merchant_id ?? sent.merchantId,
    merchantName: copied?.merchant_name ?? sent.merchantName,
    merchantCity: copied?.merchant_city ?? sent.merchantCity,
    merchantZip: copied?.merchant_zip ?? sent.merchantZip,
    confirm: sent.confirm,
  };
}

/** Props of {@link MapField}. */
interface MapFieldProps {
  readonly input: MapInput;
  readonly control: Control<Cotrn02Values>;
}

/**
 * One input with its label and hint, each in the grid cell of its map field. `FormItem` is
 * `contents`, so its cells sit on the screen's `FieldGrid` while the label and hint stay bound to
 * the input (`htmlFor`, `aria-describedby`).
 */
function MapField({ input, control }: MapFieldProps): ReactNode {
  const length = COTRN02_LENGTHS[input.name];
  const { label, hint } = input;
  return (
    <FormField
      control={control}
      name={input.name}
      render={({ field }) => (
        <FormItem className="contents">
          <GridCell row={label.row} col={label.col} span={label.span}>
            <FormLabel className={LABEL}>{label.text}</FormLabel>
          </GridCell>
          <GridCell row={label.row} col={input.col} span={length}>
            <FormControl>
              <Input
                {...field}
                maxLength={length}
                autoFocus={input.name === "acctId"}
                autoComplete="off"
                spellCheck={false}
                className={inputClass(length)}
              />
            </FormControl>
          </GridCell>
          {hint !== undefined && (
            <GridCell row={hint.row} col={hint.col} span={hint.span}>
              <FormDescription className={hint.neutral === true ? STATIC : HINT}>
                {hint.text}
              </FormDescription>
            </GridCell>
          )}
        </FormItem>
      )}
    />
  );
}

/**
 * The Add Transaction screen.
 *
 * - ENTER sends the screen with `aid` ENTER: COTRN02C edits the key and the data, prompts for the
 *   confirmation, and adds the transaction once it is `Y` (`COTRN02C-009` to `COTRN02C-051`).
 * - F5 sends it with `aid` PF5: the program copies the highest-ID transaction into the data fields
 *   and runs ENTER on them (`COTRN02C-052` to `COTRN02C-056`); only the key is checked here.
 * - F4 empties the screen and F3 goes back, both without a request (`COTRN02C-004`,
 *   `COTRN02C-005`); any other F-key shows the invalid-key message (`COTRN02C-006`).
 *
 * A 200 shows the response's fields and message; a `ScreenError` keeps every field shown and
 * displays its message (`COTRN02C-060`). The cursor goes to the field the response names. Every
 * message is red but the success message, which is green (`COTRN02C-058`). While a request is
 * outstanding the keys do nothing, as the terminal keyboard stays locked until the program
 * answers; F3 still leaves.
 */
export function COTRN02(): ReactNode {
  const formId = useId();
  const navigate = useNavigate();
  const location = useLocation();
  const add = useCotrn02cAdd();
  const form = useForm<Cotrn02Values>({
    resolver: zodResolver(cotrn02Schema),
    defaultValues: EMPTY,
  });
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);
  // The redacted card number the program last filled in; never sent back (`requestBody`).
  const filledCard = useRef<string | undefined>(undefined);
  // Set from the send until the answer, so a second key press cannot slip in before re-render.
  const pending = useRef(false);
  // The input the program's cursor goes to. Focus is applied after the render that follows, because
  // `form.reset` drops the field references until the inputs render again.
  const [cursor, setCursor] = useState<{ readonly name: FieldName } | undefined>(undefined);
  const { setFocus } = form;
  useEffect(() => {
    if (cursor !== undefined) {
      setFocus(cursor.name);
    }
  }, [cursor, setFocus]);

  function focus(name: FieldName | undefined): void {
    if (name !== undefined) {
      setCursor({ name });
    }
  }

  function showError(text: string, name: FieldName | undefined): void {
    setMessage({ text, kind: "error" });
    focus(name);
  }

  function showResult(result: components["schemas"]["TranAddResult"], sent: Cotrn02Values): void {
    const added = typeof result.new_tran_id === "string";
    if (added) {
      // `INITIALIZE-ALL-FIELDS`: every input empty, the message green (`COTRN02C-049`).
      filledCard.current = undefined;
      form.reset(EMPTY);
    } else {
      filledCard.current = result.card_num ?? filledCard.current;
      form.reset(promptValues(result, sent));
    }
    setMessage({ text: result.message, kind: added ? "info" : "error" });
    focus(inputOfMember(result.field) ?? "acctId");
  }

  function send(aid: components["schemas"]["Cotrn02cAid"], values: Cotrn02Values): void {
    if (pending.current) {
      return;
    }
    pending.current = true;
    add.mutate(
      { body: requestBody(aid, values, filledCard.current) },
      {
        onSuccess: (result) => {
          showResult(result, values);
        },
        onError: (error) => {
          showError(error.message, inputOfMember(error.field));
        },
        onSettled: () => {
          pending.current = false;
        },
      },
    );
  }

  /** The first map-length error in map order, shown in the message row; no request is sent. */
  function showInvalid(errors: FieldErrors<Cotrn02Values>): void {
    const first = INPUTS.find((input) => errors[input.name]?.message !== undefined);
    if (first !== undefined) {
      showError(errors[first.name]?.message ?? "", first.name);
    }
  }

  const submit = form.handleSubmit((values) => {
    send("ENTER", values);
  }, showInvalid);

  function copyLast(): void {
    if (pending.current) {
      return;
    }
    const values = form.getValues();
    const keys = cotrn02KeySchema.safeParse({ acctId: values.acctId, cardNum: values.cardNum });
    if (!keys.success) {
      const issue = keys.error.issues.at(0);
      showError(issue?.message ?? "", issue?.path.at(0) === "cardNum" ? "cardNum" : "acctId");
      return;
    }
    form.clearErrors();
    send("PF5", values);
  }

  function clear(): void {
    if (pending.current) {
      return;
    }
    filledCard.current = undefined;
    add.reset();
    form.reset(EMPTY);
    setMessage(undefined);
    focus("acctId");
  }

  function back(): void {
    void navigate(backPath(location.state), { state: { from: SCREENS.COTRN02.path } });
  }

  function invalidKey(): void {
    if (!pending.current) {
      setMessage({ text: INVALID_KEY, kind: "error" });
    }
  }

  const actions: PfKeyHandlers = { F3: back, F4: clear, F5: copyLast };
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({ ...item, onPress: actions[item.key] }));
  const keyOptions: PfKeyOptions = {
    foldShifted: KEY_ROWS.COTRN02.foldShifted,
    onInvalidKey: invalidKey,
  };

  return (
    <ScreenShell
      transaction={SCREENS.COTRN02.transaction}
      program={SCREENS.COTRN02.program}
      title="Add Transaction"
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={keyOptions}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={add.isPending}
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <FieldGrid>
            <MapField input={ACCOUNT_INPUT} control={form.control} />
            <GridCell row={OR_TEXT.row} col={OR_TEXT.col} span={OR_TEXT.span}>
              <span className={STATIC}>{OR_TEXT.text}</span>
            </GridCell>
            <MapField input={CARD_INPUT} control={form.control} />
            <GridCell row={RULE_TEXT.row} col={RULE_TEXT.col} span={RULE_TEXT.span}>
              <span aria-hidden="true" className={STATIC}>
                {RULE_TEXT.text}
              </span>
            </GridCell>
            {DATA_INPUTS.map((input) => (
              <MapField key={input.name} input={input} control={form.control} />
            ))}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
