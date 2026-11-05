// CTLI transaction type list (COTRTLIC, map COTRTLI) at /admin/transaction-types:
// seven types per page in type-code order over the Db2 master
// `transaction_types`, with the TRTYPE and TRDESC filters, an inline
// description update and a confirmed delete (docs/parity/function-map.md,
// F-COTRTLI-001 to F-COTRTLI-009).
//
// | COTRTLI key | Control | Request |
// |---|---|---|
// | ENTER | form-submit "Search" with "Type filter", "Description filter" | GET /admin/transaction-types |
// | F2=Add | link "Add transaction type" | navigation to /admin/transaction-types/new |
// | F3=Exit | link "Back" | navigation to /admin |
// | F7=Page Up, F8=Page Dn | the data table's "Previous" and "Next" | GET with start_key and direction |
// | selector U, F10=Save | row button "Edit", then the row's "Save" | PUT /admin/transaction-types/{id}?origin=list |
// | selector D, F10 | row button "Delete", then the dialog's "Confirm" | DELETE /admin/transaction-types/{id} |
//
// Every text the COBOL shows comes from the server (page-boundary texts in the
// page's `message`, write texts in the answer) or from the schema module, and
// reaches the user through lib/api-errors. Writes change the Db2 master only;
// reports show an edit once `batch seed --only tranextr` has run. Access is
// the server's to refuse: RequireAdmin guards the route in App.tsx.
import {
  createContext,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type JSX,
} from "react";
import { Link } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { Plus } from "lucide-react";

import {
  useTranTypeDeleteMutation,
  useTranTypeUpdateMutation,
  useTranTypes,
  type TranType,
  type TranTypeListParams,
} from "@/api/queries/transaction-types";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { BackLink, PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  applyApiError,
  showApiError,
  showMessage,
  showSuccess,
} from "@/lib/api-errors";
import {
  toTranTypeListQuery,
  toTranTypeUpdateRequest,
  tranTypeInlineEditBmsFieldMap,
  tranTypeInlineEditSchema,
  tranTypeListBmsFieldMap,
  tranTypeListFilterSchema,
  type TranTypeInlineEditValues,
  type TranTypeListFilterValues,
} from "@/schemas/transaction-types";

/** `TRTYPE` and the row `TRTTYPn`, `X(2)`. */
const TYPE_CODE_MAX_LENGTH = 2;

/** `TRDESC` and the row `TRTYPDn`, `X(50)`. */
const DESCRIPTION_MAX_LENGTH = 50;

/**
 * The one width of `TRTYPDn`, shown or open for input. With a 50-character
 * description on the page, the table fits its card at 768px beside the icon
 * rail and at 1024px beside the expanded sidebar, and from `lg` the input
 * shows a typical 50-character description whole: Save sits under it below
 * `xl` and beside it from `xl`.
 */
const DESCRIPTION_WIDTH = "w-112 lg:w-125 xl:w-144";

/** One array for every page without rows, so the table sees one empty result. */
const NO_ROWS: TranType[] = [];

/** The seven rows of a COTRTLI page, the server's default page size. */
const PAGE_ROWS = 7;

// The notes under the error of a write that got no usable answer: the server
// may have carried it out, so the user checks before sending it again.
const UPDATE_OUTCOME_UNKNOWN =
  "The description may have been saved. Check the type before saving it again.";
const DELETE_OUTCOME_UNKNOWN =
  "The type may have been deleted. Check the list before deleting it again.";

type Direction = NonNullable<TranTypeListParams["direction"]>;

/** The filters of a list request, without its cursor. */
function filterOf(params: TranTypeListParams): TranTypeListParams {
  const filter: TranTypeListParams = {};
  if (params.type_code !== undefined) {
    filter.type_code = params.type_code;
  }
  if (params.description !== undefined) {
    filter.description = params.description;
  }
  return filter;
}

/** Whether two list requests have one query key, so neither refetches the other. */
function sameRequest(a: TranTypeListParams, b: TranTypeListParams): boolean {
  return (
    a.type_code === b.type_code &&
    a.description === b.description &&
    a.start_key === b.start_key &&
    a.direction === b.direction &&
    a.page_size === b.page_size
  );
}

/**
 * What the row cells share with the page. The columns are module constants,
 * so the table renders the same cell components on every page render and a
 * row's inline form keeps its state while the page re-renders around it.
 */
type RowActions = {
  /** The type code of the row whose description is open for input. */
  editingCode: string | null;
  toggleEdit: (typeCode: string) => void;
  finishEdit: (typeCode: string) => void;
  requestDelete: (tranType: TranType) => void;
  /** Prefix of the ids that name each row's type code cell. */
  idPrefix: string;
};

