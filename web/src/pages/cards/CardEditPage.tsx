// CCUP card update: the COCRDUP map of COCRDUPC, "Update Credit Card Details"
// [app/bms/COCRDUP.bms:L75-78], at /cards/:cardNumber/edit?account_id=<acct>.
//
// - The account and card numbers are the keys the screen was entered with
//   (E-55) and are read-only inputs: ACCTSID is PROT and the route fixes the
//   card. Both come from the URL, not from the card read, because COCRDUPC
//   shows CC-ACCT-ID and CC-CARD-NUM, the keys entered
//   [app/cbl/COCRDUPC.cbl:L1088-1096]; they travel in the PUT as entered. A
//   key error the API reports receives focus, as COCRDUPC's
//   `MOVE -1 TO ACCTSIDL` places the cursor on the key.
// - Name on card, Card Active Y/N, Expiry Month and Expiry Year are the
//   unprotected fields; EXPDAY is dark and protected, so it has no control and
//   the server keeps the stored day.
// - ENTER's validation and the PF5 write are one submit, "Save"
//   (F-COCRDUP-002, F-COCRDUP-005, E-56); PF12 "Cancel" re-reads the card and
//   drops unsaved edits (F-COCRDUP-006); PF3 "Back" returns to the card view
//   (F-COCRDUP-004).
// - Every message is the server's, shown through lib/api-errors.ts.
import { zodResolver } from "@hookform/resolvers/zod";
import { ArrowLeft } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type JSX } from "react";
import {
  Controller,
  useForm,
  type Control,
  type ControllerRenderProps,
} from "react-hook-form";
import { Link, useParams, useSearchParams } from "react-router";

import { useCard, useCardUpdateMutation } from "@/api/queries/cards";
import type { components } from "@/api/schema";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  applyApiError,
  showApiError,
  showMessage,
  showSuccess,
} from "@/lib/api-errors";
import {
  cardUpdateBmsFieldMap,
  cardUpdateSchema,
  cardViewKeySchema,
  toCardUpdateRequest,
  type CardUpdateFormValues,
} from "@/schemas/cards";

type CardResponse = components["schemas"]["CardResponse"];

/** The COCRDUP title line [app/bms/COCRDUP.bms:L75-78]. */
const TITLE = "Update Credit Card Details";

/** The keys the screen was entered with: the route's card and account. */
type CardKeys = Pick<CardUpdateFormValues, "account_id" | "card_number">;

/** The text of each key error, by key. */
type KeyErrors = Partial<Record<keyof CardKeys, string>>;

/** The form fields that render a control: the unprotected COCRDUP fields. */
type EditableField = Exclude<
  keyof CardUpdateFormValues,
  keyof CardKeys | "version"
>;

/**
 * The form values of `card` as COCRDUPC 3200-SETUP-SCREEN-VARS presents it:
 * the keys as entered, the fetched name and status, and the month and year
 * cut from the stored `YYYY-MM-DD` expiry, `CARD-EXPIRY-YEAR` (1:4) and
 * `CARD-EXPIRY-MONTH` (6:2) [app/cbl/COCRDUPC.cbl:L115-121]. `version` is the
 * token the PUT echoes; no control renders it.
 */
function toFormValues(
  card: CardResponse,
  keys: CardKeys,
): CardUpdateFormValues {
  const expiry = card.card_expiraion_date;
  return {
    version: card.version,
    account_id: keys.account_id,
    card_number: keys.card_number,
    embossed_name: card.card_embossed_name,
    active_status: card.card_active_status,
    expiry_month: expiry.slice(5, 7),
    expiry_year: expiry.slice(0, 4),
  };
}

/**
 * The COCRDUPC key edits on the keys of the URL (V-COCRDUPC-002 to
 * V-COCRDUPC-006, the texts COCRDSLC shares): without both keys the card
 * cannot be read, so the screen shows the key errors instead of the form.
 */
