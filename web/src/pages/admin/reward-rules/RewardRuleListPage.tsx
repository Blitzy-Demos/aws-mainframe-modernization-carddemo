// The admin reward-rule list at /admin/reward-rules (o8): every rule that
// `GET /admin/reward-rules` returns, active and soft-deleted alike, in the
// server's `rule_id` order, with a per-row Edit link and a Delete that ends the
// rule through `DELETE /admin/reward-rules/{id}`. A new screen with no COBOL
// program, BMS map or function-map row, so its accessible names are the ones
// the web/e2e specs read (REWARD_SCREENS.ruleList and .deleteDialog).
//
// App.tsx mounts the page under RequireAdmin and owns its route and crumb; the
// server's 403 on every reward-rule endpoint is authoritative, so the page
// makes no role check of its own.
//
// Every value is shown as the string the API wrote (rule IDs, codes, the
// NUMERIC(9,4) rate, the S9(09)V99 bounds, RFC 3339 instants): no value passes
// through a JavaScript number, and none is reformatted.
//
// The whole list arrives in one answer and soft delete never shrinks it, so
// the table shows it WINDOW_SIZE rules at a time: Previous and Next move the
// window along the list the page already holds, in `rule_id` order, and send
// no request. Only the window's rows are rendered.
import { Plus } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
} from "react";
import { Link } from "react-router";

import type { components } from "@/api/schema";
import {
  useRewardRuleDeleteMutation,
  useRewardRules,
} from "@/api/queries/reward-rules";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { showApiError, showSuccess } from "@/lib/api-errors";

type RewardRuleResponse = components["schemas"]["RewardRuleResponse"];

type RuleColumns = DataTableProps<RewardRuleResponse>["columns"];

// A null amount bound (unbounded) or end (open-ended), as the list shows it.
const NONE = "—";

// The table's empty-state text when the list could not be loaded; the toast
// carries the server's message. Without it the table would read "No records",
// as if no rule existed.
const LOAD_FAILED = "Reward rules could not be loaded";

// The rules one window shows: the ten rows of an admin list page.
const WINDOW_SIZE = 10;

// The keys that enable Previous and Next while an adjacent window exists. The
// table reads a key only to enable its control; the page moves the window
// itself, so the keys name no rule.
const PREVIOUS_WINDOW = "previous-window";
const NEXT_WINDOW = "next-window";

// The list while none is shown (loading, or a failed read): one array, so the
// window over it keeps its identity across renders.
const NO_RULES: RewardRuleResponse[] = [];

// Where the window over `count` rules starts when asked to start at `start`:
// at most at the last window that holds a rule, so a list that shrank (a
// refetch after rules were removed) still shows rows.
function windowStartFor(start: number, count: number): number {
  if (count === 0) {
    return 0;
  }
  const lastStart = Math.floor((count - 1) / WINDOW_SIZE) * WINDOW_SIZE;
  return Math.min(start, lastStart);
}

// The rule's state as the server judged it at its processing clock
// (CARDDEMO_CLOCK_OVERRIDE pins that clock), the instant the reward engine
// matches transactions against. The browser's clock is never consulted.
type RuleStatus = RewardRuleResponse["status"];

// The badge's text, not its colour, carries the state.
function statusBadge(status: RuleStatus): JSX.Element {
  switch (status) {
    case "active":
      return <Badge>Active</Badge>;
    case "scheduled":
      return <Badge variant="outline">Scheduled</Badge>;
    case "ended":
      return <Badge variant="secondary">Ended</Badge>;
  }
}

function orNone(value: string | null): string {
  return value ?? NONE;
}

// The amount columns align to the end, as numeric columns do in the shadcn
// data table pattern; the value itself is the API's string.
function endAligned(text: string): JSX.Element {
  return <div className="text-end tabular-nums">{text}</div>;
}

