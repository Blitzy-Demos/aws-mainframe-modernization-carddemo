// CU02 user update, the COUSR02 "Update User" screen of COUSR02C, at
// /admin/users/:userId/edit (AAP 0.5.4.1). GET /admin/users/{id} fills the
// form, PUT /admin/users/{id} saves it (AAP 0.3.1).
//
// - The user ID is the route parameter. The user list's "Edit" link, COUSR00's
//   selector U, carries it (F-COUSR00-005, F-COUSR02-001), and the admin
//   menu's "User Update (Security)" opens that list (F-COADM01-003). So the
//   page has no USRIDIN key field and no ENTER fetch control: a fetch by key
//   is the route that carries the key (E-56, E-61), and the user ID shows
//   read-only. A route without the parameter goes back to the list.
// - The keys are the map's controls: PF5 the form's submit "Save"
//   (F-COUSR02-004); PF3 "Save and exit", the same save and then the list
//   whatever its outcome, input the schema rejects included, which is not
//   sent, as COUSR02C XCTLs back after UPDATE-USER-INFO (F-COUSR02-002,
//   P-COUSR02C-008); PF4 "Reset" (F-COUSR02-003); PF12 "Cancel", which leaves
//   without a request (F-COUSR02-005). The map targets the list from both exits.
// - The current password is never shown, since only its argon2 hash is stored
//   (E-21). The password field starts empty and is disabled until "Change
//   password" is checked; unchecked, the request leaves `password` out, which
//   the server reads as the current password. A checked password goes out
//   exactly as typed, case kept and untrimmed (E-20).
// - Every message comes from the server through lib/api-errors.ts. The
//   message-only 200 of an unchanged save (V-COUSR02C-010) is shown as a
//   prompt and keeps the form as entered; the write's success text as a
//   success, the form then holding the saved user (P-COUSR02C-013).
import { type JSX, useEffect, useId, useRef } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Controller,
  type UseFormReturn,
  useForm,
  useWatch,
} from "react-hook-form";
import { Navigate, useNavigate, useParams } from "react-router";

import { useUser, useUserUpdateMutation } from "@/api/queries/admin-users";
import type { components } from "@/api/schema";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError, showMessage, showSuccess } from "@/lib/api-errors";
import { decodeKeySegment } from "@/lib/key-segment";
import {
  toUserUpdateRequest,
  USER_UPDATE_UNCHANGED_MESSAGE,
  userUpdateBmsFieldMap,
  userUpdateSchema,
  type UserUpdateFormInput,
  type UserUpdateFormValues,
} from "@/schemas/admin-users";

type UserSummary = components["schemas"]["UserSummary"];

type UserEditForm = UseFormReturn<
  UserUpdateFormInput,
  unknown,
  UserUpdateFormValues
>;

// The form members an Input edits; `change_password` is the checkbox.
type TextFieldName = Exclude<keyof UserUpdateFormInput, "change_password">;

// The user list, where the user to update is chosen (E-61), and the target
// both exits of the map name (F-COUSR02-002, F-COUSR02-005).
const USER_LIST_ROUTE = "/admin/users";

// The COUSR2A input widths, PIC X(n) [app/cpy-bms/COUSR02.CPY:L60-84]: USRIDINI
// X(8), FNAMEI and LNAMEI X(20), PASSWDI X(8), USRTYPEI X(1).
const MAX_LENGTH = {
  user_id: 8,
  first_name: 20,
  last_name: 20,
  password: 8,
  usr_type: 1,
} as const satisfies Record<TextFieldName, number>;

// The loading placeholder: the user ID across the form, then four fields.
const SKELETON_FIELDS = [
  { key: "user_id", className: "h-9 lg:col-span-2" },
  { key: "first_name", className: "h-9" },
  { key: "last_name", className: "h-9" },
  { key: "password", className: "h-9" },
  { key: "usr_type", className: "h-9" },
] as const;

// The form as PROCESS-ENTER-KEY fills it from the record (P-COUSR02C-002), the
// password left empty and unswitched because it cannot be shown (E-21).
function loadedValues(user: UserSummary): UserUpdateFormInput {
  return {
    user_id: user.user_id,
    first_name: user.first_name,
    last_name: user.last_name,
    change_password: false,
    password: "",
    usr_type: user.usr_type,
  };
}

// INITIALIZE-ALL-FIELDS blanks every input (P-COUSR02C-013). The user ID is
// kept: it is the route's key, which this screen cannot re-enter (E-61).
function clearedValues(userId: string): UserUpdateFormInput {
  return {
    user_id: userId,
    first_name: "",
    last_name: "",
    change_password: false,
    password: "",
    usr_type: "",
  };
}

// The aria-describedby value of the ids given, or nothing when none is.
function describedBy(...ids: (string | false)[]): string | undefined {
  const present = ids.filter((id): id is string => id !== false);
  return present.length === 0 ? undefined : present.join(" ");
}

