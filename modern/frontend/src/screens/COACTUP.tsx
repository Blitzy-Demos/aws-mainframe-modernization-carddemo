import { useId, useRef, useState, type ReactNode } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  useForm,
  type Control,
  type ControllerRenderProps,
  type FieldErrors,
} from "react-hook-form";
import { useLocation, useNavigate } from "react-router";
import { useCoactupcFetch, useCoactupcSave } from "@/api/hooks";
import type { components } from "@/api/schema";
import { FieldGrid, GridCell } from "@/components/screen/FieldGrid";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell } from "@/components/screen/ScreenShell";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormDescription,
  FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import {
  KEY_ROWS,
  PF_KEYS,
  bindPfKeys,
  coactupKeys,
  keyRowItems,
  type CoactupKeyState,
  type PfKey,
  type PfKeyAvailability,
  type PfKeyHandlers,
} from "@/lib/pfkeys";
import { PATHS, SCREENS } from "@/routes";
import {
  COACTUP_LENGTHS,
  coactupKeySchema,
  coactupSchema,
  type CoactupValues,
} from "@/schemas/COACTUP";

/*
 * Map COACTUP (`CACTUPA`): account update, transaction CAUP, program COACTUPC, route
 * `/accounts/update`. Endpoint 5 fetches the account (ENTER while no details are shown) and
 * endpoint 6 validates (ENTER) and saves (F5) the changes; F3, F12 and every key pressed after a
 * finished save send no request (`modern/audit/online/COACTUPC.md`, AAP 0.3.4.1). Positions,
 * lengths and literals are the `COACTUP-<nnn>` rows of `modern/audit/screens.md`.
 *
 * The server owns every message and every edit. The screen keeps what the pseudo-conversational
 * COBOL kept in its COMMAREA: the state `ACUP-CHANGE-ACTION` and the snapshot of the fetch, which
 * every endpoint 6 request echoes with the state of the last response (COACTUPC-100).
 */

/** A field of the update form, named as in `@/schemas/COACTUP`. */
type FieldName = keyof CoactupValues;

/** A state in which the fetched details are on the screen. */
type DetailState = Exclude<CoactupKeyState, "not-fetched">;

/** What the fetch returned: the values it showed and the snapshot every save echoes. */
type FetchedDetails = {
  readonly values: CoactupValues;
  readonly snapshot: components["schemas"]["AccountUpdateSnapshot"];
};

/** The screen between two keys: its state, the fetched details and its two message rows. */
type ScreenModel = {
  /** `INFOMSG`: the info line of the state (COACTUPC-079). */
  readonly info: string;
  /** `ERRMSG`: the message of the last failure, absent after a success. */
  readonly message?: string;
  /** Which dark keys the last endpoint 6 answer shows (`save_enabled`, `cancel_enabled`). */
  readonly served?: { readonly save: boolean; readonly cancel: boolean };
} & (
  | { readonly state: "not-fetched" }
  | { readonly state: DetailState; readonly details: FetchedDetails }
);

/**
 * `INFOMSG` of each state, which `3250-SETUP-INFOMSG` derives from the state alone
 * (COACTUPC-079). A 200 brings the server's own text; these show after a `ScreenError` and after
 * the keys that send no request (COACTUPC-001, COACTUPC-006, COACTUPC-069).
 */
const INFO_MESSAGES = {
  "not-fetched": "Enter or update id of account to update",
  S: "Update account details presented above.",
  E: "Update account details presented above.",
  N: "Changes validated.Press F5 to save",
  C: "Changes committed to database",
  L: "Changes unsuccessful. Please try again",
  F: "Changes unsuccessful. Please try again",
} as const satisfies Record<CoactupKeyState, string>;

/** The `ACUP-CHANGE-ACTION` code of each state, as `snapshot.state` carries it. */
const STATE_CODES = {
  "not-fetched": " ",
  S: "S",
  E: "E",
  N: "N",
  C: "C",
  L: "L",
  F: "F",
} as const satisfies Record<CoactupKeyState, string>;

/*
 * COACTUPC messages whose failure leaves a state of its own. A `ScreenError` carries only the
 * message and the field, so the screen sets the state the audit row names from them
 * (COACTUPC.md, "Reading the table": the screen sets the state the row names).
 */
/** `NO-CHANGES-DETECTED`: the echoed state is kept (COACTUPC-043, COACTUPC-110). */
const NO_CHANGES = "No change detected with respect to values fetched.";
/** `DATA-WAS-CHANGED-BEFORE-UPDATE`: state `S` and the fetched values again (COACTUPC-116). */
const RECORD_CHANGED = "Record changed by some one else. Please review";
/** `COULD-NOT-LOCK-ACCT-FOR-UPDATE`: state `L` (COACTUPC-101). */
const LOCK_ACCOUNT_FAILED = "Could not lock account record for update";
/** `COULD-NOT-LOCK-CUST-FOR-UPDATE`: state `C`, as audited (COACTUPC-102, X-COACTUPC-6). */
const LOCK_CUSTOMER_FAILED = "Could not lock customer record for update";
/** `LOCKED-BUT-UPDATE-FAILED`: state `F` (COACTUPC-106, COACTUPC-107). */
const UPDATE_FAILED = "Update of record failed";
/** The `WHEN OTHER` abend of `2000-DECIDE-ACTION`: no state survives it. */
const UNEXPECTED_DATA = "UNEXPECTED DATA SCENARIO";
/** Starts of the re-read failures, which answer as the fetch does (COACTUPC-087, COACTUPC-095). */
const REREAD_FAILURE_PREFIXES = ["CustId:", "File Error: "] as const;

