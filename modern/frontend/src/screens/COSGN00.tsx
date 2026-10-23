import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router";
import { useForm, type FieldErrors } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useCosgn00cSignon } from "@/api/hooks";
import { clearSession, setSession } from "@/auth/session";
import { FieldGrid, GridCell, fieldBox } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
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
import { INVALID_KEY, THANK_YOU } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyOptions } from "@/lib/pfkeys";
import { SCREENS } from "@/routes";
import { COSGN00_LENGTHS, cosgn00Schema, type Cosgn00Values } from "@/schemas/COSGN00";

/*
 * Sign-on screen: map COSGN0A of `app/bms/COSGN00.bms`, transaction CC00, program COSGN00C,
 * endpoint 1 (`POST /api/v1/signon`). Field rows are `COSGN00-nnn` of `modern/audit/screens.md`;
 * behaviour rows are the `client` rows `COSGN00C-nnn` of `modern/audit/online/COSGN00C.md`.
 */

const SCREEN = SCREENS.COSGN00;

/** `id` of the sign-on `<form>`, which the `ENTER=Sign-on` button and the Enter key submit. */
const FORM_ID = "cosgn00-form";

/** COSGN00-015: the row-5 literal, the map's only screen title (it has no row-4 title). */
const TITLE = "This is a Credit Card Demo Application for Mainframe Modernization";

/** COSGN00-016 to COSGN00-024: the note drawn on rows 7 to 15 from column 21, `LENGTH=42`. */
const NOTE_ROWS = [
  "+========================================+",
  "|%%%%%%%  NATIONAL RESERVE NOTE  %%%%%%%%|",
  "|%(1)  THE UNITED STATES OF KICSLAND (1)%|",
  "|%$$              ___       ********  $$%|",
  "|%$    {x}       (o o)                 $%|",
  "|%$     ******  (  V  )      O N E     $%|",
  "|%(1)          ---m-m---             (1)%|",
  "|%%~~~~~~~~~~~ ONE DOLLAR ~~~~~~~~~~~~~%%|",
  "+========================================+",
] as const;
const NOTE_FIRST_ROW = 7;
const NOTE_COL = 21;
const NOTE_LENGTH = 42;

/** COSGN00-037: `ENTER=Sign-on` and `F3=Exit`, in map order. */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COSGN00);

/*
 * Colour tokens of AAP 0.4.3: BLUE is `muted-foreground`; TURQUOISE, GREEN and NEUTRAL are the
 * foreground. A grid column is wider than a character, so a literal fills only part of its cell:
 * the centred note and instruction keep their place on the screen, the labels end next to their
 * input as `User ID     :` ends one column before `USERID`, and the `(8 Char)` hints start
 * right after it. `whitespace-pre` keeps every literal's inner spacing. COSGN00C recolours no
 * field or literal, so a label keeps the foreground while its input is in error.
 */
const LITERAL = "flex h-full items-center whitespace-pre";
const CENTRED = `${LITERAL} justify-center`;
const BLUE = "text-muted-foreground";
const FOREGROUND = "text-foreground";
const INPUT_LABEL =
  `h-full justify-end whitespace-pre ${FOREGROUND} data-[error=true]:text-foreground`;

/** The form input a `ScreenError.field` names, by its wire name (COSGN00C-011). */
function formFieldOf(field: string | null | undefined): keyof Cosgn00Values | undefined {
  switch (field) {
    case "user_id":
      return "userId";
    case "password":
      return "password";
    default:
      return undefined;
  }
}

/**
 * The CardDemo sign-on screen (CC00 / COSGN00C) at `/signon`.
 *
 * ENTER sends the user ID and password exactly as typed: COSGN00C upper-cases both on the server
 * (COSGN00C-009, E26). On success the session is stored and the screen goes to the `next_route`
 * the server chose (COSGN00C-017); the stored user type only chooses navigation and never grants
 * anything. On a `ScreenError` the message fills the error row, the inputs keep what was typed and
 * the input the error names takes the focus (COSGN00C-011). That input is also marked invalid with
 * the server message, so assistive technology reads the message with the input; an error that
 * names no input marks none, and the next sign-on sent clears every mark. F3 ends the conversation
 * with the session-end text (COSGN00C-004); any other F-key shows the invalid-key message
 * (COSGN00C-005).
 *
 * While a sign-on is in flight every key is refused, as the terminal keyboard stays locked until
 * the program answers; a late answer can therefore never sign on a user who has pressed F3. The
 * lock is taken when ENTER is accepted, before the schema's check, and opens when the check
 * refuses the input or the answer arrives. While the request is out, `ENTER=Sign-on` and `F3=Exit`
 * are disabled, the key row and the form are busy and both inputs are read-only.
 */
