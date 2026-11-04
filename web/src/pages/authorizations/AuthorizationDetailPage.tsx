// CPVD, "View Authorization Details": one pending authorization as COPAUS1C
// shows it on map COPAU01, with the COPAUS1C keys as controls
// (docs/parity/function-map.md, F-COPAU01-*):
//
// - PF3 is the "Back" link to the summary of the authorization's account
//   (F-COPAU01-002).
// - PF5 is "Mark fraud" or "Remove fraud", by the record's fraud flag
//   (F-COPAU01-003, F-COPAU01-004). Each opens the confirm dialog, whose
//   "Confirm" sends `PUT /authorizations/{id}` (F-COPAU01-005, E-60).
// - PF8 is "Next": the summary list read one row on from this authorization
//   (F-COPAU01-006, E-59), whose detail it then opens.
// - ENTER is the route itself: opening it reads the record (F-COPAU01-001).
//
// The response carries the stored values; the page applies the edits
// POPULATE-AUTH-DETAILS makes on the move to the screen, as text only, so no
// amount passes through a JavaScript number.
import { useEffect, useRef, useState, type JSX } from "react";
import { Link, useNavigate, useParams, useSearchParams, type To } from "react-router";
import { ArrowLeft, ChevronRight } from "lucide-react";

import {
  useAuthorizationDetail,
  useAuthorizationSummary,
  useFraudToggleMutation,
  type AuthDetail,
  type AuthSummaryParams,
} from "@/api/queries/authorizations";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { showApiError, showSuccess } from "@/lib/api-errors";
import { authDetailKeySchema } from "@/schemas/authorizations";

// ---------------------------------------------------------------------------
// POPULATE-AUTH-DETAILS edits (P-COPAUS1C-005)
// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L291-357]
// ---------------------------------------------------------------------------

/**
 * An `X(n)` value at its full width. The API drops trailing spaces, which the
 * COBOL reference-modified moves still copy into the screen field.
 */
function stored(value: string, width: number): string {
  return value.padEnd(width, " ");
}

/**
 * COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: AUTHDTO, `PA-AUTH-ORIG-DATE`
 * (`YYMMDD`) moved through `WS-CURDATE-MM-DD-YY` as `MM/DD/YY`.
 */
function authDate(value: string): string {
  const date = stored(value, 6);
  return `${date.slice(2, 4)}/${date.slice(4, 6)}/${date.slice(0, 2)}`;
}

/**
 * COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: AUTHTMO, `PA-AUTH-ORIG-TIME`
 * (`HHMMSS`) moved into `WS-AUTH-TIME` `00:00:00` as `HH:MM:SS`.
 */
function authTime(value: string): string {
  const time = stored(value, 6);
  return `${time.slice(0, 2)}:${time.slice(2, 4)}:${time.slice(4, 6)}`;
}

/**
 * COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: CRDEXPO, `PA-CARD-EXPIRY-DATE`
 * (`MMYY`) as `MM/YY`.
 */
function cardExpiry(value: string): string {
  const expiry = stored(value, 4);
  return `${expiry.slice(0, 2)}/${expiry.slice(2, 4)}`;
}

/** The scale-2 decimal text the API sends for `PA-APPROVED-AMT S9(10)V99`. */
const DECIMAL_TEXT = /^(-?)([0-9]+)\.([0-9]{2})$/;

/** The integer positions of `WS-AUTH-AMT PIC -zzzzzzz9.99`. */
const AMOUNT_INTEGER_DIGITS = 8;

/**
 * COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: AUTHAMTO, `PA-APPROVED-AMT`
 * moved to `WS-AUTH-AMT PIC -zzzzzzz9.99`.
 *
 * The picture has 8 integer positions for the 10 of the field, so the move
 * drops the high-order digits of an amount of 100,000,000.00 or more, as the
 * COBOL does. Leading zeros are suppressed down to the units digit and a
 * negative amount gets its `-`. The suppressed positions are blanks, which
 * HTML collapses, so they are left out. A value that is not the declared
 * decimal text is shown as sent.
 */
