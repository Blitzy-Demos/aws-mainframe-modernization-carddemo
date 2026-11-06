// CAUP account update: COACTUPC on map COACTUP, at /accounts/:accountId/edit
// (docs/parity/function-map.md F-COACTUP-002 to F-COACTUP-006). The form is
// filled from `GET /accounts/{id}` as 3202-SHOW-ORIGINAL-VALUES fills the map
// and saved with `PUT /accounts/{id}`; the server repeats every edit, the
// fetched-values comparison and the record-changed check in that request.
//
// Controls are only those of the map rows:
// - The account number comes from the route. The key is entered in the
//   dashboard lookup (F-COACTUP-001), so this screen has no key form, and
//   ACCTSID shows read-only, as COACTUPC protects it once the details are
//   fetched (3300-SETUP-SCREEN-ATTRS). Customer id and Country stay read-only
//   too (3320-UNPROTECT-FEW-ATTRS leaves ACSTNUM and ACSCTRY protected), and
//   all three travel back as fetched. A route key the key edits reject is
//   not read: the screen shows the edit's text instead of the form.
// - ENTER's validation and the F5 write are one submit, "Save" (E-56).
// - F12 "Cancel" re-reads the record and drops unsaved changes. Save and
//   Cancel take turns: neither acts while the other's request runs.
// - F3 "Back" returns to the account view.
// - COACTUP's legend offers no F4, so the page has no "Reset" control.
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { useParams } from "react-router";
import { Controller, useForm, type Control } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { LoaderCircle } from "lucide-react";

import {
  useAccount,
  useAccountUpdateMutation,
} from "@/api/queries/accounts";
import type { components } from "@/api/schema";
import { BackLink, PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter } from "@/components/ui/card";
import {
  Field,
  FieldError,
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
  accountKeyError,
  accountUpdateBmsFieldMap,
  accountUpdateSchema,
  toAccountUpdateRequest,
  type AccountUpdateFormValues,
} from "@/schemas/accounts";

type AccountResponse = components["schemas"]["AccountResponse"];

// A form value that has an input; `version` lives only in the form values.
type InputName = Exclude<keyof AccountUpdateFormValues, "version">;

// The BMS title of COACTUP [app/bms/COACTUP.bms:L78].
const TITLE = "Update Account";

// Placeholder rows shown while the record is read.
const SKELETON_ROWS = 8;

/**
 * COACTUPC 3200-SETUP-SCREEN-VARS — P-COACTUPC-036,
 * 3202-SHOW-ORIGINAL-VALUES — P-COACTUPC-037: the form values of `account`,
 * split into the map's parts as COACTUPC moves them: the `X(10)` dates by
 * `(1:4)`, `(6:2)` and `(9:2)` [app/cbl/COACTUPC.cbl:L3832-3834]
 * [app/cbl/COACTUPC.cbl:L3857-3859], the SSN by `(1:3)`, `(4:2)` and `(6:4)`,
 * each `(xxx)xxx-xxxx` phone by `(2:3)`, `(6:3)` and `(10:4)`
 * [app/cbl/COACTUPC.cbl:L2826-2851]. The `X(10)` zip is moved to the
 * 5-character ACSZIPC, which keeps its first five characters. Amounts stay the
 * decimal strings the server sent. `version` is carried so that the save echoes
 * the token of the record shown.
 */
