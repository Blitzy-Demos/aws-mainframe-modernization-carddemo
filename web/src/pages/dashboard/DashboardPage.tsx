// CM00 main menu (COMEN01C, map COMEN01) at `/`, where COSGN00C sends a user
// of type 'U' after sign-on [app/cbl/COSGN00C.cbl:L230-240]. User-function
// routes are open to both roles, so an administrator may open it as well.
//
// - The CAVW account lookup (F-COACTVW-001): ACCTSID, the 11-digit account
//   number [app/bms/COACTVW.bms:L84-91], edited by the account view's schema
//   and answered with its texts. A found account is summarised in place: the
//   COACTVW balance figures, the reward balance and the five most recent
//   transactions, with links to the full view (F-COACTVW-002) and the update
//   screen (F-COACTUP-001). The Account View and Account Update options link
//   here for that key entry (E-61).
// - The menu: every COMEN02Y option in COBOL order, then Rewards. nav/menu.ts
//   supplies each label, route and icon (F-COMEN01-001 to F-COMEN01-011).
//   Choosing a link replaces typing the option number and pressing ENTER
//   [app/cbl/COMEN01C.cbl:L94-95], so there is no OPTION field, and the
//   option-number, admin-only and not-installed messages have no counterpart
//   (F-COMEN01-013, E-56).
// - F3=Exit, the sign-off XCTL to COSGN00C [app/cbl/COMEN01C.cbl:L96-98], is
//   the app-shell user menu's "Log out" item (F-COMEN01-012), which app-layout
//   renders; this page adds no sign-off control of its own.
// - The heading and breadcrumb come from the route handle App.tsx declares.
//
// Amounts, points and keys are the API's strings, rendered exactly as
// received and never converted to numbers: a points total such as
// 9007199254740993 lies beyond what a JavaScript number holds exactly. Every
// message comes from the schema or the server and is shown by lib/api-errors.
import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type JSX,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { Banknote, CreditCard, Eye, Gift, Pencil, Wallet } from "lucide-react";
import { Controller, useForm, type UseFormReturn } from "react-hook-form";
import { Link } from "react-router";

import type { components } from "@/api/schema";
import { useAccount } from "@/api/queries/accounts";
import { useRewardBalance } from "@/api/queries/rewards";
import { useRecentTransactions } from "@/api/queries/transactions";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { SummaryCard } from "@/components/summary-card";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError, showApiError } from "@/lib/api-errors";
import { userMenu } from "@/nav/menu";
import {
  accountKeySchema,
  accountViewBmsFieldMap,
  type AccountKeyValues,
} from "@/schemas/accounts";

type TransactionResponse = components["schemas"]["TransactionResponse"];

/** The form field of ACCTSID, the one input of the lookup. */
const ACCOUNT_FIELD = accountViewBmsFieldMap.ACCTSID;

/** ACCTSID `LENGTH=11`, `PICIN='99999999999'` [app/bms/COACTVW.bms:L84-90]. */
const ACCOUNT_ID_LENGTH = 11;

// The `accountId` the reward and transaction hooks treat as "no account":
// their queries are disabled and send nothing.
const NO_ACCOUNT = "";

// One empty page for every render without transaction rows, so the table
// sees the same array until the page arrives.
const NO_TRANSACTIONS: TransactionResponse[] = [];

/** The map title [app/bms/COMEN01.bms:L79], heading and naming the menu. */
const MENU_TITLE = "Main Menu";

/** The account submitted for lookup, and which submission it was. */
type Lookup = { accountId: string; count: number };

// Every cell renders the API string as sent. The date is the first ten
// characters of the TRAN-ORIG-TS text, `YYYY-MM-DD`, cut as text and never
// parsed as a date.
const transactionColumns: DataTableProps<TransactionResponse>["columns"] = [
  {
    accessorKey: "tran_id",
    header: "Transaction ID",
    cell: ({ row }) => (
      <Button variant="link" asChild className="h-auto p-0">
        <Link to={`/transactions/${encodeURIComponent(row.original.tran_id)}`}>
          {row.original.tran_id}
        </Link>
      </Button>
    ),
  },
  {
    id: "tran_orig_date",
    header: "Date",
    accessorFn: (row) => row.tran_orig_ts.slice(0, 10),
  },
  { accessorKey: "tran_desc", header: "Description" },
  {
    accessorKey: "tran_amt",
    header: "Amount",
    cell: ({ row }) => (
      <span className="tabular-nums">{row.original.tran_amt}</span>
    ),
  },
];

