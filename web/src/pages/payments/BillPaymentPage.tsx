// CB00 bill payment: the COBIL00 "Bill Payment" screen of COBIL00C
// [app/bms/COBIL00.bms] [app/cbl/COBIL00C.cbl], served at /payments.
//
// The 3270 keys are the controls of docs/parity/function-map.md:
// - ENTER is the form's submit "Review payment" (F-COBIL00-001). The schema
//   repeats the account-ID blank check (V-COBIL00C-001), so a blank entry
//   never reaches the network. A valid entry is sent as COBIL00C receives an
//   ENTER with CONFIRM blank: `POST /payments` with `confirm: ""`, which reads
//   the account, applies the nothing-to-pay guard and answers 200 with the
//   balance and 'Confirm to make a bill payment...', writing nothing
//   (V-COBIL00C-004 to V-COBIL00C-008). Only that request yields the COBIL00C
//   texts this screen owes ('Account ID NOT found...', 'You have nothing to
//   pay...'); `GET /accounts/{id}` alone would answer with the COACTVWC texts
//   of another screen. The 200 then opens the confirm dialog that replaces the
//   CONFIRM field, and the balance card reads the account through
//   `GET /accounts/{id}`, the preview the map and the screen mapping name.
// - CONFIRM Y is the dialog's "Confirm" (F-COBIL00-002): `confirm: "Y"` pays
//   the whole balance. CONFIRM N is its "Cancel" (F-COBIL00-003), and Escape:
//   no request, and the screen is cleared as CLEAR-CURRENT-SCREEN clears it.
// - F3=Back is the link "Back" to the main menu (F-COBIL00-004).
// - F4=Clear is "Reset" (F-COBIL00-005): account ID, balance and confirmation
//   cleared, cursor on the account ID.
// - Other AID keys have no control (F-COBIL00-006, E-56), and the map has no
//   row for a separate pay button, so the dialog opens from ENTER alone.
//
// Every outcome text is the server's and is shown through lib/api-errors.
// Amounts stay the decimal strings the API sends.
import { useCallback, useEffect, useRef, useState, type JSX } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { Controller, useForm, type UseFormReturn } from "react-hook-form";
import { LoaderCircle, Wallet } from "lucide-react";

import { useAccount } from "@/api/queries/accounts";
import { usePaymentMutation } from "@/api/queries/payments";
import type { components } from "@/api/schema";
import { ConfirmDialog, type AfterClose } from "@/components/confirm-dialog";
import { BackLink, PageHeader } from "@/components/page-header";
import { SummaryCard } from "@/components/summary-card";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  applyApiError,
  showApiError,
  showMessage,
  showSuccess,
} from "@/lib/api-errors";
import {
  billPaymentBmsFieldMap,
  paymentSchema,
  type PaymentFormValues,
} from "@/schemas/payments";

type AccountViewField = components["schemas"]["AccountViewField"];

// The blank screen INITIALIZE-ALL-FIELDS leaves [app/cbl/COBIL00C.cbl:L560-566].
const EMPTY_VALUES: PaymentFormValues = { account_id: "", confirm: "" };

// The ENTER of the review: CONFIRMI blank, which only asks (V-COBIL00C-008).
const CONFIRM_BLANK = "";
// The dialog's Confirm: CONFIRMI `Y`, which pays [app/cbl/COBIL00C.cbl:L174-177].
const CONFIRM_YES = "Y";
// The status of a completed payment; a 200 only carries a message.
const CREATED = 201;

// The note under the error of a confirmed payment that got no usable answer:
// the server may have made it, and paying again would pay twice.
const PAYMENT_OUTCOME_UNKNOWN =
  "The payment may have been made. Check the account balance before paying again.";

// `ACTIDINI PIC X(11)` [app/cpy-bms/COBIL00.CPY:L60].
const ACCOUNT_ID_LENGTH = 11;
const ACCOUNT_FIELD = "account_id";
const ACCOUNT_DESCRIPTION_ID = "account_id-description";
const ACCOUNT_ERROR_ID = "account_id-error";

// Where a `GET /accounts/{id}` error of the balance card lands: its one BMS
// field, ACCTSID, is the account ID this screen asks for. Every member of the
// enum is listed, so a new one fails `tsc`.
const previewFieldMap = {
  ACCTSID: ACCOUNT_FIELD,
} satisfies Record<AccountViewField, keyof PaymentFormValues>;

// The 200 answer to a review: the account as entered and its CURBAL.
type Review = { accountId: string; balance: string };

