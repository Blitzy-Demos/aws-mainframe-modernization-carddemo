// CT01 "View Transaction": the COTRN01C transaction view (map COTRN01) at
// /transactions/:tranId. It shows the CVTRA05Y fields PROCESS-ENTER-KEY moves
// to the map after the keyed read of TRANSACT [app/cbl/COTRN01C.cbl:L176-192]
// (P-COTRN01C-005), as GET /transactions/{id} answers them.
//
// The controls are the COTRN01 rows of docs/parity/function-map.md:
// - F-COTRN01-001, ENTER=Fetch: the transaction list's row action "View"
//   opens this route, whose `tranId` is the key COTRN01C reads (P-COTRN01C-001).
//   The page therefore has no TRNIDIN key field, no Fetch submit and no Reset:
//   a detail screen takes its key from the route (E-56, E-61, P-COTRN01C-008).
// - F-COTRN01-002, F3=Back: the "Back" link to the parent route /transactions.
//   The SPA opens this page only from the list, COTRN00C, the program PF3
//   returns to [app/cbl/COTRN01C.cbl:L115-122].
// - F-COTRN01-004, F5=Browse Tran.: the "Browse transactions" link to
//   /transactions [app/cbl/COTRN01C.cbl:L125-127].
// - F-COTRN01-003, F4=Clear, and F-COTRN01-005, other keys: no control, as the
//   view holds no input to clear (E-62) and AID-key mechanics have no web
//   counterpart (E-56).
//
// With no form to mark, a failed read goes to showApiError, the display of
// pages without a form, and not to applyApiError.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { ArrowLeft, List } from "lucide-react";
import { Link, useParams } from "react-router";

import { useTransaction, type Transaction } from "@/api/queries/transactions";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { FieldError } from "@/components/ui/field";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { showApiError } from "@/lib/api-errors";
import { transactionViewKeySchema } from "@/schemas/transactions";

/** The COTRN01 screen title [app/bms/COTRN01.bms:L75-79]. */
const TITLE = "View Transaction";

/** The F3=Back and F5=Browse Tran. target, the transaction list. */
const LIST_ROUTE = "/transactions";

/**
 * How long the read's status stays empty before it announces the read's
 * progress, DataTable's delay. The cleared status is committed first, so the
 * identical result of a repeated read is still announced as a change.
 */
const STATUS_ANNOUNCE_DELAY_MS = 150;

/** One display field of the map and the response member it shows. */
type ScreenField = {
  /** The BMS field name [app/bms/COTRN01.bms:L105-258]. */
  bms: string;
  /** The map caption, without its colon. */
  caption: string;
  field: keyof Transaction;
  /** Digits that line up from one transaction to the next. */
  tabular: boolean;
};

/**
 * The 13 display fields in map order, row by row from line 10 to line 20
 * [app/bms/COTRN01.bms:L100-258], each with the CVTRA05Y member COTRN01C
 * moves into it [app/cbl/COTRN01C.cbl:L178-190].
 */
const SCREEN_FIELDS: readonly ScreenField[] = [
  { bms: "TRNID", caption: "Transaction ID", field: "tran_id", tabular: true },
  { bms: "CARDNUM", caption: "Card Number", field: "tran_card_num", tabular: true },
  { bms: "TTYPCD", caption: "Type CD", field: "tran_type_cd", tabular: false },
  { bms: "TCATCD", caption: "Category CD", field: "tran_cat_cd", tabular: false },
  { bms: "TRNSRC", caption: "Source", field: "tran_source", tabular: false },
  { bms: "TDESC", caption: "Description", field: "tran_desc", tabular: false },
  { bms: "TRNAMT", caption: "Amount", field: "tran_amt", tabular: true },
  { bms: "TORIGDT", caption: "Orig Date", field: "tran_orig_ts", tabular: true },
  { bms: "TPROCDT", caption: "Proc Date", field: "tran_proc_ts", tabular: true },
  { bms: "MID", caption: "Merchant ID", field: "tran_merchant_id", tabular: true },
  { bms: "MNAME", caption: "Merchant Name", field: "tran_merchant_name", tabular: false },
  { bms: "MCITY", caption: "Merchant City", field: "tran_merchant_city", tabular: false },
  { bms: "MZIP", caption: "Merchant Zip", field: "tran_merchant_zip", tabular: false },
];

type KeyIssue = { message: string };

/**
 * The COTRN01C PROCESS-ENTER-KEY edit of the route key, through the schema
 * that mirrors it: `null` when the key passes, else its texts. A blank key
 * gets 'Tran ID can NOT be empty...' and is not read
 * [app/cbl/COTRN01C.cbl:L146-152] (V-COTRN01C-001); a key longer than the
 * 16-character TRNIDIN, which the 3270 field cannot send, keeps the schema's
 * length text and is not read either.
 */
function editKey(tranId: string): KeyIssue[] | null {
  const result = transactionViewKeySchema.safeParse({ tran_id: tranId });
  return result.success
    ? null
    : result.error.issues.map(({ message }) => ({ message }));
}

type DetailItemProps = {
  caption: string;
  children?: ReactNode;
  /** The key edit texts shown under the value. */
  errors?: KeyIssue[] | null;
};

/** One caption and its value. */
function DetailItem({ caption, children, errors }: DetailItemProps): JSX.Element {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-sm text-muted-foreground">{caption}</dt>
      <dd className="flex flex-col gap-1 font-medium wrap-anywhere">
        {children}
        {errors !== undefined && errors !== null && errors.length > 0 ? (
          <FieldError errors={errors} />
        ) : null}
      </dd>
    </div>
  );
}