/**
 * Calls `show` once for each failed read, when its request has settled. An
 * error the cache still holds while the query reads again, as on the remount
 * of a repeated lookup, is not shown a second time.
 */
function useSettledError(
  error: unknown,
  isFetching: boolean,
  show: (error: unknown) => void,
): void {
  const shown = useRef<unknown>(null);
  useEffect(() => {
    if (error === null || isFetching || error === shown.current) {
      return;
    }
    shown.current = error;
    show(error);
  }, [error, isFetching, show]);
}

/** Placeholder blocks while the account is read. */
function OverviewSkeleton(): JSX.Element {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-4">
      <span className="sr-only">Loading account</span>
      <Skeleton className="h-7 w-48" />
      <div className="grid gap-4 lg:grid-cols-3">
        <Skeleton className="h-32 rounded-xl" />
        <Skeleton className="h-32 rounded-xl" />
        <Skeleton className="h-32 rounded-xl" />
        <Skeleton className="h-32 rounded-xl" />
      </div>
      <Skeleton className="h-64 w-full rounded-xl" />
    </div>
  );
}

type AccountOverviewProps = {
  accountId: string;
  form: UseFormReturn<AccountKeyValues>;
};

/**
 * The summary of a submitted account: `GET /accounts/{id}`, then the reward
 * balance and the five most recent transactions of the account found.
 */
function AccountOverview({
  accountId,
  form,
}: AccountOverviewProps): JSX.Element | null {
  const headingId = useId();
  const account = useAccount(accountId);
  // The reward balance and the transactions are read once the account has
  // answered, as COACTVWC skips its reads after a key error: an account not
  // on file is then reported by its one answer, at ACCTSID, as one toast.
  const found = account.isSuccess ? accountId : NO_ACCOUNT;
  const transactions = useRecentTransactions(found);
  const rewards = useRewardBalance(found);

  // A failed account read is shown as the account view shows it: the text as
  // a toast and at ACCTSID, and the cursor placed there, as COACTVWC
  // 1300-SETUP-SCREEN-ATTRS positions it [app/cbl/COACTVWC.cbl:L545-552].
  const showAccountError = useCallback(
    (error: unknown): void => {
      applyApiError(error, form, accountViewBmsFieldMap);
    },
    [form],
  );
  useSettledError(account.error, account.isFetching, showAccountError);
  useSettledError(transactions.error, transactions.isFetching, showApiError);
  useSettledError(rewards.error, rewards.isFetching, showApiError);

  if (account.isError) {
    return null;
  }
  if (account.isPending) {
    return <OverviewSkeleton />;
  }

  const { account_id: shownId } = account.data;
  const accountRoute = `/accounts/${encodeURIComponent(shownId)}`;

  let rewardCard: JSX.Element | null = null;
  if (rewards.isSuccess) {
    rewardCard = (
      <SummaryCard
        title="Reward Points"
        value={rewards.data.available_points}
        icon={<Gift className="size-4" />}
      />
    );
  } else if (rewards.isPending) {
    rewardCard = <Skeleton aria-hidden="true" className="h-32 rounded-xl" />;
  }

  // BLITZY [A11Y]: the view and edit links keep the generated Button focus
  // ring, the built-in `ring-ring/50` token of index.css, about 1.5:1 against
  // the page background, below the WCAG AA 3:1 non-text minimum. Kept as the
  // shadcn token set defines it; flagged for designer review.
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id={headingId} className="text-lg font-semibold">
          {`Account ${shownId}`}
        </h2>
        <div className="flex flex-wrap items-center gap-2">
          {/* F-COACTVW-002: the full COACTVW details. */}
          <Button variant="outline" size="sm" asChild>
            <Link to={accountRoute}>
              <Eye aria-hidden="true" />
              View account
            </Link>
          </Button>
          {/* F-COACTUP-001: the COACTUP update screen for this account. */}
          <Button variant="outline" size="sm" asChild>
            <Link to={`${accountRoute}/edit`}>
              <Pencil aria-hidden="true" />
              Edit account
            </Link>
          </Button>
        </div>
      </div>

      {/* One column at md, three at lg; the reward card starts the second
          row, on the first column's track. */}
      <div className="grid gap-4 lg:grid-cols-3">
        <SummaryCard
          title="Current Balance"
          value={account.data.acct_curr_bal}
          icon={<Wallet className="size-4" />}
        />
        <SummaryCard
          title="Credit Limit"
          value={account.data.acct_credit_limit}
          icon={<CreditCard className="size-4" />}
        />
        <SummaryCard
          title="Cash Credit Limit"
          value={account.data.acct_cash_credit_limit}
          icon={<Banknote className="size-4" />}
        />
        {rewardCard}
      </div>

      {/* The five rows are the whole answer: there is no page before or
          after them, so both paging keys are null. */}
      {transactions.isError ? null : (
        <DataTable
          columns={transactionColumns}
          data={transactions.data?.items ?? NO_TRANSACTIONS}
          prevKey={null}
          nextKey={null}
          onPrevious={() => undefined}
          onNext={() => undefined}
          emptyMessage="No transactions on file for this account."
          caption="Recent transactions"
          loading={transactions.isPending}
        />
      )}
    </section>
  );
}