/** Blank as `1100-RECEIVE-MAP` reads a field: empty, spaces, or `*` and spaces (COACTUPC-007). */
const BLANK = /^\*? *$/;

/** The SSN parts, sent only when re-entered (COACTUPC-064, E32). */
const SSN_PARTS: ReadonlySet<FieldName> = new Set(["ssnPart1", "ssnPart2", "ssnPart3"]);

/** Fields `3320-UNPROTECT-FEW-ATTRS` leaves protected while details are shown (COACTUPC-080). */
const ALWAYS_PROTECTED: ReadonlySet<FieldName> = new Set(["custId", "country"]);

/**
 * The twelve date parts and six phone parts, `JUSTIFY=(RIGHT)` with the default zero fill
 * (COACTUPC-008, X-COACTUPC-12; COACTUP-019 to COACTUP-114).
 */
const JUSTIFIED = [
  "openYear",
  "openMonth",
  "openDay",
  "expiryYear",
  "expiryMonth",
  "expiryDay",
  "reissueYear",
  "reissueMonth",
  "reissueDay",
  "dobYear",
  "dobMonth",
  "dobDay",
  "phone1Area",
  "phone1Prefix",
  "phone1Line",
  "phone2Area",
  "phone2Prefix",
  "phone2Line",
] as const satisfies readonly FieldName[];

/**
 * Every field before a fetch, as `3201-SHOW-INITIAL-VALUES` sends the map: empty, except the SSN
 * parts, which show their declared `INITIAL` (COACTUPC-073; COACTUP-062, -064, -066).
 */
const INITIAL_VALUES: CoactupValues = {
  acctId: "",
  activeStatus: "",
  openYear: "",
  openMonth: "",
  openDay: "",
  creditLimit: "",
  expiryYear: "",
  expiryMonth: "",
  expiryDay: "",
  cashCreditLimit: "",
  reissueYear: "",
  reissueMonth: "",
  reissueDay: "",
  currentBalance: "",
  currentCycleCredit: "",
  groupId: "",
  currentCycleDebit: "",
  custId: "",
  ssnPart1: "999",
  ssnPart2: "99",
  ssnPart3: "9999",
  dobYear: "",
  dobMonth: "",
  dobDay: "",
  ficoScore: "",
  firstName: "",
  middleName: "",
  lastName: "",
  addressLine1: "",
  state: "",
  addressLine2: "",
  zip: "",
  city: "",
  country: "",
  phone1Area: "",
  phone1Prefix: "",
  phone1Line: "",
  govtId: "",
  phone2Area: "",
  phone2Prefix: "",
  phone2Line: "",
  eftAccountId: "",
  primaryCardHolder: "",
};

/** The screen as the menu opens it: state not fetched (COACTUPC-001). */
const INITIAL_SCREEN: ScreenModel = {
  state: "not-fetched",
  info: INFO_MESSAGES["not-fetched"],
};

/** Request field names of `ScreenError.field` → form field (`acct_id` is the path parameter). */
const FIELD_BY_WIRE_NAME = {
  acct_id: "acctId",
  active_status: "activeStatus",
  open_year: "openYear",
  open_month: "openMonth",
  open_day: "openDay",
  credit_limit: "creditLimit",
  expiry_year: "expiryYear",
  expiry_month: "expiryMonth",
  expiry_day: "expiryDay",
  cash_credit_limit: "cashCreditLimit",
  reissue_year: "reissueYear",
  reissue_month: "reissueMonth",
  reissue_day: "reissueDay",
  current_balance: "currentBalance",
  current_cycle_credit: "currentCycleCredit",
  group_id: "groupId",
  current_cycle_debit: "currentCycleDebit",
  customer_id: "custId",
  ssn_part1: "ssnPart1",
  ssn_part2: "ssnPart2",
  ssn_part3: "ssnPart3",
  dob_year: "dobYear",
  dob_month: "dobMonth",
  dob_day: "dobDay",
  fico_score: "ficoScore",
  first_name: "firstName",
  middle_name: "middleName",
  last_name: "lastName",
  address_line_1: "addressLine1",
  state: "state",
  address_line_2: "addressLine2",
  zip: "zip",
  city: "city",
  country: "country",
  phone_1_area: "phone1Area",
  phone_1_prefix: "phone1Prefix",
  phone_1_line: "phone1Line",
  government_id: "govtId",
  phone_2_area: "phone2Area",
  phone_2_prefix: "phone2Prefix",
  phone_2_line: "phone2Line",
  eft_account_id: "eftAccountId",
  primary_card_holder: "primaryCardHolder",
} as const satisfies Record<
  "acct_id" | keyof components["schemas"]["AccountUpdateFields"],
  FieldName
>;

/** `FIELD_BY_WIRE_NAME` under a string index, for lookup by the text a response carries. */
const FIELD_OF_WIRE_NAME: Readonly<Record<string, FieldName>> = FIELD_BY_WIRE_NAME;

/**
 * One enterable map field and the literal bound to it as its label, each at its `POS`. A
 * literal's `LENGTH` is the length of its `INITIAL`; a field's is `COACTUP_LENGTHS[name]`.
 */
type MapInput = {
  readonly name: FieldName;
  readonly row: number;
  readonly col: number;
  readonly label?: { readonly text: string; readonly row: number; readonly col: number };
  /** The accessible name where the bound literal is a `-` separator or the map has none. */
  readonly ariaLabel?: string;
};

