// CTTU transaction type maintenance (COTRTUPC, map COTRTUP) over the Db2
// master `transaction_types`, on two routes (docs/parity/function-map.md,
// section COTRTUP):
//
// - /admin/transaction-types/new is the key entry. "Look up" applies the
//   COTRTUPC key edits and reads the code: a type on file opens its edit route,
//   as COTRTUPC shows a found type for update; a code not on file enables the
//   description and "Add" (POST), as F5 on a key not found does. The legend's
//   F6=Add is that same Add (E-23). "Cancel" abandons the add and returns to
//   the empty key entry (F12, E-64).
// - /admin/transaction-types/:code/edit shows the type for update. "Save" is
//   ENTER's validation and the F5 write in one submit (PUT without `origin`,
//   so the server applies the COTRTUPC edits and texts; E-56). "Delete" opens
//   the confirm dialog of F4; its "Confirm" is the confirming F4 (DELETE with
//   `origin=form`, E-23) and its "Cancel" closes it. "Cancel" leaves for the
//   empty key entry (F12, E-64).
// - "Back" (F3) links to the list on both routes.
//
// Every COBOL text comes from the server or from the schema module; this page
// writes none. Writes reach the Db2 master alone, so reports show them only
// after `batch seed --only tranextr`. The route is admin-only, and the server's
// 403 is authoritative: the page checks no role.
import { useEffect, useId, useRef, useState, type JSX } from "react";
import {
  Controller,
  useForm,
  type FieldErrors,
  type Resolver,
  type ResolverResult,
  type UseFormReturn,
} from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { ArrowLeft } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router";

import { ApiError, api, unwrap } from "@/api/client";
import {
  useTranTypeCreateMutation,
  useTranTypeDeleteMutation,
  useTranTypes,
  useTranTypeUpdateMutation,
  type TranType,
} from "@/api/queries/transaction-types";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  applyApiError,
  showApiError,
  showMessage,
  showSuccess,
} from "@/lib/api-errors";
import { decodeKeySegment, keyPathSegment } from "@/lib/key-segment";
import {
  toTranTypeUpdateRequest,
  tranTypeCreateSchema,
  tranTypeEditFormSchema,
  tranTypeUpsertBmsFieldMap,
  type TranTypeFormValues,
} from "@/schemas/transaction-types";

/** The CTLI list, where F3 returns (F-COTRTUP-005). */
const LIST_ROUTE = "/admin/transaction-types";

/** The empty key entry (F-COTRTUP-007, F-COTRTUP-014). */
const KEY_ENTRY_ROUTE = "/admin/transaction-types/new";

/** The edit route of a type on file, its code as the key segment. */
function editRoute(typeCode: string): string {
  return `${LIST_ROUTE}/${keyPathSegment(typeCode)}/edit`;
}

/** `TRTYPCDI` `X(2)` and `TRTYDSCI` `X(50)` of COTRTUP. */
const TYPE_CODE_LENGTH = 2;
const DESCRIPTION_LENGTH = 50;

const EMPTY_VALUES: TranTypeFormValues = { type_code: "", description: "" };

const createResolver = zodResolver(tranTypeCreateSchema);
const editResolver = zodResolver(tranTypeEditFormSchema);

/**
 * The Look up's edits: the COTRTUPC key edits of tranTypeCreateSchema alone.
 * That schema edits the description only once the key passes, so any
 * description issue it reports belongs to the Add and is dropped here, as
 * COTRTUPC reads the key before the description is editable.
 */
const keyEntryResolver: Resolver<TranTypeFormValues> = async (
  values,
  context,
  options,
): Promise<ResolverResult<TranTypeFormValues>> => {
  const result = await createResolver(values, context, options);
  const errors: FieldErrors<TranTypeFormValues> = result.errors;
  if (errors.type_code === undefined) {
    return { values, errors: {} };
  }
  return { values: {}, errors: { type_code: errors.type_code } };
};

/**
 * The two digits COTRTUPC reads a key by once its key edit has passed
 * (1210-EDIT-TRANTYPE, P-COTRTUPC-015): `FUNCTION NUMVAL` stored in the
 * unsigned `9(02)` and moved back, so the sign is dropped and a fraction
 * rounds to the nearest integer (E-40): `1`, ` 1`, `+1`, `1-` and `-1` read
 * `01`, `.7` reads `01` and `.3` reads `00`. The list filter the Look up sends
 * takes two digits only, so the key is sent in this form.
 *
 * A two-byte key holds a fraction digit only when it holds no integer digit
 * (`.7`), so a key that rounds up always reads `01`. `undefined` for a
 * fraction of exactly one half, the midpoint E-40 leaves unpinned: the page
 * does not settle it, and the Add takes it to the server, whose answer stands.
 */