export default function DashboardPage(): JSX.Element {
  const inputId = useId();
  const descriptionId = useId();
  const errorId = useId();
  const menuHeadingId = useId();

  const form = useForm<AccountKeyValues>({
    resolver: zodResolver(accountKeySchema),
    defaultValues: { [ACCOUNT_FIELD]: "" },
  });

  // The last submitted account; null before the first lookup, so no account
  // hook runs with an empty key.
  const [lookup, setLookup] = useState<Lookup | null>(null);

  // ENTER (F-COACTVW-001): each submission remounts the summary, which reads
  // the account again even when the same number is resubmitted, as COACTVWC
  // reads on every ENTER.
  const onSubmit = (values: AccountKeyValues): void => {
    form.clearErrors();
    setLookup((previous) => ({
      accountId: values[ACCOUNT_FIELD],
      count: (previous?.count ?? 0) + 1,
    }));
  };

  // A rejected key shows no account: COACTVWC clears the map
  // [app/cbl/COACTVWC.cbl:L431-432] and fills the details only for a record
  // it found [app/cbl/COACTVWC.cbl:L471-472], so the previous account's
  // summary does not stay beside the key error.
  const onInvalid = (): void => {
    setLookup(null);
  };

  // BLITZY [A11Y]: the menu links keep the generated Button focus ring, the
  // built-in `ring-ring/50` token of index.css, about 1.5:1 against the card
  // background, below the WCAG AA 3:1 non-text minimum. Kept as the shadcn
  // token set defines it; flagged for designer review.
  return (
    <div className="flex flex-col gap-6">
      <PageHeader description="Look up an account, or choose an option from the menu." />

      <Card>
        <CardContent>
          <form noValidate onSubmit={form.handleSubmit(onSubmit, onInvalid)}>
            <Controller
              control={form.control}
              name={ACCOUNT_FIELD}
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
                        fieldState.invalid
                          ? `${descriptionId} ${errorId}`
                          : descriptionId
                      }
                    />
                    <Button type="submit">Look up account</Button>
                  </div>
                  <FieldDescription id={descriptionId}>
                    The 11-digit account number.
                  </FieldDescription>
                  <FieldError id={errorId} errors={[fieldState.error]} />
                </Field>
              )}
            />
          </form>
        </CardContent>
      </Card>

      {lookup === null ? null : (
        <AccountOverview
          key={lookup.count}
          accountId={lookup.accountId}
          form={form}
        />
      )}

      {/* A landmark distinct from the sidebar's "Main" navigation, holding
          the options as a list: one column below md, two at md, three at
          lg. */}
      <nav aria-labelledby={menuHeadingId} className="flex flex-col gap-4">
        <h2 id={menuHeadingId} className="text-lg font-semibold">
          {MENU_TITLE}
        </h2>
        <ul className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
          {userMenu.map((entry) => {
            const Icon = entry.icon;
            return (
              <li key={entry.id}>
                <Card className="relative h-full">
                  <CardContent className="flex items-center gap-3">
                    {/* Outside the link, so the link's accessible name is the
                        option text alone and the link variant's padding for
                        an svg child does not apply. */}
                    <Icon
                      aria-hidden="true"
                      className="size-5 shrink-0 text-muted-foreground"
                    />
                    {/* The link fills the whole card through its ::after box,
                        so the card is the click target rather than the text
                        line alone; wrapping keeps the longest option text
                        inside its column. */}
                    <Button
                      variant="link"
                      asChild
                      className="h-auto justify-start p-0 text-left whitespace-normal after:absolute after:inset-0"
                    >
                      <Link to={entry.route}>{entry.label}</Link>
                    </Button>
                  </CardContent>
                </Card>
              </li>
            );
          })}
        </ul>
      </nav>
    </div>
  );
}