type TransactionFieldsProps = {
  /** The transaction as read, or `undefined` while it is being read. */
  transaction: Transaction | undefined;
};

/**
 * The display fields, each the API string as sent: the amount stays the
 * decimal text of `TRAN-AMT S9(09)V99` and is never converted to a number.
 */
function TransactionFields({ transaction }: TransactionFieldsProps): JSX.Element {
  return (
    <dl className="grid gap-x-6 gap-y-4 lg:grid-cols-2">
      {SCREEN_FIELDS.map(({ bms, caption, field, tabular }) => (
        <DetailItem key={bms} caption={caption}>
          {transaction === undefined ? (
            <Skeleton aria-hidden="true" className="h-5 w-40" />
          ) : (
            <span className={tabular ? "tabular-nums" : undefined}>
              {transaction[field]}
            </span>
          )}
        </DetailItem>
      ))}
    </dl>
  );
}

/** The failure a toast has shown, kept as the screen's message line. */
type ShownError = { error: Error; message: string };

type TransactionPanelProps = {
  /** The route key, the TRAN-ID as the list sent it. */
  tranId: string;
};

/**
 * The map below its title: the key (TRNIDIN), the dashed rule of line 8
 * [app/bms/COTRN01.bms:L94-99] and the display fields of the transaction
 * read with that key, followed by the status that announces the read.
 */
function TransactionPanel({ tranId }: TransactionPanelProps): JSX.Element {
  const keyIssues = useMemo(() => editKey(tranId), [tranId]);
  const keyPasses = keyIssues === null;
  // A key the edit rejects reaches the hook blank, which it never sends.
  const query = useTransaction(keyPasses ? tranId : "");

  // One toast per failed read: a re-render, or React's development re-run of
  // effects, sees the same error object and shows nothing more. The text also
  // stays on the card as the 3270 message line did, until the next read.
  const { error } = query;
  const toastedFor = useRef<Error | null>(null);
  const [shown, setShown] = useState<ShownError | null>(null);
  useEffect(() => {
    if (error === null || toastedFor.current === error) {
      return;
    }
    toastedFor.current = error;
    setShown({ error, message: showApiError(error).message });
  }, [error]);
  const errorLine =
    error !== null && shown !== null && shown.error === error
      ? shown.message
      : null;

  // A failed read sends the map with its display fields cleared
  // [app/cbl/COTRN01C.cbl:L158-171], so a transaction read earlier under this
  // key is not shown beside the failure.
  const transaction = query.isError ? undefined : query.data;
  const reading = keyPasses && query.isPending;
  const showFields = transaction !== undefined || reading;
  const titleId = useId();

  // The read's progress for assistive technology: the transaction's identity
  // once read, never its amount or card number. A rejected key or a failed
  // read announces nothing here, as the toast and the message line carry it.
  const statusText =
    keyPasses && query.isFetching
      ? "Loading transaction"
      : transaction !== undefined && !query.isError
        ? `Transaction ${transaction.tran_id} loaded`
        : "";
  // The status is emptied on every change of the text and on every completed
  // read, then filled after STATUS_ANNOUNCE_DELAY_MS, as DataTable does.
  const { dataUpdatedAt } = query;
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
  }, [statusText, dataUpdatedAt]);

  // The status stays mounted beside the card, outside its busy subtree,
  // where assistive technology may hold back changes until the read ends.
  return (
    <>
      <Card role="region" aria-labelledby={titleId} aria-busy={reading}>
        <CardHeader>
          <CardTitle>
            <h2 id={titleId}>Transaction details</h2>
          </CardTitle>
          {errorLine !== null ? (
            <CardDescription className="text-destructive">
              {errorLine}
            </CardDescription>
          ) : null}
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          <dl className="grid gap-x-6 gap-y-4 lg:grid-cols-2">
            <DetailItem caption="Tran ID" errors={keyIssues}>
              <span className="tabular-nums">{tranId}</span>
            </DetailItem>
          </dl>
          {showFields ? (
            <>
              <Separator />
              <TransactionFields transaction={transaction} />
            </>
          ) : null}
        </CardContent>
      </Card>
      <p role="status" className="sr-only">
        {announcement}
      </p>
    </>
  );
}

export default function TransactionViewPage(): JSX.Element {
  // The route table gives the view no key-less route (E-61). Rendered without
  // one, it shows its header and links only, as COTRN01C's first entry without
  // a selected TRAN-ID sends an empty map and reads nothing
  // [app/cbl/COTRN01C.cbl:L99-109].
  const { tranId } = useParams();

  // BLITZY [A11Y]: the links' focus ring is the outline Button's built-in
  // `ring-ring/50` with the `--ring` border, about 2.6:1 against the light
  // background, below the WCAG AA 3:1 non-text minimum. Kept as the shadcn
  // token set defines it; flagged for designer review.
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={TITLE}
        actions={
          <>
            <Button variant="outline" asChild>
              <Link to={LIST_ROUTE}>
                <ArrowLeft aria-hidden="true" />
                Back
              </Link>
            </Button>
            <Button variant="outline" asChild>
              <Link to={LIST_ROUTE}>
                <List aria-hidden="true" />
                Browse transactions
              </Link>
            </Button>
          </>
        }
      />
      {tranId === undefined ? null : <TransactionPanel tranId={tranId} />}
    </div>
  );
}
