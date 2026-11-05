// The rewards screen (`/rewards`, both user types): an account lookup, the
// account's reward balance as three summary cards and its accrual history,
// newest first, one page at a time (AAP 0.3.5, 0.5.4.1).
//
// Rewards have no COBOL program, map or audit row. The lookup field is shaped
// like the account view's ACCTSID [app/bms/COACTVW.bms:L83-91]: 11 digits
// (`PICIN='99999999999'`), required (`VALIDN=(MUSTFILL)`) and holding the
// initial cursor (`IC`), so it is edited by the same schema and gets the same
// texts as that lookup.
//
// Every points value and identifier is the API's decimal integer string,
// rendered exactly as received and never converted to a number or reformatted:
// a balance such as 9007199254740993 lies beyond what a JavaScript number holds
// exactly. Every message comes from the server and is shown by lib/api-errors.
import { useEffect, useState, type JSX } from "react";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { Coins, Gift, Wallet } from "lucide-react";

import type { components } from "@/api/schema";
import { useRewardBalance, useRewardHistory } from "@/api/queries/rewards";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { SummaryCard, SummaryCardSkeleton } from "@/components/summary-card";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { applyApiError, showApiError } from "@/lib/api-errors";
import {
  rewardsAccountBmsFieldMap,
  rewardsAccountSchema,
  type RewardsAccountValues,
} from "@/schemas/rewards";

type RewardAccrual = components["schemas"]["RewardAccrualResponse"];

// The accruals on one history page.
const HISTORY_PAGE_SIZE = 10;

// The `accountId` both reward hooks treat as "no account": the query is
// disabled and sends nothing.
const NO_ACCOUNT = "";

// The account field's length, ACCTSID `LENGTH=11`.
const ACCOUNT_ID_LENGTH = 11;

const ACCOUNT_FIELD_ID = "rewards-account-id";
const ACCOUNT_DESCRIPTION_ID = "rewards-account-id-description";
const ACCOUNT_ERROR_ID = "rewards-account-id-error";

// DataTable enables Previous only for a non-empty key. The history pages
// forward only and its `prev_key` is always null, so the page keeps the start
// keys of the pages before the current one, and the first page's start key is
// "none" (undefined). This label stands in for it as the enabling key; it is
// never sent.
const FIRST_PAGE_KEY = "first";

// How long the balance status stays empty before it announces. The region is
// cleared and then refilled, so the identical result of a repeated lookup is
// still announced as a change; the delay is the one DataTable uses.
const STATUS_ANNOUNCE_DELAY_MS = 150;

// Every cell renders the API string as sent: the default accessor cell shows
// the value itself, with no date parsing or locale formatting. Each
// placeholder is a value as wide as the column's on a seeded history page:
// identifiers and points of the seeded accruals' digit counts, and the RFC
// 3339 `created_at` with its microseconds.
//
// `transaction_id` is the accrued transaction's row identity, not its 16-digit
// TRAN-ID, which the transaction screens label "Transaction ID"; its column is
// headed "Transaction row" so that the two are not read as one key.
const columns: DataTableProps<RewardAccrual>["columns"] = [
  {
    accessorKey: "accrual_id",
    header: "Accrual ID",
    meta: { placeholder: "000" },
  },
  {
    accessorKey: "transaction_id",
    header: "Transaction row",
    meta: { placeholder: "000" },
  },
  { accessorKey: "rule_id", header: "Rule ID", meta: { placeholder: "0" } },
  {
    accessorKey: "points_earned",
    header: "Points earned",
    meta: { placeholder: "00" },
  },
  {
    accessorKey: "created_at",
    header: "Created at",
    meta: { placeholder: "0000-00-00T00:00:00.000000Z" },
  },
];

