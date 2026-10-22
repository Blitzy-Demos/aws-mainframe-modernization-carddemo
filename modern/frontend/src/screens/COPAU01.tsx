import { Fragment, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Navigate, useLocation, useNavigate } from "react-router";
import { useCopaus1cDetail, useCopaus1cToggleFraud } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { KEY_ROWS, keyRowItems, type PfKeyHandlers } from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";

/**
 * Authorization detail: map `COPAU1A` of `app/app-authorization-ims-db2-mq/bms/COPAU01.bms`,
 * transaction `CPVD`, program `COPAUS1C` (AAP 0.3.5), over endpoints 22 and 23 (AAP 0.3.4.1).
 *
 * The map has no input field, so the screen has no form and no zod schema. Every value is shown
 * exactly as `AuthDetailResponse` carries it: COPAUS1C builds each one with its own pictures and
 * padding, the dates and times included, which are display text from `X(06)` fields and not
 * typed dates (COPAU01-015, COPAUS1C-013, COPAUS1C-014), and the card number arrives redacted
 * (COPAUS1C-012, E32). Nothing here formats, parses, logs or stores a value.
 *
 * The screen keeps what the COMMAREA kept between interactions, the account and the selected
 * key (COPAUS1C-003): the account comes from the route state COPAU00 sets on its `S` selection
 * (COPAUS0C-010) and never changes here; the key changes only when F8 reaches the next
 * authorization (COPAUS1C-024).
 */

const SCREEN = SCREENS.COPAU01;

/** COPAU01-011: `POS=(4,27)`, `INITIAL='View Authorization Details'`. */
const TITLE = "View Authorization Details";

/**
 * The account `INITIALIZE CARDDEMO-COMMAREA` leaves in `CDEMO-ACCT-ID PIC 9(11)` when CPVD starts
 * with no COMMAREA, which COPAUS1C hands to COPAUS0C (COPAUS1C-001).
 */
const INITIALIZED_ACCT_ID = "00000000000";

/**
 * `CCDA-MSG-INVALID-KEY PIC X(50)` (`app/cpy/CSMSG01Y.cpy:20-21`), padded to its 50 bytes as the
 * `MOVE` transfers it (COPAUS1C-005). The text is declared here, from its copybook, because this
 * screen's planned dependencies do not include the header module that holds the shared copy.
 */
const INVALID_KEY_MESSAGE = "Invalid key pressed. Please see below...".padEnd(50, " ");

/**
 * COPAU01-042: the 76-character `INITIAL` at `POS=(17,2)`, `Merchant Details ` followed by 59
 * hyphens across its two source lines (`COPAU01.bms:229-233`).
 */
const MERCHANT_CAPTION = `Merchant Details ${"-".repeat(59)}`;

/** COPAU01-054: `F3=Back`, `F5=Mark/Remove Fraud` and `F8=Next Auth`, in map order. */
const MAP_KEYS = keyRowItems(KEY_ROWS.COPAU01);

/** The `COLOR` the map declares for an output field. */
type MapColour = "PINK" | "BLUE" | "RED";

/**
 * Value colours under the token mapping of AAP 0.4.3: PINK, which has no token, to `--primary`;
 * BLUE to `--muted-foreground`; RED to `--destructive`.
 */
const VALUE_TONE: Readonly<Record<MapColour, string>> = {
  PINK: "text-primary",
  BLUE: "text-muted-foreground",
  RED: "text-destructive",
};

/**
 * Every label of the map is TURQUOISE, which maps to `--foreground`. `h-full` fills the grid line
 * so the label centres on the `Input` beside it; `whitespace-pre` keeps `Source   :` as declared.
 */
const LABEL_CLASS = "h-full whitespace-pre text-foreground";

/**
 * A grid column is narrower than a character where the 80 columns share the frame with their
 * gaps, so a field of one or two characters would clip its value. The `Input` is therefore at
 * least its field's cell (`min-w-full`) and at least its `LENGTH` in characters (`size`, with
 * `w-auto`); every field is followed by enough empty columns on its line for the difference.
 *
 * BLITZY [LAYOUT]: horizontal padding `px-1` instead of the `Input` default `px-3`. With the
 * default, the one-character Auth Resp box runs into `Resp Reason:` and Auth Date into
 * `Auth Time:` once the frame nears its minimum width; with `px-1` every box ends before the next
 * field down to that width.
 */
