import {
  useEffect,
  useId,
  useRef,
  useState,
  type BaseSyntheticEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldErrors } from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCousr02cFetch, useCousr03cDelete } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
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
  KEY_ROWS,
  keyRowItems,
  type PfKey,
  type PfKeyHandlers,
  type PfKeyItem,
} from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COUSR03_LENGTHS, cousr03Schema, type Cousr03Values } from "@/schemas/COUSR03";

/** Transaction CU03, program COUSR03C, route `/users/delete`. */
const SCREEN = SCREENS.COUSR03;

/** The map's screen title, `POS=(4,35)` (COUSR03-011). */
const TITLE = "Delete User";

/** The YELLOW rule of row 8, seventy asterisks at `POS=(8,6)` (COUSR03-015). */
const RULE = "*".repeat(70);

/** YELLOW, which has no token, as emphasis on the foreground; decorative, so hidden from AT. */
const RULE_CLASS =
  "flex h-full items-center overflow-hidden whitespace-pre font-semibold text-foreground";

/**
 * `CCDA-MSG-INVALID-KEY` (`app/cpy/CSMSG01Y.cpy:20-21`) without the trailing spaces of its 50-byte
 * field, as COUSR03C-009 shows it. Defined here rather than imported from `@/lib/header`, which
 * holds the padded value, because this screen imports only its declared dependencies.
 */
const INVALID_KEY_MESSAGE = "Invalid key pressed. Please see below...";

/**
 * The two endpoint 20 failures COUSR03C sends in the neutral colour that the read before the
 * delete set (`COUSR03C.cbl:283-286,323-335`; COUSR03C-025, X-COUSR03C-2). A `ScreenError` carries
 * no colour or status, so the screen tells them from the other errors by the program's text.
 */
const NEUTRAL_DELETE_ERRORS: ReadonlySet<string> = new Set([
  "User ID NOT found...",
  "Unable to Update User...",
]);

/** The wire names of the three output fields a `ScreenError` can name (`ScreenError.field`). */
const OUTPUT_FIELDS: ReadonlySet<string> = new Set(["first_name", "last_name", "user_type"]);

/** The output field the last red `ScreenError` named, by wire name, with the error's text. */
type OutputFieldError = { readonly field: string; readonly text: string };

/** Every route of the application: the only values a route-state `from` is followed to. */
const SCREEN_PATHS: ReadonlySet<string> = new Set(Object.values(PATHS));

/**
 * Width of an input: its `LENGTH` columns, and never fewer characters than its `size`, which each
 * input sets to its `LENGTH`. BLITZY [LAYOUT]: on narrow screens the 80 columns are narrower than
 * the characters, and without the floor the user ID and full 20-character names were clipped.
 */
const FIELD_WIDTH = "min-w-max";

/**
 * BLITZY [LAYOUT]: a one-column cell is narrower than the Input's default padding, so USRTYPE is
 * sized to its one character (`field-sizing-content`, `px-0.5`, at least `min-w-4`), which keeps
 * it clear of the hint two columns to the right at every width.
 */
const USER_TYPE_INPUT_CLASS =
  "w-auto min-w-4 px-0.5 text-center field-sizing-content text-muted-foreground";

/** COBOL `= SPACES`: empty or all spaces, as the `COUSR03` schema tests the typed user ID. */
const COBOL_BLANK = /^ *$/;

/** Row 24, `ENTER=Fetch  F3=Back  F4=Clear  F5=Delete` (COUSR03-026). */
const MAP_KEYS = keyRowItems(KEY_ROWS.COUSR03);

/**
 * F12, which COUSR03C serves (`COUSR03C.cbl:123-125`) but the map does not label: a button
 * labelled `F12` after the map's labels (COUSR03-037, X-SCREENS-10).
 */
const F12_KEY: PfKeyItem = { key: "F12", label: "F12", dark: false };

/** The keys refused while a request is outstanding; F3 and F12 leave at any time. */
const LOCKED_KEYS: ReadonlySet<PfKey> = new Set(["ENTER", "F4", "F5"]);

/**
 * What a transfer to this screen passes in route state, the counterparts of `CDEMO-FROM-PROGRAM`
 * and `CDEMO-CU03-USR-SELECTED`: the route of the screen that transferred here, and the user ID a
 * `D` selection on `/users` chose (COUSR00C-009). Anything else in the state is ignored, and a
 * blank user ID counts as no selection (COUSR03C-005).
 */