function ruleColumns(
  onDelete: (rule: RewardRuleResponse) => void,
): RuleColumns {
  return [
    {
      accessorKey: "rule_id",
      header: "Rule ID",
      cell: ({ row }) => row.original.rule_id,
    },
    {
      accessorKey: "transaction_type_cd",
      header: "Type",
      cell: ({ row }) => row.original.transaction_type_cd,
    },
    {
      accessorKey: "transaction_category_cd",
      header: "Category",
      cell: ({ row }) => row.original.transaction_category_cd,
    },
    {
      accessorKey: "reward_rate",
      header: () => endAligned("Rate"),
      cell: ({ row }) => endAligned(row.original.reward_rate),
    },
    {
      accessorKey: "min_transaction_amount",
      header: () => endAligned("Min amount"),
      cell: ({ row }) => endAligned(orNone(row.original.min_transaction_amount)),
    },
    {
      accessorKey: "max_transaction_amount",
      header: () => endAligned("Max amount"),
      cell: ({ row }) => endAligned(orNone(row.original.max_transaction_amount)),
    },
    {
      accessorKey: "active_from",
      header: "Active from",
      cell: ({ row }) => row.original.active_from,
    },
    {
      accessorKey: "active_to",
      header: "Active to",
      cell: ({ row }) => orNone(row.original.active_to),
    },
    {
      id: "status",
      header: "Status",
      cell: ({ row }) => statusBadge(row.original.status),
    },
    {
      id: "actions",
      header: "Actions",
      // BLITZY [A11Y]: the row actions keep the library's default Button
      // size (h-9, 36px), below the 44px touch-target recommendation;
      // implemented per the design system and flagged for designer review.
      // Delete stays offered for an ended rule: the server answers it with
      // 200 and changes nothing. Each action's accessible name stays "Edit" or
      // "Delete"; its description names the row's rule for screen readers.
      cell: ({ row }) => {
        const rule = row.original;
        const describes = `Rule ${rule.rule_id}`;
        return (
          <div className="flex items-center gap-2">
            <Button variant="link" asChild>
              <Link
                to={`/admin/reward-rules/${encodeURIComponent(rule.rule_id)}/edit`}
                aria-description={describes}
              >
                Edit
              </Link>
            </Button>
            <Button
              variant="link"
              type="button"
              aria-description={describes}
              onClick={() => {
                onDelete(rule);
              }}
            >
              Delete
            </Button>
          </div>
        );
      },
    },
  ];
}

