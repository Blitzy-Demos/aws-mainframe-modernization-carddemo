// CR00 transaction reports: the CORPT00 map (CORPT0A) and CORPT00C. ENTER
// becomes "Run report", whose form holds the map's three report-type
// selectors as one radio group and its six date parts as two YYYY-MM-DD texts
// shown for a custom range. CONFIRM becomes the confirmation dialog, whose
// "Confirm" stands for CONFIRM Y and sends the request (E-48), and F3=Back is
// the "Back" link to the main menu. The synchronous GET /reports/transactions
// replaces the TDQ JOBS submission of the TRANREPT job, so the report it
// returns is shown on this page.
//
// The server runs every CORPT00C edit, the CSUTLDTC calendar check included,
// and answers with the audited text; the form schema only mirrors the edits
// it can decide, so input it rejects is never sent. Accessible names are the
// F-CORPT00 rows of docs/parity/function-map.md.
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type JSX,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Controller,
  useForm,
  useWatch,
  type DefaultValues,
} from "react-hook-form";
import { Link } from "react-router";

import {
  useTransactionReportMutation,
  type TransactionReport,
} from "@/api/queries/reports";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Skeleton } from "@/components/ui/skeleton";
import { applyApiError, showApiError, showSuccess } from "@/lib/api-errors";
import {
  reportRequestBmsFieldMap,
  reportRequestSchema,
  type ReportRequestFormInput,
  type ReportRequestFormValues,
} from "@/schemas/reports";

type ReportType = ReportRequestFormValues["report_type"];

/** The map's selectors in screen order: MONTHLY, YEARLY, CUSTOM. */
const REPORT_TYPES = [
  "monthly",
  "yearly",
  "custom",
] as const satisfies readonly ReportType[];

/** The CORPT00 captions of the selectors [app/bms/CORPT00.bms]. */
const REPORT_TYPE_LABELS: Readonly<Record<ReportType, string>> = {
  monthly: "Monthly (Current Month)",
  yearly: "Yearly (Current Year)",
  custom: "Custom (Date Range)",
};

/**
 * The custom range inputs: SDTYYYY/SDTMM/SDTDD and EDTYYYY/EDTMM/EDTDD, one
 * YYYY-MM-DD text each, as there is no date picker.
 */
const DATE_FIELDS = [
  { name: "start_date", label: "Start date" },
  { name: "end_date", label: "End date" },
] as const satisfies readonly {
  name: keyof ReportRequestFormInput;
  label: string;
}[];

/** The OpenAPI maxLength of a date text: X(4), X(2), X(2) and two hyphens. */
const DATE_TEXT_MAX_LENGTH = 10;

/** The format each date text takes, the CSUTLDTC picture of the request. */
const DATE_FORMAT_HINT = "YYYY-MM-DD";

/**
 * CONFIRMI 'Y', the value CORPT00C accepts to submit the report
 * [app/cbl/CORPT00C.cbl:L478-479], sent by the dialog's "Confirm".
 */
const CONFIRM_YES = "Y";

/** The map's confirmation caption, without the CONFIRM field it led into. */
const CONFIRM_PROMPT =
  "The Report will be submitted for printing. Please confirm.";

/**
 * The screen INITIALIZE-ALL-FIELDS leaves [app/cbl/CORPT00C.cbl:L633-646]: no
 * selector entered and the date parts empty. CONFIRM has no form field, since
 * the dialog supplies it.
 */
const DEFAULT_VALUES: DefaultValues<ReportRequestFormInput> = {
  start_date: "",
  end_date: "",
};

/** The dialog's description of the request about to run. */
function describeRequest(request: ReportRequestFormValues): ReactNode {
  const range =
    request.report_type === "custom"
      ? `: ${request.start_date ?? ""} to ${request.end_date ?? ""}`
      : "";
  return (
    <>
      <p>
        {REPORT_TYPE_LABELS[request.report_type]}
        {range}
      </p>
      <p>{CONFIRM_PROMPT}</p>
    </>
  );
}