/** The account block of `CACTUPA`, rows 5 to 10, in map order (COACTUP-012 to COACTUP-055). */
const ACCOUNT_INPUTS: readonly MapInput[] = [
  { name: "acctId", row: 5, col: 38, label: { text: "Account Number :", row: 5, col: 19 } },
  { name: "activeStatus", row: 5, col: 70, label: { text: "Active Y/N: ", row: 5, col: 57 } },
  { name: "openYear", row: 6, col: 17, label: { text: "Opened :", row: 6, col: 8 } },
  {
    name: "openMonth",
    row: 6,
    col: 24,
    label: { text: "-", row: 6, col: 22 },
    ariaLabel: "Opened month",
  },
  {
    name: "openDay",
    row: 6,
    col: 29,
    label: { text: "-", row: 6, col: 27 },
    ariaLabel: "Opened day",
  },
  {
    name: "creditLimit",
    row: 6,
    col: 61,
    label: { text: "Credit Limit        :", row: 6, col: 39 },
  },
  { name: "expiryYear", row: 7, col: 17, label: { text: "Expiry :", row: 7, col: 8 } },
  {
    name: "expiryMonth",
    row: 7,
    col: 24,
    label: { text: "-", row: 7, col: 22 },
    ariaLabel: "Expiry month",
  },
  {
    name: "expiryDay",
    row: 7,
    col: 29,
    label: { text: "-", row: 7, col: 27 },
    ariaLabel: "Expiry day",
  },
  {
    name: "cashCreditLimit",
    row: 7,
    col: 61,
    label: { text: "Cash credit Limit   :", row: 7, col: 39 },
  },
  { name: "reissueYear", row: 8, col: 17, label: { text: "Reissue:", row: 8, col: 8 } },
  {
    name: "reissueMonth",
    row: 8,
    col: 24,
    label: { text: "-", row: 8, col: 22 },
    ariaLabel: "Reissue month",
  },
  {
    name: "reissueDay",
    row: 8,
    col: 29,
    label: { text: "-", row: 8, col: 27 },
    ariaLabel: "Reissue day",
  },
  {
    name: "currentBalance",
    row: 8,
    col: 61,
    label: { text: "Current Balance     :", row: 8, col: 39 },
  },
  {
    name: "currentCycleCredit",
    row: 9,
    col: 61,
    label: { text: "Current Cycle Credit:", row: 9, col: 39 },
  },
  { name: "groupId", row: 10, col: 23, label: { text: "Account Group:", row: 10, col: 8 } },
  {
    name: "currentCycleDebit",
    row: 10,
    col: 61,
    label: { text: "Current Cycle Debit :", row: 10, col: 39 },
  },
];

/**
 * The customer block, rows 12 to 20, in map order (COACTUP-058 to COACTUP-120). The row-14
 * captions are bound to the three name fields below them as their labels.
 */
const CUSTOMER_INPUTS: readonly MapInput[] = [
  { name: "custId", row: 12, col: 23, label: { text: "Customer id  :", row: 12, col: 8 } },
  { name: "ssnPart1", row: 12, col: 55, label: { text: "SSN:", row: 12, col: 49 } },
  {
    name: "ssnPart2",
    row: 12,
    col: 61,
    label: { text: "-", row: 12, col: 59 },
    ariaLabel: "SSN 4th and 5th digits",
  },
  {
    name: "ssnPart3",
    row: 12,
    col: 66,
    label: { text: "-", row: 12, col: 64 },
    ariaLabel: "SSN last 4 digits",
  },
  { name: "dobYear", row: 13, col: 23, label: { text: "Date of birth:", row: 13, col: 8 } },
  {
    name: "dobMonth",
    row: 13,
    col: 30,
    label: { text: "-", row: 13, col: 28 },
    ariaLabel: "Date of birth month",
  },
  {
    name: "dobDay",
    row: 13,
    col: 35,
    label: { text: "-", row: 13, col: 33 },
    ariaLabel: "Date of birth day",
  },
  { name: "ficoScore", row: 13, col: 62, label: { text: "FICO Score:", row: 13, col: 49 } },
  { name: "firstName", row: 15, col: 1, label: { text: "First Name", row: 14, col: 1 } },
  { name: "middleName", row: 15, col: 28, label: { text: "Middle Name: ", row: 14, col: 28 } },
  { name: "lastName", row: 15, col: 55, label: { text: "Last Name : ", row: 14, col: 55 } },
  { name: "addressLine1", row: 16, col: 10, label: { text: "Address:", row: 16, col: 1 } },
  { name: "state", row: 16, col: 73, label: { text: "State ", row: 16, col: 63 } },
  { name: "addressLine2", row: 17, col: 10, ariaLabel: "Address line 2" },
  { name: "zip", row: 17, col: 73, label: { text: "Zip", row: 17, col: 63 } },
  { name: "city", row: 18, col: 10, label: { text: "City ", row: 18, col: 1 } },
  { name: "country", row: 18, col: 73, label: { text: "Country", row: 18, col: 63 } },
  { name: "phone1Area", row: 19, col: 10, label: { text: "Phone 1:", row: 19, col: 1 } },
  { name: "phone1Prefix", row: 19, col: 14, ariaLabel: "Phone 1 prefix" },
  { name: "phone1Line", row: 19, col: 18, ariaLabel: "Phone 1 line number" },
  {
    name: "govtId",
    row: 19,
    col: 58,
    label: { text: "Government Issued Id Ref    : ", row: 19, col: 24 },
  },
  { name: "phone2Area", row: 20, col: 10, label: { text: "Phone 2:", row: 20, col: 1 } },
  { name: "phone2Prefix", row: 20, col: 14, ariaLabel: "Phone 2 prefix" },
  { name: "phone2Line", row: 20, col: 18, ariaLabel: "Phone 2 line number" },
  {
    name: "eftAccountId",
    row: 20,
    col: 41,
    label: { text: "EFT Account Id: ", row: 20, col: 24 },
  },
  {
    name: "primaryCardHolder",
    row: 20,
    col: 78,
    label: { text: "Primary Card Holder Y/N:", row: 20, col: 53 },
  },
];