function approvedAmount(value: string): string {
  const match = DECIMAL_TEXT.exec(value);
  if (match === null) {
    return value;
  }
  const [, sign, whole, cents] = match;
  const digits = whole.padStart(AMOUNT_INTEGER_DIGITS, "0").slice(-AMOUNT_INTEGER_DIGITS);
  const integer = digits.replace(/^0+(?=[0-9])/, "");
  const negative = sign === "-" && /[1-9]/.test(whole + cents);
  return `${negative ? "-" : ""}${integer}.${cents}`;
}

/** `PA-FRAUD-CONFIRMED`, the `PA-AUTH-FRAUD` value of a reported fraud. */
const FRAUD_CONFIRMED = "F";
/** `PA-FRAUD-REMOVED`, the value once the report was taken back. */
const FRAUD_REMOVED = "R";

/**
 * COPAUS1C POPULATE-AUTH-DETAILS — P-COPAUS1C-005: AUTHFRDO, the flag, `-` and
 * `PA-FRAUD-RPT-DATE` when fraud was confirmed or removed, otherwise `-`.
 */
function fraudStatus(detail: AuthDetail): string {
  const flag = detail.pa_auth_fraud;
  return flag === FRAUD_CONFIRMED || flag === FRAUD_REMOVED
    ? `${flag}-${detail.pa_fraud_rpt_date}`
    : "-";
}

// ---------------------------------------------------------------------------
// COPAU01 output fields
// ---------------------------------------------------------------------------

/** The response properties that hold screen text, so a field cannot name `next_id` or `message`. */
type TextKey = {
  [K in keyof AuthDetail]-?: AuthDetail[K] extends string ? K : never;
}[keyof AuthDetail];

/** The status-flag badges of the component mapping (AAP 0.4.2). */
type StatusBadge = "secondary" | "destructive";

type DetailField = Readonly<{
  /** The COPAU01 output field. */
  bms: string;
  /** Its BMS caption, without the colon. */
  label: string;
  /** The response property POPULATE-AUTH-DETAILS moves into it. */
  key: TextKey;
  /** The edit made on the move, for a field that shows more than the stored text. */
  show?: (detail: AuthDetail) => string;
  /** The badge the value is shown in, or `null` for plain text. */
  badge?: (detail: AuthDetail) => StatusBadge | null;
}>;

/**
 * COPAUS1C writes AUTHRSPO in green for `A` and in red for `D`
 * [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L311-317].
 */
function responseBadge(detail: AuthDetail): StatusBadge {
  return detail.approved === "A" ? "secondary" : "destructive";
}

/** A confirmed fraud is the destructive flag; a removed one stays marked as secondary. */
function fraudBadge(detail: AuthDetail): StatusBadge | null {
  switch (detail.pa_auth_fraud) {
    case FRAUD_CONFIRMED:
      return "destructive";
    case FRAUD_REMOVED:
      return "secondary";
    default:
      return null;
  }
}

