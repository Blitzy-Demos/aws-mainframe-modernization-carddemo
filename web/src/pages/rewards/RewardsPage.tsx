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
import {
  rewardsAccountBmsFieldMap,
  rewardsAccountSchema,
  type RewardsAccountValues,
} from "@/schemas/rewards";

type RewardAccrual = components["schemas"]["RewardAccrualResponse"];

/** The accruals on one history page. */
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

// Every cell renders the API string as sent: the default accessor cell shows
// the value itself, with no date parsing or locale formatting.
const columns: DataTableProps<RewardAccrual>["columns"] = [
  { accessorKey: "accrual_id", header: "Accrual ID" },
  { accessorKey: "transaction_id", header: "Transaction ID" },
  { accessorKey: "rule_id", header: "Rule ID" },
  { accessorKey: "points_earned", header: "Points earned" },
  { accessorKey: "created_at", header: "Created at" },
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
    if (values.account_id !== accountId) {
      setAccountId(values.account_id);
      return;
    }
    // A repeated lookup of the same account reads it again, so its result
    // (or its error) is reported again. The newest history page is reread
    // only once the balance has answered, as on a first lookup; from a later
    // page, resetting the start key already selects the newest page.
    void reloadBalance().then((result) => {
      if (result.isSuccess && onFirstPage) {
        void reloadHistory();
      }
    });
  };

  const nextKey = history.data?.next_key ?? null;
  const previousKey = visited.at(-1);

  const onNext = (): void => {
    if (nextKey === null) {
      return;
    }
    setVisited([...visited, startKey]);
    setStartKey(nextKey);
  };

  const onPrevious = (): void => {
    if (visited.length === 0) {
      return;
    }
    setStartKey(previousKey);
    setVisited(visited.slice(0, -1));
  };

  const showResult =
    accountId !== null && (balance.isLoading || balance.isSuccess);

  return (
    <div className="flex flex-col gap-6">
      <PageHeader description="Look up an account to see its reward points balance and accrual history." />

      <Card>
        <CardContent>
          <form noValidate onSubmit={form.handleSubmit(onSubmit)}>
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

      {showResult ? (
        <>
          <section
            aria-label="Reward balance"
            aria-busy={balance.isLoading ? true : undefined}
            className="grid gap-4 lg:grid-cols-3"
          >
            {balance.isSuccess ? (
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
                <Skeleton aria-hidden="true" className="h-24 rounded-xl" />
                <Skeleton aria-hidden="true" className="h-24 rounded-xl" />
                <Skeleton aria-hidden="true" className="h-24 rounded-xl" />
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
              loading={
                balance.isLoading ||
                history.isLoading ||
                history.isPlaceholderData
              }
            />
          )}
        </>
      ) : null}
    </div>
  );
}
