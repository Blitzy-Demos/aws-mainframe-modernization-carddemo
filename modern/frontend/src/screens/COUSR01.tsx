import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm, type FieldErrors } from "react-hook-form";
import { useNavigate } from "react-router";
import { useCousr01cAdd } from "@/api/hooks";
import { FieldControl } from "@/components/screen/FieldControl";
import { FIELD_BOX_HEIGHT, FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, type ShellMessage } from "@/components/screen/ScreenShell";
import {
  Form,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { INVALID_KEY } from "@/lib/header";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers, type PfKeyOptions } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import { COUSR01_LENGTHS, type Cousr01Values, cousr01Schema } from "@/schemas/COUSR01";

/** One of the five input fields of `COUSR1A`, by its form name. */
type FieldName = keyof Cousr01Values;

/**
 * One input field of `COUSR1A` with the literal that labels it and the hint after it, each at
 * its map `POS` and `LENGTH`. The input spans its field's `COUSR01_LENGTHS` entry.
 */
type MapField = {
  readonly name: FieldName;
  readonly row: number;
  /** Column of the input's `POS`. */
  readonly col: number;
  /** The TURQUOISE literal before the input. */
  readonly label: { readonly col: number; readonly span: number; readonly text: string };
  /** The BLUE literal after the input, where the map has one. */
  readonly hint?: { readonly col: number; readonly span: number; readonly text: string };
  /** `ATTRB=DRK`: a password input, never pre-filled. */
  readonly dark?: boolean;
};

/**
 * The input fields of `COUSR1A` in map order, which is also the order of COUSR01C's empty-field
 * edits (`app/bms/COUSR01.bms:80-150`; `modern/audit/screens.md` COUSR01-012 to COUSR01-026).
 * `User Type: ` keeps its INITIAL; its trailing blank is the attribute byte of `USRTYPE`
 * (X-SCREENS-3) and never shows.
 */
const MAP_FIELDS: readonly MapField[] = [
  {
    name: "firstName",
    row: 8,
    col: 18,
    label: { col: 6, span: 11, text: "First Name:" },
  },
  {
    name: "lastName",
    row: 8,
    col: 56,
    label: { col: 45, span: 10, text: "Last Name:" },
  },
  {
    name: "userId",
    row: 11,
    col: 15,
    label: { col: 6, span: 8, text: "User ID:" },
    hint: { col: 24, span: 8, text: "(8 Char)" },
  },
  {
    name: "password",
    row: 11,
    col: 55,
    label: { col: 45, span: 9, text: "Password:" },
    hint: { col: 64, span: 8, text: "(8 Char)" },
    dark: true,
  },
  {
    name: "userType",
    row: 14,
    col: 17,
    label: { col: 6, span: 11, text: "User Type: " },
    hint: { col: 19, span: 17, text: "(A=Admin, U=User)" },
  },
];

/** The form's values on first entry, after F4 and after a successful add: every field empty. */
const EMPTY: Cousr01Values = {
  firstName: "",
  lastName: "",
  userId: "",
  password: "",
  userType: "",
};

/** The `UserAddRequest` member → form field, for the field a `ScreenError` names. */
const FIELD_BY_WIRE_NAME: ReadonlyMap<string, FieldName> = new Map([
  ["first_name", "firstName"],
  ["last_name", "lastName"],
  ["user_id", "userId"],
  ["password", "password"],
  ["user_type", "userType"],
]);

/**
 * Where COUSR01C puts the cursor when nothing else names a field: `FNAME`, the map's `IC` field
 * (`MOVE -1 TO FNAMEL`, `app/cbl/COUSR01C.cbl:86,100,149,272,289`).
 */
const HOME_FIELD: FieldName = "firstName";

/**
 * The grid span and side padding of an input, chosen so that it shows every one of the `LENGTH`
 * characters it accepts at each width `ScreenShell` allows.
 *
 * BLITZY [LAYOUT]: an input spans `LENGTH + 1` columns, the field's whole terminal footprint: on
 * the 3270 `POS` holds the attribute byte and the characters follow it, so the column after the
 * last `LENGTH` column belongs to this field and no other field of the map claims it. A column
 * is narrower than a `text-sm` monospace character, so `LENGTH` columns alone leave no room for
 * the input's border: an 8-character value cannot show its last character below a 920px viewport.
 *
 * BLITZY [LAYOUT]: the `Input` default `px-3` becomes `ps-1 pe-0`, so every value fits down to
 * the shell's minimum width while the start inset keeps the text off the border. A one-character
 * field (`USRTYPE`) drops the side padding, centres its character and has `min-w-3`, its border
 * plus the widest character: at the shell's minimum width its two columns are a fraction of a
 * pixel narrower, and the input then extends only into the column gap. Every input takes the
 * shared map-field box height (`FIELD_BOX_HEIGHT`) in place of the component's; border, radius
 * and colours stay the component's.
 */