const VALUE_CLASS = "w-auto min-w-full px-1";

/** One output field of the map with the literal that labels it, both on the same line. */
type DetailField = {
  /** The field's name on the map, such as `CARDNUM`. */
  readonly name: string;
  /** The label literal's `INITIAL`. */
  readonly label: string;
  /** `POS` line of both. */
  readonly row: number;
  /** The label literal's `POS` column. */
  readonly labelCol: number;
  /** The columns the label takes: its `LENGTH`, cut where it runs into the next field. */
  readonly labelSpan: number;
  /** The field's `POS` column. */
  readonly col: number;
  /** The field's `LENGTH`. */
  readonly span: number;
  readonly colour: MapColour;
  /** The `AuthDetailResponse` member that fills the field. */
  readonly member: Exclude<
    keyof components["schemas"]["AuthDetailResponse"],
    "acct_id" | "auth_key" | "message"
  >;
};

/** Lines 7 to 15: COPAU01-012 to COPAU01-041, filled as COPAUS1C-012 to COPAUS1C-022 state. */
const AUTHORIZATION_FIELDS: readonly DetailField[] = [
  {
    name: "CARDNUM",
    label: "Card #:",
    row: 7,
    labelCol: 2,
    labelSpan: 7,
    col: 11,
    span: 16,
    colour: "PINK",
    member: "card_num",
  },
  {
    name: "AUTHDT",
    label: "Auth Date:",
    row: 7,
    labelCol: 31,
    labelSpan: 10,
    col: 43,
    span: 10,
    colour: "PINK",
    member: "auth_date",
  },
  {
    name: "AUTHTM",
    label: "Auth Time:",
    row: 7,
    labelCol: 56,
    labelSpan: 10,
    col: 68,
    span: 10,
    colour: "PINK",
    member: "auth_time",
  },
  {
    name: "AUTHRSP",
    label: "Auth Resp:",
    row: 9,
    labelCol: 2,
    labelSpan: 10,
    col: 14,
    span: 1,
    colour: "PINK",
    member: "response",
  },
  {
    name: "AUTHRSN",
    label: "Resp Reason:",
    row: 9,
    labelCol: 18,
    labelSpan: 12,
    col: 32,
    span: 20,
    colour: "BLUE",
    member: "reason",
  },
  {
    name: "AUTHCD",
    label: "Auth Code:",
    row: 9,
    labelCol: 56,
    labelSpan: 10,
    col: 68,
    span: 6,
    colour: "BLUE",
    member: "auth_code",
  },
  {
    name: "AUTHAMT",
    label: "Amount:",
    row: 11,
    labelCol: 2,
    labelSpan: 7,
    col: 11,
    span: 12,
    colour: "BLUE",
    member: "amount",
  },
  {
    name: "POSEMD",
    label: "POS Entry Mode:",
    row: 11,
    labelCol: 29,
    labelSpan: 15,
    col: 46,
    span: 4,
    colour: "BLUE",
    member: "pos_entry_mode",
  },
  {
    name: "AUTHSRC",
    label: "Source   :",
    row: 11,
    labelCol: 56,
    labelSpan: 10,
    col: 68,
    span: 10,
    colour: "BLUE",
    member: "source",
  },
  {
    name: "MCCCD",
    label: "MCC Code:",
    row: 13,
    labelCol: 2,
    labelSpan: 9,
    col: 13,
    span: 4,
    colour: "BLUE",
    member: "merchant_category_code",
  },
  {
    name: "CRDEXP",
    label: "Card Exp. Date:",
    row: 13,
    labelCol: 25,
    labelSpan: 15,
    col: 42,
    span: 5,
    colour: "BLUE",
    member: "card_expiry",
  },
  {
    name: "AUTHTYP",
    label: "Auth Type:",
    row: 13,
    labelCol: 52,
    labelSpan: 10,
    col: 64,
    span: 14,
    colour: "BLUE",
    member: "auth_type",
  },
  // COPAU01-036: the literal declares LENGTH=18, through column 20, over TRNID at (15,12). The
  // later field's attribute byte wins (X-SCREENS-3), so the label ends at column 11 and its
  // eight characters still show.
  {
    name: "TRNID",
    label: "Tran Id:",
    row: 15,
    labelCol: 2,
    labelSpan: 10,
    col: 12,
    span: 15,
    colour: "BLUE",
    member: "transaction_id",
  },
  {
    name: "AUTHMTC",
    label: "Match Status:",
    row: 15,
    labelCol: 31,
    labelSpan: 13,
    col: 46,
    span: 1,
    colour: "RED",
    member: "match_status",
  },
  {
    name: "AUTHFRD",
    label: "Fraud Status:",
    row: 15,
    labelCol: 52,
    labelSpan: 13,
    col: 67,
    span: 10,
    colour: "RED",
    member: "fraud_status",
  },
];