function toFormValues(account: AccountResponse): AccountUpdateFormValues {
  return {
    version: account.version,
    account_id: account.account_id,
    acct_active_status: account.acct_active_status,
    acct_open_year: account.acct_open_date.slice(0, 4),
    acct_open_mon: account.acct_open_date.slice(5, 7),
    acct_open_day: account.acct_open_date.slice(8, 10),
    acct_credit_limit: account.acct_credit_limit,
    acct_exp_year: account.acct_expiraion_date.slice(0, 4),
    acct_exp_mon: account.acct_expiraion_date.slice(5, 7),
    acct_exp_day: account.acct_expiraion_date.slice(8, 10),
    acct_cash_credit_limit: account.acct_cash_credit_limit,
    acct_reissue_year: account.acct_reissue_date.slice(0, 4),
    acct_reissue_mon: account.acct_reissue_date.slice(5, 7),
    acct_reissue_day: account.acct_reissue_date.slice(8, 10),
    acct_curr_bal: account.acct_curr_bal,
    acct_curr_cyc_credit: account.acct_curr_cyc_credit,
    acct_group_id: account.acct_group_id,
    acct_curr_cyc_debit: account.acct_curr_cyc_debit,
    customer_id: account.customer_id,
    cust_ssn_1: account.cust_ssn.slice(0, 3),
    cust_ssn_2: account.cust_ssn.slice(3, 5),
    cust_ssn_3: account.cust_ssn.slice(5, 9),
    cust_dob_year: account.cust_dob_yyyy_mm_dd.slice(0, 4),
    cust_dob_mon: account.cust_dob_yyyy_mm_dd.slice(5, 7),
    cust_dob_day: account.cust_dob_yyyy_mm_dd.slice(8, 10),
    cust_fico_credit_score: account.cust_fico_credit_score,
    cust_first_name: account.cust_first_name,
    cust_middle_name: account.cust_middle_name,
    cust_last_name: account.cust_last_name,
    cust_addr_line_1: account.cust_addr_line_1,
    cust_addr_state_cd: account.cust_addr_state_cd,
    cust_addr_line_2: account.cust_addr_line_2,
    cust_addr_zip: account.cust_addr_zip.slice(0, 5),
    cust_addr_line_3: account.cust_addr_line_3,
    cust_addr_country_cd: account.cust_addr_country_cd,
    cust_phone_num_1a: account.cust_phone_num_1.slice(1, 4),
    cust_phone_num_1b: account.cust_phone_num_1.slice(5, 8),
    cust_phone_num_1c: account.cust_phone_num_1.slice(9, 13),
    cust_govt_issued_id: account.cust_govt_issued_id,
    cust_phone_num_2a: account.cust_phone_num_2.slice(1, 4),
    cust_phone_num_2b: account.cust_phone_num_2.slice(5, 8),
    cust_phone_num_2c: account.cust_phone_num_2.slice(9, 13),
    cust_eft_account_id: account.cust_eft_account_id,
    cust_pri_card_holder_ind: account.cust_pri_card_holder_ind,
  };
}

type TextFieldProps = {
  control: Control<AccountUpdateFormValues>;
  name: InputName;
  // The COACTUP caption without its colon, unique on the page.
  label: ReactNode;
  // The BMS field length [app/cpy-bms/COACTUP.CPY].
  maxLength: number;
  // Digits only: a numeric keypad on touch devices.
  numeric?: boolean;
  // A field COACTUPC protects once the details are shown.
  readOnly?: boolean;
  // Placement of the field in its section's grid.
  className?: string;
};

// One map field. The Controller's `field` (with its `ref`) goes to the Input,
// so a client or server error focuses it.
function TextField({
  control,
  name,
  label,
  maxLength,
  numeric = false,
  readOnly = false,
  className,
}: TextFieldProps): JSX.Element {
  const id = `acup-${name}`;
  const errorId = `${id}-error`;
  return (
    <Controller
      control={control}
      name={name}
      render={({ field, fieldState }) => (
        <Field data-invalid={fieldState.invalid} className={className}>
          <FieldLabel htmlFor={id}>{label}</FieldLabel>
          <Input
            {...field}
            id={id}
            maxLength={maxLength}
            inputMode={numeric ? "numeric" : undefined}
            readOnly={readOnly}
            aria-invalid={fieldState.invalid}
            aria-describedby={fieldState.invalid ? errorId : undefined}
            autoComplete="off"
            spellCheck={false}
            className={readOnly ? "bg-muted dark:bg-muted" : undefined}
          />
          <FieldError id={errorId} errors={[fieldState.error]} />
        </Field>
      )}
    />
  );
}

type Part = { name: InputName; label: string; maxLength: number };

type PartsFieldProps = {
  control: Control<AccountUpdateFormValues>;
  // The COACTUP caption of the whole value, such as "Opened" or "Phone 1".
  legend: string;
  parts: readonly [Part, Part, Part];
};

// A value the map splits into three numeric fields (a date, the SSN, a phone
// number). Each part shows its short label; its accessible name is prefixed
// with the legend, so every input has a unique name.
function PartsField({ control, legend, parts }: PartsFieldProps): JSX.Element {
  return (
    <FieldSet className="gap-3">
      <FieldLegend variant="label">{legend}</FieldLegend>
      <div className="grid grid-cols-3 gap-2">
        {parts.map((part) => (
          <TextField
            key={part.name}
            control={control}
            name={part.name}
            maxLength={part.maxLength}
            numeric
            label={
              // The space stays outside the hidden span: accessible-name
              // computation trims each element's text, so a space inside it
              // would join the legend and the part ("OpenedYear").
              <>
                <span className="sr-only">{legend}</span>
                {` ${part.label}`}
              </>
            }
          />
        ))}
      </div>
    </FieldSet>
  );
}

