import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCousr00cList } from "@/api/hooks";
import type { components, paths } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
// The CSMSG01Y invalid-key text is shared by every screen through `@/lib/header` (AAP 0.5.4), so
// it is imported here rather than restated.
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COUSR00_LENGTHS, cousr00Schema, type Cousr00Values } from "@/schemas/COUSR00";

/**
 * COUSR00, the user list of transaction CU00 (program COUSR00C, map `COUSR0A`, route `/users`),
 * served by endpoint 16 `GET /api/v1/users` behind the admin gate.
 *
 * Expected results: `modern/audit/online/COUSR00C.md` (behaviour) and `modern/audit/screens.md`
 * rows `COUSR00-001` to `COUSR00-099` (fields and keys).
 */

/** The ten list rows of the map, lines 10 to 19 (`SEL0001`–`SEL0010` and their user fields). */
const ROW_COUNT = COUSR00_LENGTHS.rows;

/** `MOVE CDEMO-CU00-PAGE-NUM TO PAGENUMI`: the unsigned `9(08)` page number as eight digits. */
const PAGE_DIGITS = 8;

/** COUSR00C-010: the message `PROCESS-ENTER-KEY` moves for a selector other than U, u, D or d. */
const INVALID_SELECTION = "Invalid selection. Valid values are U and D";

/**
 * `ERRMSG LENGTH=78` (COUSR00-088): `MOVE WS-MESSAGE TO ERRMSGO` stores the 80-byte message into
 * the 78-byte field, so the row shows its first 78 characters (COUSR00C-033). The audit makes this
 * cut the screen's, not the server's; no COUSR00C literal reaches it, but any message does.
 */
const ERRMSG_LENGTH = 78;

/** COUSR00-087: the `INITIAL` of line 21 (BMS `''` is one quote). */
const INSTRUCTION = "Type 'U' to Update or 'D' to Delete a User from the list";

/**
 * The column captions of line 8 and the rules of line 9, each the `INITIAL` of its field
 * (COUSR00-017 to COUSR00-026). `Sel` and `Type` centre their one-character values, which the map
 * places in the middle of the caption (`SEL0001` at column 6 under `Sel` at 5, `UTYPE01` at 73
 * under `Type` at 72).
 */
const COLUMNS = [
  { caption: "Sel", rule: "---", align: "text-center" },
  { caption: "User ID ", rule: "--------", align: "text-left" },
  { caption: "     First Name     ", rule: "--------------------", align: "text-left" },
  { caption: "     Last Name      ", rule: "--------------------", align: "text-left" },
  { caption: "Type", rule: "----", align: "text-center" },
] as const;

/*
 * BLITZY [LAYOUT]: the two eight-character fields (USRIDIN, PAGENUM) use `px-2` instead of the
 * Input default `px-3`, so all eight characters stay visible while the eight grid columns they
 * span are narrow (about 1000 px of viewport).
 */
const EIGHT_CHARACTER_FIELD = "px-2";

/** The row-24 labels of the map (COUSR00-089, COUSR00-096 to COUSR00-099). */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COUSR00);

/** Whether a field holds only spaces or LOW-VALUES (`= SPACES OR LOW-VALUES`), empty included. */
function isBlank(text: string): boolean {
  return /^[ \u0000]*$/.test(text);
}

/**
 * The form field a `ScreenError.field` names, by its wire name. Endpoint 16 names only members
 * of its query, and `start_user_id` (USRIDIN) is the only one typed on the map: the paging state
 * it may name is the screen's own echo, and the selectors are never sent.
 */
function formFieldOf(field: string | null | undefined): keyof Cousr00Values | undefined {
  return field === "start_user_id" ? "startUserId" : undefined;
}

/**
 * The paging state a route-state object carries back to this screen: `CDEMO-CU00-USRID-FIRST`
 * and `CDEMO-CU00-USRID-LAST`, which COUSR02C and COUSR03C pass back untouched in the COMMAREA
 * (COUSR00C-003). Any other state, such as COADM01's or none, gives blank keys, as the 160-byte
 * COMMAREA from COADM01C pads the extension with spaces.
 */
function entryKeys(
  state: unknown,
): NonNullable<paths["/api/v1/users"]["get"]["parameters"]["query"]> {
  if (typeof state !== "object" || state === null) {
    return {};
  }
  return {
    first_key:
      "first_key" in state && typeof state.first_key === "string" ? state.first_key : undefined,
    last_key:
      "last_key" in state && typeof state.last_key === "string" ? state.last_key : undefined,
  };
}