/** Lines 19 and 21, under the caption: COPAU01-043 to COPAU01-052, filled as COPAUS1C-023. */
const MERCHANT_FIELDS: readonly DetailField[] = [
  {
    name: "MERNAME",
    label: "Name:",
    row: 19,
    labelCol: 2,
    labelSpan: 5,
    col: 9,
    span: 25,
    colour: "BLUE",
    member: "merchant_name",
  },
  {
    name: "MERID",
    label: "Merchant ID:",
    row: 19,
    labelCol: 41,
    labelSpan: 12,
    col: 55,
    span: 15,
    colour: "BLUE",
    member: "merchant_id",
  },
  {
    name: "MERCITY",
    label: "City:",
    row: 21,
    labelCol: 2,
    labelSpan: 5,
    col: 9,
    span: 25,
    colour: "BLUE",
    member: "merchant_city",
  },
  {
    name: "MERST",
    label: "State:",
    row: 21,
    labelCol: 41,
    labelSpan: 6,
    col: 49,
    span: 2,
    colour: "BLUE",
    member: "merchant_state",
  },
  {
    name: "MERZIP",
    label: "Zip:",
    row: 21,
    labelCol: 55,
    labelSpan: 4,
    col: 61,
    span: 10,
    colour: "BLUE",
    member: "merchant_zip",
  },
];

/**
 * The colour of a field's value. Auth Resp takes the colour COPAUS1C moves to `AUTHRSPC` in place
 * of the map's PINK: `DFHGREEN` for `A`, which maps to `--foreground`, and `DFHRED` for `D`
 * (COPAUS1C-016, COPAUS1C-036). Every other field keeps its declared colour.
 */
function valueTone(field: DetailField, value: string): string {
  if (field.name === "AUTHRSP") {
    if (value === "A") {
      return "text-foreground";
    }
    if (value === "D") {
      return "text-destructive";
    }
  }
  return VALUE_TONE[field.colour];
}

/**
 * The text of a failed call for the message row: the `ScreenError` message the API sent, or the
 * `message` of a network or status error. A body without a string message shows nothing rather
 * than `undefined`.
 */
function errorMessage(error: { readonly message?: unknown }): string {
  return typeof error.message === "string" ? error.message : "";
}

/** What the screen keeps of the COMMAREA COPAU00 passed: the account and the selected key. */
type DetailEntry = {
  readonly acctId: string;
  readonly authKey: string;
};

/**
 * The route state, narrowed. `null` when there is none, CPVD started with no COMMAREA
 * (COPAUS1C-001). A member that is missing or not text reads as empty, which the screen treats
 * as the COBOL treats an account that is not numeric or a blank key (COPAUS1C-006).
 */
function detailEntry(state: unknown): DetailEntry | null {
  if (typeof state !== "object" || state === null) {
    return null;
  }
  return {
    acctId: "acct_id" in state && typeof state.acct_id === "string" ? state.acct_id : "",
    authKey: "auth_key" in state && typeof state.auth_key === "string" ? state.auth_key : "",
  };
}

