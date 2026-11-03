// The admin reward-rule form: creates a rule at /admin/reward-rules/new
// (`POST /admin/reward-rules`) and edits one at
// /admin/reward-rules/:ruleId/edit (`PUT /admin/reward-rules/{id}`). Rewards
// have no COBOL program or BMS map, so the page follows the request's rewards
// specification. App.tsx owns both routes, their handles and the RequireAdmin
// guard; the server's 403 and its rule check stay authoritative.
//
// - The schema only keeps malformed text from being sent. The cross-field
//   checks (minimum not above maximum, `active_to` not before `active_from`)
//   are the server's, and come back as `fields[]` entries that applyApiError
//   places on `reward_rate`, `min_transaction_amount` or `active_to`.
// - Every value stays the string typed into its field; the schema turns the
//   `YYYY-MM-DD` dates into RFC 3339 instants and omits blank bounds.
// - The former PF keys are buttons in DOM order after the fields: Save (PF5),
//   Reset (PF4) and Back (PF3). Enter in any field submits through
//   handleSubmit.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ComponentProps,
  type JSX,
} from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";

import {
  useRewardRule,
  useRewardRuleCreateMutation,
  useRewardRuleUpdateMutation,
} from "@/api/queries/reward-rules";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError, showApiError } from "@/lib/api-errors";
import {
  REWARD_RULE_MESSAGES,
  rewardRuleBmsFieldMap,
  rewardRuleSchema,
  rewardRuleToFormInput,
  type RewardRuleFormInput,
  type RewardRuleFormValues,
} from "@/schemas/reward-rules";

const LIST_ROUTE = "/admin/reward-rules";

/** The new-rule form's starting values: every field empty. */
const EMPTY: RewardRuleFormInput = {
  transaction_type_cd: "",
  transaction_category_cd: "",
  reward_rate: "",
  min_transaction_amount: "",
  max_transaction_amount: "",
  active_from: "",
  active_to: "",
};

type FieldSpec = {
  name: keyof RewardRuleFormInput;
  label: string;
  description?: string;
  /** Grid placement of the field at `lg`, where the form has two columns. */
  className?: string;
  inputProps: Pick<
    ComponentProps<"input">,
    "autoComplete" | "inputMode" | "maxLength" | "placeholder"
  >;
};

// Dates are typed as text: there is no date picker among the generated
// components, so the YYYY-MM-DD pattern the schema checks is the entry format.
const DATE_INPUT: FieldSpec["inputProps"] = {
  inputMode: "numeric",
  placeholder: "YYYY-MM-DD",
  maxLength: 10,
};

// The seven fields in screen and tab order. The code widths are the
// copybooks': TRAN-TYPE PIC X(02) (CVTRA03Y) and TRAN-CAT-CD PIC 9(04)
// (CVTRA04Y). In two columns the minimum starts a row, so each pair of bounds
// (amounts, then dates) shares one row and the rate stands alone.
const FIELDS: readonly FieldSpec[] = [
  {
    name: "transaction_type_cd",
    label: "Transaction type code",
    inputProps: { maxLength: 2, autoComplete: "off" },
  },
  {
    name: "transaction_category_cd",
    label: "Transaction category code",
    inputProps: { maxLength: 4, inputMode: "numeric" },
  },
  {
    name: "reward_rate",
    label: "Reward rate",
    description: "Points per currency unit, up to 4 decimal places",
    inputProps: { inputMode: "decimal" },
  },
  {
    name: "min_transaction_amount",
    label: "Minimum transaction amount",
    description: "Leave blank for no bound",
    className: "lg:col-start-1",
    inputProps: { inputMode: "decimal" },
  },
  {
    name: "max_transaction_amount",
    label: "Maximum transaction amount",
    description: "Leave blank for no bound",
    inputProps: { inputMode: "decimal" },
  },
  {
    name: "active_from",
    label: "Active from",
    inputProps: DATE_INPUT,
  },
  {
    name: "active_to",
    label: "Active to",
    description: "Leave blank for no end",
    inputProps: DATE_INPUT,
  },
];

function BackButton(): JSX.Element {
  return (
    <Button variant="ghost" asChild>
      <Link to={LIST_ROUTE}>Back</Link>
    </Button>
  );
}

type RewardRuleFormProps = {
  /** The routed rule ID in edit mode; `undefined` creates a rule. */
  ruleId: string | undefined;
  /** The values the form starts from and Reset restores. */
  initialValues: RewardRuleFormInput;
  /** The form's accessible name: the page heading. */
  label: string;
};