function inputLayout(name: FieldName): { readonly span: number; readonly className: string } {
  const length = COUSR01_LENGTHS[name];
  return {
    span: length + 1,
    className: `${FIELD_BOX_HEIGHT} ${length === 1 ? "min-w-3 px-0 text-center" : "ps-1 pe-0"}`,
  };
}

/** The labels of row 24 (COUSR01-028): `ENTER=Add User`, `F3=Back`, `F4=Clear`, `F12=Exit`. */
const KEY_ITEMS = keyRowItems(KEY_ROWS.COUSR01);

/** A request to move the focus; a new object each time, so the same field can be asked twice. */
type FocusRequest = { readonly name: FieldName };

/**
 * The Add User screen, map `COUSR1A` of transaction `CU01` (program COUSR01C), at `/users/add`.
 *
 * ENTER sends the five fields exactly as typed to endpoint 17 (COUSR01C-006): no value is trimmed
 * or case-changed, because COUSR01C stores what it receives and sign-on upper-cases only what it
 * is given (E26). The zod schema checks the map lengths alone, so every empty field reaches the
 * server, which owns each message. The typed user ID and user type describe the user being
 * added; the caller's authority is its token's (AAP 0.5.1).
 *
 * Keys, as COUSR01C's AID `EVALUATE` serves them (`app/cbl/COUSR01C.cbl:90-103`):
 * - ENTER: the add. A 200 empties the fields and shows the program's message in the information
 *   style, the green COUSR01C gives `ERRMSG` on success (COUSR01C-022). A `ScreenError` keeps the
 *   typed fields and shows its message as an error, with the focus on the field it names, which
 *   is marked invalid until the next key (COUSR01C-025).
 * - F3: back to the admin menu, no request (COUSR01C-007).
 * - F4: empties the fields and the message row, no request (COUSR01C-008).
 * - F12 and every other function key: the invalid-key message, fields kept. The map labels
 *   `F12=Exit`, but COUSR01C has no PF12 branch (COUSR01C-009, X-SCREENS-8).
 *
 * Each key first clears the previous message (COUSR01C-005), and each outcome puts the focus
 * where the program puts the cursor. The password input is masked and never pre-filled; the
 * screen sends the password only in the request body, never shows, logs or stores it, and empties
 * it with the other fields after a successful add (Cardholder Data Redaction).
 *
 * While an add is in flight the keyboard is locked, as the 3270's was until the program answered:
 * ENTER, F4 and F12 are disabled, every other key but F3 is refused and the inputs are read-only.
 * F3 sends nothing, so it still leaves.
 */