/** The 43 inputs of `CACTUPA` in map order, the order the first failing field is found in. */
const MAP_INPUTS: readonly MapInput[] = [...ACCOUNT_INPUTS, ...CUSTOMER_INPUTS];

/*
 * TURQUOISE literals and the DEFAULT-colour separators take the foreground token (AAP 0.4.3);
 * `whitespace-pre` keeps the literals' inner and trailing spaces, and `h-7` centres them on the
 * height of the inputs beside them.
 */
const LABEL_CLASS = "h-7 whitespace-pre text-foreground";

/*
 * BLITZY [LAYOUT]: the Input's `h-9` and `px-3` are overridden with `h-7` and `px-1`, and its
 * `w-full min-w-0` with `w-auto min-w-full` beside `size` = LENGTH, so each box is as wide as its
 * map LENGTH of characters or its grid cell, whichever is wider: a 1- or 2-column cell of the
 * 80-column grid is narrower than the characters it holds.
 * BLITZY [A11Y]: the Input's default focus ring (`ring-ring/50`) measures about 1.5:1 against the
 * card, below the 3:1 of WCAG 1.4.11. Kept as the system token; flagged for designer review.
 */
const INPUT_CLASS = "h-7 w-auto min-w-full px-1 py-0";

/**
 * How the screen replaces the form values. `keepFieldsRef` keeps the inputs registered, so the
 * cursor can be placed and a field marked right after new values are shown; a plain `reset`
 * drops every registration until the next render.
 */
const RESET_OPTIONS = { keepFieldsRef: true } as const;

/** The key-row settings: COACTUPC folds PF13 to PF24 onto PF1 to PF12 (COACTUPC-003). */
const KEY_OPTIONS = { foldShifted: KEY_ROWS.COACTUP.foldShifted };

/** The row-24 labels of the map: `ENTER=Process`, `F3=Exit`, `F5=Save`, `F12=Cancel`. */
const LABELLED_KEYS = keyRowItems(KEY_ROWS.COACTUP);

/** The function keys the map labels, so that every other key can be bound unlabelled. */
const LABELLED_KEY_NAMES: ReadonlySet<string> = new Set(LABELLED_KEYS.map((item) => item.key));

/**
 * Year, month and day parts of a typed date `YYYY-MM-DD`, cut from the text as the COBOL cuts
 * positions 1-4, 6-7 and 9-10 (COACTUPC-074); a blank date (`null`) shows empty parts. The text
 * is never parsed as a date, so no time zone can shift the day.
 */
function dateParts(date: string | null | undefined): [string, string, string] {
  return date ? [date.slice(0, 4), date.slice(5, 7), date.slice(8, 10)] : ["", "", ""];
}

/**
 * The form values of an endpoint 5 answer, each field as the server sent it (COACTUPC-074 to
 * COACTUPC-077): amounts as their shown strings, dates split into their parts, and the SSN in its
 * redacted form `***-**-NNNN` split into `***`, `**` and the four digits (COACTUPC-076).
 */
function valuesFromView(view: components["schemas"]["AccountUpdateView"]): CoactupValues {
  const [openYear, openMonth, openDay] = dateParts(view.open_date);
  const [expiryYear, expiryMonth, expiryDay] = dateParts(view.expiry_date);
  const [reissueYear, reissueMonth, reissueDay] = dateParts(view.reissue_date);
  const [dobYear, dobMonth, dobDay] = dateParts(view.dob);
  return {
    acctId: view.account_id,
    activeStatus: view.active_status,
    openYear,
    openMonth,
    openDay,
    creditLimit: view.credit_limit,
    expiryYear,
    expiryMonth,
    expiryDay,
    cashCreditLimit: view.cash_credit_limit,
    reissueYear,
    reissueMonth,
    reissueDay,
    currentBalance: view.current_balance,
    currentCycleCredit: view.current_cycle_credit,
    groupId: view.group_id,
    currentCycleDebit: view.current_cycle_debit,
    custId: view.customer_id,
    ssnPart1: view.ssn.slice(0, 3),
    ssnPart2: view.ssn.slice(4, 6),
    ssnPart3: view.ssn.slice(7, 11),
    dobYear,
    dobMonth,
    dobDay,
    ficoScore: view.fico_score,
    firstName: view.first_name,
    middleName: view.middle_name,
    lastName: view.last_name,
    addressLine1: view.address_line_1,
    state: view.state,
    addressLine2: view.address_line_2,
    zip: view.zip,
    city: view.city,
    country: view.country,
    phone1Area: view.phone_1_area,
    phone1Prefix: view.phone_1_prefix,
    phone1Line: view.phone_1_line,
    govtId: view.government_id,
    phone2Area: view.phone_2_area,
    phone2Prefix: view.phone_2_prefix,
    phone2Line: view.phone_2_line,
    eftAccountId: view.eft_account_id,
    primaryCardHolder: view.primary_card_holder,
  };
}

/** `text` right-justified and zero-filled to `length`; empty text stays empty (COACTUPC-008). */
function zeroFilled(text: string, length: number): string {
  return text === "" ? "" : text.padStart(length, "0");
}

