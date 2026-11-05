// CCDL "View Credit Card Detail": the COCRDSLC card view (map COCRDSL) at
// /cards/:cardNumber, with the account number as the `account_id` search
// parameter (E-55). It shows the keys and the card fields 1200-SETUP-SCREEN-VARS
// moves to the map [app/cbl/COCRDSLC.cbl:L457-496] and sends
// GET /cards/{id}?account_id= (P-COCRDSLC-006, P-COCRDSLC-010).
//
// The screen has no key fields and no Search submit. ENTER=Search Cards is the
// card list's row action "View" (F-COCRDSL-001, route /cards), and a detail
// screen takes its keys from the route (E-56, E-61, P-COCRDSLC-011). The only
// control here is F3=Exit, the "Back" link to /cards (F-COCRDSL-002); the
// breadcrumb also leads back. With no form to mark, a failed read goes to
// showApiError, the display of pages without a form, and not to applyApiError.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type JSX,
} from "react";
import { useParams, useSearchParams } from "react-router";

import { useCard, type CardViewResult } from "@/api/queries/cards";
import {
  DetailItem,
  DetailList,
  DetailValueSkeleton,
  FlagBadge,
} from "@/components/detail-list";
import { BackLink, PageHeader } from "@/components/page-header";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { showApiError } from "@/lib/api-errors";
import { cardViewKeySchema, type CardViewKeyValues } from "@/schemas/cards";

/** The COCRDSL screen title [app/bms/COCRDSL.bms:L75-78]. */
const TITLE = "View Credit Card Detail";

/** The F3=Exit target, the card list (F-COCRDSL-002). */
const BACK_TARGET = "/cards";

/** The key edits' texts for each key, in the order the schema reports them. */
type KeyIssues = Record<keyof CardViewKeyValues, { message: string }[]>;

/**
 * The COCRDSLC 2210-EDIT-ACCOUNT and 2220-EDIT-CARD outcome of the route keys,
 * through the schema that mirrors them: `null` when both keys pass, else each
 * key's audited texts. A key error skips the card read
 * [app/cbl/COCRDSLC.cbl:L357-371], so the page sends no request for it.
 */
function editKeys(keys: CardViewKeyValues): KeyIssues | null {
  const result = cardViewKeySchema.safeParse(keys);
  if (result.success) {
    return null;
  }
  const issues: KeyIssues = { account_id: [], card_number: [] };
  for (const issue of result.error.issues) {
    const [key] = issue.path;
    if (key === "account_id" || key === "card_number") {
      issues[key].push({ message: issue.message });
    }
  }
  return issues;
}

/**
 * EXPMON '/' EXPYEAR, cut from `CARD-EXPIRAION-DATE X(10)` through its
 * redefine: CARD-EXPIRY-YEAR X(4) at (1:4) and CARD-EXPIRY-MONTH X(2) at
 * (6:2) [app/cbl/COCRDSLC.cbl:L84-90] [app/cbl/COCRDSLC.cbl:L477-482]. The
 * '/' is a constant of the map [app/bms/COCRDSL.bms:L130-132], shown even
 * when the stored date is blank.
 */
function expiryOf(date: string): string {
  return `${date.slice(5, 7)}/${date.slice(0, 4)}`;
}

type CardFieldsProps = {
  /** The card as read, or `undefined` while it is being read. */
  card: CardViewResult | undefined;
};

/** CRDNAME, CRDSTCD and EXPMON/EXPYEAR [app/bms/COCRDSL.bms:L103-138]. */
function CardFields({ card }: CardFieldsProps): JSX.Element {
  return (
    <DetailList>
      <DetailItem label="Name on card">
        {card === undefined ? (
          <DetailValueSkeleton className="w-48" />
        ) : (
          card.card_embossed_name
        )}
      </DetailItem>
      <DetailItem label="Card Active Y/N">
        {card === undefined ? (
          <DetailValueSkeleton className="w-8" />
        ) : (
          <FlagBadge value={card.card_active_status} />
        )}
      </DetailItem>
      <DetailItem label="Expiry Date">
        {card === undefined ? (
          <DetailValueSkeleton className="w-20" />
        ) : (
          <span className="tabular-nums">
            {expiryOf(card.card_expiraion_date)}
          </span>
        )}
      </DetailItem>
    </DetailList>
  );
}

/** The failure a toast has shown, kept as the screen's message line. */
type ShownError = { error: Error; message: string };

export default function CardViewPage(): JSX.Element {
  const { cardNumber = "" } = useParams();
  const [searchParams] = useSearchParams();
  const accountId = searchParams.get("account_id") ?? "";

  const keyIssues = useMemo(
    () => editKeys({ account_id: accountId, card_number: cardNumber }),
    [accountId, cardNumber],
  );
  const keysPass = keyIssues === null;
  // Keys the edits reject reach the hook as blanks, which it never sends.
  const query = useCard(
    keysPass ? cardNumber : "",
    keysPass ? accountId : "",
  );

  // One toast per failed answer: a re-render, or React's development re-run
  // of effects, sees the same error object and shows nothing more. The text
  // also stays on the card as the 3270 message line did, until the next read.
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

  const card = query.data;
  const reading = keysPass && query.isPending;
  const showFields = card !== undefined || reading;
  const titleId = useId();

  // BLITZY [A11Y]: the Back link's focus ring is the outline Button's built-in
  // `ring-ring/50` with the `--ring` border, about 2.6:1 against the light
  // background, below the WCAG AA 3:1 non-text minimum. Kept as the shadcn
  // token set defines it; flagged for designer review.
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title={TITLE}
        actions={<BackLink to={BACK_TARGET} />}
      />
      <Card role="region" aria-labelledby={titleId} aria-busy={reading}>
        <CardHeader>
          <CardTitle>
            <h2 id={titleId}>Card details</h2>
          </CardTitle>
          {errorLine !== null ? (
            <CardDescription className="text-destructive">
              {errorLine}
            </CardDescription>
          ) : card !== undefined ? (
            // INFOMSG of a found card, '   Displaying requested details'.
            <CardDescription>{card.message}</CardDescription>
          ) : reading ? (
            // The message line's place while the card is read: one text-sm
            // line (h-5), so the details do not move down when it arrives.
            <CardDescription aria-hidden="true">
              <Skeleton className="h-5 w-48" />
            </CardDescription>
          ) : null}
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          {/* ACCTSIDO and CARDSIDO show the keys as entered, CC-ACCT-ID and
              CC-CARD-NUM, not the card record's own account, which the read
              does not compare (E-55, P-COCRDSLC-010). */}
          <DetailList>
            <DetailItem
              label="Account Number"
              errors={keyIssues?.account_id}
            >
              <span className="tabular-nums">{accountId}</span>
            </DetailItem>
            <DetailItem label="Card Number" errors={keyIssues?.card_number}>
              <span className="tabular-nums">{cardNumber}</span>
            </DetailItem>
          </DetailList>
          {showFields ? (
            <>
              <Separator />
              <CardFields card={card} />
            </>
          ) : null}
        </CardContent>
      </Card>
    </div>
  );
}