// The 201 answer: the paid account and its balance after the payment.
type Paid = { accountId: string; balance: string };

type BalancePreviewProps = {
  accountId: string;
  paid: Paid | null;
  // Whether a confirmed payment, with the reloads after it, is running.
  paying: boolean;
  form: UseFormReturn<PaymentFormValues>;
  // Whether the form still holds the entry this account was reviewed from.
  ownsEntry: () => boolean;
};

// The CURBAL line of COBIL00, "Your current balance is", for the account
// under review: `GET /accounts/{id}` through the account cache the dashboard
// shares. The payment hook reloads that entry once a confirmed payment
// settles, failures included (E-19), so the card follows the balance on file;
// after a completed payment it shows the balance the 201 answered with.
// While the payment runs, and while the account is read again, the balance
// on file may be about to change, so the card is marked busy: dimmed, a
// spinner for its icon and a polite status. The 201's balance is final.
function BalancePreview({
  accountId,
  paid,
  paying,
  form,
  ownsEntry,
}: BalancePreviewProps): JSX.Element | null {
  const account = useAccount(accountId);
  const accountError = account.error;
  // Each failed read is shown once, as a toast and at the account ID, with
  // the cursor placed there. The ref keeps a re-run of the effect (a remount
  // under StrictMode, say) from showing the same failure twice.
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (accountError === null || accountError === shownError.current) {
      return;
    }
    shownError.current = accountError;
    // A read that fails once the entry was replaced (the reload after a
    // payment, say) concerns this card's account, not the entry now on the
    // form, so it is only toasted.
    if (ownsEntry()) {
      applyApiError(accountError, form, previewFieldMap);
    } else {
      showApiError(accountError);
    }
  }, [accountError, form, ownsEntry]);

  let balance: string;
  let updating: boolean;
  if (paid !== null && paid.accountId === accountId) {
    balance = paid.balance;
    updating = false;
  } else if (account.isSuccess) {
    balance = account.data.acct_curr_bal;
    updating = paying || account.isFetching;
  } else if (account.isError) {
    return null;
  } else {
    return (
      <div role="status" aria-busy="true">
        <span className="sr-only">Loading the current balance</span>
        <Skeleton aria-hidden="true" className="h-32 rounded-xl" />
      </div>
    );
  }

  return (
    <div aria-busy={updating}>
      {/* Rendered throughout, so the text is announced when it appears. */}
      <div role="status" className="sr-only">
        {updating ? "Updating the current balance" : null}
      </div>
      <SummaryCard
        title="Your current balance is"
        value={balance}
        description={`Account ${accountId}`}
        icon={
          updating ? (
            <LoaderCircle className="size-4 motion-safe:animate-spin" />
          ) : (
            <Wallet className="size-4" />
          )
        }
        className={updating ? "opacity-60" : undefined}
      />
    </div>
  );
}

type PaymentSummaryProps = { review: Review };

// The dialog's question: the BMS confirmation prompt over the reviewed entry.
function PaymentSummary({ review }: PaymentSummaryProps): JSX.Element {
  return (
    <div className="flex flex-col gap-3">
      <p>Do you want to pay your balance now? Please confirm.</p>
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-foreground">
        <dt className="font-medium">Account ID</dt>
        <dd className="font-mono break-all">{review.accountId}</dd>
        <dt className="font-medium">Your current balance is</dt>
        <dd className="font-mono tabular-nums break-all">{review.balance}</dd>
      </dl>
    </div>
  );
}

