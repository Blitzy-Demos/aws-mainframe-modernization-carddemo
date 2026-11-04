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
// The form schema mirrors the key edits, so the form never sends a key the
// server is certain to refuse; a key that reaches the server through the URL
// gets the same text back, shown at ACCTSID with the cursor there.
import {
  useEffect,
  useId,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { ArrowLeft } from "lucide-react";
import { Controller, useForm } from "react-hook-form";
import { Link, useNavigate, useParams } from "react-router";

import { useAccount, type Account } from "@/api/queries/accounts";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError } from "@/lib/api-errors";
import {
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

/** One label and value of a read-only detail list. */
type Detail = { label: string; value: ReactNode };

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

/**
 * A Y/N flag as a badge carrying its own letter, so the state is read from the
 * text and not from colour. A blank flag renders nothing rather than an empty
 * pill.
 */
function flagBadge(value: string): ReactNode {
  return value === "" ? null : <Badge variant="secondary">{value}</Badge>;
}

/**
 * Labels and values as a definition list: one column below `lg`, two from
 * `lg`. `layout` adds grid placement classes for lists whose columns carry
 * meaning.
 */
function DetailList({
  details,
  layout = "",
}: {
  details: readonly Detail[];
  layout?: string;
}): JSX.Element {
  return (
    <dl
      className={
        layout === ""
          ? "grid gap-4 lg:grid-cols-2"
          : `grid gap-4 lg:grid-cols-2 ${layout}`
      }
    >
      {details.map(({ label, value }) => (
        <div key={label} className="flex flex-col gap-1">
          <dt className="text-sm text-muted-foreground">{label}</dt>
          <dd className="wrap-break-word tabular-nums">{value}</dd>
        </div>
      ))}
    </dl>
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
 */
function AccountDetails({ account }: { account: Account }): JSX.Element {
  // The map's two columns: status, dates and group on the left, amounts on the
  // right [app/bms/COACTVW.bms:L93-198]. From `lg` the list fills column by
  // column, five entries each, so the columns keep that meaning.
  const accountDetails: readonly Detail[] = [
    { label: "Active Y/N", value: flagBadge(account.acct_active_status) },
    { label: "Opened", value: account.acct_open_date },
    { label: "Expiry", value: account.acct_expiraion_date },
    { label: "Reissue", value: account.acct_reissue_date },
    { label: "Account Group", value: account.acct_group_id },
    { label: "Credit Limit", value: account.acct_credit_limit },
    { label: "Cash credit Limit", value: account.acct_cash_credit_limit },
    { label: "Current Balance", value: account.acct_curr_bal },
    { label: "Current Cycle Credit", value: account.acct_curr_cyc_credit },
    { label: "Current Cycle Debit", value: account.acct_curr_cyc_debit },
  ];

  // Customer Details, in the map's row groups [app/bms/COACTVW.bms:L199-353]:
  // identity, names, postal address, then contact and references.
  const identity: readonly Detail[] = [
    { label: "Customer id", value: account.customer_id },
    { label: "SSN", value: ssnText(account.cust_ssn) },
    { label: "Date of birth", value: account.cust_dob_yyyy_mm_dd },
    { label: "FICO Score", value: account.cust_fico_credit_score },
  ];
  const names: readonly Detail[] = [
    { label: "First Name", value: account.cust_first_name },
    { label: "Middle Name", value: account.cust_middle_name },
    { label: "Last Name", value: account.cust_last_name },
  ];
  // ACSADL1 and ACSADL2 sit under the one "Address:" caption.
  const address: readonly Detail[] = [
    {
      label: "Address",
      value: (
        <>
          <span className="block">{account.cust_addr_line_1}</span>
          <span className="block">{account.cust_addr_line_2}</span>
        </>
      ),
    },
    { label: "City", value: account.cust_addr_line_3 },
    { label: "State", value: account.cust_addr_state_cd },
    { label: "Zip", value: account.cust_addr_zip },
    { label: "Country", value: account.cust_addr_country_cd },
  ];
  const contact: readonly Detail[] = [
    { label: "Phone 1", value: account.cust_phone_num_1 },
    { label: "Government Issued Id Ref", value: account.cust_govt_issued_id },
    { label: "Phone 2", value: account.cust_phone_num_2 },
    { label: "EFT Account Id", value: account.cust_eft_account_id },
    {
      label: "Primary Card Holder Y/N",
      value: account.cust_pri_card_holder_ind,
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <DetailsCard title="Account">
        <DetailList
          details={accountDetails}
          layout="lg:grid-flow-col lg:grid-rows-5"
        />
      </DetailsCard>
      <DetailsCard title="Customer Details">
        <DetailList details={identity} />
        <Separator />
        <DetailList details={names} />
        <Separator />
        <DetailList details={address} />
        <Separator />
        <DetailList details={contact} />
      </DetailsCard>
      <DetailsCard title="Card">
        <DetailList
          details={[{ label: "Card Number", value: account.card_number }]}
        />
      </DetailsCard>
    </div>
  );
}

/**
 * Placeholder blocks while the account is read, hidden from assistive
 * technology: the page's status region announces the read.
 */
function AccountDetailsSkeleton(): JSX.Element {
  return (
    <div aria-hidden="true" className="flex flex-col gap-6">
      <Skeleton className="h-56 w-full rounded-xl" />
      <Skeleton className="h-96 w-full rounded-xl" />
      <Skeleton className="h-24 w-full rounded-xl" />
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

  // The hook sends no request for an empty key, so the key-entry state reads
  // nothing.
  const account = useAccount(accountId);
  const { error, isFetching, refetch: readAccount } = account;

  // ACCTSID shows the key of the account on screen: a new route key replaces
  // whatever was typed, and its errors with it.
  useEffect(() => {
    form.reset({ account_id: accountId });
  }, [accountId, form]);

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
  // route key and after a failed read, whose toast and field error report it.
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

  let details: ReactNode = null;
  if (accountId !== "") {
    if (account.isSuccess) {
      details = <AccountDetails account={account.data} />;
    } else if (account.isPending) {
      details = <AccountDetailsSkeleton />;
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
          <Button asChild variant="link">
            <Link to="/">
              <ArrowLeft aria-hidden="true" />
              Back
            </Link>
          </Button>
        }
      />
      <form
        className="flex flex-col gap-4"
        onSubmit={form.handleSubmit(onSubmit)}
        noValidate
      >
        {/* ACCTSID with the label and submit names of the CAVW account lookup
            (F-COACTVW-001), the same key and function wherever it appears. */}
        <Controller
          control={form.control}
          name="account_id"
          render={({ field, fieldState }) => (
            <Field data-invalid={fieldState.invalid} className="max-w-sm">
              <FieldLabel htmlFor={inputId}>Account number</FieldLabel>
              <Input
                {...field}
                id={inputId}
                maxLength={ACCOUNT_ID_LENGTH}
                inputMode="numeric"
                autoComplete="off"
                aria-invalid={fieldState.invalid}
                aria-describedby={fieldState.invalid ? errorId : undefined}
              />
              <FieldError id={errorId} errors={[fieldState.error]} />
            </Field>
          )}
        />
        <Button type="submit" className="w-fit">
          Look up account
        </Button>
      </form>
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