function checkKeys(keys: CardKeys): { valid: boolean; errors: KeyErrors } {
  const check = cardViewKeySchema.safeParse(keys);
  if (check.success) {
    return { valid: true, errors: {} };
  }
  const errors: KeyErrors = {};
  for (const issue of check.error.issues) {
    const [field] = issue.path;
    if (
      (field === "account_id" || field === "card_number") &&
      errors[field] === undefined
    ) {
      errors[field] = issue.message;
    }
  }
  return { valid: false, errors };
}

/** ACCTSID and CARDSID with their COCRDUP captions, in map order. */
const KEY_FIELDS = [
  { name: "account_id", label: "Account Number" },
  { name: "card_number", label: "Card Number" },
] as const satisfies readonly { name: keyof CardKeys; label: string }[];

type KeyFieldProps = {
  name: keyof CardKeys;
  label: string;
  /** The key as entered in the URL. */
  value: string;
  invalid: boolean;
  /** The text of the key's error, from the key edits or the API. */
  message?: string;
  /** The form's binding of the key, while the form holds the card. */
  field?: ControllerRenderProps<CardUpdateFormValues, keyof CardKeys>;
};

/**
 * One key, a read-only input showing the value entered. Bound through
 * `field`, its `ref` is the key's, so a key error the API reports moves focus
 * here, as COCRDUPC 3300-SETUP-SCREEN-ATTRS places the cursor with
 * `MOVE -1 TO ACCTSIDL` or `CARDSIDL` [app/cbl/COCRDUPC.cbl:L1217-1234].
 */
function KeyField({
  name,
  label,
  value,
  invalid,
  message,
  field,
}: KeyFieldProps): JSX.Element {
  const id = `card-edit-${name}`;
  const errorId = `${id}-error`;
  return (
    <Field data-invalid={invalid}>
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      <Input
        {...field}
        id={id}
        value={value}
        readOnly
        autoComplete="off"
        spellCheck={false}
        aria-invalid={invalid}
        aria-describedby={invalid ? errorId : undefined}
        className="bg-muted"
      />
      <FieldError id={errorId}>{message}</FieldError>
    </Field>
  );
}

type KeyDetailsProps = {
  keys: CardKeys;
  /** The key edits' texts, shown while no form binds the keys. */
  errors: KeyErrors;
  /** The form holding the card: it binds both keys and holds their errors. */
  control?: Control<CardUpdateFormValues>;
};

/** ACCTSID and CARDSID, read-only, with any error the edits or the API report. */
function KeyDetails({ keys, errors, control }: KeyDetailsProps): JSX.Element {
  return (
    <Card>
      <CardContent>
        <FieldGroup className="grid gap-6 lg:grid-cols-2">
          {KEY_FIELDS.map(({ name, label }) =>
            control === undefined ? (
              <KeyField
                key={name}
                name={name}
                label={label}
                value={keys[name]}
                invalid={errors[name] !== undefined}
                message={errors[name]}
              />
            ) : (
              <Controller
                key={name}
                control={control}
                name={name}
                render={({ field, fieldState }) => (
                  <KeyField
                    name={name}
                    label={label}
                    value={keys[name]}
                    invalid={fieldState.invalid}
                    message={fieldState.error?.message}
                    field={field}
                  />
                )}
              />
            ),
          )}
        </FieldGroup>
      </CardContent>
    </Card>
  );
}

type CardTextFieldProps = {
  control: Control<CardUpdateFormValues>;
  name: EditableField;
  label: string;
  /** The width of the BMS field, `LENGTH=` on COCRDUP. */
  maxLength: number;
  numeric?: boolean;
};

/**
 * One unprotected COCRDUP field. `field.ref` reaches the input, so a field
 * error the resolver or the server reports moves focus here, as the COBOL
 * `MOVE -1 TO <field>L` places the cursor.
 */
function CardTextField({
  control,
  name,
  label,
  maxLength,
  numeric = false,
}: CardTextFieldProps): JSX.Element {
  const id = `card-edit-${name}`;
  const errorId = `${id}-error`;
  return (
    <Controller
      control={control}
      name={name}
      render={({ field, fieldState }) => (
        <Field data-invalid={fieldState.invalid}>
          <FieldLabel htmlFor={id}>{label}</FieldLabel>
          <Input
            {...field}
            id={id}
            maxLength={maxLength}
            inputMode={numeric ? "numeric" : undefined}
            autoComplete="off"
            aria-invalid={fieldState.invalid}
            aria-describedby={fieldState.invalid ? errorId : undefined}
          />
          <FieldError id={errorId} errors={[fieldState.error]} />
        </Field>
      )}
    />
  );
}