// The year, month and day parts of a CSUTLDPY date.
function dateParts(
  year: InputName,
  month: InputName,
  day: InputName,
): readonly [Part, Part, Part] {
  return [
    { name: year, label: "Year", maxLength: 4 },
    { name: month, label: "Month", maxLength: 2 },
    { name: day, label: "Day", maxLength: 2 },
  ];
}

// The area code, prefix and line number of a US phone number.
function phoneParts(
  area: InputName,
  prefix: InputName,
  line: InputName,
): readonly [Part, Part, Part] {
  return [
    { name: area, label: "Area code", maxLength: 3 },
    { name: prefix, label: "Prefix", maxLength: 3 },
    { name: line, label: "Line number", maxLength: 4 },
  ];
}

const SSN_PARTS: readonly [Part, Part, Part] = [
  { name: "cust_ssn_1", label: "First 3", maxLength: 3 },
  { name: "cust_ssn_2", label: "4th & 5th", maxLength: 2 },
  { name: "cust_ssn_3", label: "Last 4", maxLength: 4 },
];

// A titled group of the form, one column up to `lg` and two from there. The
// children fill the rows in order, so the reading order is the visual order
// at both widths. Each row pairs two fields of one shape, two single fields or
// two three-part values, so the inputs of a row line up; a lone field ends its
// row. The form's last group carries the form's actions in its card footer.
function FormSection({
  legend,
  children,
  footer,
}: {
  legend: string;
  children: ReactNode;
  footer?: ReactNode;
}): JSX.Element {
  return (
    <Card>
      <CardContent>
        <FieldSet>
          <FieldLegend>{legend}</FieldLegend>
          <div className="grid gap-4 lg:grid-cols-2">{children}</div>
        </FieldSet>
      </CardContent>
      {footer === undefined ? null : (
        <CardFooter className="flex-wrap gap-2">{footer}</CardFooter>
      )}
    </Card>
  );
}

// A failed save, and the account it was sent for.
type SaveFailure = { accountId: string; error: unknown };

type AccountEditFormProps = {
  accountId: string;
  // The record as last read or saved.
  account: AccountResponse;
  // Reads the record again: the reloaded record, or the cached one when the
  // read fails.
  reload: () => Promise<AccountResponse | undefined>;
  // A failed save not shown yet.
  failure: SaveFailure | null;
  setFailure: (failure: SaveFailure | null) => void;
};

