import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCousr02cFetch, useCousr02cUpdate } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FIELD_INPUT, FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
// `@/lib/header` holds the CSMSG01Y texts every screen shares, so the invalid-key message is taken
// from it rather than restated here (COUSR02C-009).
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import {
  COUSR02_LENGTHS,
  cousr02KeySchema,
  cousr02Schema,
  type Cousr02Values,
} from "@/schemas/COUSR02";

/*
 * Update User, map `COUSR2A` of `app/bms/COUSR02.bms` (transaction CU02, program COUSR02C, route
 * `/users/update`), served by endpoint 18 (`GET /api/v1/users/{user_id}`, ENTER) and endpoint 19
 * (`PUT /api/v1/users/{user_id}`, F5 and the save half of F3). Rules: `modern/audit/screens.md`
 * `COUSR02-001` to `COUSR02-040` and the `client` rows of `modern/audit/online/COUSR02C.md`.
 */

/** The map's field order, which is also the order in which the cursor looks for an error. */
const FIELD_ORDER = ["userId", "firstName", "lastName", "password", "userType"] as const;

/** One input of the form, by its form name. */
type FieldName = (typeof FIELD_ORDER)[number];

/** Every input empty: the first display with no selection (COUSR02C-005) and F4 (COUSR02C-007). */
const EMPTY: Cousr02Values = {
  userId: "",
  firstName: "",
  lastName: "",
  password: "",
  userType: "",
};

/*
 * Colour tokens of AAP 0.4.3. The GREEN and TURQUOISE labels and the GREEN inputs are the
 * foreground; the BLUE hints keep `FormDescription`'s muted foreground; the YELLOW rule has no
 * token and is emphasis on the foreground. Static texts never wrap (`whitespace-pre`) and are
 * centred on their line, which every input makes `h-9` tall, so they stand level with the inputs.
 * The inputs of two or more characters take the shared map-field box (`FIELD_INPUT`, with `size`
 * = their `LENGTH`), so a full-length value shows on the narrowest frame.
 */
const LABEL = "h-full whitespace-pre text-foreground";
const VALUE = `${FIELD_INPUT} text-foreground`;
const HINT = "flex h-full items-center whitespace-pre";
const RULE = "flex h-full items-center whitespace-pre font-semibold text-foreground";

/** COUSR02-015: the `INITIAL` of the row-8 rule, 70 asterisks. */
const RULE_TEXT = "*".repeat(70);

/*
 * BLITZY [LAYOUT]: `USRTYPE` spans one grid column (`LENGTH=1`), a few pixels wide, and its hint
 * starts two columns on, at column 19. The shared short-field box (`fieldInputClass`), and a
 * `FIELD_INPUT` box holding its character, would both reach that hint on the frame's minimum
 * width, so this input keeps the narrowest padding step instead of the shared inset and takes the
 * width of one character: it reaches into the empty attribute column after it (column 18) but not
 * into the hint. Its type size is the inputs' `text-sm` (AAP 0.4.3); its border and focus ring
 * stay the component's.
 */
const ONE_CHAR_VALUE = "min-w-4 px-0.5 text-center text-sm text-foreground";

/** `ScreenError.field` wire name → the input it names; the input of `MOVE -1 TO <field>L`. */
const FIELD_BY_WIRE: Readonly<Record<string, FieldName>> = {
  user_id: "userId",
  first_name: "firstName",
  last_name: "lastName",
  password: "password",
  user_type: "userType",
};

/**
 * The two endpoint 19 messages that keep the neutral colour the update's successful read set
 * (`DFHNEUTR`, `COUSR02C.cbl:338`), because the rewrite's `NOTFND` and `OTHER` branches set none
 * (`COUSR02C.cbl:379,386`; COUSR02C-033, X-COUSR02C-2). Used only to choose the message variant:
 * the text shown is always the server's. The `ScreenError` body carries no status, so the text is
 * the only thing that tells them apart; the read's own `User ID NOT found...` on the update path
 * shares it, and that case carries no parity claim (COUSR02C-025, halt candidate X-COUSR02C-H1).
 */
const NEUTRAL_UPDATE_MESSAGES: ReadonlySet<string> = new Set([
  "User ID NOT found...",
  "Unable to Update User...",
]);