/** The values as BMS delivers them to the program: the justified parts zero-filled. */
function receivedValues(values: CoactupValues): CoactupValues {
  const received = { ...values };
  for (const name of JUSTIFIED) {
    received[name] = zeroFilled(values[name], COACTUP_LENGTHS[name]);
  }
  return received;
}

/**
 * The three SSN parts when the operator re-entered them, otherwise `undefined` (COACTUPC-064,
 * E32). Re-entered means that a part differs from the redacted form the fetch showed and that no
 * part still holds a `*` of that form; a part the operator left as shown makes the SSN not
 * re-entered, and the request then omits all three so the stored SSN is kept.
 */
function reenteredSsn(
  values: CoactupValues,
  shown: CoactupValues,
): readonly [string, string, string] | undefined {
  const parts = [values.ssnPart1 ?? "", values.ssnPart2 ?? "", values.ssnPart3 ?? ""] as const;
  const changed =
    parts[0] !== shown.ssnPart1 || parts[1] !== shown.ssnPart2 || parts[2] !== shown.ssnPart3;
  return changed && parts.every((part) => !part.includes("*")) ? parts : undefined;
}

/**
 * `fields` of an endpoint 6 request: the raw text of every map field as received (COACTUPC-007,
 * COACTUPC-080: every field is sent on every key), date and phone parts included as text, the
 * raw-edit-input exception of Typed Dates and Timestamps. The SSN parts are `null`, omitted,
 * unless re-entered.
 */
function requestFields(
  received: CoactupValues,
  ssn: readonly [string, string, string] | undefined,
): components["schemas"]["AccountUpdateFields"] {
  return {
    active_status: received.activeStatus,
    open_year: received.openYear,
    open_month: received.openMonth,
    open_day: received.openDay,
    credit_limit: received.creditLimit,
    expiry_year: received.expiryYear,
    expiry_month: received.expiryMonth,
    expiry_day: received.expiryDay,
    cash_credit_limit: received.cashCreditLimit,
    reissue_year: received.reissueYear,
    reissue_month: received.reissueMonth,
    reissue_day: received.reissueDay,
    current_balance: received.currentBalance,
    current_cycle_credit: received.currentCycleCredit,
    group_id: received.groupId,
    current_cycle_debit: received.currentCycleDebit,
    customer_id: received.custId,
    ssn_part1: ssn?.[0] ?? null,
    ssn_part2: ssn?.[1] ?? null,
    ssn_part3: ssn?.[2] ?? null,
    dob_year: received.dobYear,
    dob_month: received.dobMonth,
    dob_day: received.dobDay,
    fico_score: received.ficoScore,
    first_name: received.firstName,
    middle_name: received.middleName,
    last_name: received.lastName,
    address_line_1: received.addressLine1,
    state: received.state,
    address_line_2: received.addressLine2,
    zip: received.zip,
    city: received.city,
    country: received.country,
    phone_1_area: received.phone1Area,
    phone_1_prefix: received.phone1Prefix,
    phone_1_line: received.phone1Line,
    government_id: received.govtId,
    phone_2_area: received.phone2Area,
    phone_2_prefix: received.phone2Prefix,
    phone_2_line: received.phone2Line,
    eft_account_id: received.eftAccountId,
    primary_card_holder: received.primaryCardHolder,
  };
}

/**
 * The `acct_id` path parameter: the 11-byte `ACCTSID` text right-padded with spaces, as the
 * program receives it, so a blank account number reaches the edit of COACTUPC-045 as spaces
 * rather than as an empty path segment.
 */
function accountPath(acctId: string): { acct_id: string } {
  return { acct_id: acctId.padEnd(COACTUP_LENGTHS.acctId, " ") };
}

/** The state a 200 of endpoint 6 leaves; a space, the restart, is `not-fetched`. */
function stateOfCode(code: string): CoactupKeyState {
  switch (code) {
    case "S":
    case "E":
    case "N":
    case "C":
    case "L":
    case "F":
      return code;
    default:
      return "not-fetched";
  }
}

/** The form field a `ScreenError.field` names, if any. */
function fieldOf(wireName: string | null | undefined): FieldName | undefined {
  return typeof wireName === "string" && Object.hasOwn(FIELD_OF_WIRE_NAME, wireName)
    ? FIELD_OF_WIRE_NAME[wireName]
    : undefined;
}

/**
 * The state an endpoint 6 failure leaves, as its COACTUPC row names it. The re-read of the save
 * answers as the fetch does, so its failures leave the details not fetched (COACTUPC-084 to
 * COACTUPC-098); an edit failure leaves `E` (COACTUPC-013). A failure with no audited text (the
 * API's store error, a network error) leaves the state as it was.
 */
function stateAfterSaveFailure(
  failure: components["schemas"]["ScreenError"],
  aid: components["schemas"]["CoactupcAid"],
  echoed: DetailState,
): CoactupKeyState {
  switch (failure.message) {
    case RECORD_CHANGED:
      return "S";
    case NO_CHANGES:
      return aid === "PF5" && echoed === "N" ? "S" : echoed;
    case LOCK_ACCOUNT_FAILED:
      return "L";
    case LOCK_CUSTOMER_FAILED:
      return "C";
    case UPDATE_FAILED:
      return "F";
    case UNEXPECTED_DATA:
      return "not-fetched";
    default:
      break;
  }
  const field = fieldOf(failure.field);
  if (
    field === "acctId" ||
    REREAD_FAILURE_PREFIXES.some((prefix) => failure.message.startsWith(prefix))
  ) {
    return "not-fetched";
  }
  return field === undefined ? echoed : "E";
}

