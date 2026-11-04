// CT02 transaction add: the COTRN02 "Add Transaction" screen of COTRN02C
// [app/bms/COTRN02.bms] [app/cbl/COTRN02C.cbl], served at /transactions/new and
// sending POST /transactions.
//
// The 3270 keys are the controls of docs/parity/function-map.md:
// - ENTER is the form's submit "Add transaction" (F-COTRN02-001). Enter in any
//   field submits; handleSubmit runs the schema that mirrors
//   VALIDATE-INPUT-KEY-FIELDS and VALIDATE-INPUT-DATA-FIELDS, so input it
//   rejects never reaches the network, and a valid entry opens the confirm
//   dialog that replaces the CONFIRM field.
// - CONFIRM Y is the dialog's "Confirm" (F-COTRN02-002), which sends the add
//   with `confirm: "Y"`; CONFIRM N is its "Cancel" (F-COTRN02-003), and Escape,
//   which send nothing.
// - F3=Back is the link "Back" to the transaction list (F-COTRN02-004), the
//   parent route the map's PF3 rule names.
// - F4=Clear is "Reset" (F-COTRN02-005): every field blanked, cursor on the
//   account number, as CLEAR-CURRENT-SCREEN and INITIALIZE-ALL-FIELDS do.
// - F5=Copy Last Tran. is "Copy last transaction" (F-COTRN02-006), the whole of
//   COPY-LAST-TRAN-DATA described at `copyLastTransaction` below.
// - Other AID keys have no control (F-COTRN02-007, E-56).
//
// The cross-reference look-ups, the CSUTLDTC calendar checks, the TRAN-ID
// allocation and the write are the server's; so is the reward accrual, which
// commits in the same database transaction as the insert and rolls back with
// it, so a failed accrual reaches this page as the 500 of the add.
import {
  Fragment,
  useEffect,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { Link } from "react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm, type Control } from "react-hook-form";

import { ApiError } from "@/api/client";
import {
  useCopyLastTransactionMutation,
  useTransactionAddMutation,
} from "@/api/queries/transactions";
import type { components } from "@/api/schema";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSeparator,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { applyApiError, showMessage, showSuccess } from "@/lib/api-errors";
import {
  transactionAddBmsFieldMap,
  transactionAddFormSchema,
  transactionToAddFormValues,
  type TransactionAddFormValues,
} from "@/schemas/transactions";

type TransactionAddField = components["schemas"]["TransactionAddField"];

/** A COTRN02 input field. CONFIRM has none: the confirm dialog replaces it. */
type InputField = Exclude<TransactionAddField, "CONFIRM">;

/** One COTRN02 input; its form key is `transactionAddBmsFieldMap[bms]`. */
type FieldConfig = {
  readonly bms: InputField;
  /** The BMS caption without its colon. */
  readonly label: string;
  /** The `PIC X(n)` width of the BMS input [app/cpy-bms/COTRN02.CPY]. */
  readonly maxLength: number;
  /** Digits only: a numeric keypad on touch devices. */
  readonly inputMode?: "numeric";
  readonly placeholder?: string;
  /** The hint the map prints beside the input. */
  readonly description?: string;
};

/** A group of COTRN02 inputs, in screen order, rendered as one fieldset. */
type FieldSetConfig = {
  readonly legend: string;
  /** Shown under the legend and linked to every input of the group. */
  readonly description?: string;
  readonly fields: readonly [FieldConfig, ...FieldConfig[]];
};

/** The COTRN02 date hint, the CSUTLDTC format TORIGDT and TPROCDT ask for. */
const DATE_FORMAT = "YYYY-MM-DD";

/**
 * The COTRN02 inputs in screen order [app/bms/COTRN02.bms:L84-266]. The dates
 * are text inputs, as there is no date picker (AAP gap inventory); whether a
 * `YYYY-MM-DD` text is a calendar date stays the server's CSUTLDTC check.
 */
const FIELD_SETS: readonly FieldSetConfig[] = [
  {
    legend: "Account or card",
    // The map's '(or)' between the keys, and VALIDATE-INPUT-KEY-FIELDS'
    // 'Account or Card Number must be entered...' [app/cbl/COTRN02C.cbl:L224-229].
    description: "Enter an account number or a card number.",
    fields: [
      { bms: "ACTIDIN", label: "Enter Acct #", maxLength: 11, inputMode: "numeric" },
      { bms: "CARDNIN", label: "Card #", maxLength: 16, inputMode: "numeric" },
    ],
  },
  {
    legend: "Transaction",
    fields: [
      { bms: "TTYPCD", label: "Type CD", maxLength: 2, inputMode: "numeric" },
      { bms: "TCATCD", label: "Category CD", maxLength: 4, inputMode: "numeric" },
      { bms: "TRNSRC", label: "Source", maxLength: 10 },
      { bms: "TDESC", label: "Description", maxLength: 60 },
      // The map's '(-99999999.99)', the form the amount edit asks for
      // [app/cbl/COTRN02C.cbl:L337-348].
      { bms: "TRNAMT", label: "Amount", maxLength: 12, description: "(-99999999.99)" },
    ],
  },
  {
    legend: "Dates",
    fields: [
      {
        bms: "TORIGDT",
        label: "Orig Date",
        maxLength: 10,
        inputMode: "numeric",
        placeholder: DATE_FORMAT,
        description: `(${DATE_FORMAT})`,
      },
      {
        bms: "TPROCDT",
        label: "Proc Date",
        maxLength: 10,
        inputMode: "numeric",
        placeholder: DATE_FORMAT,
        description: `(${DATE_FORMAT})`,
      },
    ],
  },
  {
    legend: "Merchant",
    fields: [
      { bms: "MID", label: "Merchant ID", maxLength: 9, inputMode: "numeric" },
      { bms: "MNAME", label: "Merchant Name", maxLength: 30 },
      { bms: "MCITY", label: "Merchant City", maxLength: 25 },
      { bms: "MZIP", label: "Merchant Zip", maxLength: 10 },
    ],
  },
];

/**
 * INITIALIZE-ALL-FIELDS [app/cbl/COTRN02C.cbl:L758-778]: every input SPACES,
 * which a browser field holds as "", CONFIRMI included. A blank confirmation
 * passes the form schema; the dialog's "Confirm" sends `Y` in its place.
 */
const EMPTY_VALUES: TransactionAddFormValues = {
  account_id: "",
  card_number: "",
  tran_type_cd: "",
  tran_cat_cd: "",
  tran_source: "",
  tran_desc: "",
  tran_amt: "",
  tran_orig_date: "",
  tran_proc_date: "",
  tran_merchant_id: "",
  tran_merchant_name: "",
  tran_merchant_city: "",
  tran_merchant_zip: "",
  confirm: "",
};

/** The form keys of the two key fields VALIDATE-INPUT-KEY-FIELDS edits. */
const KEY_FIELDS = [
  transactionAddBmsFieldMap.ACTIDIN,
  transactionAddBmsFieldMap.CARDNIN,
] as const;

/** The BMS names of the key fields, as an error's `fields[]` carries them. */
const KEY_BMS_FIELDS: ReadonlySet<string> = new Set<InputField>(["ACTIDIN", "CARDNIN"]);

// ACTIDIN carries the map's IC attribute, and every successful add and every
// clear moves the cursor back to it (MOVE -1 TO ACTIDINL).
const INITIAL_FIELD = transactionAddBmsFieldMap.ACTIDIN;

/** CONFIRMI 'Y', the value ADD-TRANSACTION runs on [app/cbl/COTRN02C.cbl:L169-172]. */
const CONFIRM_YES = "Y";

/**
 * CONFIRMI SPACES: PROCESS-ENTER-KEY runs every edit and, when all pass,
 * answers 'Confirm to add this transaction...' without writing
 * [app/cbl/COTRN02C.cbl:L173-181].
 */
const CONFIRM_BLANK = "";

/** The status of a completed add: the transaction written, its accrual committed. */
const CREATED = 201;

/** The map's confirmation caption, without the CONFIRM field it led into. */
const CONFIRM_PROMPT = "You are about to add this transaction. Please confirm.";

/**
 * Whether a PF5 check was answered with an edit COTRN02C makes after the key
 * fields: an error naming fields, none of them ACTIDIN or CARDNIN. Only then
 * did COPY-LAST-TRAN-DATA get past VALIDATE-INPUT-KEY-FIELDS and copy the
 * record, since SEND-TRNADD-SCREEN ends the task at the first failing check
 * [app/cbl/COTRN02C.cbl:L471-499] [app/cbl/COTRN02C.cbl:L516-535]. A key
 * rejection, an error without fields or a failed request leaves the screen
 * as it was.
 */
function passedKeyFields(error: unknown): boolean {
  if (!(error instanceof ApiError)) {
    return false;
  }
  const fields = error.body.fields ?? [];
  return fields.length > 0 && fields.every(({ field }) => !KEY_BMS_FIELDS.has(field));
}

/** The entries the confirm dialog repeats, as entered. Blank ones are left out. */
function describeTransaction(values: TransactionAddFormValues): ReactNode {
  const entries = [
    { term: "Account", value: values.account_id },
    { term: "Card", value: values.card_number },
    { term: "Type CD", value: values.tran_type_cd },
    { term: "Category CD", value: values.tran_cat_cd },
    { term: "Amount", value: values.tran_amt },
  ].filter(({ value }) => value.trim() !== "");
  return (
    <div className="flex flex-col gap-2">
      <p>{CONFIRM_PROMPT}</p>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1">
        {entries.map(({ term, value }) => (
          <div key={term} className="contents">
            <dt className="font-medium text-foreground">{term}</dt>
            <dd className="font-mono break-all">{value}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

type TextFieldProps = {
  control: Control<TransactionAddFormValues>;
  config: FieldConfig;
  /** The id of the group description the input is also described by. */
  groupDescriptionId?: string;
  /**
   * The keyboard lock of a running PF5 copy or confirmed add: the input keeps
   * its value, and stays focusable, so the cursor is not lost.
   */
  readOnly: boolean;
};

/**
 * One COTRN02 input. The Controller's `field`, its `ref` included, goes to the
 * Input, so a client or server error focuses it, as the COBOL `MOVE -1 TO
 * <field>L` places the cursor.
 */
function TextField({
  control,
  config,
  groupDescriptionId,
  readOnly,
}: TextFieldProps): JSX.Element {
  const { bms, label, maxLength, inputMode, placeholder, description } = config;
  const descriptionId = description === undefined ? undefined : `${bms}-description`;
  const errorId = `${bms}-error`;
  return (
    <Controller
      control={control}
      name={transactionAddBmsFieldMap[bms]}
      render={({ field, fieldState }) => {
        const describedBy = [
          groupDescriptionId,
          descriptionId,
          fieldState.invalid ? errorId : undefined,
        ]
          .filter((id) => id !== undefined)
          .join(" ");
        return (
          <Field data-invalid={fieldState.invalid}>
            <FieldLabel htmlFor={bms}>{label}</FieldLabel>
            <Input
              {...field}
              id={bms}
              value={field.value ?? ""}
              maxLength={maxLength}
              inputMode={inputMode}
              placeholder={placeholder}
              autoComplete="off"
              readOnly={readOnly}
              aria-invalid={fieldState.invalid}
              aria-describedby={describedBy === "" ? undefined : describedBy}
            />
            {description === undefined ? null : (
              <FieldDescription id={descriptionId}>{description}</FieldDescription>
            )}
            <FieldError id={errorId} errors={[fieldState.error]} />
          </Field>
        );
      }}
    />
  );
}

export default function TransactionAddPage(): JSX.Element {
  const form = useForm<TransactionAddFormValues>({
    resolver: zodResolver(transactionAddFormSchema),
    defaultValues: EMPTY_VALUES,
  });
  const add = useTransactionAddMutation();
  // PF5's PROCESS-ENTER-KEY: the add sent with a blank confirmation, which
  // the server answers with its first failing edit and never writes. Its own
  // instance, so the dialog's add keeps a state of its own.
  const check = useTransactionAddMutation();
  const copyLast = useCopyLastTransactionMutation();
  const submitRef = useRef<HTMLButtonElement>(null);

  // The entry a valid ENTER produced, kept while the dialog closes so its
  // description does not empty during the closing animation.
  const [confirmation, setConfirmation] = useState<TransactionAddFormValues | null>(null);
  const [confirming, setConfirming] = useState(false);

  // The keyboard lock, as the 3270 keyboard locks while the task runs: from a
  // PF5 press until its key edits, read and check have all answered
  // (`copying`), and while a confirmed add runs. The entry stays the one its
  // request was built from, so the copy, the clear or the field errors of the
  // answer land on that entry:
  // - every input is read-only, keeping its value and, unlike a disabled
  //   input, the focus it holds;
  // - Reset is disabled;
  // - ENTER is ignored before the edits run, so it neither marks a field nor
  //   opens the confirm dialog;
  // - the submit is disabled during PF5 only. During the add, the closed
  //   dialog gives the focus back to its opener (the submit, or the input
  //   ENTER was pressed in), and a disabled submit would leave it on the body;
  // - "Copy last transaction" stays enabled, since it holds the focus during
  //   PF5, and ignores presses instead.
  // `copyingRef` changes with `copying`, so a second press before the next
  // render cannot start a second copy.
  const [copying, setCopying] = useState(false);
  const copyingRef = useRef(false);
  const busy = copying || add.isPending;
  // Set when a PF5 check answers with the confirmation prompt alone. The
  // submit is still disabled while that answer's callback runs, and a disabled
  // button takes no focus, so the focus moves in the effect that follows the
  // render re-enabling it.
  const [focusSubmit, setFocusSubmit] = useState(false);

  useEffect(() => {
    form.setFocus(INITIAL_FIELD);
  }, [form]);

  useEffect(() => {
    if (focusSubmit && !copying) {
      submitRef.current?.focus();
      setFocusSubmit(false);
    }
  }, [focusSubmit, copying]);

  // keepFieldsRef: a plain reset drops every registered input ref until the
  // next render, so a setFocus right after it would find no element to focus.
  function clearForm(): void {
    form.reset(EMPTY_VALUES, { keepFieldsRef: true });
    form.setFocus(INITIAL_FIELD);
  }

  function askForConfirmation(values: TransactionAddFormValues): void {
    if (busy) {
      return;
    }
    setConfirmation(values);
    setConfirming(true);
  }

  function addTransaction(): void {
    const values = confirmation;
    if (values === null) {
      return;
    }
    // The inputs stay read-only while the add runs, so its answer clears or
    // marks the entry confirmed, never a newer one.
    add.mutate(
      { ...values, confirm: CONFIRM_YES },
      {
        onSuccess: ({ data, status }) => {
          if (status === CREATED) {
            // WRITE-TRANSACT-FILE NORMAL: INITIALIZE-ALL-FIELDS, then
            // 'Transaction added successfully.  Your Tran ID is <id>.' in
            // green [app/cbl/COTRN02C.cbl:L723-734].
            showSuccess(data.message);
            clearForm();
          } else {
            showMessage(data.message);
          }
        },
        onError: (error) => {
          applyApiError(error, form, transactionAddBmsFieldMap);
        },
      },
    );
  }

  // F-COTRN02-006, COPY-LAST-TRAN-DATA [app/cbl/COTRN02C.cbl:L471-499]
  // (P-COTRN02C-026), in its COBOL order:
  // 1. VALIDATE-INPUT-KEY-FIELDS: the key edits the schema decides run first,
  //    on the key fields alone.
  // 2. STARTBR at HIGH-VALUES and READPREV: the highest-keyed transaction in
  //    the whole file, whatever its account. An empty master has none to
  //    copy, and the browse's own text is shown instead.
  // 3. PROCESS-ENTER-KEY on the entered keys and the copied data with CONFIRM
  //    blank: the server's cross-reference look-up of the key (V-COTRN02C-004
  //    to V-COTRN02C-007), every data edit, then 'Confirm to add this
  //    transaction...'. A key rejection ends the task before the copy, so the
  //    copy is applied only when the answer is a later edit; that answer is
  //    then shown on the copied screen.
  // The key fields keep what the user entered: the COBOL copies none of them
  // [app/cbl/COTRN02C.cbl:L479-492].
  // The keyboard lock is taken before the key edits and released on every
  // way the task ends: rejected keys, key edits that throw, a failed read, an
  // empty master, and the check's answer. The answers release it in
  // onSettled, which TanStack Query runs even when onSuccess or onError throws.
  async function copyLastTransaction(): Promise<void> {
    if (copyingRef.current || add.isPending) {
      return;
    }
    copyingRef.current = true;
    setCopying(true);
    const releaseCopy = (): void => {
      copyingRef.current = false;
      setCopying(false);
    };
    const keysValid = await form
      .trigger([...KEY_FIELDS], { shouldFocus: true })
      .catch((error: unknown) => {
        releaseCopy();
        throw error;
      });
    if (!keysValid) {
      releaseCopy();
      return;
    }
    // The entry the edited keys belong to. The check is built from it and its
    // answer applied to it, never to the live form.
    const entry = form.getValues();
    copyLast.mutate(undefined, {
      onSuccess: ({ transaction, message }) => {
        if (transaction === undefined) {
          showMessage(message);
          return;
        }
        const copied = transactionToAddFormValues(transaction);
        const applyCopy = (): void => {
          form.reset({ ...entry, ...copied }, { keepFieldsRef: true });
        };
        check.mutate(
          { ...entry, ...copied, confirm: CONFIRM_BLANK },
          {
            onSuccess: ({ data }) => {
              applyCopy();
              showMessage(data.message);
            },
            onError: (error) => {
              if (passedKeyFields(error)) {
                applyCopy();
              }
              const applied = applyApiError(error, form, transactionAddBmsFieldMap);
              // Every edit passed and only the confirmation is asked for: the
              // cursor goes to CONFIRM (MOVE -1 TO CONFIRML), whose place the
              // submit that opens the confirm dialog takes.
              setFocusSubmit(
                applied.fields.length === 1 &&
                  applied.fields[0] === transactionAddBmsFieldMap.CONFIRM,
              );
            },
            onSettled: releaseCopy,
          },
        );
      },
      onError: (error) => {
        applyApiError(error, form, transactionAddBmsFieldMap);
      },
      // A failed read or an empty master ends the task here. A record read
      // hands it to the check, whose answer ends it.
      onSettled: (last) => {
        if (last?.transaction === undefined) {
          releaseCopy();
        }
      },
    });
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        actions={
          <Button variant="outline" asChild>
            <Link to="/transactions">Back</Link>
          </Button>
        }
      />
      <form
        noValidate
        onSubmit={(event) => {
          // ENTER under the keyboard lock is ignored before the edits run.
          if (busy) {
            event.preventDefault();
            return;
          }
          void form.handleSubmit(askForConfirmation)(event);
        }}
      >
        <Card>
          <CardContent>
            <FieldGroup>
              {FIELD_SETS.map((set, index) => {
                const groupDescriptionId =
                  set.description === undefined
                    ? undefined
                    : `${set.fields[0].bms}-group-description`;
                return (
                  <Fragment key={set.legend}>
                    {index > 0 ? <FieldSeparator /> : null}
                    <FieldSet>
                      <FieldLegend>{set.legend}</FieldLegend>
                      {set.description === undefined ? null : (
                        <FieldDescription id={groupDescriptionId}>
                          {set.description}
                        </FieldDescription>
                      )}
                      {/* One column at md, two at lg. */}
                      <FieldGroup className="grid gap-4 lg:grid-cols-2">
                        {set.fields.map((config) => (
                          <TextField
                            key={config.bms}
                            control={form.control}
                            config={config}
                            groupDescriptionId={groupDescriptionId}
                            readOnly={busy}
                          />
                        ))}
                      </FieldGroup>
                    </FieldSet>
                  </Fragment>
                );
              })}
            </FieldGroup>
          </CardContent>
          {/* The map's key line order: ENTER, F4, F5. */}
          <CardFooter className="flex-wrap gap-2">
            <Button ref={submitRef} type="submit" disabled={copying}>
              Add transaction
            </Button>
            <Button type="button" variant="outline" disabled={busy} onClick={clearForm}>
              Reset
            </Button>
            <Button
              type="button"
              variant="secondary"
              onClick={() => {
                void copyLastTransaction();
              }}
            >
              Copy last transaction
            </Button>
          </CardFooter>
        </Card>
      </form>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Confirm the transaction"
        description={confirmation === null ? null : describeTransaction(confirmation)}
        confirmLabel="Confirm"
        cancelLabel="Cancel"
        onConfirm={addTransaction}
        pending={add.isPending}
      />
    </div>
  );
}
