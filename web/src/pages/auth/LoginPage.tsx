// CC00 sign-on (COSGN00C, map COSGN00): the public sign-on screen at /login,
// row F-COSGN00-001 of docs/parity/function-map.md.
//
// - ENTER becomes the form's submit: Enter in either field, or the "Sign on"
//   button, sends POST /auth/login once loginSchema accepts the values.
// - The user ID and password are sent exactly as typed. COSGN00C upper-cases
//   both before it reads the user [app/cbl/COSGN00C.cbl:L132-136]; doing it
//   here would sign on a user whose password was stored in lower case (E-20).
// - A failed sign-on keeps both values, as COSGN00C resends the map with the
//   fields it received. applyApiError toasts the COSGN00C message, marks the
//   field the server names and puts the cursor there, the `MOVE -1 TO
//   USERIDL` or `PASSWDL` of each failure [app/cbl/COSGN00C.cbl:L117-127,
//   L241-256]. The messages are the server's and are never written here.
// - Success always lands on the response's `landing`, the menu for the user's
//   type (XCTL COADM01C or COMEN01C [app/cbl/COSGN00C.cbl:L230-240]), never on
//   the route a guard redirected from.
// - The map's F3=Exit has no control on this screen. The sign-on screen holds
//   no session to end, so F-COSGN00-002 places the sign-off on the "Log out"
//   menu item of the signed-on shell, and other AID keys have none (E-56).
import { useId, type HTMLInputTypeAttribute, type JSX } from "react";
import { Controller, useForm } from "react-hook-form";
import { useNavigate } from "react-router";
import { zodResolver } from "@hookform/resolvers/zod";
import { LoaderCircle } from "lucide-react";

import { useLoginMutation } from "@/api/queries/auth";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { applyApiError } from "@/lib/api-errors";
import {
  loginBmsFieldMap,
  loginSchema,
  type LoginFormValues,
} from "@/schemas/auth";

// The banner line of the COSGN00 map [app/bms/COSGN00.bms:L94-99].
const BANNER =
  "This is a Credit Card Demo Application for Mainframe Modernization";

// The width of USERIDI and PASSWDI, PIC X(8) [app/cpy-bms/COSGN00.CPY:L72,
// L78], which the map's "(8 Char)" hints state.
const MAP_FIELD_LENGTH = 8;

const DEFAULT_VALUES: LoginFormValues = { user_id: "", password: "" };

type SignOnField = {
  name: keyof LoginFormValues;
  label: string;
  type: HTMLInputTypeAttribute;
  autoComplete: string;
  autoFocus: boolean;
};

// The two input fields of the map in screen order, which is also the Tab
// order. The cursor starts on the user ID, the field's IC attribute
// [app/bms/COSGN00.bms:L156] and COSGN00C's `MOVE -1 TO USERIDL` on first
// display [app/cbl/COSGN00C.cbl:L80-83]. The password is a password input,
// the map's dark (DRK) field [app/bms/COSGN00.bms:L175].
const SIGN_ON_FIELDS: readonly SignOnField[] = [
  {
    name: "user_id",
    label: "User ID",
    type: "text",
    autoComplete: "username",
    autoFocus: true,
  },
  {
    name: "password",
    label: "Password",
    type: "password",
    autoComplete: "current-password",
    autoFocus: false,
  },
];

export default function LoginPage(): JSX.Element {
  const navigate = useNavigate();
  const login = useLoginMutation();
  const form = useForm<LoginFormValues>({
    resolver: zodResolver(loginSchema),
    defaultValues: DEFAULT_VALUES,
  });
  const baseId = useId();
  const titleId = `${baseId}-title`;

  // handleSubmit calls this only for values loginSchema accepts, so input
  // the server would reject on its blank or length edits is never sent.
  // useLoginMutation has stored the session before onSuccess runs.
  const onSubmit = (values: LoginFormValues): void => {
    login.mutate(values, {
      onSuccess: (response) => {
        void navigate(response.landing, { replace: true });
      },
      onError: (error) => {
        applyApiError(error, form, loginBmsFieldMap);
      },
    });
  };

  // BLITZY [A11Y]: the inputs and the button keep the shadcn defaults: a 36px
  // control height (h-9), below the 44px touch-target guideline, and a focus
  // ring in the built-in `ring/50` colour, below the WCAG AA 3:1 non-text
  // contrast against the light card. Kept as the design system defines them;
  // flagged for designer review.
  return (
    <main className="flex min-h-svh flex-col items-center justify-center gap-6 p-6">
      <div className="flex w-full max-w-sm flex-col gap-6">
        <PageHeader description={BANNER} />
        <Card>
          <CardHeader>
            <CardTitle id={titleId}>Sign on to CardDemo</CardTitle>
            <CardDescription>
              Type your User ID and Password, then press Enter.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              aria-labelledby={titleId}
              aria-busy={login.isPending}
              noValidate
              onSubmit={form.handleSubmit(onSubmit)}
            >
              <FieldGroup>
                {SIGN_ON_FIELDS.map((spec) => (
                  <Controller
                    key={spec.name}
                    name={spec.name}
                    control={form.control}
                    render={({ field, fieldState }) => {
                      const inputId = `${baseId}-${spec.name}`;
                      const descriptionId = `${inputId}-description`;
                      const errorId = `${inputId}-error`;
                      const hasErrorText =
                        fieldState.invalid &&
                        (fieldState.error?.message ?? "") !== "";
                      return (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor={inputId}>{spec.label}</FieldLabel>
                          <Input
                            {...field}
                            id={inputId}
                            type={spec.type}
                            maxLength={MAP_FIELD_LENGTH}
                            autoComplete={spec.autoComplete}
                            autoFocus={spec.autoFocus}
                            autoCapitalize="none"
                            autoCorrect="off"
                            spellCheck={false}
                            aria-invalid={fieldState.invalid}
                            aria-describedby={
                              hasErrorText
                                ? `${descriptionId} ${errorId}`
                                : descriptionId
                            }
                          />
                          <FieldDescription id={descriptionId}>
                            {MAP_FIELD_LENGTH} characters
                          </FieldDescription>
                          {fieldState.invalid ? (
                            <FieldError
                              id={errorId}
                              errors={[fieldState.error]}
                            />
                          ) : null}
                        </Field>
                      );
                    }}
                  />
                ))}
                <Button
                  type="submit"
                  className="w-full"
                  disabled={login.isPending}
                >
                  {login.isPending ? (
                    <LoaderCircle
                      aria-hidden="true"
                      className="motion-safe:animate-spin"
                    />
                  ) : null}
                  Sign on
                </Button>
              </FieldGroup>
            </form>
          </CardContent>
        </Card>
      </div>
    </main>
  );
}