/**
 * Whether a field is enterable in a state (COACTUPC-080): the account number while no details
 * are shown and in `L` and `F`; every other field in `S` and `E`, except the customer ID and the
 * country; nothing in `N` and `C`.
 */
function enterable(name: FieldName, state: CoactupKeyState): boolean {
  if (name === "acctId") {
    return state === "not-fetched" || state === "L" || state === "F";
  }
  return (state === "S" || state === "E") && !ALWAYS_PROTECTED.has(name);
}

/** The field the cursor goes to when a response names none (COACTUPC-081). */
function cursorField(state: CoactupKeyState): FieldName {
  return state === "S" || state === "E" || state === "N" ? "activeStatus" : "acctId";
}

/** The `from` path of the route state the transferring screen set, if any. */
function fromPath(state: unknown): string | undefined {
  return typeof state === "object" &&
    state !== null &&
    "from" in state &&
    typeof state.from === "string" &&
    state.from.startsWith("/")
    ? state.from
    : undefined;
}

/**
 * The hidden description of a field: its map `LENGTH`, which a screen reader does not announce
 * from `maxLength`, and for the SSN parts how the masked SSN is changed (E32).
 */
function fieldDescription(name: FieldName): string {
  const length = COACTUP_LENGTHS[name];
  const size = `Up to ${length} ${length === 1 ? "character" : "characters"}.`;
  return SSN_PARTS.has(name)
    ? `${size} The SSN is shown masked; replace the masked parts to change it.`
    : size;
}

/**
 * The key row of a state. The four labels of the map carry their key's action, and `F5=Save` and
 * `F12=Cancel` show as the last endpoint 6 answer says or, after any other event, as
 * `coactupKeys` gives them (COACTUPC-082, COACTUPC-083). ENTER carries no action, so its button
 * and the Enter key submit the form, whose handler runs ENTER's role.
 *
 * COACTUPC processes every key it does not accept as ENTER (COACTUPC-003), and in states `C`, `L`
 * and `F` starts over on every key but F3 (COACTUPC-006). `PfKeyBar` binds only the keys it is
 * given, so the nine keys the map does not label are added as hidden entries carrying their role's
 * action; their buttons are never shown.
 *
 * While `busy`, every key but F3 is disabled, as the keyboard stays locked until the program's
 * answer arrives: each would send a request or change the screen the answer replaces. F3 sends no
 * request and stays available (COACTUPC-005).
 */
function keyEntries(
  availability: PfKeyAvailability,
  handlers: PfKeyHandlers,
  served: ScreenModel["served"],
  busy: boolean,
): PfKeyBarEntry[] {
  const shows = (key: PfKey): boolean => {
    if (key === "F5" && served !== undefined) {
      return served.save;
    }
    if (key === "F12" && served !== undefined) {
      return served.cancel;
    }
    return availability.shown.has(key);
  };
  const labelled = LABELLED_KEYS.map(
    (item): PfKeyBarEntry => ({
      ...item,
      onPress: item.key === "ENTER" ? undefined : handlers[item.key],
      hidden: !shows(item.key),
      disabled: busy && item.key !== "F3",
    }),
  );
  const unlabelled = PF_KEYS.filter((key) => !LABELLED_KEY_NAMES.has(key)).map(
    (key): PfKeyBarEntry => ({
      key,
      label: key,
      dark: true,
      onPress: handlers[key],
      hidden: true,
      disabled: busy,
    }),
  );
  return [...labelled, ...unlabelled];
}

/** Props of {@link MapField}. */
type MapFieldProps = {
  readonly control: Control<CoactupValues>;
  readonly input: MapInput;
  readonly readOnly: boolean;
};

/**
 * One input of the map and the literal bound to it, each in its own `GridCell`. The `FormItem`
 * is `display: contents`, so both cells are items of the screen's `FieldGrid` while the label
 * stays bound to the input and turns red with it when a response names the field (COACTUPC-081).
 */
function MapField({ control, input, readOnly }: MapFieldProps): ReactNode {
  return (
    <FormField
      control={control}
      name={input.name}
      render={({ field }) => (
        <FormItem className="contents">
          {input.label !== undefined && (
            <GridCell row={input.label.row} col={input.label.col} span={input.label.text.length}>
              <FormLabel className={LABEL_CLASS}>{input.label.text}</FormLabel>
            </GridCell>
          )}
          <GridCell row={input.row} col={input.col} span={COACTUP_LENGTHS[input.name]}>
            <FieldInput field={field} input={input} readOnly={readOnly} />
          </GridCell>
        </FormItem>
      )}
    />
  );
}

/** Props of {@link FieldInput}. */
type FieldInputProps = {
  readonly field: ControllerRenderProps<CoactupValues, FieldName>;
  readonly input: MapInput;
  readonly readOnly: boolean;
};

/**
 * The input of one map field, inside its `FormItem`.
 *
 * `FormControl` describes the input by the item's description and, while a failure names the
 * field, by its message; both are visually hidden, because the map shows no field help and its
 * messages appear in the `ERRMSG` row. A read-only field leaves the Tab order, as the 3270 Tab key
 * skips protected fields, and still takes the cursor the program places on it (COACTUPC-081).
 */