/** The editable fields while the card is being read. */
function EditorSkeleton(): JSX.Element {
  return (
    <Card role="status" aria-busy="true">
      <span className="sr-only">Loading</span>
      <CardContent className="grid gap-6 lg:grid-cols-2">
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
      </CardContent>
    </Card>
  );
}

export default function CardEditPage(): JSX.Element {
  const { cardNumber = "" } = useParams();
  const [searchParams] = useSearchParams();
  const accountId = searchParams.get("account_id") ?? "";
  const keys = useMemo<CardKeys>(
    () => ({ account_id: accountId, card_number: cardNumber }),
    [accountId, cardNumber],
  );
  const keyCheck = useMemo(() => checkKeys(keys), [keys]);

  // Keys the edits reject reach the hook as blanks, which it never sends:
  // COCRDUPC performs 9000-READ-DATA only when both key edits pass
  // [app/cbl/COCRDUPC.cbl:L959-962].
  const cardQuery = useCard(
    keyCheck.valid ? cardNumber : "",
    keyCheck.valid ? accountId : "",
  );
  const mutation = useCardUpdateMutation(cardNumber);
  const card = cardQuery.data;

  const form = useForm<CardUpdateFormValues>({
    resolver: zodResolver(cardUpdateSchema),
    defaultValues: card === undefined ? undefined : toFormValues(card, keys),
  });
  // The card the form was last reset from. The form renders only once it
  // holds a card, so its inputs never show blank values before the read.
  const [formCard, setFormCard] = useState<CardResponse | undefined>(card);
  // A failed save, shown by an effect rather than in the mutation callback.
  // The hook settles a 409 only after this card's detail has been re-read,
  // but TanStack Query tells this page about the new data on a later timer
  // task, after the callback: errors set in the callback would be cleared by
  // the reset that new data triggers. A render of this page reads the re-read
  // card at once, so the form is reset to it first and the fields are marked
  // afterwards, as COCRDUPC redisplays the changed record with its message.
  const [failure, setFailure] = useState<{ error: unknown } | null>(null);
  const toastedLoadError = useRef<unknown>(null);

  // Every new read of the card replaces the entry: the first read, the
  // re-read after a save or a 409, and a background refresh whose content
  // changed (an unchanged refresh keeps the same object).
  useEffect(() => {
    if (card !== undefined && card !== formCard) {
      form.reset(toFormValues(card, keys));
      setFormCard(card);
    }
  }, [card, formCard, form, keys]);

  // The failure waits for the render after a reset: a reset drops the
  // inputs' registrations until they render again, so focus would find no
  // input before it.
  useEffect(() => {
    if (failure !== null && (card === undefined || card === formCard)) {
      applyApiError(failure.error, form, cardUpdateBmsFieldMap);
      setFailure(null);
    }
  }, [failure, card, formCard, form]);

  // A failed read, the first or a Cancel re-read, is toasted once; the screen
  // keeps the card it already holds.
  const loadError = cardQuery.error;
  useEffect(() => {
    if (loadError !== null && loadError !== toastedLoadError.current) {
      toastedLoadError.current = loadError;
      showApiError(loadError);
    }
  }, [loadError]);

  // One request at a time, a save or Cancel's re-read: a re-read answered with
  // the card as it stood before a save could otherwise land after it and
  // bring back the old entries and token. The ref closes the gap before the
  // busy state re-renders, so a second Enter or click sends nothing.
  const inFlight = useRef(false);
  const [reading, setReading] = useState(false);

  function saveCard(values: CardUpdateFormValues): void {
    if (inFlight.current) {
      return;
    }
    const body = toCardUpdateRequest(values);
    inFlight.current = true;
    mutation.mutate(body, {
      onSuccess: (result) => {
        // An entry equal to the fetched card writes nothing and answers 'No
        // change detected with respect to values fetched.' (V-COCRDUPC-009),
        // a prompt rather than a success; the card then keeps the token sent.
        if (result.card.version === values.version) {
          showMessage(result.message);
        } else {
          showSuccess(result.message);
        }
        form.reset(toFormValues(result.card, keys));
      },
      onError: (error) => {
        setFailure({ error });
      },
      // Runs after either outcome, once the hook's re-reads have settled.
      // Per-call callbacks run only while the page is on screen; a page that
      // has left has no guard to release.
      onSettled: () => {
        inFlight.current = false;
      },
    });
  }

  // PF12: 9000-READ-DATA reads the card again and the unsaved entry is
  // dropped. An unchanged card keeps its object, so the form is reset here
  // rather than by the read effect.
  async function cancelEdits(): Promise<void> {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setReading(true);
    try {
      const reread = await cardQuery.refetch();
      if (reread.isSuccess) {
        form.reset(toFormValues(reread.data, keys));
        setFormCard(reread.data);
      }
    } finally {
      inFlight.current = false;
      setReading(false);
    }
  }

  // aria-disabled rather than disabled: a focused button that becomes
  // disabled loses focus to the document, which would strand a keyboard user
  // after Save or Cancel; the guard above keeps the inert buttons from acting.
  const busy = mutation.isPending || reading;
  const busyClass = "aria-disabled:pointer-events-none aria-disabled:opacity-50";

  const backTo = {
    pathname: `/cards/${encodeURIComponent(cardNumber)}`,
    search:
      accountId === ""
        ? ""
        : `?${new URLSearchParams({ account_id: accountId }).toString()}`,
  };

  // Once the form holds the card it binds the keys, and they sit inside it, so
  // Enter on a focused key submits as on every other field.
  const holdsCard =
    keyCheck.valid && card !== undefined && formCard !== undefined;

  let editor: JSX.Element | null = null;
  if (holdsCard) {
    editor = (
      <form
        noValidate
        onSubmit={form.handleSubmit(saveCard)}
        className="flex flex-col gap-6"
      >
        <KeyDetails
          keys={keys}
          errors={keyCheck.errors}
          control={form.control}
        />
        <Card>
          <CardContent>
            <FieldGroup className="grid gap-6 lg:grid-cols-2">
              <CardTextField
                control={form.control}
                name="embossed_name"
                label="Name on card"
                maxLength={50}
              />
              <CardTextField
                control={form.control}
                name="active_status"
                label="Card Active Y/N"
                maxLength={1}
              />
              <FieldSet className="lg:col-span-2">
                <FieldLegend variant="label">Expiry Date</FieldLegend>
                <div className="grid gap-6 lg:grid-cols-2">
                  <CardTextField
                    control={form.control}
                    name="expiry_month"
                    label="Expiry Month"
                    maxLength={2}
                    numeric
                  />
                  <CardTextField
                    control={form.control}
                    name="expiry_year"
                    label="Expiry Year"
                    maxLength={4}
                    numeric
                  />
                </div>
              </FieldSet>
            </FieldGroup>
          </CardContent>
          <CardFooter className="flex-wrap gap-2">
            <Button type="submit" aria-disabled={busy} className={busyClass}>
              Save
            </Button>
            <Button
              type="button"
              variant="outline"
              aria-disabled={busy}
              className={busyClass}
              onClick={() => {
                void cancelEdits();
              }}
            >
              Cancel
            </Button>
          </CardFooter>
        </Card>
      </form>
    );
  } else if (keyCheck.valid && (cardQuery.isPending || card !== undefined)) {
    // Still reading, or read and not yet in the form. A read that failed
    // with no card to show leaves only the keys and the toasted message.
    editor = <EditorSkeleton />;
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={TITLE}
        actions={
          <Button variant="ghost" asChild>
            <Link to={backTo}>
              <ArrowLeft aria-hidden="true" />
              Back
            </Link>
          </Button>
        }
      />
      {holdsCard ? null : <KeyDetails keys={keys} errors={keyCheck.errors} />}
      {editor}
    </div>
  );
}
