// CU01 user add: the COUSR01 "Add User" screen of COUSR01C [app/bms/COUSR01.bms]
// [app/cbl/COUSR01C.cbl], served at /admin/users/new (COADM02Y option 2, "User Add
// (Security)") and sending POST /admin/users.
//
// The 3270 keys are the controls of docs/parity/function-map.md:
// - ENTER=Add User is the form's submit button "Add user" (F-COUSR01-001). Enter in
//   any field submits, and handleSubmit keeps input the schema rejects off the network.
// - F4=Clear is "Reset" (F-COUSR01-003): every field emptied, errors cleared, cursor on
//   the first name, as CLEAR-CURRENT-SCREEN and INITIALIZE-ALL-FIELDS do.
// - F3=Back is the link "Back" to /admin/users (F-COUSR01-002). The map's PF3 rule
//   sends Back links to the parent route in the URL hierarchy, so it targets the user
//   list rather than the COADM01C menu the COBOL XCTLs to; the breadcrumb leads there
//   too.
// - F12=Exit has no control: COUSR01C handles no PF12 and answers it as an invalid key
//   (F-COUSR01-004, E-57). Other AID keys have none either (F-COUSR01-005, E-56).
//
// The empty-field edits of PROCESS-ENTER-KEY (V-COUSR01C-001 to V-COUSR01C-005) run in
// userAddSchema; the duplicate and write-failure texts (V-COUSR01C-006, V-COUSR01C-007)
// come from the server and land on the field its `fields[]` names. Every value is sent
// exactly as typed, case kept and never trimmed or padded: COUSR01C stores the user ID
// and the password as entered (E-20).
import { useEffect, type JSX } from "react";
import { Link } from "react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm, type Control } from "react-hook-form";

import { useUserAddMutation } from "@/api/queries/admin-users";
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
import { applyApiError, showSuccess } from "@/lib/api-errors";
import {
  userAddBmsFieldMap,
  userAddSchema,
  type UserAddFormValues,
} from "@/schemas/admin-users";

// INITIALIZE-ALL-FIELDS: every input SPACES, which a browser field holds as "".
const EMPTY_FORM: UserAddFormValues = {
  first_name: "",
  last_name: "",
  user_id: "",
  password: "",
  usr_type: "",
};

// FNAME carries the map's IC attribute, and every successful add and every clear moves
// the cursor back to it (MOVE -1 TO FNAMEL).
const INITIAL_FIELD = "first_name" satisfies keyof UserAddFormValues;

type TextFieldProps = {
  control: Control<UserAddFormValues>;
  name: keyof UserAddFormValues;
  /** The BMS caption without its colon. */
  label: string;
  /** The PIC X(n) width of the BMS input. */
  maxLength: number;
  /** The hint the map prints beside the input. */
  description?: string;
  type?: "text" | "password";
  autoComplete: string;
  /**
   * Keeps an on-screen keyboard from capitalising or correcting the value, which
   * must reach the server as typed (E-20).
   */
  verbatim?: boolean;
};

function TextField({
  control,
  name,
  label,
  maxLength,
  description,
  type = "text",
  autoComplete,
  verbatim = false,
}: TextFieldProps): JSX.Element {
  const inputId = `user-add-${name}`;
  const descriptionId = description === undefined ? undefined : `${inputId}-description`;
  const errorId = `${inputId}-error`;
  return (
    <Controller
      control={control}
      name={name}
      render={({ field, fieldState }) => {
        const describedBy = [descriptionId, fieldState.invalid ? errorId : undefined]
          .filter((id) => id !== undefined)
          .join(" ");
        return (
          <Field data-invalid={fieldState.invalid}>
            <FieldLabel htmlFor={inputId}>{label}</FieldLabel>
            <Input
              {...field}
              id={inputId}
              type={type}
              maxLength={maxLength}
              autoComplete={autoComplete}
              autoCapitalize={verbatim ? "none" : undefined}
              autoCorrect={verbatim ? "off" : undefined}
              spellCheck={verbatim ? false : undefined}
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

export default function UserAddPage(): JSX.Element {
  const form = useForm<UserAddFormValues>({
    resolver: zodResolver(userAddSchema),
    defaultValues: EMPTY_FORM,
  });
  const mutation = useUserAddMutation();

  useEffect(() => {
    form.setFocus(INITIAL_FIELD);
  }, [form]);

  // keepFieldsRef: a plain reset drops every registered input ref until the next
  // render, so the setFocus that follows it would find no element to focus.
  function clearForm(): void {
    form.reset(EMPTY_FORM, { keepFieldsRef: true });
    form.setFocus(INITIAL_FIELD);
  }

  function onSubmit(values: UserAddFormValues): void {
    mutation.mutate(values, {
      // WRITE-USER-SEC-FILE NORMAL: 'User <id> has been added ...' in green on a
      // cleared screen.
      onSuccess: (response) => {
        showSuccess(response.message);
        clearForm();
      },
      onError: (error) => {
        applyApiError(error, form, userAddBmsFieldMap);
      },
    });
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader />
      <form noValidate onSubmit={form.handleSubmit(onSubmit)}>
        <Card>
          <CardContent>
            {/* Two columns at lg pair the inputs as the map lays them out: names on
                line 8, user ID and password on line 11, user type on line 14. */}
            <FieldGroup className="grid gap-4 lg:grid-cols-2">
              <TextField
                control={form.control}
                name="first_name"
                label="First Name"
                maxLength={20}
                autoComplete="off"
              />
              <TextField
                control={form.control}
                name="last_name"
                label="Last Name"
                maxLength={20}
                autoComplete="off"
              />
              <TextField
                control={form.control}
                name="user_id"
                label="User ID"
                maxLength={8}
                description="(8 Char)"
                autoComplete="off"
                verbatim
              />
              <TextField
                control={form.control}
                name="password"
                label="Password"
                maxLength={8}
                description="(8 Char)"
                type="password"
                autoComplete="new-password"
                verbatim
              />
              {/* A textbox, not a choice of A and U: COUSR01C edits USRTYPE only for
                  blank and stores any other value as entered, and userAddSchema
                  accepts any non-blank ASCII character. */}
              <TextField
                control={form.control}
                name="usr_type"
                label="User Type"
                maxLength={1}
                description="(A=Admin, U=User)"
                autoComplete="off"
                verbatim
              />
            </FieldGroup>
          </CardContent>
          <CardFooter className="flex-wrap gap-2">
            <Button type="submit" disabled={mutation.isPending}>
              Add user
            </Button>
            <Button type="button" variant="outline" onClick={clearForm}>
              Reset
            </Button>
            <Button variant="ghost" asChild>
              <Link to="/admin/users">Back</Link>
            </Button>
          </CardFooter>
        </Card>
      </form>
    </div>
  );
}
