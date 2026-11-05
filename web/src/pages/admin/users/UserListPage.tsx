// CU00 user list, COUSR00C on the COUSR00 map, and the CU03 user delete,
// COUSR03C on the COUSR03 map, as one page at /admin/users (AAP 0.3.1). It is
// the destination of COADM02Y options 1, 3 and 4 (web/src/nav/menu.ts, E-61).
//
// - ENTER=Continue is the Search form: USRIDIN is the start key of a forward
//   browse, blank from the first user (F-COUSR00-001). F7=Backward and
//   F8=Forward are the data table's Previous and Next (F-COUSR00-003,
//   F-COUSR00-004), F3=Back the Back link to /admin (F-COUSR00-002).
// - The row selectors become row actions (E-54): U is the Edit link to the
//   CU02 route (F-COUSR00-005), its user ID the key segment of
//   lib/key-segment.ts, so a user `.` or `..` keeps its own route; D the
//   Delete button (F-COUSR00-006).
// - COUSR03 has no screen of its own (E-58). Delete reads the user, as
//   COUSR03C's ENTER does on entry from the list (F-COUSR03-001), and the
//   confirm dialog shows what the read returned. F5=Delete is the dialog's
//   Confirm (F-COUSR03-004); F3, F4 and F12 are its Cancel (F-COUSR03-002,
//   F-COUSR03-003, F-COUSR03-005). The dialog is bound to one row, so this page
//   renders no COUSR03 key field, Fetch or Clear control: the function map
//   places none, and a user ID reaches the delete only through its row.
//
// Every message comes from the API: page-boundary texts in the 200 page's
// `message` (V-COUSR00C-001 to V-COUSR00C-009), errors as ApiError bodies,
// and the delete's success text (P-COUSR03C-005).
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type JSX,
} from "react";
import { Link } from "react-router";
import { Controller, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";

import {
  useUser,
  useUserDeleteMutation,
  useUsers,
  type User,
  type UserListParams,
} from "@/api/queries/admin-users";
import type { components } from "@/api/schema";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataTable, type DataTableProps } from "@/components/data-table";
import { BackLink, PageHeader } from "@/components/page-header";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
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
import {
  applyApiError,
  showApiError,
  showMessage,
  showSuccess,
} from "@/lib/api-errors";
import { keyPathSegment } from "@/lib/key-segment";
import { clearSession, getSession } from "@/lib/session";
import {
  userListBmsFieldMap,
  userListFilterSchema,
  type UserListFilterValues,
} from "@/schemas/admin-users";

// USRIDINI PIC X(8) of COUSR0A [app/cpy-bms/COUSR00.CPY:L66].
const USER_ID_WIDTH = 8;

// The list this page is, the `from` the CU02 edit page returns to: COUSR02C
// goes back to its calling program on PF3 (CDEMO-FROM-PROGRAM).
const LIST_ROUTE = "/admin/users";

// The admin menu, COADM01C, where PF3 leads [app/cbl/COUSR00C.cbl:L125-127].
const ADMIN_ROUTE = "/admin";

// The 10 rows of a COUSR0A page, the server's default page size.
const PAGE_ROWS = 10;

// The table keys each row by its user ID, the USRSEC record key, so a row's
// Edit and Delete stay with their user when a delete moves the next user into
// its place.
function userRowId(user: User): string {
  return user.user_id;
}

// One request of the browse: the key a page starts from and the direction.
// No page size is sent, so every page holds the 10 rows of the COUSR0A screen.
type Cursor = Pick<UserListParams, "start_key" | "direction">;

// The USRIDIN entry an ENTER browses from, forward. A blank entry (the
// schema's undefined) sends no start key, which starts at the top of the
// file, as LOW-VALUES does [app/cbl/COUSR00C.cbl:L218-222].
function searchCursor(startKey: string | undefined): Cursor {
  return startKey === undefined ? {} : { start_key: startKey };
}

function sameCursor(a: Cursor, b: Cursor): boolean {
  return a.start_key === b.start_key && a.direction === b.direction;
}

// One page of the browse as `GET /admin/users` answers it.
type UserPage = components["schemas"]["Page_UserSummary"];