function entryState(state: unknown): { from?: string; userId?: string } {
  if (typeof state !== "object" || state === null) {
    return {};
  }
  return {
    from:
      "from" in state && typeof state.from === "string" && SCREEN_PATHS.has(state.from)
        ? state.from
        : undefined,
    userId:
      "user_id" in state && typeof state.user_id === "string" && !COBOL_BLANK.test(state.user_id)
        ? state.user_id
        : undefined,
  };
}

/** Props of {@link OutputField}. */
type OutputFieldProps = {
  /** Map line of the label and the field. */
  row: number;
  /** The label literal's `POS` column, `LENGTH` and `INITIAL`. */
  labelCol: number;
  labelSpan: number;
  label: string;
  /** The field's `POS` column and `LENGTH`. */
  col: number;
  span: number;
  /** The value endpoint 18 returned, or the empty string. */
  value: string;
  /** Focus target for a `ScreenError` that names this field. */
  inputRef: RefObject<HTMLInputElement | null>;
  /** Width and colour utilities in place of the default `FIELD_WIDTH` and BLUE value colour. */
  inputClassName?: string;
  /** The text of a red `ScreenError` that names this field, while the message row shows it. */
  error?: string;
};

/**
 * One protected (`ASKIP`) output field with its TURQUOISE label: a read-only `Input` in the BLUE
 * value colour, bound to its `Label`. While a red `ScreenError` names it, the field is marked
 * invalid and described by that text, which only assistive technology reads: the message row is
 * the screen's one visible message.
 */
function OutputField({
  row,
  labelCol,
  labelSpan,
  label,
  col,
  span,
  value,
  inputRef,
  inputClassName,
  error,
}: OutputFieldProps): ReactNode {
  const id = useId();
  const messageId = `${id}-message`;
  const invalid = error !== undefined;
  return (
    <>
      <GridCell row={row} col={labelCol} span={labelSpan}>
        <Label htmlFor={id} className="h-full whitespace-pre text-foreground">
          {label}
        </Label>
      </GridCell>
      <GridCell row={row} col={col} span={span}>
        <Input
          id={id}
          ref={inputRef}
          readOnly
          value={value}
          maxLength={span}
          size={span}
          aria-invalid={invalid || undefined}
          aria-describedby={invalid ? messageId : undefined}
          className={inputClassName ?? `${FIELD_WIDTH} text-muted-foreground`}
        />
        {invalid && (
          <p id={messageId} className="sr-only">
            {error}
          </p>
        )}
      </GridCell>
    </>
  );
}

/**
 * The user delete screen, map `COUSR3A` of `app/bms/COUSR03.bms`, served by COUSR03C through
 * endpoint 18 (`GET /users/{user_id}`, the read COUSR02C shares) and endpoint 20
 * (`DELETE /users/{user_id}`), both admin-only (`modern/audit/online/COUSR03C.md`).
 *
 * - First display: with a user ID in route state the input holds it and the screen fetches at
 *   once, exactly as ENTER (COUSR03C-004); otherwise it is empty and sends nothing (COUSR03C-005).
 * - ENTER fetches the typed user ID and shows the names, the type and COUSR03C's own prompt
 *   (COUSR03C-011, COUSR03C-012); a failed fetch empties them (COUSR03C-015).
 * - F5 deletes the user ID in the input as it stands, with no fetch or confirmation first
 *   (COUSR03C-023); success empties every field (COUSR03C-025).
 * - F4 clears the screen, F3 returns to the screen that transferred here, else `/admin`, and F12
 *   goes to `/admin` (COUSR03C-006 to COUSR03C-008); any other F-key shows the invalid-key
 *   message (COUSR03C-009).
 *
 * The user ID is the target user, validated input only: the API's admin gate decides who may
 * delete. No password is shown or asked for. While a request is outstanding ENTER, F4 and F5 are
 * disabled and the user ID is read-only, as the 3270 keyboard stays locked until the program's
 * answer; F3 and F12 still leave.
 */
