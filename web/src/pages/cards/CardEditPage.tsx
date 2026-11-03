// CCUP card update: the COCRDUP map of COCRDUPC, "Update Credit Card Details"
// [app/bms/COCRDUP.bms:L75-78], at /cards/:cardNumber/edit?account_id=<acct>.
//
// - The account and card numbers are the keys the screen was entered with
//   (E-55) and are shown read-only: ACCTSID is PROT and the route fixes the
//   card. Both come from the URL, not from the card read, because COCRDUPC
//   shows CC-ACCT-ID and CC-CARD-NUM, the keys entered
//   [app/cbl/COCRDUPC.cbl:L1088-1096]; they travel in the PUT as entered.
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
import { Controller, useForm, type Control } from "react-hook-form";
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
 * token the PUT echoes; no control renders it or the keys.
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

type KeyItemProps = { label: string; value: string; error?: string };

function KeyItem({ label, value, error }: KeyItemProps): JSX.Element {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-sm text-muted-foreground">{label}</dt>
      <dd className="flex flex-col gap-1">
        <span className="font-medium break-all">{value}</span>
        {error !== undefined ? <FieldError>{error}</FieldError> : null}
      </dd>
    </div>
  );
}

type KeyDetailsProps = { keys: CardKeys; errors: KeyErrors };

/** ACCTSID and CARDSID, read-only, with any error the edits or the API report. */
function KeyDetails({ keys, errors }: KeyDetailsProps): JSX.Element {
  return (
    <Card>
      <CardContent>
        <dl className="grid gap-6 lg:grid-cols-2">
          <KeyItem
            label="Account Number"
            value={keys.account_id}
            error={errors.account_id}
          />
          <KeyItem
            label="Card Number"
            value={keys.card_number}
            error={errors.card_number}
          />
        </dl>
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

  const cardQuery = useCard(cardNumber, accountId);
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

  function saveCard(values: CardUpdateFormValues): void {
    mutation.mutate(toCardUpdateRequest(values), {
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
    });
  }

  // PF12: 9000-READ-DATA reads the card again and the unsaved entry is
  // dropped. An unchanged card keeps its object, so the form is reset here
  // rather than by the read effect.
  async function cancelEdits(): Promise<void> {
    const reread = await cardQuery.refetch();
    if (reread.isSuccess) {
      form.reset(toFormValues(reread.data, keys));
      setFormCard(reread.data);
    }
  }

  const backTo = {
    pathname: `/cards/${encodeURIComponent(cardNumber)}`,
    search:
      accountId === ""
        ? ""
        : `?${new URLSearchParams({ account_id: accountId }).toString()}`,
  };

  const { errors } = form.formState;
  const keyErrors: KeyErrors = keyCheck.valid
    ? {
        account_id: errors.account_id?.message,
        card_number: errors.card_number?.message,
      }
    : keyCheck.errors;

  let editor: JSX.Element | null = null;
  if (keyCheck.valid && card !== undefined && formCard !== undefined) {
    editor = (
      <form noValidate onSubmit={form.handleSubmit(saveCard)}>
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
            <Button type="submit" disabled={mutation.isPending}>
              Save
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={mutation.isPending}
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
      <KeyDetails keys={keys} errors={keyErrors} />
      {editor}
    </div>
  );
}