// An answer with no rows, only its page-boundary text: for PF7 on the first
// page 'You are already at the top of the page...' (P-COUSR00C-004).
function isBoundaryAnswer(page: UserPage): boolean {
  return (
    page.items.length === 0 &&
    typeof page.message === "string" &&
    page.message.trim().length > 0
  );
}

// What the table showed when the user last asked for a browse: the page, the
// query's answer it was shown for, the cursor that read the page, and whether
// the browse is a PF7 or PF8 press rather than an ENTER.
type OnScreen = {
  answer: UserPage | undefined;
  page: UserPage | undefined;
  cursor: Cursor;
  press: boolean;
};

// The page the table shows for the query's answer `page`.
//
// COUSR00C answers PF7 on the first page and PF8 without a next page with
// SEND-ERASE-NO: the rows on screen stay, and only the header and the message
// line are written (P-COUSR00C-004, P-COUSR00C-005, P-COUSR00C-013)
// [app/cbl/COUSR00C.cbl:L250-254] [app/cbl/COUSR00C.cbl:L272-276]. The API
// answers such a key with a page that has no rows and carries the text (E-54),
// so such an answer to a press keeps the page that was on screen, with its
// keys. ENTER sends its screen with ERASE, so the empty page of a search that
// finds no user stands in the table. While a new browse loads, a page kept
// stays in place of the empty answer the query holds over it.
function pageShown(
  page: UserPage | undefined,
  onScreen: OnScreen,
  loadingNext: boolean,
): UserPage | undefined {
  if (page === undefined || onScreen.page === undefined) {
    return page;
  }
  if (loadingNext) {
    return page === onScreen.answer ? onScreen.page : page;
  }
  return onScreen.press && isBoundaryAnswer(page) ? onScreen.page : page;
}

/** The fields COUSR03 shows before F5=Delete [app/bms/COUSR03.bms:L80-139]. */
function UserToDelete({ user }: { user: User }): JSX.Element {
  return (
    <div className="flex flex-col gap-4">
      <p>Confirm deletes this user. Cancel keeps it.</p>
      <dl className="grid grid-cols-3 gap-x-4 gap-y-2">
        <dt>User ID</dt>
        <dd className="col-span-2 font-medium break-words text-foreground">
          {user.user_id}
        </dd>
        <dt>First Name</dt>
        <dd className="col-span-2 font-medium break-words text-foreground">
          {user.first_name}
        </dd>
        <dt>Last Name</dt>
        <dd className="col-span-2 font-medium break-words text-foreground">
          {user.last_name}
        </dd>
        <dt>User Type</dt>
        <dd className="col-span-2 flex flex-wrap items-center gap-2">
          <span className="font-medium text-foreground">{user.usr_type}</span>
          <span>(A=Admin, U=User)</span>
        </dd>
      </dl>
    </div>
  );
}

