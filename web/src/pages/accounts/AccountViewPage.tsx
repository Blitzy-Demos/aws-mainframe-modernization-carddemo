// CAVW account view: COACTVWC and its map COACTVW, at /accounts/:accountId.
//
// The route carries the account number: the dashboard lookup links here
// (F-COACTVW-002) and the page reads `GET /accounts/{id}` for it. The map's
// one input field, ACCTSID, stays on the page so another account can be
// looked up in place, as ENTER on COACTVW does; without a route key the page
// shows only that field, the map COACTVWC sends on first entry. Every other
// value is read-only and rendered as the API returns it: amounts are decimal
// strings and are never turned into numbers.
//
// The key edits and the read failures are the server's V-COACTVWC answers.
// The form schema mirrors the key edits and the route key goes through them
// too, so the page never sends a key the server is certain to refuse: a key
// they reject, typed or in the URL, gets its text at ACCTSID with the cursor
// there and no read, as a key error skips 9000-READ-ACCT
// [app/cbl/COACTVWC.cbl:L361-374]. A failed read is shown the same way, with
// a toast as well.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm } from "react-hook-form";
import { useNavigate, useParams } from "react-router";

import { useAccount, type Account } from "@/api/queries/accounts";
import {
  DetailItem,
  DetailList,
  DetailValueSkeleton,
  FlagBadge,
} from "@/components/detail-list";
import { BackLink, PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { applyApiError } from "@/lib/api-errors";
import {
  accountKeyError,
  accountKeySchema,
  accountViewBmsFieldMap,
  type AccountKeyValues,
} from "@/schemas/accounts";

/**
 * The INFOMSG line of COACTVW. COACTVWC sets WS-PROMPT-FOR-INPUT whenever no
 * other information message is set, and it never sets one, so the map always
 * carries this text [app/cbl/COACTVWC.cbl:L113-114]
 * [app/cbl/COACTVWC.cbl:L528-534].
 */
const INFO_MESSAGE = "Enter or update id of account to display";

/** ACCTSID `LENGTH=11`, `PICIN='99999999999'` [app/bms/COACTVW.bms:L84-90]. */
const ACCOUNT_ID_LENGTH = 11;

// How long the read's status stays empty before it announces. The region is
// cleared and then refilled, so the identical result of a repeated lookup is
// still announced as a change; the delay is the one DataTable uses.
const STATUS_ANNOUNCE_DELAY_MS = 150;

/**
 * One caption of the account view, the value it shows from the account read
 * and, for a value of more than one line, the placeholder that stands in for
 * it while the account is read.
 */
type AccountField = {
  label: string;
  value: (account: Account) => ReactNode;
  placeholder?: ReactNode;
};

/**
 * ACSTSSN as 1200-SETUP-SCREEN-VARS builds it: `CUST-SSN(1:3) '-'
 * CUST-SSN(4:2) '-' CUST-SSN(6:4)` [app/cbl/COACTVWC.cbl:L496-504]. This only
 * places the map's separators between the received characters; a value that
 * is not the nine digits of `CUST-SSN 9(09)` is shown as received.
 */
function ssnText(ssn: string): string {
  return /^[0-9]{9}$/.test(ssn)
    ? `${ssn.slice(0, 3)}-${ssn.slice(3, 5)}-${ssn.slice(5)}`
    : ssn;
}

// The map's two columns: status, dates and group on the left, amounts on the
// right [app/bms/COACTVW.bms:L93-198]. From `lg` the list fills column by
// column, five entries each, so the columns keep that meaning.
const ACCOUNT_FIELDS: readonly AccountField[] = [
  {
    label: "Active Y/N",
    value: (account) => <FlagBadge value={account.acct_active_status} />,
  },
  { label: "Opened", value: (account) => account.acct_open_date },
  { label: "Expiry", value: (account) => account.acct_expiraion_date },
  { label: "Reissue", value: (account) => account.acct_reissue_date },
  { label: "Account Group", value: (account) => account.acct_group_id },
  { label: "Credit Limit", value: (account) => account.acct_credit_limit },
  {
    label: "Cash credit Limit",
    value: (account) => account.acct_cash_credit_limit,
  },
  { label: "Current Balance", value: (account) => account.acct_curr_bal },
  {
    label: "Current Cycle Credit",
    value: (account) => account.acct_curr_cyc_credit,
  },
  {
    label: "Current Cycle Debit",
    value: (account) => account.acct_curr_cyc_debit,
  },
];

// Customer Details, in the map's row groups [app/bms/COACTVW.bms:L199-353]:
// identity, names, postal address, then contact and references.
const IDENTITY_FIELDS: readonly AccountField[] = [
  { label: "Customer id", value: (account) => account.customer_id },
  { label: "SSN", value: (account) => ssnText(account.cust_ssn) },
  { label: "Date of birth", value: (account) => account.cust_dob_yyyy_mm_dd },
  { label: "FICO Score", value: (account) => account.cust_fico_credit_score },
];
const NAME_FIELDS: readonly AccountField[] = [
  { label: "First Name", value: (account) => account.cust_first_name },
  { label: "Middle Name", value: (account) => account.cust_middle_name },
  { label: "Last Name", value: (account) => account.cust_last_name },
];
const ADDRESS_FIELDS: readonly AccountField[] = [
  // ACSADL1 and ACSADL2 sit under the one "Address:" caption, as the two
  // lines of one value.
  {
    label: "Address",
    value: (account) => (
      <span className="flex flex-col">
        <span>{account.cust_addr_line_1}</span>
        <span>{account.cust_addr_line_2}</span>
      </span>
    ),
    placeholder: (
      <div>
        <DetailValueSkeleton className="w-48" />
        <DetailValueSkeleton />
      </div>
    ),
  },
  { label: "City", value: (account) => account.cust_addr_line_3 },
  { label: "State", value: (account) => account.cust_addr_state_cd },
  { label: "Zip", value: (account) => account.cust_addr_zip },
  { label: "Country", value: (account) => account.cust_addr_country_cd },
];
const CONTACT_FIELDS: readonly AccountField[] = [
  { label: "Phone 1", value: (account) => account.cust_phone_num_1 },
  {
    label: "Government Issued Id Ref",
    value: (account) => account.cust_govt_issued_id,
  },
  { label: "Phone 2", value: (account) => account.cust_phone_num_2 },
  { label: "EFT Account Id", value: (account) => account.cust_eft_account_id },
  {
    label: "Primary Card Holder Y/N",
    value: (account) => <FlagBadge value={account.cust_pri_card_holder_ind} />,
  },
];
const CARD_FIELDS: readonly AccountField[] = [
  { label: "Card Number", value: (account) => account.card_number },
];

/**
 * One list of account fields, each value from the account read or, while it
 * is read (`account` undefined), its placeholder: one text line unless the
 * field names its own.
 */
function AccountFieldList({
  fields,
  account,
  className,
}: {
  fields: readonly AccountField[];
  account: Account | undefined;
  className?: string;
}): JSX.Element {
  return (
    <DetailList className={className}>
      {fields.map(({ label, value, placeholder }) => (
        <DetailItem key={label} label={label}>
          {account === undefined
            ? (placeholder ?? <DetailValueSkeleton />)
            : value(account)}
        </DetailItem>
      ))}
    </DetailList>
  );
}

/** A details card whose title is the section's h2 (PageHeader holds the h1). */
function DetailsCard({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>{title}</h2>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">{children}</CardContent>
    </Card>
  );
}

/**
 * The COACTVW output fields 1200-SETUP-SCREEN-VARS fills from the account and
 * customer records [app/cbl/COACTVWC.cbl:L473-522], labelled with the map
 * captions without their colons [app/bms/COACTVW.bms:L93-353], plus the
 * cross-referenced card number the API adds.
 *
 * While the account is read (`account` undefined) the same cards, captions
 * and lines hold placeholders, so nothing moves when the record arrives. The
 * details are then hidden from assistive technology: the page's status region
 * announces the read.
 */
function AccountDetails({
  account,
}: {
  account: Account | undefined;
}): JSX.Element {
  return (
    <div
      aria-hidden={account === undefined ? true : undefined}
      className="flex flex-col gap-6"
    >
      <DetailsCard title="Account">
        <AccountFieldList
          fields={ACCOUNT_FIELDS}
          account={account}
          className="lg:grid-flow-col lg:grid-rows-5"
        />
      </DetailsCard>
      <DetailsCard title="Customer Details">
        <AccountFieldList fields={IDENTITY_FIELDS} account={account} />
        <Separator />
        <AccountFieldList fields={NAME_FIELDS} account={account} />
        <Separator />
        <AccountFieldList fields={ADDRESS_FIELDS} account={account} />
        <Separator />
        <AccountFieldList fields={CONTACT_FIELDS} account={account} />
      </DetailsCard>
      <DetailsCard title="Card">
        <AccountFieldList fields={CARD_FIELDS} account={account} />
      </DetailsCard>
    </div>
  );
}

export default function AccountViewPage(): JSX.Element {
  const { accountId = "" } = useParams();
  const navigate = useNavigate();
  const inputId = useId();
  const errorId = useId();

  const form = useForm<AccountKeyValues>({
    resolver: zodResolver(accountKeySchema),
    defaultValues: { account_id: accountId },
  });

  // 2210-EDIT-ACCOUNT on the route key: its text, or `null` for a key that
  // passes and for none at all, the key-entry state.
  const routeKeyError = useMemo(
    () => (accountId === "" ? null : accountKeyError(accountId)),
    [accountId],
  );
  const readsAccount = accountId !== "" && routeKeyError === null;
  // The hook sends no request for an empty key, so the key-entry state and a
  // route key the edits reject, passed as a blank, read nothing.
  const account = useAccount(readsAccount ? accountId : "");
  const { error, isFetching, refetch: readAccount } = account;

  // ACCTSID shows the key of the account on screen: a new route key replaces
  // whatever was typed, and its errors with it. A route key the edits reject
  // is then marked with its text and the cursor placed on it, as
  // 1300-SETUP-SCREEN-ATTRS positions it [app/cbl/COACTVWC.cbl:L545-552].
  // keepFieldsRef keeps the input's registered ref, which a plain reset drops
  // until the next render, so setFocus finds it now.
  useEffect(() => {
    form.reset({ account_id: accountId }, { keepFieldsRef: true });
    if (routeKeyError !== null) {
      form.setError("account_id", { type: "custom", message: routeKeyError });
      form.setFocus("account_id");
    }
  }, [accountId, routeKeyError, form]);

  // A failed read is shown once, when its request has settled: the text as a
  // toast and at ACCTSID, and the cursor placed there, as
  // 1300-SETUP-SCREEN-ATTRS positions it [app/cbl/COACTVWC.cbl:L545-552]. An
  // error object kept in the cache while the query reads again is not shown a
  // second time.
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (error === null || isFetching || error === shownError.current) {
      return;
    }
    shownError.current = error;
    applyApiError(error, form, accountViewBmsFieldMap);
  }, [error, isFetching, form]);

  // ENTER (F-COACTVW-001, F-COACTVW-002): a new key opens its view; the key
  // already on screen is read again, as COACTVWC re-reads on every ENTER.
  const onSubmit = ({ account_id }: AccountKeyValues): void => {
    if (account_id === accountId) {
      void readAccount();
      return;
    }
    void navigate(`/accounts/${encodeURIComponent(account_id)}`);
  };

  // The read as one polite status line: its progress, then the account it
  // answered for, never a detail value such as the SSN. It is empty without a
  // route key, for a route key the edits reject, whose field error reports
  // it, and after a failed read, whose toast and field error report it.
  let statusText = "";
  if (accountId !== "" && isFetching) {
    statusText = "Loading account";
  } else if (account.isSuccess && !isFetching) {
    statusText = `Account ${account.data.account_id} loaded`;
  }

  // Each change of the text, and each new answer of a repeated lookup even
  // when its text is unchanged, clears the region and fills it after the
  // delay.
  const [announcement, setAnnouncement] = useState("");
  useEffect(() => {
    setAnnouncement("");
    if (statusText === "") {
      return undefined;
    }
    const timer = setTimeout(() => {
      setAnnouncement(statusText);
    }, STATUS_ANNOUNCE_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [statusText, account.dataUpdatedAt]);

  // A query the page never enables stays pending, so only a key that is read
  // gets the placeholder.
  let details: ReactNode = null;
  if (readsAccount) {
    if (account.isSuccess) {
      details = <AccountDetails account={account.data} />;
    } else if (account.isPending) {
      details = <AccountDetails account={undefined} />;
    }
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description={INFO_MESSAGE}
        actions={
          // PF3 (F-COACTVW-003): XCTL to the main menu
          // [app/cbl/COACTVWC.cbl:L324-352]. COACTVW offers no key into the
          // update screen, so there is no Edit link here: the function map
          // places "Edit account" on the dashboard lookup (F-COACTUP-001).
          <BackLink to="/" />
        }
      />
      <Card>
        <CardContent>
          <form onSubmit={form.handleSubmit(onSubmit)} noValidate>
            {/* ACCTSID with the label and submit names of the CAVW account
                lookup (F-COACTVW-001), the same key and function wherever it
                appears. */}
            <Controller
              control={form.control}
              name="account_id"
              render={({ field, fieldState }) => (
                <Field data-invalid={fieldState.invalid} className="max-w-md">
                  <FieldLabel htmlFor={inputId}>Account number</FieldLabel>
                  <div className="flex gap-2">
                    <Input
                      {...field}
                      id={inputId}
                      maxLength={ACCOUNT_ID_LENGTH}
                      inputMode="numeric"
                      autoComplete="off"
                      aria-invalid={fieldState.invalid}
                      aria-describedby={
                        fieldState.invalid ? errorId : undefined
                      }
                    />
                    <Button type="submit">Look up account</Button>
                  </div>
                  <FieldError id={errorId} errors={[fieldState.error]} />
                </Field>
              )}
            />
          </form>
        </CardContent>
      </Card>
      {/* Always rendered, across reads and outside every busy element, so
          assistive technology tracks the region from the start and announces
          its changes politely. */}
      <p role="status" className="sr-only">
        {announcement}
      </p>
      {details}
    </div>
  );
}