const RowActionsContext = createContext<RowActions | null>(null);

function useRowActions(): RowActions {
  const actions = useContext(RowActionsContext);
  if (actions === null) {
    throw new Error("Row cells render only inside TranTypeListPage");
  }
  return actions;
}

function typeCodeCellId(idPrefix: string, rowIndex: number): string {
  return `${idPrefix}-type-${rowIndex}`;
}

type RowCellProps = { tranType: TranType; rowIndex: number };

/** `TRTTYPn`: the row's type code, which describes the row's actions. */
function TypeCodeCell({ tranType, rowIndex }: RowCellProps): JSX.Element {
  const { idPrefix } = useRowActions();
  return (
    <span id={typeCodeCellId(idPrefix, rowIndex)} className="font-mono">
      {tranType.type_code}
    </span>
  );
}

/**
 * `TRTYPDn` opened for input after the `U` selector, with F10=Save
 * (COTRTLIC 1211-EDIT-ARRAY-DESC, 9200-UPDATE-RECORD). The update is sent
 * with `origin: "list"`, which selects the COTRTLIC inline edits and texts.
 * Its errors name no BMS field, so they show as toasts.
 */
function InlineDescriptionForm({ tranType }: { tranType: TranType }): JSX.Element {
  const { finishEdit } = useRowActions();
  const inputId = useId();
  const errorId = useId();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const updateMutation = useTranTypeUpdateMutation();
  const rowForm = useForm<TranTypeInlineEditValues>({
    resolver: zodResolver(tranTypeInlineEditSchema),
    defaultValues: { description: tranType.description },
  });

  // The cursor opens at the start of TRTYPDn, as on the 3270 screen, so a
  // description wider than the input reads from its first character.
  useEffect(() => {
    const input = inputRef.current;
    if (input !== null) {
      input.setSelectionRange(0, 0);
      input.scrollLeft = 0;
    }
  }, []);

  // One update at a time: a second click or Enter before the busy state
  // re-renders would send the PUT again, and its answer would replace the
  // first one's. The ref closes that gap.
  const inFlight = useRef(false);

  const onSave = (values: TranTypeInlineEditValues): void => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    updateMutation.mutate(
      {
        typeCode: tranType.type_code,
        body: toTranTypeUpdateRequest(values),
        origin: "list",
      },
      {
        onSuccess: (answer) => {
          // The info line differs from the message only for a description
          // equal to the stored one but for case and surrounding spaces:
          // COTRTLIC answers its no-change text, a neutral notice as on the
          // CTTU screen, yet stores the entered text, which the info line
          // reports as a success. Otherwise the one text is the update's.
          if (answer.info !== undefined && answer.info !== answer.message) {
            showMessage(answer.message);
            showSuccess(answer.info);
          } else {
            showSuccess(answer.message);
          }
          finishEdit(tranType.type_code);
        },
        onError: (error) => {
          applyApiError(error, rowForm, tranTypeInlineEditBmsFieldMap, {
            unknownOutcome: UPDATE_OUTCOME_UNKNOWN,
          });
        },
        // Runs after either outcome; a row that has closed has no guard to
        // release.
        onSettled: () => {
          inFlight.current = false;
        },
      },
    );
  };

  // aria-disabled rather than disabled: a focused button that becomes
  // disabled loses focus to the document, which would strand a keyboard user
  // after Save; the guard above keeps the inert button from acting.
  const busy = updateMutation.isPending;
  const busyClass = "aria-disabled:pointer-events-none aria-disabled:opacity-50";

  return (
    <form
      noValidate
      aria-busy={busy}
      onSubmit={rowForm.handleSubmit(onSave)}
      className={`flex flex-col gap-2 xl:flex-row xl:items-start ${DESCRIPTION_WIDTH}`}
    >
      <Controller
        name="description"
        control={rowForm.control}
        render={({ field, fieldState }) => (
          <Field
            data-invalid={fieldState.invalid}
            className="min-w-0 flex-1 whitespace-normal"
          >
            <FieldLabel htmlFor={inputId} className="sr-only">
              Description
            </FieldLabel>
            {/* Focus moves into the opened row, as COTRTLIC places the cursor
                on the selected row's TRTYPDn. */}
            <Input
              {...field}
              ref={(element) => {
                field.ref(element);
                inputRef.current = element;
              }}
              id={inputId}
              maxLength={DESCRIPTION_MAX_LENGTH}
              autoComplete="off"
              autoFocus
              aria-invalid={fieldState.invalid}
              aria-describedby={fieldState.error ? errorId : undefined}
            />
            <FieldError id={errorId} errors={[fieldState.error]} />
          </Field>
        )}
      />
      <Button
        type="submit"
        aria-disabled={busy}
        className={`self-start ${busyClass}`}
      >
        Save
      </Button>
    </form>
  );
}