export default function UserListPage(): JSX.Element {
  const baseId = useId();
  const startKeyId = `${baseId}-start-key`;
  const startKeyHintId = `${baseId}-start-key-hint`;
  const startKeyErrorId = `${baseId}-start-key-error`;

  // ---------------------------------------------------------------------
  // CU00: the browse
  // ---------------------------------------------------------------------

  const filterForm = useForm<UserListFilterValues>({
    resolver: zodResolver(userListFilterSchema),
    defaultValues: { start_key: "" },
  });

  // On first entry COUSR00C browses from the top of the file
  // [app/cbl/COUSR00C.cbl:L115-119].
  const [cursor, setCursor] = useState<Cursor>({});
  const usersQuery = useUsers(cursor);
  // A failed read of the same request keeps the query's last page; it is not
  // shown, because it does not answer the read that failed. Its rows, row
  // actions and paging keys leave with it, and the failure's text stands in
  // the table in their place.
  const page = usersQuery.isError ? undefined : usersQuery.data;

  // The page the table shows (pageShown): the answer, or the page kept on
  // screen in its place after a PF7 or PF8 that found no page. `kept` tells
  // the two apart, and `shownCursor` is the cursor that read the page shown.
  const [onScreen, setOnScreen] = useState<OnScreen>({
    answer: undefined,
    page: undefined,
    cursor: {},
    press: false,
  });
  const shown = pageShown(page, onScreen, usersQuery.isPlaceholderData);
  const kept = shown !== page;
  const shownCursor = kept ? onScreen.cursor : cursor;

  // Counts the browses the user asked for (first entry, ENTER, PF7, PF8), so a
  // page's text is shown for the answer to a browse and not again when the
  // list is re-read after a delete.
  const browseCount = useRef(1);

  // Starts a browse the user asked for, noting what the table shows. ENTER
  // re-reads the file whatever the screen shows, and so does every PF7 and
  // PF8, so a browse from the cursor already read is a new read, not the
  // cached page: Previous pressed again on the first page asks the server
  // again and shows its text again.
  const browse = (next: Cursor, press: boolean): void => {
    browseCount.current += 1;
    setOnScreen({ answer: page, page: shown, cursor: shownCursor, press });
    if (sameCursor(next, cursor)) {
      void usersQuery.refetch();
    } else {
      setCursor(next);
    }
  };

  const onSearch = (values: UserListFilterValues): void => {
    browse(searchCursor(values.start_key), false);
  };

  // PF7 browses back from the first key shown, PF8 forward from the key the
  // page found after its last row. The table calls these only while the key
  // is present.
  const onPrevious = (): void => {
    if (shown?.prev_key) {
      browse({ start_key: shown.prev_key, direction: "backward" }, true);
    }
  };
  const onNext = (): void => {
    if (shown?.next_key) {
      browse({ start_key: shown.next_key, direction: "forward" }, true);
    }
  };

  // A failed browse is shown once per error, when its read settles: toasted,
  // set on USRIDIN, where every COUSR00C path puts the cursor, and kept in
  // place of the rows, which the answer did not supply. The query keeps a
  // failed read's error beside the page read before it, so a return to the
  // page, or a repeated Search, finds that error while its own read of the
  // browse is still out; that read's answer, the page or a new error, is what
  // the page reports, so no error is shown while a read is out, fetching or
  // paused offline. The ref keeps StrictMode's repeated effect and re-renders
  // from showing the same error twice. A layout effect, so the table never
  // paints its no-records text for a failed read.
  const listError = usersQuery.error;
  const reading = usersQuery.fetchStatus !== "idle";
  const shownListError = useRef<unknown>(null);
  const [listErrorText, setListErrorText] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (
      listError === null ||
      reading ||
      listError === shownListError.current
    ) {
      return;
    }
    shownListError.current = listError;
    setListErrorText(
      applyApiError(listError, filterForm, userListBmsFieldMap).message,
    );
  }, [listError, reading, filterForm]);

  // The page-boundary text of a page with rows (V-COUSR00C-005,
  // V-COUSR00C-008) is toasted once per browse, when the answer to the read
  // that browse started arrives. While a read is out, fetching or paused
  // offline, the page shown is not that answer: it is the page kept on screen
  // while the next one loads, or the cache's copy of the requested page, which
  // a return to the list or a Previous or Next onto a page read before shows
  // while the page is read again, and whose text an earlier read found. A
  // re-read the user did not ask for, such as the one after a delete, shows
  // nothing, and a failed read leaves no page to show. An empty page shows its
  // text in place of the rows, and its status region announces it, so it is
  // not toasted as well; the empty answer to a PF7 or PF8 leaves the page
  // shown instead (pageShown), so its text is toasted, once for each press,
  // the message line COUSR00C rewrites (P-COUSR00C-013). dataUpdatedAt marks
  // every answer, including a repeated search or key whose page equals the
  // one shown.
  const answeredBrowse = useRef(0);
  const { dataUpdatedAt, isPlaceholderData } = usersQuery;
  useEffect(() => {
    if (
      page === undefined ||
      isPlaceholderData ||
      reading ||
      answeredBrowse.current === browseCount.current
    ) {
      return;
    }
    answeredBrowse.current = browseCount.current;
    if (page.items.length > 0 || kept) {
      showMessage(page.message);
    }
  }, [page, dataUpdatedAt, isPlaceholderData, reading, kept]);

  // The table waits for the read that is out: the first one, a page loading
  // behind the one kept on screen, or a re-read of a browse whose last read
  // failed. That failure stays in the query until the re-read answers, but it
  // does not answer the re-read, so its text does not stand in the table, and
  // Previous and Next stay disabled, until the re-read's own answer arrives.
  const tableLoading =
    usersQuery.isPending ||
    isPlaceholderData ||
    (usersQuery.isError && reading);

  // ---------------------------------------------------------------------
  // CU03: read the user, confirm, delete
  // ---------------------------------------------------------------------

  // The user ID a Delete is reading, "" when none is: COUSR03C's ENTER on
  // entry from the list, READ-USER-SEC-FILE [app/cbl/COUSR03C.cbl:L95-104].
  const [lookupId, setLookupId] = useState("");
  const lookup = useUser(lookupId);
  // The user the dialog shows, kept while it closes so its content does not
  // empty during the exit animation.
  const [dialogUser, setDialogUser] = useState<User | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const deleteMutation = useUserDeleteMutation();

  // The dialog opens once the read has answered, with the fields it returned;
  // a read that fails (V-COUSR03C-002, V-COUSR03C-003) is toasted and opens
  // nothing. Cached data is not enough: the effect waits
  // for the read the new key starts, so the dialog never shows a user the
  // file no longer holds.
  useEffect(() => {
    if (lookupId === "" || lookup.isFetching) {
      return;
    }
    if (lookup.isSuccess) {
      setDialogUser(lookup.data);
      setDialogOpen(true);
    } else if (lookup.isError) {
      showApiError(lookup.error);
    } else {
      return;
    }
    setLookupId("");
  }, [
    lookupId,
    lookup.isFetching,
    lookup.isSuccess,
    lookup.isError,
    lookup.data,
    lookup.error,
  ]);

  // F5=Delete, DELETE-USER-INFO [app/cbl/COUSR03C.cbl:L174-196]. The delete
  // hook refreshes the list, so the deleted row leaves the page it was on. It
  // settles on the delete's answer without waiting for that refresh, so the
  // outcome is shown before the refresh answers.
  //
  // That re-read answers no PF7 or PF8, so an empty page it finds is shown as
  // it is. A page kept on screen after such a key is not the one the query
  // re-reads, which is the key's empty answer, so once the delete has
  // answered, the kept page is read again from its own cursor, unless the
  // user has browsed meanwhile.
  //
  // An admin who deletes their own account ends their session with it: the
  // server answers every later request of that token 401, because its `sub`
  // names no stored user. The session is therefore ended here, once the text
  // is shown, and without a cause, so RequireAuth sends the admin to sign on
  // with no session-end notice and the delete's text is the only toast. The
  // refresh already out then belongs to an ended session, and api/client.ts
  // ends no session for its 401; the unmounted page shows nothing for it and
  // reads no kept page again.
  const onConfirmDelete = (): void => {
    if (dialogUser === null) {
      return;
    }
    setDialogOpen(false);
    const keptPage = kept
      ? { answer: page, page: shown, cursor: shownCursor }
      : null;
    const browseAtConfirm = browseCount.current;
    if (keptPage === null) {
      setOnScreen((current) => ({ ...current, press: false }));
    }
    deleteMutation.mutate(dialogUser.user_id, {
      onSuccess: (response, deletedId) => {
        showSuccess(response.message);
        if (getSession()?.user_id === deletedId) {
          clearSession();
          return;
        }
        if (keptPage !== null && browseCount.current === browseAtConfirm) {
          setOnScreen({ ...keptPage, press: false });
          setCursor(keptPage.cursor);
        }
      },
      onError: (error) => {
        showApiError(error);
      },
    });
  };

  // ---------------------------------------------------------------------
  // The rows: USRIDnn, FNAMEnn, LNAMEnn and UTYPEnn under the COUSR00
  // headings [app/bms/COUSR00.bms:L103-127], then the row actions.
  // ---------------------------------------------------------------------

  // Every row's Edit and Delete share their names, as the function map fixes
  // them, so each is described by its row's user ID cell.
  //
  // The definitions must keep their identity across renders: flexRender
  // renders each `cell` function as a component, so a new function remounts
  // its cells, and the focused Edit or Delete would lose focus. They
  // therefore depend on nothing but the stable id prefix and the stable state
  // setter.
  //
  // Each placeholder is its cell's markup around a value as wide as the
  // column's on the seeded first page: a user ID and names as wide as its
  // widest, the type letter and the row actions.
  const columns = useMemo<DataTableProps<User>["columns"]>(
    () => [
      {
        accessorKey: "user_id",
        header: "User ID",
        cell: ({ row }) => (
          <span id={`${baseId}-user-${row.index}`}>
            {row.original.user_id}
          </span>
        ),
        meta: { placeholder: <span>ADMIN000</span> },
      },
      {
        accessorKey: "first_name",
        header: "First Name",
        meta: { placeholder: "FIRSTNAME" },
      },
      {
        accessorKey: "last_name",
        header: "Last Name",
        meta: { placeholder: "MACDONALD" },
      },
      {
        accessorKey: "usr_type",
        header: "Type",
        cell: ({ row }) => (
          <Badge variant="secondary">{row.original.usr_type}</Badge>
        ),
        meta: { placeholder: <Badge variant="secondary">U</Badge> },
      },
      {
        id: "actions",
        header: "Actions",
        cell: ({ row }) => {
          const user = row.original;
          const describedBy = `${baseId}-user-${row.index}`;
          return (
            <div className="flex items-center gap-2">
              <Button variant="link" asChild>
                <Link
                  to={`${LIST_ROUTE}/${keyPathSegment(user.user_id)}/edit`}
                  state={{ from: LIST_ROUTE }}
                  aria-describedby={describedBy}
                >
                  Edit
                </Link>
              </Button>
              <Button
                type="button"
                variant="link"
                aria-describedby={describedBy}
                onClick={() => {
                  setLookupId(user.user_id);
                }}
              >
                Delete
              </Button>
            </div>
          );
        },
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
    ],
    [baseId],
  );

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        actions={<BackLink to={ADMIN_ROUTE} />}
      />

      <Card>
        <CardHeader>
          <CardTitle>
            <h2>Search User ID</h2>
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form
            noValidate
            className="flex flex-col gap-4"
            onSubmit={filterForm.handleSubmit(onSearch)}
          >
            <Controller
              name="start_key"
              control={filterForm.control}
              render={({ field, fieldState }) => (
                <Field data-invalid={fieldState.invalid} className="max-w-sm">
                  <FieldLabel htmlFor={startKeyId}>User ID</FieldLabel>
                  <Input
                    id={startKeyId}
                    name={field.name}
                    ref={field.ref}
                    value={field.value ?? ""}
                    onChange={field.onChange}
                    onBlur={field.onBlur}
                    maxLength={USER_ID_WIDTH}
                    autoComplete="off"
                    spellCheck={false}
                    aria-invalid={fieldState.invalid}
                    aria-describedby={
                      fieldState.invalid
                        ? `${startKeyHintId} ${startKeyErrorId}`
                        : startKeyHintId
                    }
                  />
                  <FieldDescription id={startKeyHintId}>
                    The list starts at this user ID. Leave it blank to start at
                    the first user.
                  </FieldDescription>
                  <FieldError id={startKeyErrorId} errors={[fieldState.error]} />
                </Field>
              )}
            />
            <div>
              <Button type="submit">Search</Button>
            </div>
          </form>
        </CardContent>
      </Card>

      <DataTable<User>
        columns={columns}
        getRowId={userRowId}
        data={shown?.items ?? []}
        prevKey={shown?.prev_key}
        nextKey={shown?.next_key}
        onPrevious={onPrevious}
        onNext={onNext}
        caption="Users, in user ID order"
        emptyMessage={usersQuery.isError ? listErrorText : shown?.message}
        loading={tableLoading}
        pageSize={PAGE_ROWS}
        rowHeight="default"
      />

      {dialogUser === null ? null : (
        <ConfirmDialog
          open={dialogOpen}
          onOpenChange={setDialogOpen}
          title="Delete User"
          description={<UserToDelete user={dialogUser} />}
          confirmLabel="Confirm"
          cancelLabel="Cancel"
          onConfirm={onConfirmDelete}
          pending={deleteMutation.isPending}
        />
      )}
    </div>
  );
}