export function COUSR03(): ReactNode {
  const navigate = useNavigate();
  const location = useLocation();
  // Route state is read once, at the first display, as COUSR03C reads its COMMAREA on first entry.
  const [entry] = useState(() => entryState(location.state));
  const formId = useId();
  // The cursor is placed through the screen's own element refs: `form.reset` drops the field refs
  // `form.setFocus` relies on, which would leave the cursor on the pressed key after F4 or F5.
  const form = useForm<Cousr03Values>({
    resolver: zodResolver(cousr03Schema),
    defaultValues: { userId: entry.userId ?? "" },
    shouldFocusError: false,
  });
  const fetchUser = useCousr02cFetch();
  const deleteUser = useCousr03cDelete();
  const [shown, setShown] = useState<components["schemas"]["UserView"] | null>(null);
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);
  const [outputError, setOutputError] = useState<OutputFieldError | null>(null);
  const userIdRef = useRef<HTMLInputElement>(null);
  const firstNameRef = useRef<HTMLInputElement>(null);
  const lastNameRef = useRef<HTMLInputElement>(null);
  const userTypeRef = useRef<HTMLInputElement>(null);
  const entered = useRef(false);
  // Whether a request is outstanding, from the key that sends it to its answer. The ref decides
  // at once; the state renders the locked keys, which a key pressed before that render would pass.
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  /** Locks the keyboard for one request, before its edits run. */
  function lock(): void {
    busyRef.current = true;
    setBusy(true);
  }

  /** Unlocks the keyboard: the edits refused the request, or its answer is shown. */
  function unlock(): void {
    busyRef.current = false;
    setBusy(false);
  }

  /**
   * Puts the cursor where the program's `MOVE -1 TO <field>L` puts it: the field a `ScreenError`
   * names by its wire name, and the user ID for any other value or none.
   */
  function focusField(field?: string | null): void {
    const outputs: Readonly<Record<string, RefObject<HTMLInputElement | null>>> = {
      first_name: firstNameRef,
      last_name: lastNameRef,
      user_type: userTypeRef,
    };
    const target =
      typeof field === "string" && Object.hasOwn(outputs, field) ? outputs[field] : userIdRef;
    target.current?.focus();
  }

  /**
   * Marks the field a red `ScreenError` names as invalid, described by the error's text: the user
   * ID through the form, an output field through its own state. Any other field marks nothing.
   */
  function markField(error: components["schemas"]["ScreenError"]): void {
    if (error.field === "user_id") {
      form.setError("userId", { type: "server", message: error.message });
    } else if (typeof error.field === "string" && OUTPUT_FIELDS.has(error.field)) {
      setOutputError({ field: error.field, text: error.message });
    }
  }

  /** The text of the error marking the output field `field`, while one does. */
  function outputErrorOf(field: string): string | undefined {
    return outputError?.field === field ? outputError.text : undefined;
  }

  /** The schema's blank-user-ID edit (COUSR03C-010, COUSR03C-016); the fields keep their values. */
  function showEditError(errors: FieldErrors<Cousr03Values>): void {
    unlock();
    setOutputError(null);
    setMessage({ kind: "error", text: errors.userId?.message });
    focusField();
  }

  /**
   * ENTER (`PROCESS-ENTER-KEY`): endpoint 18 with the typed user ID. The schema has already
   * replaced the form's errors; an output field's mark goes too.
   */
  function fetchById({ userId }: Cousr03Values): void {
    setOutputError(null);
    void fetchUser
      .mutateAsync(
        { path: { user_id: userId } },
        {
          onSuccess: (view) => {
            setShown(view);
            setMessage({ kind: "info", text: view.delete_prompt });
            focusField();
          },
          onError: (error) => {
            setShown(null);
            setMessage({ kind: "error", text: error.message });
            markField(error);
            focusField(error.field);
          },
        },
      )
      .then(unlock, unlock);
  }

  /** F5 (`DELETE-USER-INFO`): endpoint 20 with the typed user ID, the marks cleared as for ENTER. */
  function deleteById({ userId }: Cousr03Values): void {
    setOutputError(null);
    void deleteUser
      .mutateAsync(
        { path: { user_id: userId } },
        {
          onSuccess: (result) => {
            form.reset({ userId: "" });
            setShown(null);
            setMessage({ kind: "info", text: result.message });
            focusField();
          },
          onError: (error) => {
            const neutral = NEUTRAL_DELETE_ERRORS.has(error.message);
            setMessage({ kind: neutral ? "info" : "error", text: error.message });
            // A neutral failure only moves the cursor (COUSR03C-025).
            if (!neutral) {
              markField(error);
            }
            focusField(error.field);
          },
        },
      )
      .then(unlock, unlock);
  }

  /**
   * `submit`, the edits and request of one key, refused while a request is outstanding. The lock
   * is taken before the edits, which `handleSubmit` runs asynchronously, and released at their
   * refusal or failure, or at the request's answer.
   */
  function locked(
    submit: (event?: BaseSyntheticEvent) => Promise<void>,
  ): (event?: BaseSyntheticEvent) => Promise<void> {
    return (event) => {
      if (busyRef.current) {
        event?.preventDefault();
        return Promise.resolve();
      }
      lock();
      return submit(event).catch((error: unknown) => {
        unlock();
        throw error;
      });
    };
  }

  const submitFetch = locked(form.handleSubmit(fetchById, showEditError));
  const submitDelete = locked(form.handleSubmit(deleteById, showEditError));

  // The first-entry fetch of COUSR03C-004, run with the first render's handlers and form values.
  // The ref keeps StrictMode's second effect run in development from fetching again.
  useEffect(() => {
    if (entered.current || entry.userId === undefined) {
      return;
    }
    entered.current = true;
    void submitFetch();
  }, []);

  /** F4 (`CLEAR-CURRENT-SCREEN`). */
  function clear(): void {
    if (busyRef.current) {
      return;
    }
    form.reset({ userId: "" });
    setShown(null);
    setOutputError(null);
    setMessage(undefined);
    focusField();
  }

  const actions: PfKeyHandlers = {
    F3: () => {
      navigate(entry.from ?? PATHS.COADM01);
    },
    F4: clear,
    F5: () => {
      void submitDelete();
    },
  };
  const keys: PfKeyBarEntry[] = [
    ...MAP_KEYS.map((item) => ({
      ...item,
      onPress: actions[item.key],
      disabled: busy && LOCKED_KEYS.has(item.key),
    })),
    {
      ...F12_KEY,
      onPress: () => {
        navigate(PATHS.COADM01);
      },
    },
  ];

  return (
    <ScreenShell
      transaction={SCREEN.transaction}
      program={SCREEN.program}
      title={TITLE}
      formId={formId}
      message={message}
      keys={keys}
      busy={busy}
      keyOptions={{
        foldShifted: KEY_ROWS.COUSR03.foldShifted,
        onInvalidKey: () => {
          // The invalid-key message replaces the error that marked a field, so the mark goes.
          form.clearErrors();
          setOutputError(null);
          setMessage({ kind: "error", text: INVALID_KEY_MESSAGE });
        },
      }}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={busy} onSubmit={submitFetch}>
          <FieldGrid>
            <FormField
              control={form.control}
              name="userId"
              render={({ field }) => (
                // `contents` lets the label and the input sit in their own grid cells while the
                // form item still binds them.
                <FormItem className="contents">
                  <GridCell row={6} col={6} span={14}>
                    <FormLabel className="h-full text-foreground">Enter User ID:</FormLabel>
                  </GridCell>
                  <GridCell row={6} col={21} span={COUSR03_LENGTHS.userId}>
                    <FormControl>
                      <Input
                        {...field}
                        ref={(element) => {
                          field.ref(element);
                          userIdRef.current = element;
                        }}
                        readOnly={busy}
                        maxLength={COUSR03_LENGTHS.userId}
                        size={COUSR03_LENGTHS.userId}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        className={FIELD_WIDTH}
                      />
                    </FormControl>
                    {/* For assistive technology only: the message row is the visible message. */}
                    <FormMessage className="sr-only" />
                  </GridCell>
                </FormItem>
              )}
            />
            <GridCell row={8} col={6} span={RULE.length}>
              <span aria-hidden="true" className={RULE_CLASS}>
                {RULE}
              </span>
            </GridCell>
            <OutputField
              row={11}
              labelCol={6}
              labelSpan={11}
              label="First Name:"
              col={18}
              span={20}
              value={shown?.first_name ?? ""}
              inputRef={firstNameRef}
              error={outputErrorOf("first_name")}
            />
            <OutputField
              row={13}
              labelCol={6}
              labelSpan={10}
              label="Last Name:"
              col={18}
              span={20}
              value={shown?.last_name ?? ""}
              inputRef={lastNameRef}
              error={outputErrorOf("last_name")}
            />
            <OutputField
              row={15}
              labelCol={6}
              labelSpan={11}
              label="User Type: "
              col={17}
              span={1}
              value={shown?.user_type ?? ""}
              inputRef={userTypeRef}
              inputClassName={USER_TYPE_INPUT_CLASS}
              error={outputErrorOf("user_type")}
            />
            <GridCell row={15} col={19} span={17}>
              <span className="flex h-full items-center whitespace-pre text-muted-foreground">
                (A=Admin, U=User)
              </span>
            </GridCell>
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