function FieldInput({ field, input, readOnly }: FieldInputProps): ReactNode {
  const length = COACTUP_LENGTHS[input.name];
  return (
    <>
      <FormControl>
        <Input
          {...field}
          value={field.value ?? ""}
          maxLength={length}
          size={length}
          readOnly={readOnly}
          tabIndex={readOnly ? -1 : undefined}
          autoFocus={input.name === "acctId"}
          autoComplete="off"
          spellCheck={false}
          aria-label={input.ariaLabel}
          className={INPUT_CLASS}
        />
      </FormControl>
      <FormDescription className="sr-only">{fieldDescription(input.name)}</FormDescription>
      <FormMessage className="sr-only" />
    </>
  );
}

/**
 * The account update screen, map COACTUP, transaction CAUP (AAP 0.3.5).
 *
 * - ENTER while no details are shown: endpoint 5 with the account number (COACTUPC-011,
 *   COACTUPC-068); the answer fills every field, the SSN as its redacted parts.
 * - ENTER with details shown: endpoint 6 with `aid` ENTER, the change check and the edits;
 *   `F5=Save` then appears in state `N`, and F5 sends `aid` PF5 (COACTUPC-014, COACTUPC-071).
 * - F12 in `S`, `E` and `N`: the values of the fetch again, no request (COACTUPC-069).
 * - F3: the screen that transferred here, else the main menu, no request (COACTUPC-005).
 * - Any key but F3 after a finished save, in `C`, `L` and `F`: the empty screen again
 *   (COACTUPC-006).
 *
 * The SSN never leaves the server in full: the screen shows its redacted parts and sends the
 * parts only when the operator re-entered them (Cardholder Data Redaction, E32). No value of a
 * request or response is logged or stored outside the form.
 */