/**
 * The detail screen of one account and key, mounted afresh for each route entry.
 *
 * Keys (COPAU01-054, COPAU01-061 to COPAU01-064; COPAUS1C-002 to COPAUS1C-005):
 * - entry and ENTER read the kept key's authorization: endpoint 22 with no `direction`;
 * - F8 reads the account's next authorization (`direction` `forward`) and keeps its key;
 * - F5 marks or unmarks the kept authorization as fraud: endpoint 23;
 * - F3 returns to COPAU00 with the account, sending nothing;
 * - any other F-key reads as ENTER does, then shows the invalid-key message in place of the
 *   read's message.
 *
 * A request key pressed while a call is in flight is ignored, as the 3270 keyboard stays locked
 * until the program's answer arrives; F3 always leaves.
 */
function AuthorizationDetail({ acctId, authKey: enteredKey }: DetailEntry): ReactNode {
  const navigate = useNavigate();
  const fieldId = useId();
  const detailCall = useCopaus1cDetail();
  const fraudCall = useCopaus1cToggleFraud();
  const [authKey, setAuthKey] = useState(enteredKey);
  const [detail, setDetail] = useState<components["schemas"]["AuthDetailResponse"] | null>(
    null,
  );
  const [message, setMessage] = useState("");
  const inFlight = useRef(false);
  const entered = useRef(false);
  const cardNumInput = useRef<HTMLInputElement>(null);

  // An empty account or key would leave a path segment empty, a request the API cannot route.
  // COPAUS1C reads nothing for such a state and shows every field blank with an empty message
  // row (COPAUS1C-006), which is what the screen shows without sending.
  const readable = acctId !== "" && authKey !== "";

  /**
   * COPAUS1C-036: every send of the map puts the cursor on Card #. Each answer the screen shows,
   * from endpoint 22 or 23, found or failed, or the blank screen of a state it cannot read, moves
   * the focus back there from the key that was pressed or clicked.
   */
  function placeCursor(): void {
    cardNumInput.current?.focus();
  }

  /**
   * Endpoint 22. A found authorization fills the fields and its key becomes the kept key (the
   * same key after ENTER and at the last authorization, the next one after F8; COPAUS1C-024,
   * COPAUS1C-025). A failed read shows no fields and its message, and the kept key stays
   * (COPAUS1C-009 to COPAUS1C-011, COPAUS1C-026). `shown`, when given, replaces whatever message
   * the read set (COPAUS1C-005).
   */
  function read(direction: "forward" | undefined, shown?: string): void {
    if (inFlight.current) {
      return;
    }
    if (!readable) {
      setDetail(null);
      setMessage(shown ?? "");
      placeCursor();
      return;
    }
    inFlight.current = true;
    detailCall.mutate(
      {
        path: { acct_id: acctId, auth_key: authKey },
        query: direction === undefined ? {} : { direction },
      },
      {
        onSuccess: (found) => {
          setDetail(found);
          setAuthKey(found.auth_key);
          setMessage(shown ?? found.message ?? "");
        },
        onError: (error) => {
          setDetail(null);
          setMessage(shown ?? errorMessage(error));
        },
        onSettled: () => {
          inFlight.current = false;
          placeCursor();
        },
      },
    );
  }

  /**
   * Endpoint 23. A committed toggle shows its message and the authorization as replaced, its key
   * unchanged (COPAUS1C-032, COPAUS1C-034). A failure shows its message over the fields already
   * shown, because nothing was stored and the error body carries no detail (COPAUS1C-031,
   * X-COPAUS1C-3). A 200 without a detail is, by the response's contract, such a rolled-back
   * outcome, so it too keeps the fields rather than reading them again.
   */
  function toggleFraud(): void {
    if (inFlight.current) {
      return;
    }
    if (!readable) {
      setMessage("");
      placeCursor();
      return;
    }
    inFlight.current = true;
    fraudCall.mutate(
      { path: { acct_id: acctId, auth_key: authKey } },
      {
        onSuccess: (result) => {
          setMessage(result.message);
          if (result.detail !== undefined && result.detail !== null) {
            setDetail(result.detail);
          }
        },
        onError: (error) => {
          setMessage(errorMessage(error));
        },
        onSettled: () => {
          inFlight.current = false;
          placeCursor();
        },
      },
    );
  }

  /** F3: XCTL COPAUS0C, which loads the account on its first entry (COPAUS1C-004, COPAUS0C-002). */
  function back(): void {
    void navigate(PATHS.COPAU00, { state: { from: PATHS.COPAU01, acct_id: acctId } });
  }

  // First entry reads at once, as ENTER does (COPAUS1C-002). The ref keeps the development
  // double-run of effects from sending the read twice.
  useEffect(() => {
    if (entered.current) {
      return;
    }
    entered.current = true;
    read(undefined);
  }, []);

  const actions: PfKeyHandlers = {
    F3: back,
    F5: toggleFraud,
    F8: () => {
      read("forward");
    },
  };

  // COPAU01-064: the map labels no ENTER key although COPAUS1C serves it, so ENTER gets a button
  // with the bare key name at the start of the key row, shown, because every PF action must exist
  // as a button (X-SCREENS-10, AAP 0.4.4).
  const keys: PfKeyBarEntry[] = [
    {
      key: "ENTER",
      label: "ENTER",
      dark: false,
      onPress: () => {
        read(undefined);
      },
    },
    ...MAP_KEYS.map((item) => ({ ...item, onPress: actions[item.key] })),
  ];

  function renderField(field: DetailField): ReactNode {
    const id = `${fieldId}-${field.name}`;
    const value = detail === null ? "" : detail[field.member];
    return (
      <Fragment key={field.name}>
        <GridCell row={field.row} col={field.labelCol} span={field.labelSpan}>
          <Label htmlFor={id} className={LABEL_CLASS}>
            {field.label}
          </Label>
        </GridCell>
        <GridCell row={field.row} col={field.col} span={field.span}>
          <Input
            id={id}
            readOnly
            value={value}
            size={field.span}
            // COPAUS1C-036: the program puts the cursor on Card # with every send. `autoFocus`
            // places it for the first one; `placeCursor` returns it after each answer.
            ref={field.name === "CARDNUM" ? cardNumInput : undefined}
            autoFocus={field.name === "CARDNUM"}
            data-field={field.name}
            className={`${VALUE_CLASS} ${valueTone(field, value)}`}
          />
        </GridCell>
      </Fragment>
    );
  }

  return (
    <ScreenShell
      transaction={SCREEN.transaction}
      program={SCREEN.program}
      title={TITLE}
      message={{ text: message, kind: "error" }}
      keys={keys}
      keyOptions={{
        foldShifted: KEY_ROWS.COPAU01.foldShifted,
        onInvalidKey: () => {
          read(undefined, INVALID_KEY_MESSAGE);
        },
      }}
    >
      <FieldGrid>
        {AUTHORIZATION_FIELDS.map(renderField)}
        <GridCell row={17} col={2} span={76}>
          {/* NEUTRAL static text, which maps to the inherited foreground. */}
          <p className="flex h-full items-center whitespace-pre">{MERCHANT_CAPTION}</p>
        </GridCell>
        {MERCHANT_FIELDS.map(renderField)}
      </FieldGrid>
    </ScreenShell>
  );
}

/**
 * The CPVD screen at `/authorizations/detail`.
 *
 * With no route state, CPVD has no COMMAREA: COPAUS1C initialises one and transfers straight to
 * COPAUS0C, so the screen goes to COPAU00 with the initialised account and reads nothing
 * (COPAUS1C-001). Otherwise the detail is keyed by the route entry, so a new entry with another
 * account or key starts afresh, as each XCTL to COPAUS1C does.
 */
export function COPAU01(): ReactNode {
  const location = useLocation();
  const entry = detailEntry(location.state);
  if (entry === null) {
    return (
      <Navigate
        to={PATHS.COPAU00}
        replace
        state={{ from: PATHS.COPAU01, acct_id: INITIALIZED_ACCT_ID }}
      />
    );
  }
  return <AuthorizationDetail key={location.key} acctId={entry.acctId} authKey={entry.authKey} />;
}