export function COUSR01(): ReactNode {
  const formId = useId();
  const navigate = useNavigate();
  const add = useCousr01cAdd();
  const form = useForm<Cousr01Values>({
    resolver: zodResolver(cousr01Schema),
    defaultValues: EMPTY,
  });
  const { clearErrors, handleSubmit, reset, setError, setFocus } = form;
  const [message, setMessage] = useState<ShellMessage | undefined>(undefined);
  const [focusRequest, setFocusRequest] = useState<FocusRequest | null>(null);
  // Set while an add is in flight, so a second ENTER cannot send the same user twice before the
  // first response arrives, as the 3270 keyboard stayed locked until the program answered.
  const sending = useRef(false);
  // `sending` as rendered: the disabled keys, the read-only inputs and `aria-busy`.
  const [pending, setPending] = useState(false);

  // Focus moves after the render that follows the outcome. `reset` drops the inputs' focus
  // references until the fields render again, so focusing in the same tick would miss.
  useEffect(() => {
    if (focusRequest !== null) {
      setFocus(focusRequest.name);
    }
  }, [focusRequest, setFocus]);

  const focusField = useCallback((name: FieldName) => {
    setFocusRequest({ name });
  }, []);

  const back = useCallback(() => {
    void navigate(PATHS.COADM01);
  }, [navigate]);

  const clear = useCallback(() => {
    if (sending.current) {
      return;
    }
    reset(EMPTY);
    setMessage(undefined);
    focusField(HOME_FIELD);
  }, [reset, focusField]);

  // The previous outcome's field error goes with its message (COUSR01C-005).
  const invalidKey = useCallback(() => {
    if (sending.current) {
      return;
    }
    clearErrors();
    setMessage({ kind: "error", text: INVALID_KEY });
    focusField(HOME_FIELD);
  }, [clearErrors, focusField]);

  const keys = useMemo<PfKeyBarEntry[]>(() => {
    const actions: PfKeyHandlers = { F3: back, F4: clear, F12: invalidKey };
    // The keys `submit`, `clear` and `invalidKey` refuse while an add is in flight: all but F3.
    return KEY_ITEMS.map((item) => ({
      ...item,
      onPress: actions[item.key],
      disabled: pending && item.key !== "F3",
    }));
  }, [back, clear, invalidKey, pending]);

  const keyOptions = useMemo<PfKeyOptions>(() => ({ onInvalidKey: invalidKey }), [invalidKey]);

  function addUser(values: Cousr01Values): void {
    if (sending.current) {
      return;
    }
    sending.current = true;
    setPending(true);
    add.mutate(
      {
        body: {
          first_name: values.firstName,
          last_name: values.lastName,
          user_id: values.userId,
          password: values.password,
          user_type: values.userType,
        },
      },
      {
        onSuccess: (result) => {
          reset(EMPTY);
          setMessage({ kind: "info", text: result.message });
          focusField(HOME_FIELD);
        },
        onError: (error) => {
          const named =
            typeof error.field === "string" ? FIELD_BY_WIRE_NAME.get(error.field) : undefined;
          setMessage({ kind: "error", text: error.message });
          // The input the error names is marked invalid and described by the same text; the
          // message row stays the only place it shows.
          if (named !== undefined) {
            setError(named, { type: "server", message: error.message });
          }
          focusField(named ?? HOME_FIELD);
        },
        onSettled: () => {
          sending.current = false;
          setPending(false);
        },
      },
    );
  }

  // Only a value longer than its map field fails the schema, which `maxLength` already stops at
  // the keyboard; react-hook-form focuses the first such field itself.
  function showSchemaError(errors: FieldErrors<Cousr01Values>): void {
    const first = MAP_FIELDS.find((field) => errors[field.name] !== undefined);
    setMessage({
      kind: "error",
      text: first === undefined ? undefined : errors[first.name]?.message,
    });
  }

  function submit(event: FormEvent<HTMLFormElement>): void {
    if (sending.current) {
      // Refused while an add is in flight: the message stays and the browser submits nothing.
      event.preventDefault();
      return;
    }
    setMessage(undefined);
    void handleSubmit(addUser, showSchemaError)(event);
  }

  return (
    <ScreenShell
      transaction={SCREENS.COUSR01.transaction}
      program={SCREENS.COUSR01.program}
      title="Add User"
      formId={formId}
      message={message}
      keys={keys}
      keyOptions={keyOptions}
      busy={pending}
    >
      <Form {...form}>
        <form id={formId} noValidate aria-busy={pending} onSubmit={submit}>
          <FieldGrid>
            {MAP_FIELDS.map(({ name, row, col, label, hint, dark }) => {
              const layout = inputLayout(name);
              return (
                <FormField
                  key={name}
                  control={form.control}
                  name={name}
                  render={({ field }) => (
                    // `contents` lets the label, input and hint cells sit on the screen grid while
                    // the item still ties the label and the hint to the input. The control names a
                    // description only on the fields whose hint renders one.
                    <FormItem className="contents">
                      <GridCell row={row} col={label.col} span={label.span}>
                        {/* `whitespace-pre`: a literal never wraps, as on the terminal. */}
                        <FormLabel className="h-full whitespace-pre text-foreground">
                          {label.text}
                        </FormLabel>
                      </GridCell>
                      <GridCell row={row} col={col} span={layout.span}>
                        <FieldControl described={hint !== undefined}>
                          {/*
                           * Every value is data about the user being added, so the browser must
                           * neither fill in the operator's own credentials nor change the case.
                           */}
                          <Input
                            {...field}
                            readOnly={pending}
                            className={layout.className}
                            type={dark === true ? "password" : "text"}
                            maxLength={COUSR01_LENGTHS[name]}
                            autoFocus={name === HOME_FIELD}
                            autoComplete="off"
                            autoCapitalize="off"
                            autoCorrect="off"
                            spellCheck={false}
                          />
                        </FieldControl>
                      </GridCell>
                      {hint !== undefined && (
                        <GridCell row={row} col={hint.col} span={hint.span}>
                          <FormDescription className="flex h-full items-center whitespace-pre">
                            {hint.text}
                          </FormDescription>
                        </GridCell>
                      )}
                      {/*
                       * The message row is the only visible message, so the field's own error
                       * text is for assistive technology only, read with the input it names. It
                       * is the item's direct child rather than a cell's: an `sr-only` grid item
                       * takes no cell and sits at the grid's origin, while one in a cell past the
                       * edge of a narrow viewport would widen the page.
                       */}
                      <FormMessage className="sr-only" />
                    </FormItem>
                  )}
                />
              );
            })}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