function readKeyOf(entry: string): string | undefined {
  const [integer = "", fraction = ""] = entry.replace(/[ +-]/g, "").split(".");
  const firstFraction = fraction.charAt(0);
  if (firstFraction === "5" && /^0*$/.test(fraction.slice(1))) {
    return undefined;
  }
  if (firstFraction > "5") {
    return "01";
  }
  return integer.padStart(TYPE_CODE_LENGTH, "0");
}

/** F3: back to the list, on both routes (F-COTRTUP-005). */
function BackLink(): JSX.Element {
  return (
    <Button asChild variant="outline">
      <Link to={LIST_ROUTE}>
        <ArrowLeft aria-hidden="true" />
        Back
      </Link>
    </Button>
  );
}

type TranTypeFieldsProps = {
  form: UseFormReturn<TranTypeFormValues>;
  /** The key is the looked-up code (add) or the route's code (edit). */
  typeCodeReadOnly: boolean;
  /** The description stays protected until a type to add or update is known. */
  descriptionDisabled: boolean;
  /** The field focused on mount: the key on entry (BMS `IC`), else the description. */
  autoFocus: keyof TranTypeFormValues;
};

/** TRTYPCD and TRTYDSC: one column at md, two at lg. */
function TranTypeFields({
  form,
  typeCodeReadOnly,
  descriptionDisabled,
  autoFocus,
}: TranTypeFieldsProps): JSX.Element {
  const id = useId();
  const typeCodeId = `${id}-type-code`;
  const descriptionId = `${id}-description`;
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Controller
        control={form.control}
        name="type_code"
        render={({ field, fieldState }) => (
          <Field data-invalid={fieldState.invalid}>
            <FieldLabel htmlFor={typeCodeId}>Type code</FieldLabel>
            <Input
              {...field}
              id={typeCodeId}
              maxLength={TYPE_CODE_LENGTH}
              inputMode="numeric"
              autoComplete="off"
              readOnly={typeCodeReadOnly}
              autoFocus={autoFocus === "type_code"}
              aria-invalid={fieldState.invalid}
              aria-describedby={
                fieldState.invalid ? `${typeCodeId}-error` : undefined
              }
            />
            <FieldError
              id={`${typeCodeId}-error`}
              errors={[fieldState.error]}
            />
          </Field>
        )}
      />
      <Controller
        control={form.control}
        name="description"
        render={({ field, fieldState }) => (
          <Field data-invalid={fieldState.invalid}>
            <FieldLabel htmlFor={descriptionId}>Description</FieldLabel>
            <Input
              {...field}
              id={descriptionId}
              maxLength={DESCRIPTION_LENGTH}
              autoComplete="off"
              disabled={descriptionDisabled}
              autoFocus={autoFocus === "description"}
              aria-invalid={fieldState.invalid}
              aria-describedby={
                fieldState.invalid ? `${descriptionId}-error` : undefined
              }
            />
            <FieldError
              id={`${descriptionId}-error`}
              errors={[fieldState.error]}
            />
          </Field>
        )}
      />
    </div>
  );
}

/** Both fields' place while the type of the edit route loads. */
function FieldsSkeleton(): JSX.Element {
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <span className="sr-only">Loading</span>
      <Skeleton className="h-16 w-full" />
      <Skeleton className="h-16 w-full" />
    </div>
  );
}

/** The key entry (Look up) and, for a code not on file, the add (Add). */
type AddPhase = "entry" | "add";