// Mounted only once its starting values are known, so an edited rule's values
// are the form's defaults from the first paint and a later refetch of the rule
// list never overwrites what the admin has typed.
function RewardRuleForm({
  ruleId,
  initialValues,
  label,
}: RewardRuleFormProps): JSX.Element {
  const navigate = useNavigate();
  const idPrefix = useId();
  // Both hooks run on every render (rules of hooks); the mode picks one.
  const createMutation = useRewardRuleCreateMutation();
  const updateMutation = useRewardRuleUpdateMutation(ruleId ?? "");
  const mutation = ruleId === undefined ? createMutation : updateMutation;

  const form = useForm<RewardRuleFormInput, unknown, RewardRuleFormValues>({
    resolver: zodResolver(rewardRuleSchema),
    defaultValues: initialValues,
    mode: "onSubmit",
  });

  // The rule as stored is the only success answer: it carries no message, so
  // no success toast is shown and the list itself shows the change.
  const onValid = (values: RewardRuleFormValues): void => {
    mutation.mutate(values, {
      onSuccess: () => {
        void navigate(LIST_ROUTE);
      },
      onError: (error) => {
        applyApiError(error, form, rewardRuleBmsFieldMap);
      },
    });
  };

  return (
    <Card>
      <form
        noValidate
        aria-label={label}
        className="flex flex-col gap-6"
        onSubmit={form.handleSubmit(onValid)}
      >
        <CardContent>
          <FieldGroup className="grid gap-4 lg:grid-cols-2">
            {FIELDS.map((spec) => (
              <Controller
                key={spec.name}
                name={spec.name}
                control={form.control}
                render={({ field, fieldState }) => {
                  const id = `${idPrefix}-${spec.name}`;
                  const descriptionId = `${id}-description`;
                  const errorId = `${id}-error`;
                  // Only ids of rendered elements: FieldError renders nothing
                  // without a message.
                  const describedBy = [
                    spec.description === undefined ? "" : descriptionId,
                    fieldState.error?.message ? errorId : "",
                  ]
                    .filter((part) => part !== "")
                    .join(" ");
                  return (
                    <Field
                      className={spec.className}
                      data-invalid={fieldState.invalid}
                    >
                      <FieldLabel htmlFor={id}>{spec.label}</FieldLabel>
                      <Input
                        id={id}
                        {...spec.inputProps}
                        {...field}
                        value={field.value ?? ""}
                        aria-invalid={fieldState.invalid}
                        aria-describedby={
                          describedBy === "" ? undefined : describedBy
                        }
                      />
                      {spec.description === undefined ? null : (
                        <FieldDescription id={descriptionId}>
                          {spec.description}
                        </FieldDescription>
                      )}
                      <FieldError id={errorId} errors={[fieldState.error]} />
                    </Field>
                  );
                }}
              />
            ))}
          </FieldGroup>
        </CardContent>
        <CardFooter className="flex-wrap gap-2">
          <Button type="submit" disabled={mutation.isPending}>
            Save
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => form.reset(initialValues)}
          >
            Reset
          </Button>
          <BackButton />
        </CardFooter>
      </form>
    </Card>
  );
}

// The placeholder while the rule list loads, shaped like the form.
function FormSkeleton(): JSX.Element {
  return (
    <Card aria-busy="true">
      <CardContent className="flex flex-col gap-6">
        <p role="status" className="sr-only">
          Loading reward rule
        </p>
        <div className="grid gap-4 lg:grid-cols-2">
          {FIELDS.map((spec) => (
            <div
              key={spec.name}
              className={`flex flex-col gap-3 ${spec.className ?? ""}`}
            >
              <Skeleton className="h-4 w-40" />
              <Skeleton className="h-9 w-full" />
              {spec.description === undefined ? null : (
                <Skeleton className="h-4 w-48" />
              )}
            </div>
          ))}
        </div>
        <div className="flex gap-2">
          <Skeleton className="h-9 w-16" />
          <Skeleton className="h-9 w-16" />
          <Skeleton className="h-9 w-16" />
        </div>
      </CardContent>
    </Card>
  );
}

// A card in place of the form, with the Back link: the not-found text, or the
// message of a failed rule-list load.
function MessageCard({ message }: { message: string }): JSX.Element {
  return (
    <Card>
      <CardContent>
        <p className="text-sm">{message}</p>
      </CardContent>
      <CardFooter>
        <BackButton />
      </CardFooter>
    </Card>
  );
}

export default function RewardRuleFormPage(): JSX.Element {
  const { ruleId } = useParams();
  const isEdit = ruleId !== undefined;
  // No GET of a single rule exists: the rule is picked from the rule list, and
  // an empty ID sends no request (new mode).
  const ruleQuery = useRewardRule(ruleId ?? "");
  const rule = ruleQuery.data;
  const initialValues = useMemo(
    () => (rule === undefined ? EMPTY : rewardRuleToFormInput(rule)),
    [rule],
  );

  // Each failed load is toasted once, also across StrictMode's re-run of
  // effects, and its text stays on the page after the toast has gone.
  const [loadError, setLoadError] = useState<string>("");
  const shownError = useRef<unknown>(null);
  const { isError, error } = ruleQuery;
  useEffect(() => {
    if (isEdit && isError && error !== shownError.current) {
      shownError.current = error;
      setLoadError(showApiError(error).message);
    }
  }, [isEdit, isError, error]);

  const title = isEdit ? `Edit reward rule ${ruleId}` : "New reward rule";

  let body: JSX.Element;
  if (!isEdit) {
    body = (
      <RewardRuleForm
        key="new"
        ruleId={undefined}
        initialValues={EMPTY}
        label={title}
      />
    );
  } else if (rule !== undefined) {
    body = (
      <RewardRuleForm
        key={ruleId}
        ruleId={ruleId}
        initialValues={initialValues}
        label={title}
      />
    );
  } else if (ruleId === "") {
    // The disabled query would stay pending: no rule has an empty ID.
    body = <MessageCard message={REWARD_RULE_MESSAGES.notFound} />;
  } else if (ruleQuery.isPending) {
    body = <FormSkeleton />;
  } else if (isError) {
    body = <MessageCard message={loadError} />;
  } else {
    body = <MessageCard message={REWARD_RULE_MESSAGES.notFound} />;
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={title} />
      {body}
    </div>
  );
}