/** COPAU01 lines 7 to 15, in screen order [app/app-authorization-ims-db2-mq/bms/COPAU01.bms:L80-228]. */
const AUTHORIZATION_FIELDS: ReadonlyArray<DetailField> = [
  { bms: "CARDNUM", label: "Card #", key: "card_number" },
  {
    bms: "AUTHDT",
    label: "Auth Date",
    key: "pa_auth_orig_date",
    show: (detail) => authDate(detail.pa_auth_orig_date),
  },
  {
    bms: "AUTHTM",
    label: "Auth Time",
    key: "pa_auth_orig_time",
    show: (detail) => authTime(detail.pa_auth_orig_time),
  },
  { bms: "AUTHRSP", label: "Auth Resp", key: "approved", badge: responseBadge },
  { bms: "AUTHRSN", label: "Resp Reason", key: "decline_reason" },
  { bms: "AUTHCD", label: "Auth Code", key: "pa_processing_code" },
  {
    bms: "AUTHAMT",
    label: "Amount",
    key: "pa_approved_amt",
    show: (detail) => approvedAmount(detail.pa_approved_amt),
  },
  { bms: "POSEMD", label: "POS Entry Mode", key: "pa_pos_entry_mode" },
  { bms: "AUTHSRC", label: "Source", key: "pa_message_source" },
  { bms: "MCCCD", label: "MCC Code", key: "pa_merchant_catagory_code" },
  {
    bms: "CRDEXP",
    label: "Card Exp. Date",
    key: "pa_card_expiry_date",
    show: (detail) => cardExpiry(detail.pa_card_expiry_date),
  },
  { bms: "AUTHTYP", label: "Auth Type", key: "pa_auth_type" },
  { bms: "TRNID", label: "Tran Id", key: "pa_transaction_id" },
  { bms: "AUTHMTC", label: "Match Status", key: "pa_match_status" },
  {
    bms: "AUTHFRD",
    label: "Fraud Status",
    key: "pa_auth_fraud",
    show: fraudStatus,
    badge: fraudBadge,
  },
];

/** The "Merchant Details" lines 19 and 21 [app/app-authorization-ims-db2-mq/bms/COPAU01.bms:L229-283]. */
const MERCHANT_FIELDS: ReadonlyArray<DetailField> = [
  { bms: "MERNAME", label: "Name", key: "pa_merchant_name" },
  { bms: "MERID", label: "Merchant ID", key: "pa_merchant_id" },
  { bms: "MERCITY", label: "City", key: "pa_merchant_city" },
  { bms: "MERST", label: "State", key: "pa_merchant_state" },
  { bms: "MERZIP", label: "Zip", key: "pa_merchant_zip" },
];

/** One column at `md`, two at `lg` (AAP 0.5.4.4). */
const FIELD_GRID = "grid grid-cols-1 gap-x-6 gap-y-2 lg:grid-cols-2";