/** /admin/transaction-types/new: key entry and add. */
function TranTypeAddForm(): JSX.Element {
  const navigate = useNavigate();
  const createMutation = useTranTypeCreateMutation();
  const [phase, setPhase] = useState<AddPhase>("entry");
  const titleId = useId();
  const form = useForm<TranTypeFormValues>({
    resolver: phase === "entry" ? keyEntryResolver : createResolver,
    defaultValues: EMPTY_VALUES,
  });

  // The cursor follows the phase: the key on entry, the description on the
  // add. It moves once the phase's render is committed, because the
  // description is enabled by that render and a reset re-registers the fields
  // only then.
  useEffect(() => {
    form.setFocus(phase === "add" ? "description" : "type_code");
  }, [form, phase]);

  /** Back to the empty key entry. */
  function toKeyEntry(): void {
    form.reset(EMPTY_VALUES);
    setPhase("entry");
  }

  /**
   * Reads the edited key through the list's type code filter: a type on file
   * opens its edit route; none (the list's 404, or a page without that code)
   * opens the add. The list's own text for an empty result is COTRTLIC's, not
   * COTRTUPC's, so it is not shown; any other failure is.
   */
  async function lookUp(entry: string): Promise<void> {
    const key = readKeyOf(entry);
    if (key !== undefined) {
      try {
        const page = unwrap(
          await api.GET("/admin/transaction-types", {
            params: { query: { type_code: key } },
          }),
        );
        const found = page.items.find((type) => type.type_code === key);
        if (found !== undefined) {
          navigate(editRoute(found.type_code));
          return;
        }
      } catch (error) {
        if (!(error instanceof ApiError && error.status === 404)) {
          applyApiError(error, form, tranTypeUpsertBmsFieldMap);
          return;
        }
      }
    }
    setPhase("add");
  }

  const onSubmit = form.handleSubmit(async (values) => {
    if (phase === "entry") {
      await lookUp(values.type_code);
      return;
    }
    createMutation.mutate(
      { type_code: values.type_code, description: values.description },
      {
        onSuccess: (response) => {
          showSuccess(response.message);
          toKeyEntry();
        },
        onError: (error) => {
          applyApiError(error, form, tranTypeUpsertBmsFieldMap);
        },
      },
    );
  });

  const adding = phase === "add";
  return (
    <div className="flex flex-col gap-6">
      <PageHeader actions={<BackLink />} />
      <Card>
        <form
          className="flex flex-col gap-6"
          aria-labelledby={titleId}
          noValidate
          onSubmit={onSubmit}
        >
          <CardHeader>
            <CardTitle>
              <h2 id={titleId}>Transaction type</h2>
            </CardTitle>
            <CardDescription>
              {adding
                ? "Enter the description of the new type, then add it."
                : "Look up a type code to update that type, or to add it when no type has it."}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {/* The Add posts the key the Look up found absent, so the key is
                fixed until Cancel. COTRTUPC leaves it editable on a key not
                found, but never reads a key typed over it there and can write
                a type it never showed (E-70); with the key fixed, an existing
                code is added only by a race, which POST answers 409 (E-51). */}
            <TranTypeFields
              form={form}
              typeCodeReadOnly={adding}
              descriptionDisabled={!adding}
              autoFocus="type_code"
            />
          </CardContent>
          <CardFooter className="flex flex-wrap gap-2">
            {adding ? (
              <>
                <Button type="submit" disabled={createMutation.isPending}>
                  Add
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  disabled={createMutation.isPending}
                  onClick={toKeyEntry}
                >
                  Cancel
                </Button>
              </>
            ) : (
              <Button type="submit" disabled={form.formState.isSubmitting}>
                Look up
              </Button>
            )}
          </CardFooter>
        </form>
      </Card>
    </div>
  );
}

type TranTypeEditFormProps = {
  /** The `:code` route parameter, the key of the type shown. */
  code: string;
};

