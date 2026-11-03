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
import { Plus } from "lucide-react";
import { useCallback, useEffect, useMemo, useState, type JSX } from "react";
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

// The whole list arrives in one answer, so Previous and Next never page.
const noop = (): void => undefined;

/**
 * Whether a rule has ended: its exclusive `active_to` lies at or before the
 * browser's current instant. A rule that starts in the future still counts as
 * active, since it has not ended.
 */
function isEnded(activeTo: string | null | undefined): boolean {
  return activeTo != null && new Date(activeTo).getTime() <= Date.now();
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
      // The badge's text, not its colour, carries the state.
      cell: ({ row }) =>
        isEnded(row.original.active_to) ? (
          <Badge variant="secondary">Ended</Badge>
        ) : (
          <Badge>Active</Badge>
        ),
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

// What confirming the delete of `rule` does. The server ends an open-ended or
// later-ending rule at its current instant and leaves an ended one unchanged.
function deleteDescription(rule: RewardRuleResponse | null): JSX.Element {
  if (rule === null) {
    return <p />;
  }
  const subject = `Rule ${rule.rule_id} (type ${rule.transaction_type_cd}, category ${rule.transaction_category_cd})`;
  if (isEnded(rule.active_to)) {
    return <p>{`${subject} has already ended, so deleting it changes nothing.`}</p>;
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
  const deleteMutation = useRewardRuleDeleteMutation();

  // The rule whose delete awaits confirmation; null while the dialog is
  // closed.
  const [pending, setPending] = useState<RewardRuleResponse | null>(null);
  // The rule the dialog text names. It outlives `pending`, so the dialog
  // keeps its text while it animates closed.
  const [named, setNamed] = useState<RewardRuleResponse | null>(null);

  useEffect(() => {
    if (rules.error) {
      showApiError(rules.error);
    }
  }, [rules.error]);

  const requestDelete = useCallback((rule: RewardRuleResponse) => {
    setNamed(rule);
    setPending(rule);
  }, []);

  const columns = useMemo(() => ruleColumns(requestDelete), [requestDelete]);

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
    // new Active to and status without a refetch here. The DELETE answer is
    // the rule itself and carries no message, hence the page's own text,
    // which tells apart a rule that had already ended and stayed unchanged.
    const alreadyEnded = isEnded(rule.active_to);
    deleteMutation.mutate(rule.rule_id, {
      onSuccess: () => {
        showSuccess(
          alreadyEnded
            ? `Reward rule ${rule.rule_id} had already ended`
            : `Reward rule ${rule.rule_id} ended`,
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
      <DataTable
        data={rules.data ?? []}
        columns={columns}
        prevKey={null}
        nextKey={null}
        onPrevious={noop}
        onNext={noop}
        emptyMessage={rules.isError ? LOAD_FAILED : undefined}
        caption="Reward rules"
        loading={rules.isPending}
      />
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