export default function BillPaymentPage(): JSX.Element {
  const form = useForm<PaymentFormValues>({
    resolver: zodResolver(paymentSchema),
    defaultValues: EMPTY_VALUES,
  });
  // Two instances, so the review and the payment keep states of their own.
  const review = usePaymentMutation();
  const payment = usePaymentMutation();

  // The account whose balance card shows, from a review's 200 until the
  // screen is cleared.
  const [previewId, setPreviewId] = useState<string | null>(null);
  // The entry the dialog asks about, kept while the dialog closes so its
  // description does not empty during the closing animation.
  const [reviewed, setReviewed] = useState<Review | null>(null);
  const [paid, setPaid] = useState<Paid | null>(null);
  const [confirmOpen, setConfirmOpen] = useState(false);
  // The dialog's action reports through onConfirm and then closes through
  // onOpenChange(false), as Cancel and Escape do; this tells them apart.
  const confirmedRef = useRef(false);
  // The request in flight, one at a time, as the 3270 keyboard locks while
  // the task runs. A mutation's isPending only changes with the next render,
  // and handleSubmit runs the schema asynchronously, so two submits in one
  // tick (a double click) would both pass a check of isPending and send two
  // reviews, and two presses of Confirm two payments. The ref is set before
  // each mutate call and released in its onSettled, which TanStack Query runs
  // even when onSuccess or onError throws; clearScreen releases a review it
  // drops, whose callbacks then never run.
  const inFlightRef = useRef<"review" | "payment" | null>(null);

  // The account input itself, beside the Controller's ref, for the one
  // synchronous focus a review answer needs (see reviewPayment).
  const accountInputRef = useRef<HTMLInputElement | null>(null);

  // Counts the replacements of the entry: each user edit of the account ID
  // and each clearing of the screen (Reset, Cancel, Escape), whose reset
  // fires no change event. The blanking after a payment is not counted. The
  // account ID stays usable while a review or a payment runs, and Reset while
  // a review runs (it abandons the review; it is disabled while the payment
  // writes), so reviewPayment, pay and the balance card compare it to tell
  // whether the form still holds the entry their request was made for.
  const entryRevision = useRef(0);
  useEffect(() => {
    const subscription = form.watch((_values, { type }) => {
      if (type === "change") {
        entryRevision.current += 1;
      }
    });
    return () => {
      subscription.unsubscribe();
    };
  }, [form]);
  // The revision of the entry the reviewed account (the balance card's and
  // the dialog's) was sent from.
  const previewRevision = useRef(0);
  const ownsPreviewEntry = useCallback(
    (): boolean => entryRevision.current === previewRevision.current,
    [],
  );

  // INITIALIZE-ALL-FIELDS: the account ID and the confirmation blanked.
  // keepFieldsRef keeps the input's registered ref, which a plain reset drops
  // until the next render, so a later setFocus still finds it.
  function blankEntry(): void {
    form.reset(EMPTY_VALUES, { keepFieldsRef: true });
  }

  // CLEAR-CURRENT-SCREEN [app/cbl/COBIL00C.cbl:L552-555]: the entry and the
  // balance. A review still in flight is dropped, so its answer opens no
  // dialog over the cleared screen, and its hold on inFlightRef is released
  // so the next ENTER is sent.
  function clearScreen(): void {
    entryRevision.current += 1;
    if (inFlightRef.current === "review") {
      inFlightRef.current = null;
    }
    review.reset();
    blankEntry();
    setPreviewId(null);
    setPaid(null);
  }

  // F4=Clear: the cleared screen with the cursor on ACTIDIN.
  function resetScreen(): void {
    clearScreen();
    form.setFocus(ACCOUNT_FIELD);
  }

  function reviewPayment(values: PaymentFormValues): void {
    if (inFlightRef.current !== null) {
      return;
    }
    const accountId = values.account_id;
    // The entry this review was sent from. An answer that arrives after it
    // was replaced concerns the reviewed account, not the entry now on the
    // form.
    const sentRevision = entryRevision.current;
    // A new ENTER sends a new screen: the previous balance goes until this
    // one is read.
    setPreviewId(null);
    setPaid(null);
    inFlightRef.current = "review";
    review.mutate(
      { account_id: accountId, confirm: CONFIRM_BLANK },
      {
        onSuccess: ({ data }) => {
          showMessage(data.message);
          setReviewed({ accountId, balance: data.current_balance });
          previewRevision.current = sentRevision;
          setPreviewId(accountId);
          // Every way out of the dialog leaves the cursor on ACTIDIN (MOVE -1
          // TO ACTIDINL): Cancel and Escape clear the screen, a payment
          // blanks it, and its failures name ACTIDIN. The dialog refocuses
          // the element that held focus when it opened, once it has left the
          // page, so that element is made the account input, whichever
          // control sent the ENTER. Focusing the input from here after the
          // dialog closes instead would land inside the page while the
          // closing dialog still hides it from assistive technology.
          accountInputRef.current?.focus();
          confirmedRef.current = false;
          setConfirmOpen(true);
        },
        onError: (error) => {
          if (entryRevision.current === sentRevision) {
            applyApiError(error, form, billPaymentBmsFieldMap);
          } else {
            showApiError(error);
          }
        },
        onSettled: () => {
          inFlightRef.current = null;
        },
      },
    );
  }

  function pay(afterClose: AfterClose): void {
    // Set first, so a press the guard drops still closes the dialog as a
    // Confirm, not as the Cancel that clears the screen.
    confirmedRef.current = true;
    if (reviewed === null || inFlightRef.current !== null) {
      return;
    }
    const { accountId } = reviewed;
    // An answer that arrives after the entry was replaced belongs to the
    // account paid, not to the one on the form: its text is still shown and
    // the card still takes the paid balance (it names the paid account), but
    // the newer entry is neither blanked, marked nor focused. The entry paid
    // is the one the review was sent from, so one typed while the review ran
    // is newer too.
    const sentRevision = previewRevision.current;
    const ownsEntry = (): boolean => entryRevision.current === sentRevision;
    inFlightRef.current = "payment";
    payment.mutate(
      { account_id: accountId, confirm: CONFIRM_YES },
      {
        onSuccess: ({ data, status }) => {
          if (status === CREATED) {
            // WRITE-TRANSACT-FILE NORMAL: the screen is blanked and
            // 'Payment successful.  Your Transaction ID is <id>.' shown in
            // green [app/cbl/COBIL00C.cbl:L523-531]; the closed dialog has
            // returned the cursor to the account input, and the card keeps
            // the paid account's new balance.
            showSuccess(data.message);
            setPaid({ accountId, balance: data.current_balance });
            if (ownsEntry()) {
              blankEntry();
            }
          } else {
            showMessage(data.message);
          }
        },
        onError: (error) => {
          const options = { unknownOutcome: PAYMENT_OUTCOME_UNKNOWN };
          // The marking focuses ACTIDIN, which waits for the dialog to leave
          // the page.
          if (ownsEntry()) {
            afterClose(() => {
              applyApiError(error, form, billPaymentBmsFieldMap, options);
            });
          } else {
            showApiError(error, options);
          }
        },
        // Runs once the hook's reloads after the payment have settled.
        onSettled: () => {
          inFlightRef.current = null;
        },
      },
    );
  }

  function onConfirmOpenChange(open: boolean): void {
    setConfirmOpen(open);
    if (!open && !confirmedRef.current) {
      clearScreen();
    }
  }

  // The task running: the review, or the payment with the reloads after it.
  // The submit shows it through aria-disabled rather than disabled, because a
  // focused button that becomes disabled drops the focus to the document;
  // inFlightRef keeps the inert submit, and Enter in the input, from sending.
  // Its text stays its function-map name, the spinner being hidden.
  const busy = review.isPending || payment.isPending;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description="Pay the current balance of an account in full."
        actions={<BackLink to="/" />}
      />
      {/* One column at md, two at lg. */}
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <form noValidate onSubmit={form.handleSubmit(reviewPayment)}>
          <Card>
            <CardHeader>
              <CardTitle>
                <h2>Account</h2>
              </CardTitle>
              <CardDescription>
                Enter the account whose balance you want to pay.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <Controller
                control={form.control}
                name={ACCOUNT_FIELD}
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor={ACCOUNT_FIELD}>Account ID</FieldLabel>
                    {/* autoFocus: the IC attribute of ACTIDIN
                        [app/bms/COBIL00.bms:L85]. */}
                    <Input
                      {...field}
                      ref={(element) => {
                        field.ref(element);
                        accountInputRef.current = element;
                      }}
                      id={ACCOUNT_FIELD}
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
            </CardContent>
            {/* The map's key line order: ENTER, then F4; F3 is the header's
                Back link. */}
            <CardFooter className="flex-wrap gap-2">
              <Button
                type="submit"
                aria-disabled={busy}
                aria-busy={busy}
                className="aria-disabled:pointer-events-none aria-disabled:opacity-50"
              >
                {busy ? (
                  <LoaderCircle
                    aria-hidden="true"
                    className="motion-safe:animate-spin"
                  />
                ) : null}
                Review payment
              </Button>
              {/* Disabled while the payment writes. During the read-only
                  review it stays usable, as the way to abandon it. */}
              <Button
                type="button"
                variant="outline"
                disabled={payment.isPending}
                onClick={resetScreen}
              >
                Reset
              </Button>
            </CardFooter>
          </Card>
        </form>
        {previewId === null ? null : (
          <BalancePreview
            accountId={previewId}
            paid={paid}
            paying={payment.isPending}
            form={form}
            ownsEntry={ownsPreviewEntry}
          />
        )}
      </div>
      <ConfirmDialog
        open={confirmOpen}
        onOpenChange={onConfirmOpenChange}
        title="Confirm the bill payment"
        description={
          reviewed === null ? null : <PaymentSummary review={reviewed} />
        }
        confirmLabel="Confirm"
        cancelLabel="Cancel"
        onConfirm={pay}
        pending={payment.isPending}
      />
    </div>
  );
}