// The COACTUP input fields of one fetched account, with Save and Cancel.
function AccountEditForm({
  accountId,
  account,
  reload,
  failure,
  setFailure,
}: AccountEditFormProps): JSX.Element {
  const form = useForm<AccountUpdateFormValues>({
    resolver: zodResolver(accountUpdateSchema),
    defaultValues: toFormValues(account),
  });
  const { control, reset } = form;
  const mutation = useAccountUpdateMutation(accountId);
  // The record the form was last reset from.
  const [formAccount, setFormAccount] = useState(account);
  // Whether the form is on screen: a save can settle after it has left.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // A record that is not the one the form shows replaces the entries: the
  // reload after a 409 'Record changed by some one else. Please review'
  // brings its new values and token. A reload that returns identical data
  // keeps the same object (structural sharing), so the user's entries stay.
  useEffect(() => {
    if (account !== formAccount) {
      reset(toFormValues(account));
      setFormAccount(account);
    }
  }, [account, formAccount, reset]);

  // A failed save marks its fields only once the form shows the record its
  // reload brought, as COACTUPC redisplays the changed record with its
  // message: a reset clears the errors and drops the inputs' registrations
  // until they render again, so focus would find no input before it. A
  // failure of another account, whose form the route replaced while the save
  // ran, is still toasted.
  useEffect(() => {
    if (failure === null) {
      return;
    }
    if (failure.accountId !== accountId) {
      showApiError(failure.error);
      setFailure(null);
    } else if (account === formAccount) {
      applyApiError(failure.error, form, accountUpdateBmsFieldMap);
      setFailure(null);
    }
  }, [failure, accountId, account, formAccount, form, setFailure]);

  // One request at a time, a save or Cancel's re-read: a read answered with
  // the record as it stood before a save could otherwise land after it and
  // bring back the old values and token. The ref closes the gap before the
  // busy state re-renders, so a second Enter or click sends nothing.
  const inFlight = useRef(false);
  const [reading, setReading] = useState(false);

  const onSave = async (values: AccountUpdateFormValues): Promise<void> => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    try {
      const response = await mutation.mutateAsync(
        toAccountUpdateRequest(values),
      );
      // Values equal to the fetched record write nothing and answer 'No
      // change detected with respect to values fetched.' (V-COACTUPC-010), a
      // prompt rather than a success; the record then keeps the token sent.
      if (response.account.version === values.version) {
        showMessage(response.message);
      } else {
        showSuccess(response.message);
      }
      reset(toFormValues(response.account));
    } catch (error) {
      // Shown by the failure effect, or at once when the form has left.
      if (mounted.current) {
        setFailure({ accountId, error });
      } else {
        showApiError(error);
      }
    } finally {
      inFlight.current = false;
    }
  };

  // F12: 9000-READ-ACCT reads the record again and the map is refilled. The
  // form is reset here, since an unchanged record keeps its object.
  // `formAccount` is left to the record effect: the prop holds the earlier
  // record until the page hears of the read, and the effect would reset the
  // form back to it.
  const onCancel = async (): Promise<void> => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    setReading(true);
    try {
      const reloaded = await reload();
      reset(toFormValues(reloaded ?? account));
    } finally {
      inFlight.current = false;
      setReading(false);
    }
  };

  // aria-disabled rather than disabled: a focused button that becomes
  // disabled loses focus to the document, which would strand a keyboard user
  // after Save or Cancel; the guard above keeps the inert buttons from acting.
  // While a request runs the form is aria-busy and the button that sent it
  // shows a spinner, as on sign-on, so the wait is seen and announced.
  const busy = mutation.isPending || reading;
  const busyClass = "aria-disabled:pointer-events-none aria-disabled:opacity-50";

  return (
    <form
      aria-busy={busy}
      noValidate
      onSubmit={form.handleSubmit(onSave)}
      className="flex flex-col gap-6"
    >
      <FormSection legend="Account">
        <TextField
          control={control}
          name="account_id"
          label="Account Number"
          maxLength={11}
          numeric
          readOnly
        />
        <TextField
          control={control}
          name="acct_active_status"
          label="Active Y/N"
          maxLength={1}
        />
        <TextField
          control={control}
          name="acct_credit_limit"
          label="Credit Limit"
          maxLength={15}
        />
        <TextField
          control={control}
          name="acct_cash_credit_limit"
          label="Cash credit Limit"
          maxLength={15}
        />
        <TextField
          control={control}
          name="acct_curr_bal"
          label="Current Balance"
          maxLength={15}
        />
        <TextField
          control={control}
          name="acct_group_id"
          label="Account Group"
          maxLength={10}
        />
        <TextField
          control={control}
          name="acct_curr_cyc_credit"
          label="Current Cycle Credit"
          maxLength={15}
        />
        <TextField
          control={control}
          name="acct_curr_cyc_debit"
          label="Current Cycle Debit"
          maxLength={15}
        />
        <PartsField
          control={control}
          legend="Opened"
          parts={dateParts("acct_open_year", "acct_open_mon", "acct_open_day")}
        />
        <PartsField
          control={control}
          legend="Expiry"
          parts={dateParts("acct_exp_year", "acct_exp_mon", "acct_exp_day")}
        />
        <PartsField
          control={control}
          legend="Reissue"
          parts={dateParts(
            "acct_reissue_year",
            "acct_reissue_mon",
            "acct_reissue_day",
          )}
        />
      </FormSection>

      <FormSection legend="Customer Details">
        <TextField
          control={control}
          name="customer_id"
          label="Customer id"
          maxLength={9}
          numeric
          readOnly
        />
        <TextField
          control={control}
          name="cust_fico_credit_score"
          label="FICO Score"
          maxLength={3}
          numeric
        />
        <PartsField control={control} legend="SSN" parts={SSN_PARTS} />
        <PartsField
          control={control}
          legend="Date of birth"
          parts={dateParts("cust_dob_year", "cust_dob_mon", "cust_dob_day")}
        />
        <TextField
          control={control}
          name="cust_first_name"
          label="First Name"
          maxLength={25}
        />
        <TextField
          control={control}
          name="cust_middle_name"
          label="Middle Name"
          maxLength={25}
        />
        <TextField
          control={control}
          name="cust_last_name"
          label="Last Name"
          maxLength={25}
        />
        <TextField
          control={control}
          name="cust_addr_line_1"
          label="Address line 1"
          maxLength={50}
          className="lg:col-start-1"
        />
        <TextField
          control={control}
          name="cust_addr_line_2"
          label="Address line 2"
          maxLength={50}
        />
        <TextField
          control={control}
          name="cust_addr_line_3"
          label="City"
          maxLength={50}
        />
        <TextField
          control={control}
          name="cust_addr_state_cd"
          label="State"
          maxLength={2}
        />
        <TextField
          control={control}
          name="cust_addr_zip"
          label="Zip"
          maxLength={5}
          numeric
        />
        <TextField
          control={control}
          name="cust_addr_country_cd"
          label="Country"
          maxLength={3}
          readOnly
        />
      </FormSection>

      <FormSection
        legend="Contact and other details"
        footer={
          <>
            <Button type="submit" aria-disabled={busy} className={busyClass}>
              {mutation.isPending ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="motion-safe:animate-spin"
                />
              ) : null}
              Save
            </Button>
            <Button
              type="button"
              variant="outline"
              aria-disabled={busy}
              className={busyClass}
              onClick={() => {
                void onCancel();
              }}
            >
              {reading ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="motion-safe:animate-spin"
                />
              ) : null}
              Cancel
            </Button>
          </>
        }
      >
        <PartsField
          control={control}
          legend="Phone 1"
          parts={phoneParts(
            "cust_phone_num_1a",
            "cust_phone_num_1b",
            "cust_phone_num_1c",
          )}
        />
        <PartsField
          control={control}
          legend="Phone 2"
          parts={phoneParts(
            "cust_phone_num_2a",
            "cust_phone_num_2b",
            "cust_phone_num_2c",
          )}
        />
        <TextField
          control={control}
          name="cust_govt_issued_id"
          label="Government Issued Id Ref"
          maxLength={20}
        />
        <TextField
          control={control}
          name="cust_eft_account_id"
          label="EFT Account Id"
          maxLength={10}
          numeric
        />
        <TextField
          control={control}
          name="cust_pri_card_holder_ind"
          label="Primary Card Holder Y/N"
          maxLength={1}
        />
      </FormSection>
    </form>
  );
}