/** The row-24 labels of the map, in field order. */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COUSR02);

/** Every route a screen can transfer here from, for the F3 return (`CDEMO-FROM-PROGRAM`). */
const SCREEN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/**
 * The route state of the transfer here: the screen it came from and, from the user list's `U`
 * selection, the selected user ID (`CDEMO-FROM-PROGRAM`, `CDEMO-CU02-USR-SELECTED`).
 */
type EntryState = { readonly from?: string; readonly userId?: string };

/** Reads the route state, which the router types as `unknown`; anything else reads as absent. */
function entryOf(state: unknown): EntryState {
  if (typeof state !== "object" || state === null) {
    return {};
  }
  return {
    from: "from" in state && typeof state.from === "string" ? state.from : undefined,
    userId: "user_id" in state && typeof state.user_id === "string" ? state.user_id : undefined,
  };
}

/**
 * Endpoint 19's variables from the form: the typed user ID as the path, the other inputs as the
 * body. An empty password is omitted, which keeps the stored one, because the dark field is never
 * pre-filled (E27); anything typed is sent, so all spaces reaches the server's blank edit.
 */
function updateVars(values: Cousr02Values) {
  return {
    path: { user_id: values.userId },
    body: {
      first_name: values.firstName,
      last_name: values.lastName,
      user_type: values.userType,
      ...(values.password ? { password: values.password } : {}),
    },
  };
}

/** The first message of a failed form validation, in map field order. */
function firstErrorMessage(errors: FieldErrors<Cousr02Values>): string | undefined {
  for (const name of FIELD_ORDER) {
    const message = errors[name]?.message;
    if (message !== undefined) {
      return message;
    }
  }
  return undefined;
}

/**
 * The request the screen waits for, from the key that starts it to its settlement: the action
 * that sent it and the user ID input it was sent for. The object itself is the token a response
 * is checked against, so a response touches the screen only while its own request is awaited.
 */
type Operation = { readonly kind: "fetch" | "save" | "saveAndExit"; readonly userId: string };

/**
 * CU02, the user update screen.
 *
 * - ENTER fetches the user by the typed ID (COUSR02C-010 to COUSR02C-017). The four other inputs
 *   are emptied when the request is sent, as the program clears them before its read, and filled
 *   from a 200 response; the password is never filled (E27). Entered with a selected user, the
 *   screen fetches it at once (COUSR02C-004).
 * - F5 saves through endpoint 19 and shows its message; every input keeps what was typed
 *   (COUSR02C-033). F3 saves the same way, then leaves whatever the outcome (COUSR02C-006).
 * - F4 empties the screen (COUSR02C-007); F12 returns to the admin menu (COUSR02C-008).
 *
 * One request at a time, as the 3270 keyboard stays locked while the program runs: from ENTER,
 * F5 or F3 until its response, ENTER, F3, F4 and F5 are disabled, every other key but F12 is
 * refused and the inputs are read-only. F12 sends nothing, so it still leaves; once F3 has
 * started, it is refused too, because the screen is leaving.
 *
 * The target user ID and type are data for the API; the caller acts only as the verified token,
 * which the API's admin gate checks (AAP 0.5.1). No password is logged, stored or shown.
 */