type TextFieldProps = {
  form: UserEditForm;
  name: TextFieldName;
  label: string;
  description?: string;
  className?: string;
  type?: "text" | "password";
  readOnly?: boolean;
  disabled?: boolean;
  autoComplete?: string;
};

// One labelled COUSR2A input with its description and its error, which the
// schema or the server's `fields[]` sets.
function TextField({
  form,
  name,
  label,
  description,
  className,
  type = "text",
  readOnly = false,
  disabled = false,
  autoComplete = "off",
}: TextFieldProps): JSX.Element {
  const id = useId();
  const inputId = `${id}-input`;
  const descriptionId = `${id}-description`;
  const errorId = `${id}-error`;
  return (
    <Controller
      control={form.control}
      name={name}
      render={({ field, fieldState }) => (
        <Field
          data-invalid={fieldState.invalid}
          data-disabled={disabled}
          className={className}
        >
          <FieldLabel htmlFor={inputId}>{label}</FieldLabel>
          <Input
            id={inputId}
            ref={field.ref}
            name={field.name}
            type={type}
            value={field.value ?? ""}
            onChange={field.onChange}
            onBlur={field.onBlur}
            maxLength={MAX_LENGTH[name]}
            readOnly={readOnly}
            aria-readonly={readOnly ? "true" : undefined}
            // The dark variant is repeated so it replaces the Input's own
            // dark:bg-input/30, which a plain bg-muted leaves in place.
            className={readOnly ? "bg-muted dark:bg-muted" : undefined}
            disabled={disabled}
            autoComplete={autoComplete}
            spellCheck={false}
            aria-invalid={fieldState.invalid}
            aria-describedby={describedBy(
              description !== undefined && descriptionId,
              fieldState.invalid && errorId,
            )}
          />
          {description !== undefined ? (
            <FieldDescription id={descriptionId}>{description}</FieldDescription>
          ) : null}
          <FieldError id={errorId} errors={[fieldState.error]} />
        </Field>
      )}
    />
  );
}

// The "Change password" switch: checked, the password is entered and sent;
// unchecked, it is emptied and left out of the request (E-21).
function ChangePasswordField({ form }: { form: UserEditForm }): JSX.Element {
  const id = useId();
  const checkboxId = `${id}-checkbox`;
  const descriptionId = `${id}-description`;
  return (
    <Controller
      control={form.control}
      name="change_password"
      render={({ field }) => (
        <Field orientation="horizontal">
          <Checkbox
            id={checkboxId}
            ref={field.ref}
            checked={field.value === true}
            onCheckedChange={(checked) => {
              const on = checked === true;
              field.onChange(on);
              if (!on) {
                form.setValue("password", "");
                form.clearErrors("password");
              }
            }}
            onBlur={field.onBlur}
            aria-describedby={descriptionId}
          />
          <FieldContent>
            <FieldLabel htmlFor={checkboxId}>Change password</FieldLabel>
            <FieldDescription id={descriptionId}>
              The current password is never shown. Leave this unchecked to keep
              it.
            </FieldDescription>
          </FieldContent>
        </Field>
      )}
    />
  );
}

function UserEditSkeleton(): JSX.Element {
  return (
    <Card aria-busy="true">
      <CardContent className="grid gap-4 lg:grid-cols-2">
        <p role="status" className="sr-only">
          Loading user
        </p>
        {SKELETON_FIELDS.map(({ key, className }) => (
          <Skeleton key={key} className={className} />
        ))}
      </CardContent>
    </Card>
  );
}

type UserEditFormCardProps = {
  userId: string;
  user: UserSummary | undefined;
  fetchError: Error | null;
};