export default function TransactionReportPage(): JSX.Element {
  const baseId = useId();
  const reportTypeLegendId = `${baseId}-report-type`;
  const reportTypeErrorId = `${baseId}-report-type-error`;
  const reportTitleId = `${baseId}-report-title`;

  const form = useForm<
    ReportRequestFormInput,
    unknown,
    ReportRequestFormValues
  >({
    resolver: zodResolver(reportRequestSchema),
    defaultValues: DEFAULT_VALUES,
  });
  const reportType = useWatch({ control: form.control, name: "report_type" });
  const mutation = useTransactionReportMutation();

  // The request a valid ENTER produced, kept while the dialog closes so its
  // description does not empty during the closing animation.
  const [request, setRequest] = useState<ReportRequestFormValues | null>(null);
  const [confirming, setConfirming] = useState(false);
  // The last report produced. It stays shown after the form is cleared and
  // while a later request fails, and is replaced by the next report.
  const [report, setReport] = useState<TransactionReport | null>(null);
  // The records as one text, joined once per report rather than on every
  // render of the form above it: a report has no line cap.
  const reportText = useMemo(() => report?.lines.join("\n") ?? "", [report]);

  // Counts the user's edits of the entry: each radio choice and each change
  // of a date text. The reset after a report fires no change event and is
  // not counted. runReport compares it to tell whether the form still holds
  // the entry it sent, since the inputs stay editable while a report runs.
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

  const askForConfirmation = (values: ReportRequestFormValues): void => {
    if (mutation.isPending) {
      return;
    }
    setRequest(values);
    setConfirming(true);
  };

  const runReport = (): void => {
    if (request === null) {
      return;
    }
    // An answer that arrives after the entry was edited belongs to the entry
    // sent, not to the one on the form: its report and message are still
    // shown, but the newer entry is neither cleared, marked nor focused.
    const sentRevision = entryRevision.current;
    const ownsEntry = (): boolean => entryRevision.current === sentRevision;
    mutation.mutate(
      { ...request, confirm: CONFIRM_YES },
      {
        onSuccess: (data) => {
          setReport(data);
          // '<name> report submitted for printing ...', which CORPT00C shows
          // in green after INITIALIZE-ALL-FIELDS, the cursor on MONTHLY
          // [app/cbl/CORPT00C.cbl:L445-454].
          showSuccess(data.message);
          if (!ownsEntry()) {
            return;
          }
          // keepFieldsRef keeps the registered elements, so setFocus finds
          // the MONTHLY item now rather than after the fields re-register.
          form.reset(DEFAULT_VALUES, { keepFieldsRef: true });
          form.setFocus("report_type");
        },
        onError: (error) => {
          if (ownsEntry()) {
            applyApiError(error, form, reportRequestBmsFieldMap);
          } else {
            showApiError(error);
          }
        },
      },
    );
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader />

      <Card>
        <CardContent>
          <form
            className="flex flex-col gap-6"
            noValidate
            onSubmit={form.handleSubmit(askForConfirmation)}
          >
            <Controller
              control={form.control}
              name="report_type"
              render={({ field, fieldState }) => (
                <FieldSet>
                  <FieldLegend id={reportTypeLegendId} variant="label">
                    Report type
                  </FieldLegend>
                  <RadioGroup
                    name={field.name}
                    value={field.value ?? ""}
                    onValueChange={field.onChange}
                    disabled={field.disabled}
                    aria-labelledby={reportTypeLegendId}
                    aria-invalid={fieldState.invalid}
                    aria-describedby={
                      fieldState.invalid ? reportTypeErrorId : undefined
                    }
                  >
                    {REPORT_TYPES.map((type, index) => {
                      const id = `${baseId}-${type}`;
                      return (
                        <Field key={type} orientation="horizontal">
                          {/* The field's ref is the MONTHLY item, so focus on
                              this field lands where CORPT00C puts the cursor:
                              MOVE -1 TO MONTHLYL [app/cbl/CORPT00C.cbl:L180,
                              L441, L453]. */}
                          <RadioGroupItem
                            id={id}
                            value={type}
                            ref={index === 0 ? field.ref : undefined}
                            onBlur={field.onBlur}
                            aria-invalid={fieldState.invalid}
                          />
                          <FieldLabel htmlFor={id} className="font-normal">
                            {REPORT_TYPE_LABELS[type]}
                          </FieldLabel>
                        </Field>
                      );
                    })}
                  </RadioGroup>
                  <FieldError
                    id={reportTypeErrorId}
                    errors={[fieldState.error]}
                  />
                </FieldSet>
              )}
            />

            {reportType === "custom" ? (
              <div className="grid gap-4 lg:grid-cols-2">
                {DATE_FIELDS.map(({ name, label }) => (
                  <Controller
                    key={name}
                    control={form.control}
                    name={name}
                    render={({ field, fieldState }) => {
                      const id = `${baseId}-${name}`;
                      const descriptionId = `${id}-description`;
                      const errorId = `${id}-error`;
                      return (
                        <Field data-invalid={fieldState.invalid}>
                          <FieldLabel htmlFor={id}>{label}</FieldLabel>
                          <Input
                            {...field}
                            id={id}
                            value={field.value ?? ""}
                            inputMode="numeric"
                            maxLength={DATE_TEXT_MAX_LENGTH}
                            placeholder={DATE_FORMAT_HINT}
                            autoComplete="off"
                            aria-invalid={fieldState.invalid}
                            aria-describedby={
                              fieldState.invalid
                                ? `${descriptionId} ${errorId}`
                                : descriptionId
                            }
                          />
                          <FieldDescription id={descriptionId}>
                            {DATE_FORMAT_HINT}
                          </FieldDescription>
                          <FieldError
                            id={errorId}
                            errors={[fieldState.error]}
                          />
                        </Field>
                      );
                    }}
                  />
                ))}
              </div>
            ) : null}

            <div className="flex flex-wrap items-center gap-2">
              <Button type="submit" disabled={mutation.isPending}>
                Run report
              </Button>
              <Button asChild variant="outline">
                <Link to="/">Back</Link>
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {mutation.isPending ? (
        <div aria-busy="true" className="flex flex-col gap-2">
          <span className="sr-only">Running report</span>
          <Skeleton className="h-32 w-full" />
        </div>
      ) : report !== null ? (
        <section aria-labelledby={reportTitleId}>
          <Card>
            <CardHeader>
              <CardTitle>
                <h2 id={reportTitleId}>{report.report_name}</h2>
              </CardTitle>
              <CardDescription>
                {report.start_date} – {report.end_date}
              </CardDescription>
            </CardHeader>
            <CardContent>
              {/* The CVTRA07Y records exactly as returned: fixed width, with
                  significant trailing spaces, so they are never trimmed or
                  wrapped. Focusable so the keyboard can scroll the wide
                  lines. */}
              <pre
                tabIndex={0}
                className="overflow-x-auto font-mono text-sm whitespace-pre"
              >
                {reportText}
              </pre>
            </CardContent>
          </Card>
        </section>
      ) : null}

      {/* Cancel and Escape (F-CORPT00-003) only close the dialog and send
          nothing. The entries stay on the form, unlike the CONFIRM N that
          clears the screen [app/cbl/CORPT00C.cbl:L480-483], so the same
          request can be run again. */}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Confirm the report"
        description={request === null ? null : describeRequest(request)}
        confirmLabel="Confirm"
        cancelLabel="Cancel"
        onConfirm={runReport}
        pending={mutation.isPending}
      />
    </div>
  );
}