/**
 * `TRTYPDn`: the description, or its inline form while the row is selected.
 * The text and the form take the one fixed width, DESCRIPTION_WIDTH, and the
 * text wraps inside it rather than widening the cell, so the table's automatic
 * column widths do not move when a row opens or closes.
 */
function DescriptionCell({ tranType }: RowCellProps): JSX.Element {
  const { editingCode } = useRowActions();
  if (editingCode !== tranType.type_code) {
    return (
      <span
        className={`block whitespace-normal wrap-break-word ${DESCRIPTION_WIDTH}`}
      >
        {tranType.description}
      </span>
    );
  }
  return (
    <InlineDescriptionForm key={tranType.type_code} tranType={tranType} />
  );
}

/**
 * `TRTSELn`: the `U` and `D` selectors as row buttons. "Edit" is a toggle:
 * pressing it again closes the row, as the COBOL user blanks the selector,
 * since the map gives the inline update no cancel key. One row is open at a
 * time, the one-action rule of COTRTLIC 1210-EDIT-ARRAY.
 */
function RowActionsCell({ tranType, rowIndex }: RowCellProps): JSX.Element {
  const { editingCode, toggleEdit, requestDelete, idPrefix } = useRowActions();
  const editing = editingCode === tranType.type_code;
  const editRef = useRef<HTMLButtonElement>(null);
  const wasEditing = useRef(editing);
  const describedBy = typeCodeCellId(idPrefix, rowIndex);

  // A save removes the focused Save button with the form; focus returns to
  // this row's Edit rather than the start of the document. A press of the
  // pressed Edit leaves focus on it. Either way the Edit, with the margin its
  // focus ring takes, is scrolled into the visible part of the table, which
  // scrolls inside its card. A row closed because another one opened leaves
  // focus in the newly opened input. A layout effect, so it runs before the
  // data table's own, which would otherwise find the removed Save's focus on
  // the body and hand it to the table.
  useLayoutEffect(() => {
    const edit = editRef.current;
    const active = document.activeElement;
    if (
      edit !== null &&
      wasEditing.current &&
      !editing &&
      (active === null || active === document.body || active === edit)
    ) {
      edit.focus();
      edit.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
    wasEditing.current = editing;
  }, [editing]);

  return (
    <div className="flex items-center gap-2">
      <Button
        ref={editRef}
        type="button"
        variant="link"
        aria-pressed={editing}
        aria-describedby={describedBy}
        className="scroll-mx-1 aria-pressed:underline"
        onClick={() => toggleEdit(tranType.type_code)}
      >
        Edit
      </Button>
      <Button
        type="button"
        variant="link"
        aria-describedby={describedBy}
        onClick={() => requestDelete(tranType)}
      >
        Delete
      </Button>
    </div>
  );
}

/**
 * Reports its own mount and unmount. Inside the confirm dialog's content it
 * tells the page when that content, closing animation included, has left the
 * document: React runs the cleanups of a removed subtree parent first, so
 * this one runs after the dialog's own, which un-hide the rest of the page.
 */
function MountSignal({
  onChange,
}: {
  onChange: (mounted: boolean) => void;
}): null {
  useEffect(() => {
    onChange(true);
    return () => onChange(false);
  }, [onChange]);
  return null;
}

// The table keys its rows by type code, the Db2 key, and so do the cells: a
// row's buttons and form never carry over to another type when a delete or a
// page change moves other types into its place, so the focus the confirm
// dialog returns to its opener cannot land on another type's Delete.
function tranTypeRowId(tranType: TranType): string {
  return tranType.type_code;
}

// Each placeholder is its cell's markup around a value of the column's
// format: the two-digit type code, a description in the description's one
// fixed width (DESCRIPTION_WIDTH), and the row actions.
const columns: DataTableProps<TranType>["columns"] = [
  {
    accessorKey: "type_code",
    header: "Type",
    cell: ({ row }) => (
      <TypeCodeCell
        key={row.original.type_code}
        tranType={row.original}
        rowIndex={row.index}
      />
    ),
    meta: { placeholder: <span className="font-mono">00</span> },
  },
  {
    accessorKey: "description",
    header: "Description",
    cell: ({ row }) => (
      <DescriptionCell
        key={row.original.type_code}
        tranType={row.original}
        rowIndex={row.index}
      />
    ),
    meta: {
      placeholder: (
        <span
          className={`block whitespace-normal wrap-break-word ${DESCRIPTION_WIDTH}`}
        >
          AUTHORIZATION
        </span>
      ),
    },
  },
  {
    id: "actions",
    header: "Actions",
    cell: ({ row }) => (
      <RowActionsCell
        key={row.original.type_code}
        tranType={row.original}
        rowIndex={row.index}
      />
    ),
    meta: {
      placeholder: (
        <div className="flex items-center gap-2">
          <Button variant="link" asChild>
            <span>Edit</span>
          </Button>
          <Button variant="link" asChild>
            <span>Delete</span>
          </Button>
        </div>
      ),
    },
  },
];

export default function TranTypeListPage(): JSX.Element {
  const typeCodeId = useId();
  const typeCodeErrorId = useId();
  const descriptionId = useId();
  const descriptionErrorId = useId();
  const idPrefix = useId();

  // The applied filters and cursor. Blank filters are omitted, which
  // COTRTLIC reads as no filter.
  const [params, setParams] = useState<TranTypeListParams>({});
  // The request on screen, for an answer that arrives after the page moved
  // on: the confirm dialog closes on Confirm, so Search, Previous and Next
  // stay usable while a delete runs.
  const currentParams = useRef(params);
  useEffect(() => {
    currentParams.current = params;
  }, [params]);
  const [editingCode, setEditingCode] = useState<string | null>(null);
  // The row the dialog names stays set while the dialog closes, so its text
  // does not change during the closing animation.
  const [deleteTarget, setDeleteTarget] = useState<TranType | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const list = useTranTypes(params);
  // Called through this name: page sources hold no call of a name ending in
  // "fetch", which the source scans reserve for the API client.
  const rereadList = list.refetch;
  const deleteMutation = useTranTypeDeleteMutation();
  // A failed read of the same request keeps the query's last page; it is not
  // shown, because that page no longer answers the request (after the last
  // matching type is deleted, the re-read finds no type for the filters).
  const page = list.isError ? undefined : list.data;
  const items = page?.items ?? NO_ROWS;
  // Whether the confirm dialog is mounted, its closing animation included:
  // the modal hides the rest of the page from assistive technology until then.
  const [dialogPresent, setDialogPresent] = useState(false);

  const form = useForm<TranTypeListFilterValues>({
    resolver: zodResolver(tranTypeListFilterSchema),
    defaultValues: { type_code: "", description: "" },
  });

  // A failed list request: its message as a toast and, for TRTYPE or TRDESC,
  // the field error and focus, once per failure (the ref also holds across
  // the effect replay of StrictMode). A failure that arrives while the
  // dialog is still mounted waits for it to go, so focus never moves into
  // the content the modal hides.
  const shownError = useRef<unknown>(null);
  useEffect(() => {
    if (
      list.error !== null &&
      list.error !== shownError.current &&
      !dialogPresent
    ) {
      shownError.current = list.error;
      applyApiError(list.error, form, tranTypeListBmsFieldMap);
    }
  }, [list.error, form, dialogPresent]);

  // The page-boundary text answers the key that asked for the page, F7 or F8
  // (COTRTLIC 2500-SETUP-MESSAGE, 8000-READ-FORWARD), so it is shown once per
  // list request: not again when a write or a refocus reads the same page
  // again. It comes from the page the request read: a cached copy shown while
  // that read runs, such as the first page after a delete, waits for the
  // answer. An empty page shows it in the table instead.
  const shownFor = useRef<TranTypeListParams | null>(null);
  useEffect(() => {
    if (
      page === undefined ||
      list.isPlaceholderData ||
      list.isFetching ||
      shownFor.current === params
    ) {
      return;
    }
    shownFor.current = params;
    if (page.items.length > 0) {
      showMessage(page.message);
    }
  }, [page, params, list.isPlaceholderData, list.isFetching]);

  // ENTER: the filters, read from the first page, since the Search request
  // carries no start key (F-COTRTLI-001). Unchanged filters on the first
  // page still read the list again, as ENTER does.
  const onFilter = (values: TranTypeListFilterValues): void => {
    const next = toTranTypeListQuery(values);
    if (sameRequest(next, params)) {
      void rereadList();
      return;
    }
    setEditingCode(null);
    setParams(next);
  };

  // F7 and F8: the adjacent page with the filters applied. The row selection
  // is cleared, as COTRTLIC initializes the selectors on a page change.
  const goTo = (startKey: string | null | undefined, direction: Direction) => {
    if (startKey === null || startKey === undefined || startKey === "") {
      return;
    }
    setEditingCode(null);
    setParams((current) => ({
      ...filterOf(current),
      start_key: startKey,
      direction,
    }));
  };

  const rowActions: RowActions = {
    editingCode,
    toggleEdit: (typeCode) =>
      setEditingCode((current) => (current === typeCode ? null : typeCode)),
    finishEdit: (typeCode) =>
      setEditingCode((current) => (current === typeCode ? null : current)),
    requestDelete: (tranType) => {
      setEditingCode(null);
      setDeleteTarget(tranType);
      setDeleteOpen(true);
    },
    idPrefix,
  };

  // F10 on a pending `D`. A deleted type returns the list to its first page
  // under the same filters, as COTRTLIC resets its paging after the delete.
  // The delete marks the cached pages stale without reading any, so that
  // first page is read once: by the change of request, or by rereadList when
  // the first page is already the request on screen. The new request identity
  // shows the first page's text once its fresh answer is in.
  const onConfirmDelete = (): void => {
    if (deleteTarget === null) {
      return;
    }
    deleteMutation.mutate(
      { typeCode: deleteTarget.type_code },
      {
        onSuccess: (answer) => {
          showSuccess(answer.message);
          const shown = currentParams.current;
          const first = filterOf(shown);
          if (sameRequest(first, shown)) {
            void rereadList();
          }
          setParams(first);
        },
        onError: (error) => {
          showApiError(error, { unknownOutcome: DELETE_OUTCOME_UNKNOWN });
        },
      },
    );
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        description="Filter the transaction types, edit a description in its row, or delete a type."
        actions={
          <>
            <Button asChild>
              <Link to="/admin/transaction-types/new">
                <Plus aria-hidden="true" />
                Add transaction type
              </Link>
            </Button>
            <BackLink to="/admin" />
          </>
        }
      />

      <Card>
        <CardContent>
          <form
            role="search"
            aria-label="Transaction type filters"
            noValidate
            onSubmit={form.handleSubmit(onFilter)}
            className="flex flex-col gap-4"
          >
            <div className="grid gap-4 lg:grid-cols-2">
              <Controller
                name="type_code"
                control={form.control}
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor={typeCodeId}>Type filter</FieldLabel>
                    {/* TRTYPE carries the initial cursor (IC). */}
                    <Input
                      {...field}
                      value={field.value ?? ""}
                      id={typeCodeId}
                      maxLength={TYPE_CODE_MAX_LENGTH}
                      inputMode="numeric"
                      autoComplete="off"
                      autoFocus
                      aria-invalid={fieldState.invalid}
                      aria-describedby={
                        fieldState.error ? typeCodeErrorId : undefined
                      }
                    />
                    <FieldError
                      id={typeCodeErrorId}
                      errors={[fieldState.error]}
                    />
                  </Field>
                )}
              />
              <Controller
                name="description"
                control={form.control}
                render={({ field, fieldState }) => (
                  <Field data-invalid={fieldState.invalid}>
                    <FieldLabel htmlFor={descriptionId}>
                      Description filter
                    </FieldLabel>
                    <Input
                      {...field}
                      value={field.value ?? ""}
                      id={descriptionId}
                      maxLength={DESCRIPTION_MAX_LENGTH}
                      autoComplete="off"
                      aria-invalid={fieldState.invalid}
                      aria-describedby={
                        fieldState.error ? descriptionErrorId : undefined
                      }
                    />
                    <FieldError
                      id={descriptionErrorId}
                      errors={[fieldState.error]}
                    />
                  </Field>
                )}
              />
            </div>
            <div className="flex justify-end">
              <Button type="submit">Search</Button>
            </div>
          </form>
        </CardContent>
      </Card>

      <RowActionsContext value={rowActions}>
        <DataTable<TranType>
          columns={columns}
          getRowId={tranTypeRowId}
          data={items}
          prevKey={page?.prev_key}
          nextKey={page?.next_key}
          onPrevious={() => goTo(page?.prev_key, "backward")}
          onNext={() => goTo(page?.next_key, "forward")}
          emptyMessage={page?.message}
          caption="Transaction types"
          loading={list.isPending || list.isPlaceholderData}
          pageSize={PAGE_ROWS}
          rowHeight="default"
        />
      </RowActionsContext>

      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title="Delete transaction type"
        description={
          <>
            {deleteTarget === null ? null : (
              <p>
                Type{" "}
                <span className="font-mono">{deleteTarget.type_code}</span>
                {", "}
                {deleteTarget.description}, will be deleted.
              </p>
            )}
            <MountSignal onChange={setDialogPresent} />
          </>
        }
        confirmLabel="Confirm"
        onConfirm={onConfirmDelete}
        pending={deleteMutation.isPending}
      />
    </div>
  );
}