function UserEditFormCard({
  userId,
  user,
  fetchError,
}: UserEditFormCardProps): JSX.Element {
  const navigate = useNavigate();
  const mutation = useUserUpdateMutation(userId);
  const form: UserEditForm = useForm<
    UserUpdateFormInput,
    unknown,
    UserUpdateFormValues
  >({
    resolver: zodResolver(userUpdateSchema),
    defaultValues:
      user === undefined ? clearedValues(userId) : loadedValues(user),
  });
  const changePassword =
    useWatch({ control: form.control, name: "change_password" }) === true;

  // A newer read of the user, such as the one that follows a save, refills
  // the form.
  useEffect(() => {
    if (user !== undefined) {
      form.reset(loadedValues(user));
    }
  }, [form, user]);

  // A failed read shows its text and marks USRIDIN or FNAME, as COUSR02C
  // resends the map with the user ID and blank fields (V-COUSR02C-001 to
  // V-COUSR02C-003). Each failure is shown once, StrictMode's repeated effect
  // included.
  const shownFetchError = useRef<Error | null>(null);
  useEffect(() => {
    if (fetchError === null || shownFetchError.current === fetchError) {
      return;
    }
    shownFetchError.current = fetchError;
    applyApiError(fetchError, form, userUpdateBmsFieldMap);
  }, [form, fetchError]);

  const exitToList = (): void => {
    void navigate(USER_LIST_ROUTE);
  };

  // One save at a time, from Save or Save and exit: a second click or Enter
  // before the busy state re-renders would send the PUT again, and its answer
  // would replace the first one's. The ref closes that gap.
  const inFlight = useRef(false);

  function save(values: UserUpdateFormValues, exit: boolean): void {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    mutation.mutate(toUserUpdateRequest(values), {
      onSuccess: (answer) => {
        if (answer.message === USER_UPDATE_UNCHANGED_MESSAGE) {
          showMessage(answer.message);
          return;
        }
        showSuccess(answer.message);
        form.reset(loadedValues(answer.user));
      },
      onError: (error) => {
        applyApiError(error, form, userUpdateBmsFieldMap);
      },
      // Runs after either outcome; a page that has left has no guard to
      // release.
      onSettled: () => {
        inFlight.current = false;
        if (exit) {
          exitToList();
        }
      },
    });
  }

  const submit = form.handleSubmit((values) => {
    save(values, false);
  });

  // PF3 leaves for the list whatever the outcome (P-COUSR02C-008). Input the
  // schema rejects is not sent, and the list replaces the screen that marks
  // it, as the XCTL replaces the one UPDATE-USER-INFO sent.
  const submitAndExit = form.handleSubmit(
    (values) => {
      save(values, true);
    },
    () => {
      exitToList();
    },
  );

  // CLEAR-CURRENT-SCREEN: every input blank, the cursor on the user ID. The
  // field refs are kept so the focus can move before the form re-renders.
  const clear = (): void => {
    form.reset(clearedValues(form.getValues("user_id")), {
      keepFieldsRef: true,
    });
    form.setFocus("user_id");
  };

  // aria-disabled rather than disabled: a focused button that becomes
  // disabled loses focus to the document, which would strand a keyboard user
  // after Save; the guards keep the inert buttons from acting.
  const busy = mutation.isPending;
  const busyClass = "aria-disabled:pointer-events-none aria-disabled:opacity-50";

  return (
    <Card>
      <CardContent>
        <form
          noValidate
          aria-busy={busy}
          className="flex flex-col gap-6"
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <FieldGroup className="grid gap-4 lg:grid-cols-2">
            <TextField
              form={form}
              name="user_id"
              label="User ID"
              readOnly
              className="lg:col-span-2"
            />
            <TextField form={form} name="first_name" label="First Name" />
            <TextField form={form} name="last_name" label="Last Name" />
            <div className="flex flex-col gap-3">
              <ChangePasswordField form={form} />
              <TextField
                form={form}
                name="password"
                label="Password"
                type="password"
                autoComplete="new-password"
                disabled={!changePassword}
                description="Up to 8 characters, sent exactly as typed."
              />
            </div>
            {/* USRTYPE X(1) stays a textbox, not an A/U choice: COUSR02C edits it
                only for blank and stores any other value, which a choice list
                could neither show nor keep. */}
            <TextField
              form={form}
              name="usr_type"
              label="User Type"
              description="(A=Admin, U=User)"
            />
          </FieldGroup>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" aria-disabled={busy} className={busyClass}>
              Save
            </Button>
            <Button
              type="button"
              variant="secondary"
              aria-disabled={busy}
              className={busyClass}
              onClick={(event) => {
                if (busy) {
                  return;
                }
                void submitAndExit(event);
              }}
            >
              Save and exit
            </Button>
            <Button
              type="button"
              variant="outline"
              aria-disabled={busy}
              className={busyClass}
              onClick={() => {
                if (busy) {
                  return;
                }
                clear();
              }}
            >
              Reset
            </Button>
            <Button
              type="button"
              variant="ghost"
              aria-disabled={busy}
              className={busyClass}
              onClick={() => {
                if (busy) {
                  return;
                }
                exitToList();
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}

// One user's screen. The page keys it by user ID, so a new user gets a fresh
// form and the failure of a previous read is not carried over.
function UserEditScreen({ userId }: { userId: string }): JSX.Element {
  const user = useUser(userId);
  return (
    <div className="flex flex-col gap-6">
      <PageHeader />
      {user.isPending ? (
        <UserEditSkeleton />
      ) : (
        <UserEditFormCard
          userId={userId}
          user={user.data}
          fetchError={user.error}
        />
      )}
    </div>
  );
}

export default function UserEditPage(): JSX.Element {
  const { userId: segment } = useParams();
  if (segment === undefined || segment === "") {
    return <Navigate to={USER_LIST_ROUTE} replace />;
  }
  // The route carries the key segment of the user ID (lib/key-segment.ts).
  const userId = decodeKeySegment(segment);
  return <UserEditScreen key={userId} userId={userId} />;
}