export function COACTUP(): ReactNode {
  const formId = useId();
  const navigate = useNavigate();
  const location = useLocation();
  const fetchAccount = useCoactupcFetch();
  const saveAccount = useCoactupcSave();
  const form = useForm<CoactupValues>({
    resolver: zodResolver(coactupSchema),
    defaultValues: INITIAL_VALUES,
  });
  const [screen, setScreen] = useState<ScreenModel>(INITIAL_SCREEN);
  // The terminal keyboard stays locked from the key that sends a request until the program's
  // answer has been shown. `locked` is the lock the key handlers read at once, so a key pressed
  // before the next render is refused too; `busy` renders it: the keys but F3 disabled, the key
  // row and the form busy, every field read-only.
  const locked = useRef(false);
  const [busy, setBusy] = useState(false);

  /** Locks the keyboard for the request a key has just been accepted to send. */
  function lock(): void {
    locked.current = true;
    setBusy(true);
  }

  /** Opens the lock: the answer has been shown, or the request was not sent. */
  function unlock(): void {
    locked.current = false;
    setBusy(false);
  }

  /**
   * Marks the field a failure names: red through the form error, `*` in it when it was received
   * blank, and the cursor on it (COACTUPC-081); without a field the cursor goes to `fallback`.
   * The SSN parts get no `*`, which would read as a part not re-entered (E32).
   */
  function markField(
    failure: components["schemas"]["ScreenError"],
    received: CoactupValues,
    fallback: FieldName,
  ): void {
    const name = fieldOf(failure.field);
    if (name === undefined) {
      form.setFocus(fallback);
      return;
    }
    form.setError(name, { type: "server", message: failure.message });
    if (!SSN_PARTS.has(name) && BLANK.test(received[name] ?? "")) {
      form.setValue(name, "*".padEnd(COACTUP_LENGTHS[name], " "));
    }
    form.setFocus(name, { shouldSelect: true });
  }

  /**
   * A length the schema rejects: zod's message in the error row, the field marked. Nothing is
   * sent, so the lock the key took opens again.
   */
  function showSchemaFailure(errors: FieldErrors<CoactupValues>): void {
    const first = MAP_INPUTS.find((input) => errors[input.name] !== undefined);
    const message = first === undefined ? undefined : errors[first.name]?.message;
    setScreen((current) => ({ ...current, message }));
    unlock();
  }

  /** The empty screen of the first entry, the account number cleared (COACTUPC-001, -006). */
  function startOver(): void {
    form.reset(INITIAL_VALUES, RESET_OPTIONS);
    setScreen(INITIAL_SCREEN);
    form.setFocus("acctId");
  }

  /** ENTER while no details are shown: endpoint 5 (COACTUPC-010, COACTUPC-011). */
  function fetchDetails(): void {
    const acctId = form.getValues("acctId");
    const parsed = coactupKeySchema.safeParse({ acctId });
    if (!parsed.success) {
      const message = parsed.error.issues.at(0)?.message;
      form.setError("acctId", { type: "schema", message });
      setScreen((current) => ({ ...current, message }));
      form.setFocus("acctId", { shouldSelect: true });
      return;
    }
    form.clearErrors();
    lock();
    const request = fetchAccount.mutateAsync(
      { path: accountPath(parsed.data.acctId) },
      {
        onSuccess: (view) => {
          const values = valuesFromView(view);
          form.reset(values, RESET_OPTIONS);
          // Endpoint 5 answers in state `S` only (COACTUPC-068).
          setScreen({
            state: "S",
            details: { values, snapshot: view.snapshot },
            info: view.info_message,
          });
          form.setFocus("activeStatus");
        },
        onError: (failure) => {
          // The account number keeps the text sent, every other field its initial value
          // (COACTUPC-073).
          setScreen({ ...INITIAL_SCREEN, message: failure.message });
          markField(failure, { ...INITIAL_VALUES, acctId: accountPath(acctId).acct_id }, "acctId");
        },
      },
    );
    // The lock opens once the answer has been shown, whichever way the request settles.
    void request.then(unlock, unlock);
  }

  /** ENTER (`aid` ENTER) or F5 (`aid` PF5) with details shown: endpoint 6. */
  function sendChanges(aid: components["schemas"]["CoactupcAid"]): void {
    // Endpoint 6 echoes the fetched snapshot; before a fetch ENTER is the fetch and F5 is ENTER.
    if (screen.state === "not-fetched") {
      return;
    }
    const { state: echoed, details } = screen;
    // The lock is taken before the schema's asynchronous check, so a key that arrives during the
    // check or while the request is out sends nothing.
    lock();
    const validation = form.handleSubmit((values) => {
      const received = receivedValues(values);
      // The screen shows what the program received (COACTUPC-078).
      for (const name of JUSTIFIED) {
        form.setValue(name, received[name]);
      }
      const request = saveAccount.mutateAsync(
        {
          path: accountPath(received.acctId),
          body: {
            aid,
            fields: requestFields(received, reenteredSsn(received, details.values)),
            snapshot: { ...details.snapshot, state: STATE_CODES[echoed] },
          },
        },
        {
          onSuccess: (result) => {
            const state = stateOfCode(result.state);
            if (state === "not-fetched") {
              startOver();
              return;
            }
            // The amounts show as the program formats them; every other field as received.
            if (result.amounts) {
              form.setValue("creditLimit", result.amounts.credit_limit);
              form.setValue("cashCreditLimit", result.amounts.cash_credit_limit);
              form.setValue("currentBalance", result.amounts.current_balance);
              form.setValue("currentCycleCredit", result.amounts.current_cycle_credit);
              form.setValue("currentCycleDebit", result.amounts.current_cycle_debit);
            }
            setScreen({
              state,
              details,
              info: result.message,
              served: { save: result.save_enabled, cancel: result.cancel_enabled },
            });
            form.setFocus(cursorField(state));
          },
          onError: (failure) => {
            const state = stateAfterSaveFailure(failure, aid, echoed);
            if (state === "not-fetched") {
              form.reset({ ...INITIAL_VALUES, acctId: received.acctId }, RESET_OPTIONS);
              setScreen({ ...INITIAL_SCREEN, message: failure.message });
              markField(failure, received, "acctId");
              return;
            }
            if (failure.message === RECORD_CHANGED) {
              // The snapshot's values again, in place of the unsaved edits (COACTUPC-116).
              form.reset(details.values, RESET_OPTIONS);
            }
            setScreen({ state, details, info: INFO_MESSAGES[state], message: failure.message });
            markField(failure, received, cursorField(state));
          },
        },
      );
      // The lock opens once the answer has been shown, whichever way the request settles.
      void request.then(unlock, unlock);
    }, showSchemaFailure)();
    // A check that fails unexpectedly sends nothing, so the lock opens; its error still surfaces.
    void validation.catch((error: unknown) => {
      unlock();
      throw error;
    });
  }

  /** ENTER's own action: the fetch, or the validation once details are shown. */
  function processEnter(): void {
    if (locked.current) {
      return;
    }
    if (screen.state === "not-fetched") {
      fetchDetails();
    } else {
      sendChanges("ENTER");
    }
  }

  /** F5 in state `N`, the only state whose role for F5 is its own (COACTUPC-071). */
  function save(): void {
    if (!locked.current) {
      sendChanges("PF5");
    }
  }

  /** F12 in `S`, `E` and `N`: the values of the fetch again, no request (COACTUPC-069). */
  function cancel(): void {
    if (locked.current || screen.state === "not-fetched") {
      return;
    }
    form.reset(screen.details.values, RESET_OPTIONS);
    setScreen({ state: "S", details: screen.details, info: INFO_MESSAGES.S });
    form.setFocus("activeStatus");
  }

  /** F3: back to the screen that transferred here, else the main menu (COACTUPC-005). */
  function exit(): void {
    navigate(fromPath(location.state) ?? PATHS.COMEN01, { state: { from: PATHS.COACTUP } });
  }

  const availability = coactupKeys(screen.state);
  const handlers = bindPfKeys(availability, {
    own: { ENTER: processEnter, F3: exit, F5: save, F12: cancel },
    reset: startOver,
  });
  const keys = keyEntries(availability, handlers, screen.served, busy);

  return (
    <ScreenShell
      transaction={SCREENS.COACTUP.transaction}
      program={SCREENS.COACTUP.program}
      title="Update Account"
      formId={formId}
      message={{ kind: "error", text: screen.message }}
      info={screen.info}
      keys={keys}
      keyOptions={KEY_OPTIONS}
      busy={busy}
    >
      <Form {...form}>
        <form
          id={formId}
          noValidate
          aria-busy={busy}
          onSubmit={(event) => {
            event.preventDefault();
            handlers.ENTER?.();
          }}
        >
          <FieldGrid>
            {ACCOUNT_INPUTS.map((input) => (
              <MapField
                key={input.name}
                control={form.control}
                input={input}
                readOnly={busy || !enterable(input.name, screen.state)}
              />
            ))}
            {/* COACTUP-057: the NEUTRAL caption of the customer block. */}
            <GridCell row={11} col={32} span={16}>
              <span
                role="heading"
                aria-level={2}
                className="flex h-7 items-center whitespace-pre text-foreground"
              >
                Customer Details
              </span>
            </GridCell>
            {CUSTOMER_INPUTS.map((input) => (
              <MapField
                key={input.name}
                control={form.control}
                input={input}
                readOnly={busy || !enterable(input.name, screen.state)}
              />
            ))}
          </FieldGrid>
        </form>
      </Form>
    </ScreenShell>
  );
}