/**
 * The paging state a response leaves (COUSR00C-006): echoed unchanged by the next request. A
 * blank key arrives as `null` and is echoed by leaving it out.
 */
function echoOf(
  page: components["schemas"]["UserListResponse"],
): NonNullable<paths["/api/v1/users"]["get"]["parameters"]["query"]> {
  return {
    first_key: page.first_key ?? undefined,
    last_key: page.last_key ?? undefined,
    page_no: page.page_no,
    has_next_page: page.has_next_page,
  };
}

/** The ten map rows a page fills: row 1 first, `null` for a blank row. */
function rowsOf(
  page: readonly (components["schemas"]["UserListRow"] | null)[],
): (components["schemas"]["UserListRow"] | null)[] {
  return Array.from({ length: ROW_COUNT }, (_, index) => page[index] ?? null);
}

/**
 * What ENTER does with the selectors (COUSR00C-007 to COUSR00C-011): only the first non-blank
 * selector, in row order, counts. On a row that shows a user, `U` or `u` transfers to the update
 * screen and `D` or `d` to the delete screen with that row's user ID; any other character lists
 * with the invalid-selection message. A selector on a blank row, or none, just lists.
 */
function selectionOutcome(
  selections: readonly string[],
  rows: readonly (components["schemas"]["UserListRow"] | null)[],
):
  | { readonly action: "update" | "delete"; readonly userId: string }
  | { readonly action: "list"; readonly fallback?: string } {
  const index = selections.findIndex((value) => !isBlank(value));
  const userId = index < 0 ? "" : (rows[index]?.user_id ?? "");
  if (isBlank(userId)) {
    return { action: "list" };
  }
  switch (selections[index]) {
    case "U":
    case "u":
      return { action: "update", userId };
    case "D":
    case "d":
      return { action: "delete", userId };
    default:
      return { action: "list", fallback: INVALID_SELECTION };
  }
}

/** What the screen does with a page response besides showing it. */
type AfterPage = {
  /** ENTER and F8 clear the search field once a page is returned; F7 keeps it (COUSR00C-031). */
  readonly clearsSearch: boolean;
  /** The message shown when the response carries none (COUSR00C-010). */
  readonly fallback?: string;
};

/**
 * The CU00 user list screen. It lists on entry, lists from `Search User ID:` on ENTER, pages
 * with F7 and F8, transfers on a `U` or `D` selection and returns to the admin menu on F3. The
 * paging state travels with every request and grants nothing; the API's admin gate decides
 * whether the list is served (COUSR00C-001, AAP 0.5.1).
 */