// Toasts each failed read of the record once, as the message line shows it,
// and returns its text for the page while the read stays failed.
function useLoadErrorMessage(error: Error | null): string | null {
  const shown = useRef<Error | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    if (error === null || error === shown.current) {
      return;
    }
    shown.current = error;
    setMessage(showApiError(error).message);
  }, [error]);
  return error === null ? null : message;
}

// The record while it is read.
function LoadingAccount(): JSX.Element {
  return (
    <Card aria-busy="true">
      <CardContent className="grid gap-4 lg:grid-cols-2">
        {Array.from({ length: SKELETON_ROWS }, (_, row) => (
          <Skeleton key={row} className="h-9 w-full" />
        ))}
      </CardContent>
    </Card>
  );
}

export default function AccountEditPage(): JSX.Element {
  const { accountId = "" } = useParams();
  // The key edits on the route key, with the texts `GET /accounts/{id}`
  // answers them with: COACTUPC performs 9000-READ-ACCT only once the key
  // passes [app/cbl/COACTUPC.cbl:L2568-2576], so a rejected key reaches the
  // hook as a blank, which it never sends.
  const keyError = useMemo(
    () => (accountId === "" ? null : accountKeyError(accountId)),
    [accountId],
  );
  const { data: account, error, refetch: reread } = useAccount(
    keyError === null ? accountId : "",
  );
  const loadMessage = useLoadErrorMessage(error);
  // A failed save is held here rather than in the form. The save settles
  // once its reload is in the cache, but TanStack Query tells this page about
  // the new data only on a later timer task; a render of this page reads it
  // at once, so the form gets the reloaded record with the failure.
  const [failure, setFailure] = useState<SaveFailure | null>(null);

  let body: ReactNode = null;
  if (keyError !== null) {
    // The edit's text where a failed read's would stand. No request carried
    // it, so there is no toast.
    body = (
      <Card>
        <CardContent>
          <FieldError>{keyError}</FieldError>
        </CardContent>
      </Card>
    );
  } else if (account !== undefined) {
    body = (
      <AccountEditForm
        key={accountId}
        accountId={accountId}
        account={account}
        reload={async () => (await reread()).data}
        failure={failure}
        setFailure={setFailure}
      />
    );
  } else if (error !== null) {
    body =
      loadMessage === null ? null : (
        <Card>
          <CardContent>
            <p className="text-sm text-destructive">{loadMessage}</p>
          </CardContent>
        </Card>
      );
  } else if (accountId !== "") {
    body = <LoadingAccount />;
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={TITLE}
        actions={
          accountId === "" ? null : (
            <BackLink to={`/accounts/${encodeURIComponent(accountId)}`} />
          )
        }
      />
      {body}
    </div>
  );
}