/** /admin/transaction-types/:code/edit: update and delete of a type on file. */
function TranTypeEditForm({ code }: TranTypeEditFormProps): JSX.Element {
  const navigate = useNavigate();
  const titleId = useId();
  const query = useTranTypes({ type_code: code });
  const updateMutation = useTranTypeUpdateMutation();
  const deleteMutation = useTranTypeDeleteMutation();
  const [confirmingDelete, setConfirmingDelete] = useState(false);

  // The API has no single-type read: the list filtered by the code holds the
  // type. `00` filters nothing (the COBOL ZEROS), so the row is matched on its
  // code rather than taken first.
  const record: TranType | undefined = query.isPlaceholderData
    ? undefined
    : query.data?.items.find((type) => type.type_code === code);

  // `values` resets the form whenever the stored type changes, a save
  // included, and leaves it alone when a refetch brings the same row back.
  const form = useForm<TranTypeFormValues>({
    resolver: editResolver,
    defaultValues: { type_code: code, description: "" },
    values:
      record === undefined
        ? undefined
        : { type_code: record.type_code, description: record.description },
  });

  // A delete's success marks this read stale without repeating it, and the
  // page then leaves for the key entry. A read of the type that still fails
  // while the delete runs or after it succeeded finds the type gone: that
  // answer is the delete's outcome, not an error of this page, so it is not
  // shown. Each other failure of the read is shown once.
  const deleting = deleteMutation.isPending || deleteMutation.isSuccess;
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (
      query.error === null ||
      query.error === shownError.current ||
      deleting
    ) {
      return;
    }
    shownError.current = query.error;
    showApiError(query.error);
  }, [deleting, query.error]);

  const onSubmit = form.handleSubmit((values) => {
    if (record === undefined) {
      return;
    }
    const fetched = record;
    updateMutation.mutate(
      { typeCode: fetched.type_code, body: toTranTypeUpdateRequest(values) },
      {
        onSuccess: (response) => {
          const stored = response.transaction_type;
          // An unchanged stored description means nothing was written:
          // COTRTUPC's no-change answer (V-COTRTUPC-006) is a prompt, not a
          // success.
          if (stored !== null && stored.description === fetched.description) {
            showMessage(response.message);
          } else {
            showSuccess(response.message);
          }
          if (stored !== null) {
            form.reset({
              type_code: stored.type_code,
              description: stored.description,
            });
          }
        },
        onError: (error) => {
          applyApiError(error, form, tranTypeUpsertBmsFieldMap);
        },
      },
    );
  });

  function confirmDelete(): void {
    if (record === undefined) {
      return;
    }
    deleteMutation.mutate(
      { typeCode: record.type_code, origin: "form" },
      {
        onSuccess: (response) => {
          showSuccess(response.message);
          navigate(KEY_ENTRY_ROUTE);
        },
        onError: (error) => {
          applyApiError(error, form, tranTypeUpsertBmsFieldMap);
        },
      },
    );
  }

  const busy = updateMutation.isPending || deleting;
  // BLITZY [A11Y]: the destructive Button keeps its built-in focus ring,
  // ring-destructive/20, a pale halo below the WCAG AA 3:1 non-text minimum
  // against the red button. Kept as the shadcn token set defines it; flagged
  // for designer review.
  return (
    <div className="flex flex-col gap-6">
      <PageHeader actions={<BackLink />} />
      <Card>
        <form
          className="flex flex-col gap-6"
          aria-labelledby={titleId}
          aria-busy={query.isPending}
          noValidate
          onSubmit={onSubmit}
        >
          <CardHeader>
            <CardTitle>
              <h2 id={titleId}>Transaction type</h2>
            </CardTitle>
            {record === undefined ? null : (
              <CardDescription>
                Change the description and save it, or delete the type.
              </CardDescription>
            )}
          </CardHeader>
          <CardContent>
            {query.isPending ? (
              <FieldsSkeleton />
            ) : (
              <TranTypeFields
                form={form}
                typeCodeReadOnly
                descriptionDisabled={record === undefined}
                autoFocus="description"
              />
            )}
          </CardContent>
          <CardFooter className="flex flex-wrap gap-2">
            {record === undefined ? null : (
              <>
                <Button type="submit" disabled={busy}>
                  Save
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  disabled={busy}
                  onClick={() => setConfirmingDelete(true)}
                >
                  Delete
                </Button>
              </>
            )}
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => navigate(KEY_ENTRY_ROUTE)}
            >
              Cancel
            </Button>
          </CardFooter>
        </form>
      </Card>
      {record === undefined ? null : (
        <ConfirmDialog
          open={confirmingDelete}
          onOpenChange={setConfirmingDelete}
          title="Delete transaction type"
          description={`Transaction type ${record.type_code}, ${record.description}, will be deleted.`}
          confirmLabel="Confirm"
          onConfirm={confirmDelete}
          pending={deleteMutation.isPending}
        />
      )}
    </div>
  );
}

/**
 * CTTU: the key entry and add at /admin/transaction-types/new, the update and
 * delete at /admin/transaction-types/:code/edit. The edit form is keyed by its
 * code, so another type's route starts from a fresh form.
 */
export default function TranTypeFormPage(): JSX.Element {
  const { code: segment } = useParams();
  if (segment === undefined) {
    return <TranTypeAddForm />;
  }
  // The route carries the key segment of the code (lib/key-segment.ts).
  const code = decodeKeySegment(segment);
  return <TranTypeEditForm key={code} code={code} />;
}