export function COUSR02(): ReactNode {
  const formId = useId();
  const navigate = useNavigate();
  const location = useLocation();
  const [entry] = useState(() => entryOf(location.state));
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);
  const form = useForm<Cousr02Values>({
    resolver: zodResolver(cousr02Schema),
    defaultValues: { ...EMPTY, userId: entry.userId ?? "" },
  });
  const fetchUser = useCousr02cFetch();
  const updateUser = useCousr02cUpdate();
  // Set once F3 has started: the screen is leaving, as the XCTL follows the save, so no later key
  // acts on it. The 3270 keyboard stays locked while the program runs.
  const leaving = useRef(false);
  // The request in flight: set before its edit runs, so a key pressed before the next render is
  // refused too, and cleared when its response has been applied. Endpoints 18 and 19 are separate
  // hooks, so this one token is what keeps a read and a save, or two saves, from overlapping.
  const inFlight = useRef<Operation | null>(null);
  // The kind of `inFlight` as rendered: the disabled keys, the read-only inputs and `aria-busy`.
  const [pending, setPending] = useState<Operation["kind"] | null>(null);
  // StrictMode runs mount effects twice in development; the entry fetch must be sent once.
  const entered = useRef(false);

  const exitPath =
    entry.from !== undefined && SCREEN_PATHS.has(entry.from) ? entry.from : PATHS.COADM01;

  /**
   * Shows a `ScreenError` and puts the cursor on the input it names, else on `fallback`. An error
   * shown in the error variant also marks that input invalid, as a failed form edit does; a
   * neutral message only moves the cursor.
   */
  function showError(
    error: components["schemas"]["ScreenError"],
    kind: ShellMessage["kind"],
    fallback: FieldName,
  ): void {
    setMessage({ kind, text: error.message });
    const wire = error.field;
    const named =
      wire != null && Object.hasOwn(FIELD_BY_WIRE, wire) ? FIELD_BY_WIRE[wire] : undefined;
    if (named !== undefined && kind === "error") {
      form.setError(named, { type: "server", message: error.message }, { shouldFocus: true });
    } else {
      form.setFocus(named ?? fallback);
    }
  }

  /** Leaves for `path`, telling the next screen where it came from (`CDEMO-FROM-PROGRAM`). */
  function transfer(path: string): void {
    void navigate(path, { state: { from: PATHS.COUSR02 } });
  }

  /**
   * Starts the request `kind` for the user ID `userId` holds now, before its edit runs. Returns
   * its token, or `null` when the key is refused: a request is in flight or the screen is leaving.
   */
  function begin(kind: Operation["kind"], userId: string): Operation | null {
    if (leaving.current || inFlight.current !== null) {
      return null;
    }
    const operation: Operation = { kind, userId };
    inFlight.current = operation;
    setPending(kind);
    return operation;
  }

  /** Ends `operation` once its edit has failed or its response has been applied. */
  function end(operation: Operation): void {
    if (inFlight.current === operation) {
      inFlight.current = null;
      setPending(null);
    }
  }

  /** Whether a response of `operation` may change the screen: it is awaited, for the shown key. */
  function awaited(operation: Operation): boolean {
    return inFlight.current === operation && form.getValues("userId") === operation.userId;
  }

  /**
   * ENTER, `PROCESS-ENTER-KEY`: the user ID edit, then endpoint 18. Refused while a request is in
   * flight; its response fills the screen only for the user ID it was sent for.
   */
  function fetchByUserId(userId: string): void {
    const operation = begin("fetch", userId);
    if (operation === null) {
      return;
    }
    form.clearErrors();
    const parsed = cousr02KeySchema.safeParse({ userId });
    if (!parsed.success) {
      // COUSR02C-010 and COUSR02C-017: the other inputs keep what the screen showed.
      const text = parsed.error.issues.at(0)?.message;
      setMessage({ kind: "error", text });
      form.setError("userId", { type: "custom", message: text }, { shouldFocus: true });
      end(operation);
      return;
    }
    for (const name of ["firstName", "lastName", "password", "userType"] as const) {
      form.setValue(name, "");
    }
    void fetchUser
      .mutateAsync({ path: { user_id: parsed.data.userId } })
      .then(
        (view) => {
          if (!awaited(operation)) {
            return;
          }
          // COUSR02C-013 and COUSR02C-014: the user ID keeps its value and the cursor.
          form.setValue("firstName", view.first_name);
          form.setValue("lastName", view.last_name);
          form.setValue("userType", view.user_type);
          setMessage({ kind: "info", text: view.update_prompt });
          form.setFocus("userId");
        },
        (error: components["schemas"]["ScreenError"]) => {
          if (awaited(operation)) {
            showError(error, "error", "userId");
          }
        },
      )
      .finally(() => {
        end(operation);
      });
  }

  /** The message and cursor of an endpoint 19 failure (COUSR02C-033). */
  function showUpdateError(error: components["schemas"]["ScreenError"]): void {
    const neutral = NEUTRAL_UPDATE_MESSAGES.has(error.message.trimEnd());
    showError(error, neutral ? "info" : "error", "firstName");
  }

  /**
   * F5, `UPDATE-USER-INFO`: the user ID edit, then endpoint 19. Refused while a request is in
   * flight, so two saves never race; the request stays awaited until its response is shown.
   */
  function save(): void {
    const operation = begin("save", form.getValues("userId"));
    if (operation === null) {
      return;
    }
    void form
      .handleSubmit(
        async (values) => {
          await updateUser.mutateAsync(updateVars(values)).then(
            (result) => {
              if (awaited(operation)) {
                setMessage({ kind: "info", text: result.message });
                form.setFocus("firstName");
              }
            },
            (error: components["schemas"]["ScreenError"]) => {
              if (awaited(operation)) {
                showUpdateError(error);
              }
            },
          );
        },
        (errors) => {
          // The resolver has already put the cursor on the first input in error.
          setMessage({ kind: "error", text: firstErrorMessage(errors) });
        },
      )()
      .finally(() => {
        end(operation);
      });
  }

  /**
   * F3, `UPDATE-USER-INFO` then `RETURN-TO-PREV-SCREEN`: saves exactly as F5 and leaves once the
   * save has settled, whatever its outcome, without showing its message (COUSR02C-006). A blank
   * user ID fails the edit, so nothing is sent before leaving. Refused while a fetch or a save is
   * in flight, so it never sends a second save; its own request is never ended, because the
   * screen it would unlock is gone.
   */
  function saveAndExit(): void {
    if (begin("saveAndExit", form.getValues("userId")) === null) {
      return;
    }
    leaving.current = true;
    const leave = (): void => {
      transfer(exitPath);
    };
    void form.handleSubmit(
      async (values) => {
        await updateUser.mutateAsync(updateVars(values)).then(leave, leave);
      },
      leave,
    )();
  }

  /**
   * F4, `CLEAR-CURRENT-SCREEN`: every input and the message emptied, the cursor on the user ID.
   * Refused while a request is in flight, whose response is the next thing the screen shows.
   */
  function clear(): void {
    if (leaving.current || inFlight.current !== null) {
      return;
    }
    // Only settled requests remain, so this drops their results and cancels nothing.
    fetchUser.reset();
    updateUser.reset();
    // `keepFieldsRef` keeps the inputs registered, so `setFocus` still finds the user ID input.
    form.reset(EMPTY, { keepFieldsRef: true });
    setMessage(undefined);
    form.setFocus("userId");
  }

  /** F12: back to the admin menu, saving nothing (COUSR02C-008). */
  function cancel(): void {
    if (leaving.current) {
      return;
    }
    transfer(PATHS.COADM01);
  }

  /**
   * An F-key the program does not serve, its `WHEN OTHER` (COUSR02C-009). Refused while a request
   * is in flight, as the key row's `busy` refuses it. The input an earlier error marked is no
   * longer the subject of the message row, so its mark goes.
   */
  function invalidKey(): void {
    if (leaving.current || inFlight.current !== null) {
      return;
    }
    form.clearErrors();
    setMessage({ kind: "error", text: INVALID_KEY.trimEnd() });
  }

  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    // COUSR02C-004 and COUSR02C-005: a selected user is fetched as ENTER would; a blank one is not.
    // The entry captured at mount is read once, on the first display only, as COUSR02C's
    // first-entry branch runs once per transfer here.
    const selected = entry.userId;
    if (selected !== undefined && cousr02KeySchema.safeParse({ userId: selected }).success) {
      fetchByUserId(selected);
    }
  }, []);

  const actions: PfKeyHandlers = {
    F3: saveAndExit,
    F4: clear,
    F5: save,
    F12: cancel,
  };
  const busy = pending !== null;
  // The keys `begin`, `clear` and `cancel` refuse: every key while F3's save-and-exit runs, every
  // key but F12 while a fetch or a save does.
  const keys: PfKeyBarEntry[] = KEY_ITEMS.map((item) => ({
    ...item,
    onPress: actions[item.key],
    disabled: item.key === "F12" ? pending === "saveAndExit" : busy,
  }));

  return (
    <ScreenShell
      transaction={SCREENS.COUSR02.transaction}
      program={SCREENS.COUSR02.program}
      title="Update User"
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={{ foldShifted: KEY_ROWS.COUSR02.foldShifted, onInvalidKey: invalidKey }}
      busy={busy}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={busy}
          onSubmit={(event) => {
            event.preventDefault();
            if (!leaving.current) {
              fetchByUserId(form.getValues("userId"));
            }
          }}
        >
          <FieldGrid>
            {/* COUSR02-012, COUSR02-013: GREEN label and input, initial cursor (`IC`). */}
            <FormField
              control={form.control}
              name="userId"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={6} col={6} span={14}>
                    <FormLabel className={LABEL}>Enter User ID:</FormLabel>
                  </GridCell>
                  <GridCell row={6} col={21} span={COUSR02_LENGTHS.userId}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={COUSR02_LENGTHS.userId}
                        size={COUSR02_LENGTHS.userId}
                        autoComplete="off"
                        spellCheck={false}
                        autoFocus
                        className={VALUE}
                      />
                    </FormControl>
                    {/*
                     * The message row is the only visible message, so each input's own error text
                     * is for assistive technology only, read with the input it marks.
                     */}
                    <FormMessage className="sr-only" />
                  </GridCell>
                </FormItem>
              )}
            />
            {/* COUSR02-015: the YELLOW rule, decorative. */}
            <GridCell row={8} col={6} span={70}>
              <span aria-hidden="true" className={RULE}>
                {RULE_TEXT}
              </span>
            </GridCell>
            {/* COUSR02-016, COUSR02-017. */}
            <FormField
              control={form.control}
              name="firstName"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={11} col={6} span={11}>
                    <FormLabel className={LABEL}>First Name:</FormLabel>
                  </GridCell>
                  <GridCell row={11} col={18} span={COUSR02_LENGTHS.firstName}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={COUSR02_LENGTHS.firstName}
                        size={COUSR02_LENGTHS.firstName}
                        autoComplete="off"
                        spellCheck={false}
                        className={VALUE}
                      />
                    </FormControl>
                    <FormMessage className="sr-only" />
                  </GridCell>
                </FormItem>
              )}
            />
            {/* COUSR02-019, COUSR02-020. */}
            <FormField
              control={form.control}
              name="lastName"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={11} col={45} span={10}>
                    <FormLabel className={LABEL}>Last Name:</FormLabel>
                  </GridCell>
                  <GridCell row={11} col={56} span={COUSR02_LENGTHS.lastName}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={COUSR02_LENGTHS.lastName}
                        size={COUSR02_LENGTHS.lastName}
                        autoComplete="off"
                        spellCheck={false}
                        className={VALUE}
                      />
                    </FormControl>
                    <FormMessage className="sr-only" />
                  </GridCell>
                </FormItem>
              )}
            />
            {/* COUSR02-022 to COUSR02-024: the dark field, never pre-filled (E27). */}
            <FormField
              control={form.control}
              name="password"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={13} col={6} span={9}>
                    <FormLabel className={LABEL}>Password:</FormLabel>
                  </GridCell>
                  <GridCell row={13} col={16} span={COUSR02_LENGTHS.password}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        type="password"
                        maxLength={COUSR02_LENGTHS.password}
                        size={COUSR02_LENGTHS.password}
                        autoComplete="off"
                        className={VALUE}
                      />
                    </FormControl>
                    <FormMessage className="sr-only" />
                  </GridCell>
                  <GridCell row={13} col={25} span={8}>
                    <FormDescription className={HINT}>(8 Char)</FormDescription>
                  </GridCell>
                </FormItem>
              )}
            />
            {/* COUSR02-025 to COUSR02-027. */}
            <FormField
              control={form.control}
              name="userType"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={15} col={6} span={11}>
                    <FormLabel className={LABEL}>User Type: </FormLabel>
                  </GridCell>
                  <GridCell row={15} col={17} span={COUSR02_LENGTHS.userType}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={busy}
                        maxLength={COUSR02_LENGTHS.userType}
                        size={COUSR02_LENGTHS.userType}
                        autoComplete="off"
                        spellCheck={false}
                        className={ONE_CHAR_VALUE}
                      />
                    </FormControl>
                    <FormMessage className="sr-only" />
                  </GridCell>
                  <GridCell row={15} col={19} span={17}>
                    <FormDescription className={HINT}>(A=Admin, U=User)</FormDescription>
                  </GridCell>
                </FormItem>
              )}
            />
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