function DetailList({
  fields,
  detail,
}: {
  fields: ReadonlyArray<DetailField>;
  detail: AuthDetail;
}): JSX.Element {
  return (
    <dl className={FIELD_GRID}>
      {fields.map((field) => {
        const value = field.show?.(detail) ?? detail[field.key];
        const badge = field.badge?.(detail) ?? null;
        return (
          <div key={field.bms} className="flex min-w-0 flex-col gap-1">
            <dt className="text-sm text-muted-foreground">{field.label}</dt>
            <dd className="min-h-5 font-medium break-words tabular-nums">
              {badge === null ? value : <Badge variant={badge}>{value}</Badge>}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}

function AuthorizationCard({ detail }: { detail: AuthDetail }): JSX.Element {
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Authorization</h2>
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        <DetailList fields={AUTHORIZATION_FIELDS} detail={detail} />
        <Separator />
        <section className="flex flex-col gap-4">
          <h2 className="leading-none font-semibold">Merchant Details</h2>
          <DetailList fields={MERCHANT_FIELDS} detail={detail} />
        </section>
      </CardContent>
    </Card>
  );
}

/**
 * Skeleton blocks in place of the card's fields while the record is read,
 * hidden from assistive technology: the page's status announces the read.
 */
function AuthorizationSkeleton(): JSX.Element {
  return (
    <Card aria-hidden="true">
      <CardHeader>
        <Skeleton className="h-4 w-32" />
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        <div className={FIELD_GRID}>
          {AUTHORIZATION_FIELDS.map((field) => (
            <Skeleton key={field.bms} className="h-11 w-full" />
          ))}
        </div>
        <Separator />
        <div className={FIELD_GRID}>
          {MERCHANT_FIELDS.map((field) => (
            <Skeleton key={field.bms} className="h-11 w-full" />
          ))}
        </div>
      </CardContent>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Read status
// ---------------------------------------------------------------------------

/**
 * How long the read's status stays empty before it announces the read's
 * progress, DataTable's delay. The cleared status is committed first, so the
 * identical result of a repeated read is still announced as a change.
 */
const STATUS_ANNOUNCE_DELAY_MS = 150;

/**
 * The completed read as the status announces it: the authorization's identity,
 * its transaction ID, or the authorization's own ID when the record carries no
 * transaction ID, never its amount or card number.
 */
function loadedText(detail: AuthDetail): string {
  return detail.pa_transaction_id.trim() === ""
    ? `Authorization ${detail.auth_id} loaded`
    : `Authorization of transaction ${detail.pa_transaction_id} loaded`;
}

// ---------------------------------------------------------------------------
// The route key (COPAUS1C PROCESS-ENTER-KEY)
// ---------------------------------------------------------------------------

/**
 * COPAUS1C PROCESS-ENTER-KEY — V-COPAUS1C-001, E-49: the route key as read,
 * or `""` when the schema that mirrors the server's parse rejects it. A key
 * that is not a positive integer up to `i64::MAX` is the COBOL path that sets
 * its error flag with no message and leaves the detail fields empty, so it is
 * never sent: the hook reads nothing for `""`, and with no record shown no
 * PF5 or PF8 control exists to send it either. An accepted key is passed on
 * exactly as routed, its `+` and leading zeros included, so no digit of an ID
 * above 2^53 is lost to a conversion.
 */
function readKeyOf(authId: string): string {
  return authDetailKeySchema.safeParse({ auth_id: authId }).success ? authId : "";
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

/** The CPVS route, the parent of this one. */
const SUMMARY_PATH = "/authorizations";

/** `pathname` with the `account_id` the CPVS page searches for, when one is known. */
function withAccount(pathname: string, accountId: string): To {
  return accountId === ""
    ? pathname
    : { pathname, search: `?${new URLSearchParams({ account_id: accountId }).toString()}` };
}

function detailLocation(authId: string, accountId: string): To {
  return withAccount(`${SUMMARY_PATH}/${encodeURIComponent(authId)}`, accountId);
}

/** No lookup: an empty account ID keeps the summary query from sending anything. */
const NO_LOOKUP: AuthSummaryParams = { account_id: "" };

/**
 * COPAUS1C PROCESS-PF8-KEY — P-COPAUS1C-009, E-59: "Next" reads the summary
 * list one row forward from the authorization shown, which the list skips, so
 * the row it returns is the next older authorization under the same summary,
 * the one GNP reads. The press sends that request and the answer opens the
 * row's detail. It is not a link to the detail's `next_id`: the map row names
 * this request as the control's endpoint, so `next_id` only decides whether
 * Next is enabled.
 *
 * A lookup belongs to the authorization it started from and is dropped once
 * another one is shown. When the list returns no row, the authorization that
 * followed is gone, so the detail is read again for a current `next_id`.
 */
function useNextAuthorization(
  detail: AuthDetail | undefined,
  rereadDetail: () => unknown,
): () => void {
  const navigate = useNavigate();
  const [lookup, setLookup] = useState<AuthSummaryParams | null>(null);
  const current =
    lookup !== null && detail !== undefined && lookup.start_key === detail.auth_id
      ? lookup
      : null;
  const page = useAuthorizationSummary(current ?? NO_LOOKUP);
  const { isFetching, isPlaceholderData, isError, error, data } = page;

  useEffect(() => {
    // Waits for the answer to this request: a key change starts its read, and
    // data kept from an earlier request is only a placeholder.
    if (current === null || isFetching || isPlaceholderData) {
      return;
    }
    if (isError) {
      setLookup(null);
      showApiError(error);
      return;
    }
    if (data === undefined) {
      return;
    }
    setLookup(null);
    const [following] = data.items;
    if (following === undefined) {
      rereadDetail();
      return;
    }
    navigate(detailLocation(following.auth_id, current.account_id));
  }, [current, isFetching, isPlaceholderData, isError, error, data, navigate, rereadDetail]);

  return () => {
    if (detail === undefined || detail.account_id === "") {
      return;
    }
    setLookup({
      account_id: detail.account_id,
      start_key: detail.auth_id,
      direction: "forward",
      page_size: 1,
    });
  };
}

// ---------------------------------------------------------------------------
// PF5, the fraud mark (COPAUS1C MARK-AUTH-FRAUD with COPAUS2C)
// ---------------------------------------------------------------------------

type FraudAction = "mark" | "remove";

/**
 * COPAUS1C MARK-AUTH-FRAUD — P-COPAUS1C-006: a record flagged `F` has its
 * fraud removed (`R`); any other record is reported as fraud (`F`). The
 * control names are those of F-COPAU01-003 and F-COPAU01-004.
 */
const FRAUD_ACTIONS: Readonly<
  Record<FraudAction, Readonly<{ control: string; title: string; describe: (tranId: string) => string }>>
> = {
  mark: {
    control: "Mark fraud",
    title: "Mark this authorization as fraud?",
    describe: (tranId) => `The authorization of transaction ${tranId} is reported as fraud.`,
  },
  remove: {
    control: "Remove fraud",
    title: "Remove the fraud mark?",
    describe: (tranId) => `The fraud mark of the authorization of transaction ${tranId} is removed.`,
  },
};

/**
 * The dialog's action and transaction ID stay set while it closes, so its
 * text does not change mid-animation. `authId` is the route key of the record
 * it was opened on, the only one its Confirm may toggle.
 */
type FraudDialog = Readonly<{ open: boolean; action: FraudAction; authId: string; tranId: string }>;

const CLOSED_DIALOG: FraudDialog = { open: false, action: "mark", authId: "", tranId: "" };

/** The failure a toast has shown, kept as the screen's message line. */
type ShownError = Readonly<{ error: Error; message: string }>;

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function AuthorizationDetailPage(): JSX.Element {
  const { authId = "" } = useParams();
  const [searchParams] = useSearchParams();
  // A key the edit rejects reaches the hook blank, which it never sends. The
  // screen then holds its header and Back alone: no message (E-49), no toast
  // and nothing announced, as COPAUS1C sends the map with its fields empty.
  const readKey = readKeyOf(authId);
  const query = useAuthorizationDetail(readKey);
  // A failed read sends COPAU01 with its detail fields cleared, so a record
  // read earlier under this key (before a purge's 404 or a failed read's 500)
  // is not presented beside the failure: no card, no PF5 and no PF8 until a
  // read of the key succeeds again. Back stays to leave the screen.
  const detail = query.isError ? undefined : query.data;
  // The key's read outstanding, in flight or waiting offline: its first read,
  // or the read again of a key whose last read failed, which shows the
  // skeleton as the first read does.
  const reading = readKey !== "" && (query.isPending || query.fetchStatus !== "idle");
  const fraudToggle = useFraudToggleMutation();
  const openNext = useNextAuthorization(detail, query.refetch);
  const [dialog, setDialog] = useState<FraudDialog>(CLOSED_DIALOG);

  // The dialog confirms the PF5 of the record it was opened on. Once that
  // record is no longer shown, withheld after a failed read or left for
  // another route key, it closes rather than offer Confirm for it.
  const dialogOpen = dialog.open && detail !== undefined && dialog.authId === readKey;
  if (dialog.open && !dialogOpen) {
    setDialog({ ...dialog, open: false });
  }

  // A failed read is toasted once per error, the repeated effects of
  // StrictMode included. The toasted text also stays on the screen as the
  // 3270 message line did, the same for a first read and a read again, until
  // the key is read again; the Back link stays to leave the screen. A failure
  // a read again already supersedes, such as the cached one a return to the
  // route reads again, is neither toasted again nor shown.
  const failed = query.isError && query.fetchStatus === "idle" ? query.error : null;
  const toastedFor = useRef<Error | null>(null);
  const [shown, setShown] = useState<ShownError | null>(null);
  useEffect(() => {
    if (failed === null || toastedFor.current === failed) {
      return;
    }
    toastedFor.current = failed;
    setShown({ error: failed, message: showApiError(failed).message });
  }, [failed]);
  const errorLine =
    failed !== null && shown !== null && shown.error === failed ? shown.message : null;

  // The read's progress for assistive technology, the reads of PF8 Next and
  // of a PF5 result included. A failed read announces nothing here, as its
  // toast and the message line carry it. The status is emptied on every
  // change of the text and on every completed read, then filled after
  // STATUS_ANNOUNCE_DELAY_MS, as DataTable does.
  const statusText =
    readKey !== "" && query.isFetching
      ? "Loading authorization"
      : detail !== undefined
        ? loadedText(detail)
        : "";
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

  // The detail's `message` is only ever `Already at the last Authorization...`,
  // which COPAUS1C shows after PF8 at the last record. Here Next is disabled
  // there instead and the text is not shown (E-59), so the page never toasts
  // it on opening the record.

  // The account of the record, then the one the summary's View link passed.
  const accountId = detail?.account_id ?? searchParams.get("account_id") ?? "";
  const fraudAction: FraudAction = detail?.pa_auth_fraud === FRAUD_CONFIRMED ? "remove" : "mark";
  const dialogText = FRAUD_ACTIONS[dialog.action];

  // The PUT names the key of the record shown when Confirm is pressed, fixed
  // for its whole flight: PF8 may open another authorization before it
  // answers, and the answer still belongs to this one.
  const confirmFraud = (): void => {
    if (!dialogOpen) {
      return;
    }
    setDialog((current) => ({ ...current, open: false }));
    fraudToggle.mutate(readKey, {
      onSuccess: (result) => showSuccess(result.message),
      onError: (failure) => showApiError(failure),
    });
  };

  // PF3, PF5 and PF8 in that order. COPAUS1C has no PF7 and the map no
  // Previous row, so no Previous is offered (E-56, E-59). The fraud button is
  // never disabled while a mark is sent, so the dialog can return focus to
  // it; a second mark is a second PF5, which the server serializes.
  const actions = (
    <>
      <Button asChild variant="outline">
        <Link to={withAccount(SUMMARY_PATH, accountId)}>
          <ArrowLeft aria-hidden="true" />
          Back
        </Link>
      </Button>
      {detail === undefined ? null : (
        <>
          <Button
            type="button"
            onClick={() =>
              setDialog({
                open: true,
                action: fraudAction,
                authId: readKey,
                tranId: detail.pa_transaction_id,
              })
            }
          >
            {FRAUD_ACTIONS[fraudAction].control}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={detail.next_id === null}
            onClick={openNext}
          >
            Next
            <ChevronRight aria-hidden="true" />
          </Button>
        </>
      )}
    </>
  );

  // The status stays mounted from the first render, before any record exists,
  // and outside the cards, so it announces every read of the route.
  return (
    <>
      <PageHeader actions={actions} />
      {detail !== undefined ? (
        <AuthorizationCard detail={detail} />
      ) : reading ? (
        <AuthorizationSkeleton />
      ) : errorLine !== null ? (
        <p className="text-sm text-destructive">{errorLine}</p>
      ) : null}
      <p role="status" className="sr-only">
        {announcement}
      </p>
      <ConfirmDialog
        open={dialogOpen}
        onOpenChange={(open) => setDialog((current) => ({ ...current, open }))}
        title={dialogText.title}
        description={dialogText.describe(dialog.tranId)}
        confirmLabel="Confirm"
        onConfirm={confirmFraud}
        pending={fraudToggle.isPending}
      />
    </>
  );
}