export function COUSR00(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  const formId = useId();
  const pageId = useId();
  const list = useCousr00cList();

  const form = useForm<Cousr00Values>({
    resolver: zodResolver(cousr00Schema),
    // Selectors start blank only on a new entry; afterwards they keep what was typed in them,
    // row by row, across every redisplay (COUSR00C-030).
    defaultValues: { startUserId: "", selections: Array.from({ length: ROW_COUNT }, () => "") },
    // After an ENTER, an edit of a field re-runs its schema check at once, which also drops a
    // server rejection the field carried: the edited value is the server's to judge again.
    reValidateMode: "onChange",
  });

  // `CDEMO-CU00-INFO` between interactions: keys, page number and next-page indicator.
  const [echo, setEcho] = useState<
    NonNullable<paths["/api/v1/users"]["get"]["parameters"]["query"]>
  >(() => entryKeys(location.state));
  const [rows, setRows] = useState<(components["schemas"]["UserListRow"] | null)[]>(() =>
    rowsOf([]),
  );
  // PAGENUM's INITIAL is a space: nothing shows until the first page (COUSR00C-029).
  const [pagenum, setPagenum] = useState("");
  const [message, setMessage] = useState("");

  /** Puts the cursor on the search field, as every send does (`USRIDINL` = -1, COUSR00C-032). */
  function focusSearch(): void {
    form.setFocus("startUserId");
  }

  /**
   * Requests a page and shows the response. A response without rows read nothing: the rows, the
   * page number and the search field stay as shown (COUSR00C-020, COUSR00C-021). The last message
   * of the interaction wins; a failure shows the `ScreenError` message, the 403 of the admin gate
   * included, marks the search field invalid when the error names it, and leaves the screen
   * otherwise unchanged.
   */
  function requestPage(
    query: NonNullable<paths["/api/v1/users"]["get"]["parameters"]["query"]>,
    after: AfterPage,
  ): void {
    // A request starts a new interaction: the field errors of the last one go with its message,
    // and only this answer marks a field again.
    form.clearErrors();
    list.mutate(
      { query },
      {
        onSuccess: (page) => {
          setEcho(echoOf(page));
          if (page.rows !== null && page.rows !== undefined) {
            setRows(rowsOf(page.rows));
            setPagenum(String(page.page_no).padStart(PAGE_DIGITS, "0"));
            if (after.clearsSearch) {
              form.setValue("startUserId", "");
            }
          }
          const text = page.message ?? "";
          setMessage(isBlank(text) ? (after.fallback ?? "") : text);
          focusSearch();
        },
        onError: (error) => {
          setMessage(error.message);
          // The field keeps the text the error row shows, so its `FormMessage` repeats the row.
          const name = formFieldOf(error.field);
          if (name !== undefined) {
            form.setError(name, { type: "server", message: error.message.slice(0, ERRMSG_LENGTH) });
          }
          focusSearch();
        },
      },
    );
  }

  // The first entry is ENTER with no selection and a blank search field (COUSR00C-003). The ref
  // keeps the development double effect of StrictMode from sending it twice.
  const entered = useRef(false);
  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    // `echo` still holds the keys the entry carried, the only state this request sends.
    requestPage(echo, { clearsSearch: true });
  }, []);

  /** ENTER (COUSR00-096): a selection transfers; otherwise list from the search field. */
  function onEnter(values: Cousr00Values): void {
    // The terminal keyboard stays locked until the program's answer arrives.
    if (list.isPending) {
      return;
    }
    const outcome = selectionOutcome(values.selections, rows);
    if (outcome.action !== "list") {
      // XCTL COUSR02C or COUSR03C with the selected user ID (COUSR00C-008, COUSR00C-009). The
      // keys ride along as the COMMAREA does, for a partner screen that returns its state.
      navigate(outcome.action === "update" ? PATHS.COUSR02 : PATHS.COUSR03, {
        state: {
          from: PATHS.COUSR00,
          user_id: outcome.userId,
          first_key: echo.first_key,
          last_key: echo.last_key,
        },
      });
      return;
    }
    requestPage(
      {
        ...echo,
        start_user_id: values.startUserId === "" ? undefined : values.startUserId,
      },
      { clearsSearch: true, fallback: outcome.fallback },
    );
  }

  /** A map length the schema refuses: its message in the error row, no request. */
  function onInvalid(): void {
    const result = cousr00Schema.safeParse(form.getValues());
    setMessage(result.success ? "" : (result.error.issues[0]?.message ?? ""));
    focusSearch();
  }

  /** F7 (COUSR00-098): the page before, read back from the first user ID shown. */
  function pageBackward(): void {
    if (list.isPending) {
      return;
    }
    requestPage(
      { ...echo, direction: "backward", page_key: echo.first_key },
      { clearsSearch: false },
    );
  }

  /** F8 (COUSR00-099): the page after, read on from the last user ID shown. */
  function pageForward(): void {
    if (list.isPending) {
      return;
    }
    requestPage({ ...echo, direction: "forward", page_key: echo.last_key }, { clearsSearch: true });
  }

  /** An F-key COUSR00C does not serve (COUSR00C-005): its message, no request. */
  function showInvalidKey(): void {
    setMessage(INVALID_KEY);
    focusSearch();
  }

  const actions: PfKeyHandlers = {
    // F3 (COUSR00-097): back to the admin menu, XCTL COADM01C (COUSR00C-004).
    F3: () => {
      navigate(PATHS.COADM01);
    },
    F7: pageBackward,
    F8: pageForward,
  };
  // ENTER has no action of its own: its button and the Enter key submit the form.
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({ ...item, onPress: actions[item.key] }));

  return (
    <ScreenShell
      transaction={SCREENS.COUSR00.transaction}
      program={SCREENS.COUSR00.program}
      title="List Users"
      formId={formId}
      message={{ text: message.slice(0, ERRMSG_LENGTH), kind: "error" }}
      keys={keys}
      keyOptions={{ onInvalidKey: showInvalidKey }}
    >
      <Form {...form}>
        <form id={formId} noValidate onSubmit={form.handleSubmit(onEnter, onInvalid)}>
          {/*
           * BLITZY [LAYOUT]: the map's line tracks are sized to content (`grid-rows-none`, which
           * overrides FieldGrid's 24 equal tracks). The list block is one `Table` on line 8 that
           * stands for lines 8 to 19; with equal tracks every line would take the table's height.
           */}
          <FieldGrid className="grid-rows-none">
            {/* COUSR00-012, COUSR00-013: `Page:` (TURQUOISE, BRT) and PAGENUM (BLUE output). */}
            <GridCell row={4} col={65} span={5}>
              <Label
                htmlFor={pageId}
                className="h-full whitespace-pre font-semibold text-foreground"
              >
                Page:
              </Label>
            </GridCell>
            <GridCell row={4} col={71} span={PAGE_DIGITS}>
              {/* ASKIP: the cursor skips the field, so it is not in the tab order. */}
              <Input
                id={pageId}
                readOnly
                tabIndex={-1}
                value={pagenum}
                className={`${EIGHT_CHARACTER_FIELD} text-muted-foreground`}
              />
            </GridCell>

            {/* COUSR00-014, COUSR00-015: `Search User ID:` (TURQUOISE) and USRIDIN. */}
            <FormField
              control={form.control}
              name="startUserId"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={6} col={5} span={15}>
                    <FormLabel className="h-full whitespace-pre text-foreground">
                      Search User ID:
                    </FormLabel>
                  </GridCell>
                  <GridCell row={6} col={21} span={COUSR00_LENGTHS.startUserId}>
                    <FormControl>
                      <Input
                        {...field}
                        maxLength={COUSR00_LENGTHS.startUserId}
                        autoComplete="off"
                        autoFocus
                        className={EIGHT_CHARACTER_FIELD}
                      />
                    </FormControl>
                  </GridCell>
                  {/*
                   * The error row is the map's only visible message, so a field's own message is
                   * for assistive technology only, read with the input it rejects.
                   */}
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            {/* COUSR00-017 to COUSR00-086: the captions, rules and ten rows, lines 8 to 19. */}
            <GridCell row={8} col={5} span={71}>
              <Table aria-label="Users">
                <TableHeader>
                  <TableRow>
                    {COLUMNS.map((column) => (
                      <TableHead
                        key={column.caption}
                        scope="col"
                        className={`whitespace-pre ${column.align}`}
                      >
                        {column.caption}
                      </TableHead>
                    ))}
                  </TableRow>
                  {/* The dashed rule of line 9 is decoration, so screen readers skip it. */}
                  <TableRow aria-hidden="true">
                    {COLUMNS.map((column) => (
                      <TableHead key={column.caption} className={`whitespace-pre ${column.align}`}>
                        {column.rule}
                      </TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row, index) => (
                    // Rows are positions on the map, not records: line 10 + index.
                    <TableRow key={index}>
                      <TableCell className="text-center">
                        <FormField
                          control={form.control}
                          name={`selections.${index}`}
                          render={({ field }) => (
                            <FormItem className="justify-items-center">
                              <FormLabel className="sr-only">
                                {`Selection for row ${index + 1}`}
                              </FormLabel>
                              <FormControl>
                                <Input
                                  {...field}
                                  maxLength={COUSR00_LENGTHS.selection}
                                  autoComplete="off"
                                  className="w-10 text-center"
                                />
                              </FormControl>
                              <FormMessage className="sr-only" />
                            </FormItem>
                          )}
                        />
                      </TableCell>
                      {/* USRIDnn, FNAMEnn, LNAMEnn, UTYPEnn: BLUE outputs, never a password. */}
                      <TableCell className="whitespace-pre text-muted-foreground">
                        {row?.user_id ?? ""}
                      </TableCell>
                      <TableCell className="whitespace-pre text-muted-foreground">
                        {row?.first_name ?? ""}
                      </TableCell>
                      <TableCell className="whitespace-pre text-muted-foreground">
                        {row?.last_name ?? ""}
                      </TableCell>
                      <TableCell className="whitespace-pre text-center text-muted-foreground">
                        {row?.user_type ?? ""}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </GridCell>

            {/* COUSR00-087: line 21 (NEUTRAL, BRT). */}
            <GridCell row={21} col={12} span={INSTRUCTION.length}>
              <span className="whitespace-pre font-semibold text-foreground">{INSTRUCTION}</span>
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