export default function RewardsPage(): JSX.Element {
  const form = useForm<RewardsAccountValues>({
    resolver: zodResolver(rewardsAccountSchema),
    defaultValues: { account_id: "" },
  });

  // The looked-up account; null before the first lookup.
  const [accountId, setAccountId] = useState<string | null>(null);
  // The start key of the history page shown; undefined for the newest page.
  const [startKey, setStartKey] = useState<string | undefined>(undefined);
  // The start keys of the pages before the shown one, oldest visit first.
  const [visited, setVisited] = useState<Array<string | undefined>>([]);
  // Whether the history page shown is the newest page a lookup selected:
  // true from each lookup until Next or Previous moves the table.
  const [lookupPage, setLookupPage] = useState(false);

  const balance = useRewardBalance(accountId ?? NO_ACCOUNT);
  // The history is read only once the balance has answered for the account,
  // so an account not on file is reported by its one 404, as one toast.
  const history = useRewardHistory(
    balance.isSuccess && accountId !== null ? accountId : NO_ACCOUNT,
    { start_key: startKey, page_size: HISTORY_PAGE_SIZE },
  );

  // TanStack Query binds `refetch` to its observer, so it is called detached.
  const reloadBalance = balance.refetch;
  const reloadHistory = history.refetch;

  useEffect(() => {
    if (balance.error) {
      applyApiError(balance.error, form, rewardsAccountBmsFieldMap);
    }
  }, [balance.error, form]);

  useEffect(() => {
    if (history.error) {
      showApiError(history.error);
    }
  }, [history.error]);

  const onSubmit = (values: RewardsAccountValues): void => {
    const onFirstPage = startKey === undefined;
    setStartKey(undefined);
    setVisited([]);
    setLookupPage(true);
    if (values.account_id !== accountId) {
      setAccountId(values.account_id);
      return;
    }
    // A repeated lookup of the same account reads it again, so its result
    // (or its error) is reported again. The newest history page is reread
    // only once the balance has answered, as on a first lookup; from a later
    // page, resetting the start key already selects the newest page. It is
    // reread only when the history is already keyed to the account, that is
    // when the balance had answered before this lookup: a lookup repeated
    // while the first balance read still runs (a double click) finds the
    // history holding no account, whose reread would request the history of
    // an empty account number, and the key change that follows the balance's
    // answer reads the newest page by itself.
    const historyKeyed = balance.isSuccess;
    void reloadBalance().then((result) => {
      if (result.isSuccess && onFirstPage && historyKeyed) {
        void reloadHistory();
      }
    });
  };

  // An entry the schema rejects shows no account, as the account view's
  // lookup of the same key does: its field error does not stand beside the
  // previous account's balance and history. A later lookup of that account
  // then reads it as a new one.
  const onInvalid = (): void => {
    setAccountId(null);
    setStartKey(undefined);
    setVisited([]);
    setLookupPage(false);
  };

  const nextKey = history.data?.next_key ?? null;
  const previousKey = visited.at(-1);

  const onNext = (): void => {
    if (nextKey === null) {
      return;
    }
    setVisited([...visited, startKey]);
    setStartKey(nextKey);
    setLookupPage(false);
  };

  const onPrevious = (): void => {
    if (visited.length === 0) {
      return;
    }
    setStartKey(previousKey);
    setVisited(visited.slice(0, -1));
    setLookupPage(false);
  };

  const showResult =
    accountId !== null && (balance.isLoading || balance.isSuccess);

  // The balance cards load while the balance is first read and, for the page
  // a lookup selected, while that lookup's balance read runs. The cache can
  // already hold a balance of the account (another lookup, or the
  // dashboard's reward card, read it earlier), and without the last case
  // that older figure would be shown as the lookup's result until its answer
  // replaces it.
  const balanceLoading =
    balance.isLoading || (lookupPage && balance.isFetching);

  // The history table loads while its page is first read, while a new page
  // is read over the previous one (placeholder data), and, for the page a
  // lookup selected, while that lookup's balance or history read runs. A
  // repeated lookup reads the same keys again over the rows they hold, so
  // without the last case those rows would be shown as its result before
  // its answer arrives. A page that Next or Previous returns to from the
  // cache keeps its rows on screen while it is read again.
  const historyLoading =
    balance.isLoading ||
    history.isLoading ||
    history.isPlaceholderData ||
    (lookupPage && (balance.isFetching || history.isFetching));

  // The balance read as one polite status line: its progress, then the
  // account it answered for. It never reads a points value, and it is empty
  // before a lookup and after a failed read, whose toast reports it.
  let statusText = "";
  if (accountId !== null && balance.isFetching) {
    statusText = "Loading reward balance";
  } else if (balance.isSuccess && !balance.isFetching) {
    statusText = `Reward balance of account ${balance.data.account_id} loaded`;
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
  }, [statusText, balance.dataUpdatedAt]);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader description="Look up an account to see its reward points balance and accrual history." />

      <Card>
        <CardContent>
          <form noValidate onSubmit={form.handleSubmit(onSubmit, onInvalid)}>
            <Controller
              name="account_id"
              control={form.control}
              render={({ field, fieldState }) => (
                <Field data-invalid={fieldState.invalid} className="max-w-md">
                  <FieldLabel htmlFor={ACCOUNT_FIELD_ID}>
                    Account number
                  </FieldLabel>
                  <div className="flex gap-2">
                    <Input
                      {...field}
                      id={ACCOUNT_FIELD_ID}
                      inputMode="numeric"
                      maxLength={ACCOUNT_ID_LENGTH}
                      autoComplete="off"
                      autoFocus
                      aria-invalid={fieldState.invalid}
                      aria-describedby={
                        fieldState.invalid
                          ? `${ACCOUNT_DESCRIPTION_ID} ${ACCOUNT_ERROR_ID}`
                          : ACCOUNT_DESCRIPTION_ID
                      }
                    />
                    <Button type="submit">Look up</Button>
                  </div>
                  <FieldDescription id={ACCOUNT_DESCRIPTION_ID}>
                    The 11-digit account number.
                  </FieldDescription>
                  <FieldError
                    id={ACCOUNT_ERROR_ID}
                    errors={[fieldState.error]}
                  />
                </Field>
              )}
            />
          </form>
        </CardContent>
      </Card>

      {/* Always rendered, and outside the busy balance section, so assistive
          technology tracks the region from the start and announces its
          changes politely. */}
      <p role="status" className="sr-only">
        {announcement}
      </p>

      {showResult ? (
        <>
          <section
            aria-label="Reward balance"
            aria-busy={balanceLoading ? true : undefined}
            className="grid gap-4 lg:grid-cols-3"
          >
            {balance.isSuccess && !balanceLoading ? (
              <>
                <SummaryCard
                  title="Points earned"
                  value={balance.data.total_points_earned}
                  description={`Account ${balance.data.account_id}`}
                  icon={<Coins className="size-4" />}
                />
                <SummaryCard
                  title="Points redeemed"
                  value={balance.data.total_points_redeemed}
                  description={`Account ${balance.data.account_id}`}
                  icon={<Gift className="size-4" />}
                />
                <SummaryCard
                  title="Available points"
                  value={balance.data.available_points}
                  description={`Account ${balance.data.account_id}`}
                  icon={<Wallet className="size-4" />}
                />
              </>
            ) : (
              <>
                <SummaryCardSkeleton description />
                <SummaryCardSkeleton description />
                <SummaryCardSkeleton description />
              </>
            )}
          </section>

          {history.isError ? null : (
            <DataTable
              columns={columns}
              data={history.data?.items ?? []}
              prevKey={
                visited.length > 0 ? (previousKey ?? FIRST_PAGE_KEY) : null
              }
              nextKey={nextKey}
              onPrevious={onPrevious}
              onNext={onNext}
              emptyMessage="No reward points have been earned on this account."
              caption="Reward history"
              loading={historyLoading}
              pageSize={HISTORY_PAGE_SIZE}
            />
          )}
        </>
      ) : null}
    </div>
  );
}