export function COSGN00(): ReactNode {
  const navigate = useNavigate();
  const signon = useCosgn00cSignon();
  const form = useForm<Cosgn00Values>({
    resolver: zodResolver(cosgn00Schema),
    defaultValues: { userId: "", password: "" },
  });
  const { reset, clearErrors, setError } = form;
  const [message, setMessage] = useState("");
  const [ended, setEnded] = useState(false);
  // `inFlight` is the lock the key handlers read at once; `pending` renders it while the sign-on
  // request is out.
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const endedRef = useRef<HTMLElement>(null);

  // COSGN00C-002: no COMMAREA reaches COSGN00C, so every entry starts without a session.
  useEffect(() => {
    clearSession();
  }, []);

  // The session-end text replaces the focused control, so the focus moves to the text.
  useEffect(() => {
    if (ended) {
      endedRef.current?.focus();
    }
  }, [ended]);

  // COSGN00C-004: nothing typed is received, so the typed values are dropped with the session.
  const endSession = useCallback(() => {
    if (inFlight.current) {
      return;
    }
    reset();
    clearSession();
    setEnded(true);
  }, [reset]);

  // COSGN00C-005.
  const showInvalidKey = useCallback(() => {
    if (!inFlight.current) {
      setMessage(INVALID_KEY);
    }
  }, []);

  const keys = useMemo<PfKeyBarEntry[]>(
    () =>
      KEY_ITEMS.map((item) =>
        item.key === "F3"
          ? { ...item, onPress: endSession, disabled: pending }
          : { ...item, disabled: pending },
      ),
    [endSession, pending],
  );
  const keyOptions = useMemo<PfKeyOptions>(
    () => ({ foldShifted: KEY_ROWS.COSGN00.foldShifted, onInvalidKey: showInvalidKey }),
    [showInvalidKey],
  );

  // Reached only through `onSubmit`, which has already taken the lock.
  function submit(values: Cosgn00Values): void {
    setPending(true);
    // COSGN00C-001: the message row, and the input an earlier answer marked, show only the
    // outcome of the key just pressed.
    setMessage("");
    clearErrors();
    signon.mutate(
      { body: { user_id: values.userId, password: values.password } },
      {
        onSuccess: (response) => {
          setSession(response.token, response.user_id, response.user_type);
          void navigate(response.next_route);
        },
        onError: (error) => {
          setMessage(error.message);
          // COSGN00C-011: the error row stays the only visible message; the input it names is
          // marked invalid with it, read through its `FormMessage`, and takes the cursor.
          const name = formFieldOf(error.field);
          if (name !== undefined) {
            setError(name, { type: "server", message: error.message }, { shouldFocus: true });
          }
        },
        onSettled: () => {
          inFlight.current = false;
          setPending(false);
        },
      },
    );
  }

  // A value longer than its map LENGTH (COSGN00-027, COSGN00-031) is never sent; the schema's
  // message takes the message row, and the form focuses the first rejected input. No request is
  // out, so the lock ENTER took opens again.
  function reject(errors: FieldErrors<Cosgn00Values>): void {
    inFlight.current = false;
    setMessage(errors.userId?.message ?? errors.password?.message ?? "");
  }

  /**
   * ENTER, by the button, the key or an Enter in a field. The lock is taken here, before the
   * schema's asynchronous check, so an ENTER that arrives during the check or while the sign-on is
   * out submits nothing and never reaches the browser's own form submission. A check that fails
   * unexpectedly opens the lock again, as no request was sent, and still reports its error.
   */
  function onSubmit(event: FormEvent<HTMLFormElement>): void {
    if (inFlight.current) {
      event.preventDefault();
      return;
    }
    inFlight.current = true;
    void form.handleSubmit(submit, reject)(event).catch((error: unknown) => {
      inFlight.current = false;
      setPending(false);
      throw error;
    });
  }

  if (ended) {
    // COSGN00C-004 records `SEND TEXT ... ERASE`: the plain text alone, with no map, header or
    // keys, so it is not framed by ScreenShell.
    return (
      <main ref={endedRef} tabIndex={-1} className="p-4 font-mono text-sm text-foreground">
        <p className="whitespace-pre">{THANK_YOU}</p>
      </main>
    );
  }

  return (
    <ScreenShell
      transaction={SCREEN.transaction}
      program={SCREEN.program}
      title={TITLE}
      formId={FORM_ID}
      message={{ kind: "error", text: message }}
      keys={keys}
      keyOptions={keyOptions}
      busy={pending}
      // COSGN00-001 to COSGN00-014: the shell's sign-on layout holds the map's header rows 1 to 3,
      // with `APPLID` and `SYSID` empty outside a CICS region (COSGN00-044, COSGN00-045).
      headerLayout="signon"
    >
      <Form {...form}>
        <form id={FORM_ID} aria-busy={pending} onSubmit={onSubmit}>
          <FieldGrid>
            {NOTE_ROWS.map((text, index) => (
              <GridCell
                key={NOTE_FIRST_ROW + index}
                row={NOTE_FIRST_ROW + index}
                col={NOTE_COL}
                span={NOTE_LENGTH}
              >
                {/* Decorative drawing: hidden from assistive technology, shown unchanged. */}
                <span aria-hidden="true" className={`${CENTRED} ${BLUE}`}>
                  {text}
                </span>
              </GridCell>
            ))}

            {/* COSGN00-025. */}
            <GridCell row={17} col={16} span={49}>
              <span className={`${CENTRED} ${FOREGROUND}`}>
                {"Type your User ID and Password, then press ENTER:"}
              </span>
            </GridCell>

            {/*
             * COSGN00-026, COSGN00-027 (IC: the initial cursor), COSGN00-029. Each `FormItem` is
             * `contents`, so its label, input and hint sit on the map grid in cells of their own.
             * The `(8 Char)` hint is the input's description. The error row is the only visible
             * message, so the schema or server message `FormMessage` adds is for assistive
             * technology only, read with the input it rejects or names.
             */}
            <FormField
              control={form.control}
              name="userId"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={19} col={29} span={13}>
                    <FormLabel className={INPUT_LABEL}>{"User ID     :"}</FormLabel>
                  </GridCell>
                  <GridCell row={19} col={43} span={COSGN00_LENGTHS.userId}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={pending}
                        maxLength={COSGN00_LENGTHS.userId}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        className={fieldBox(COSGN00_LENGTHS.userId)}
                      />
                    </FormControl>
                  </GridCell>
                  <GridCell row={19} col={52} span={8}>
                    <FormDescription className={LITERAL}>(8 Char)</FormDescription>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />

            {/*
             * COSGN00-030, COSGN00-031 (DRK: never pre-filled; the declared underscores are not
             * shown), COSGN00-033. The unnamed dark field at (20,61) is never read and is not
             * rendered (COSGN00-034).
             */}
            <FormField
              control={form.control}
              name="password"
              render={({ field }) => (
                <FormItem className="contents">
                  <GridCell row={20} col={29} span={13}>
                    <FormLabel className={INPUT_LABEL}>{"Password    :"}</FormLabel>
                  </GridCell>
                  <GridCell row={20} col={43} span={COSGN00_LENGTHS.password}>
                    <FormControl>
                      <Input
                        {...field}
                        readOnly={pending}
                        type="password"
                        maxLength={COSGN00_LENGTHS.password}
                        autoComplete="off"
                        className={fieldBox(COSGN00_LENGTHS.password)}
                      />
                    </FormControl>
                  </GridCell>
                  <GridCell row={20} col={52} span={8}>
                    <FormDescription className={LITERAL}>(8 Char)</FormDescription>
                  </GridCell>
                  <FormMessage className="sr-only" />
                </FormItem>
              )}
            />
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