// What confirming the delete of `rule` does, by the status the server gave it.
// The server ends an open-ended or later-ending rule at its current instant
// and leaves an ended one unchanged; a rule that has not started is ended
// before its Active from, so it never matches.
function deleteDescription(rule: RewardRuleResponse | null): JSX.Element {
  if (rule === null) {
    return <p />;
  }
  const subject = `Rule ${rule.rule_id} (type ${rule.transaction_type_cd}, category ${rule.transaction_category_cd})`;
  if (rule.status === "ended") {
    return <p>{`${subject} has already ended, so deleting it changes nothing.`}</p>;
  }
  if (rule.status === "scheduled") {
    return (
      <div className="flex flex-col gap-2">
        <p>{`${subject} has not started yet. Deleting it ends it now, before its Active from, so it never matches a transaction.`}</p>
        <p>The rule stays listed as Ended.</p>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <p>{`${subject} ends from now on: its Active to becomes the current time, and it no longer matches new transactions.`}</p>
      <p>
        Points already accrued under it are kept, and the rule stays listed as
        Ended.
      </p>
    </div>
  );
}

export default function RewardRuleListPage(): JSX.Element {
  const rules = useRewardRules();
  // A failed read keeps the query's last list; it is not shown, because it
  // does not answer the read that failed (a refresh after a delete, say), so
  // its rows and their Edit and Delete leave with it and LOAD_FAILED stands in
  // the table in their place.
  const loaded = rules.isError ? undefined : rules.data;
  const deleteMutation = useRewardRuleDeleteMutation();

  // The rule whose delete awaits confirmation; null while the dialog is
  // closed.
  const [pending, setPending] = useState<RewardRuleResponse | null>(null);
  // The rule the dialog text names. It outlives `pending`, so the dialog
  // keeps its text while it animates closed.
  const [named, setNamed] = useState<RewardRuleResponse | null>(null);

  // Each failed read is toasted once, when it settles. The query keeps a
  // failed read's error beside the rows read before it, so a return to the
  // page finds that error while its own read of the list is still out; that
  // read's answer, the rows or a new error, is what the page reports, so no
  // error is toasted while a read is out, fetching or paused offline. The ref
  // keeps one error from being toasted twice: by StrictMode's re-run of
  // effects, or by its return when a cancelled read reverts the query to it.
  const listError = rules.error;
  const reading = rules.fetchStatus !== "idle";
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (listError === null || reading || listError === shownError.current) {
      return;
    }
    shownError.current = listError;
    showApiError(listError);
  }, [listError, reading]);

  const requestDelete = useCallback((rule: RewardRuleResponse) => {
    setNamed(rule);
    setPending(rule);
  }, []);

  const columns = useMemo(() => ruleColumns(requestDelete), [requestDelete]);

  // The window over the list. `requestedStart` is the index Previous or Next
  // last moved its first rule to; `windowStart` is where it starts as shown,
  // held to the last window that still holds a rule. The window's rows keep
  // their array identity until the list or the window changes, as the table
  // requires of one result.
  const all = loaded ?? NO_RULES;
  const [requestedStart, setRequestedStart] = useState(0);
  const windowStart = windowStartFor(requestedStart, all.length);
  const shown = useMemo(
    () => all.slice(windowStart, windowStart + WINDOW_SIZE),
    [all, windowStart],
  );
  const prevKey = windowStart > 0 ? PREVIOUS_WINDOW : null;
  const nextKey = windowStart + WINDOW_SIZE < all.length ? NEXT_WINDOW : null;
  const showPrevious = useCallback(() => {
    setRequestedStart(Math.max(0, windowStart - WINDOW_SIZE));
  }, [windowStart]);
  const showNext = useCallback(() => {
    setRequestedStart(windowStart + WINDOW_SIZE);
  }, [windowStart]);

  // The table element changes only with what it shows, so opening, closing
  // or confirming the delete dialog re-renders none of its rows. It waits for
  // the read that is out: the first one, or a re-read of a list whose last
  // read failed. That failure stays in the query until the re-read answers,
  // but it does not answer the re-read, so LOAD_FAILED does not stand in the
  // table, and Previous and Next stay disabled, until the re-read's own
  // answer arrives.
  const loadFailed = rules.isError;
  const loading = rules.isPending || (loadFailed && reading);
  const table = useMemo(
    () => (
      <DataTable
        data={shown}
        columns={columns}
        prevKey={prevKey}
        nextKey={nextKey}
        onPrevious={showPrevious}
        onNext={showNext}
        emptyMessage={loadFailed ? LOAD_FAILED : undefined}
        caption="Reward rules"
        loading={loading}
      />
    ),
    [
      shown,
      columns,
      prevKey,
      nextKey,
      showPrevious,
      showNext,
      loadFailed,
      loading,
    ],
  );

  // Closes the dialog only while it still asks about `rule`, so a request
  // that settles after another row's dialog was opened leaves that one open.
  const settle = (rule: RewardRuleResponse): void => {
    setPending((current) =>
      current?.rule_id === rule.rule_id ? null : current,
    );
  };

  const confirmDelete = (): void => {
    const rule = pending;
    if (rule === null) {
      return;
    }
    // The hook refetches the list before it settles, so the row shows its
    // new Active to and status without a refetch here. The DELETE answer
    // carries no message, hence the page's own text, chosen by the answer's
    // outcome: what the server did, not what the row showed before.
    deleteMutation.mutate(rule.rule_id, {
      onSuccess: (deletion) => {
        showSuccess(
          deletion.outcome === "unchanged"
            ? `Reward rule ${deletion.rule_id} had already ended`
            : `Reward rule ${deletion.rule_id} ended`,
        );
        settle(rule);
      },
      onError: (error) => {
        showApiError(error);
        settle(rule);
      },
    });
  };

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Reward Rules"
        description="Points per currency unit for each transaction type and category. Deleting a rule ends it; it stays listed."
        actions={
          <Button asChild>
            <Link to="/admin/reward-rules/new">
              <Plus aria-hidden="true" />
              New rule
            </Link>
          </Button>
        }
      />
      {table}
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(open) => {
          if (!open) {
            setPending(null);
          }
        }}
        title="Delete reward rule"
        description={deleteDescription(named)}
        confirmLabel="Delete"
        cancelLabel="Cancel"
        onConfirm={confirmDelete}
        pending={deleteMutation.isPending}
      />
    </div>
  );
}
