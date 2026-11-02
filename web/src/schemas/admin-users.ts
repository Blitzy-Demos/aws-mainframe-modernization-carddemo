// Client edits of user administration, CU00 to CU03: the COUSR00C list start key, the
// COUSR01C add form, the COUSR02C edit form and the COUSR03C delete key. They mirror the
// width of each BMS field, the ASCII text the API requires of every member and key (with
// NUL characters in a name or new user ID only as LOW-VALUES), and the empty-field edits
// of the validation audit so a form never submits what the API rejects first; the API
// stays authoritative. Its record-state
// and I/O texts ('User ID already exist...', 'User ID NOT found...', 'Unable to ...') and
// 'Please modify to update ...', which needs the stored user and argon2 verification
// (E-21), come from the server only.
//
// Every value is kept as entered: never trimmed and never case-changed. The admin
// programs store the user ID and the password exactly as typed [app/cbl/COUSR01C.cbl:L154-158]
// [app/cbl/COUSR02C.cbl:L227-229], and sign-on upper-cases only its own input (E-20).
import { z } from "zod";

import type { components, paths } from "@/api/schema";

// COUSR01C PROCESS-ENTER-KEY, one EVALUATE in screen order [app/cbl/COUSR01C.cbl:L117-151].
export const USER_ADD_MESSAGES = {
  firstNameEmpty: "First Name can NOT be empty...", // V-COUSR01C-001
  lastNameEmpty: "Last Name can NOT be empty...", // V-COUSR01C-002
  userIdEmpty: "User ID can NOT be empty...", // V-COUSR01C-003
  passwordEmpty: "Password can NOT be empty...", // V-COUSR01C-004
  userTypeEmpty: "User Type can NOT be empty...", // V-COUSR01C-005
} as const;

// COUSR02C UPDATE-USER-INFO, one EVALUATE in screen order [app/cbl/COUSR02C.cbl:L179-213].
export const USER_UPDATE_MESSAGES = {
  userIdEmpty: "User ID can NOT be empty...", // V-COUSR02C-004
  firstNameEmpty: "First Name can NOT be empty...", // V-COUSR02C-005
  lastNameEmpty: "Last Name can NOT be empty...", // V-COUSR02C-006
  passwordEmpty: "Password can NOT be empty...", // V-COUSR02C-007
  userTypeEmpty: "User Type can NOT be empty...", // V-COUSR02C-008
} as const;

// COUSR03C DELETE-USER-INFO [app/cbl/COUSR03C.cbl:L176-186].
export const USER_DELETE_MESSAGES = {
  userIdEmpty: "User ID can NOT be empty...", // V-COUSR03C-004
} as const;

// The BMS input widths, PIC X(n) of the symbolic maps: USRIDINI of COUSR0A, COUSR2A and
// COUSR3A and USERIDI of COUSR1A X(8), FNAMEI and LNAMEI X(20), PASSWDI X(8), USRTYPEI X(1)
// [app/cpy-bms/COUSR00.CPY:L66] [app/cpy-bms/COUSR01.CPY:L60-84]
// [app/cpy-bms/COUSR02.CPY:L60-84] [app/cpy-bms/COUSR03.CPY:L60]. They equal the maxLength
// of UserAddRequest and UserUpdateRequest, and the 8-character limit the API applies to
// the start_key parameter and the {id} path segment.
const WIDTH = {
  userId: 8,
  firstName: 20,
  lastName: 20,
  password: 8,
  userType: 1,
} as const;

// ASCII text, the OpenAPI `pattern` of the password and user type members, the {id} path
// segment and the start_key parameter. The COUSR0nC programs move each value byte for byte
// into its PIC X(n) field of SEC-USER-DATA, and a 3270 field holds one byte per character,
// so the API refuses a character above U+007F with no message, as it refuses an
// over-length value: the issue keeps zod's default regex text. Spaces and NUL characters
// are ASCII, so a blank field still gets its empty-field text.
const ASCII_TEXT = /^[\x00-\x7F]*$/;

// Storable ASCII text, the OpenAPI `pattern` of the names and the new user's ID, which the
// rules store as text: PostgreSQL text cannot hold a NUL character (E-07), so the API
// refuses NUL characters with no message, as it refuses a non-ASCII value, except as the
// LOW-VALUES its empty-field edit answers. COUSR02C reads NUL characters alone as blank,
// whatever their number; COUSR01C first moves the entry into its PIC X(n) field, padded
// with spaces, so only exactly n of them stay LOW-VALUES (WIDTH.firstName and
// WIDTH.lastName 20, WIDTH.userId 8). The password keeps ASCII_TEXT: it is stored as its
// argon2id hash, never as text.
const STORABLE_TEXT = /^(?:\x00+|[\x01-\x7F]*)$/;
const STORABLE_ADD_NAME = /^(?:\x00{20}|[\x01-\x7F]*)$/;
const STORABLE_ADD_USER_ID = /^(?:\x00{8}|[\x01-\x7F]*)$/;

// `= SPACES OR LOW-VALUES` as a browser field meets it: a field left unentered arrives as
// the empty string, and one holding only spaces is blank. Any other character, a tab
// included, is entered text. A field of NUL characters cannot be typed, and the API's own
// edit still answers one.
const BLANK = /^ *$/;

// Adds the audited empty-field issue of `field` when its value is blank. A value longer
// than its field already carries zod's too_big issue and gets no second one, because the
// API answers its width check before any COBOL edit runs; a non-ASCII value is never
// blank. Callers invoke it in the program's EVALUATE order, so the issues follow the
// audit Order.
function rejectBlank<K extends string>(
  ctx: z.RefinementCtx,
  values: Partial<Record<NoInfer<K>, string>>,
  field: K,
  width: number,
  message: string,
): void {
  const entered = values[field] ?? "";
  if (entered.length <= width && BLANK.test(entered)) {
    ctx.addIssue({ code: z.ZodIssueCode.custom, message, path: [field] });
  }
}

// ---------------------------------------------------------------------------------------
// CU00: GET /admin/users?start_key=
// ---------------------------------------------------------------------------------------

// COUSR00C has no edit of USRIDIN: it is the key the browse starts at, and a blank key
// starts at the top of the file. Every COUSR00C text (V-COUSR00C-001 to V-COUSR00C-009) is
// a page-boundary or browse outcome the API returns.
export const userListFilterSchema = z.object({
  // An empty search field is no start key, so the first page is requested.
  start_key: z
    .string()
    .max(WIDTH.userId)
    .regex(ASCII_TEXT)
    .optional()
    .transform((key) => (key === "" ? undefined : key)),
}) satisfies z.ZodType<
  Pick<NonNullable<paths["/admin/users"]["get"]["parameters"]["query"]>, "start_key">
>;

export type UserListFilterValues = z.infer<typeof userListFilterSchema>;

// Every COUSR00C path moves -1 to USRIDINL, the search field.
export const userListBmsFieldMap: Record<
  components["schemas"]["UserListField"],
  keyof UserListFilterValues
> = {
  USRIDIN: "start_key",
};

// ---------------------------------------------------------------------------------------
// CU01: POST /admin/users
// ---------------------------------------------------------------------------------------

// A missing member is the empty field to the API (LOW-VALUES), so it gets the same text.
export const userAddSchema = z
  .object({
    first_name: z
      .string({ required_error: USER_ADD_MESSAGES.firstNameEmpty })
      .max(WIDTH.firstName)
      .regex(STORABLE_ADD_NAME),
    last_name: z
      .string({ required_error: USER_ADD_MESSAGES.lastNameEmpty })
      .max(WIDTH.lastName)
      .regex(STORABLE_ADD_NAME),
    user_id: z
      .string({ required_error: USER_ADD_MESSAGES.userIdEmpty })
      .max(WIDTH.userId)
      .regex(STORABLE_ADD_USER_ID),
    password: z
      .string({ required_error: USER_ADD_MESSAGES.passwordEmpty })
      .max(WIDTH.password)
      .regex(ASCII_TEXT),
    // Not limited to A and U: COUSR01C edits only for blank and stores any other value as
    // entered, and the OpenAPI member declares no enum, only the ASCII pattern.
    usr_type: z
      .string({ required_error: USER_ADD_MESSAGES.userTypeEmpty })
      .max(WIDTH.userType)
      .regex(ASCII_TEXT),
  })
  .superRefine((values, ctx) => {
    rejectBlank(ctx, values, "first_name", WIDTH.firstName, USER_ADD_MESSAGES.firstNameEmpty);
    rejectBlank(ctx, values, "last_name", WIDTH.lastName, USER_ADD_MESSAGES.lastNameEmpty);
    rejectBlank(ctx, values, "user_id", WIDTH.userId, USER_ADD_MESSAGES.userIdEmpty);
    rejectBlank(ctx, values, "password", WIDTH.password, USER_ADD_MESSAGES.passwordEmpty);
    rejectBlank(ctx, values, "usr_type", WIDTH.userType, USER_ADD_MESSAGES.userTypeEmpty);
  }) satisfies z.ZodType<components["schemas"]["UserAddRequest"]>;

export type UserAddFormValues = z.infer<typeof userAddSchema>;

export const userAddBmsFieldMap: Record<
  components["schemas"]["UserAddField"],
  keyof UserAddFormValues
> = {
  FNAME: "first_name",
  LNAME: "last_name",
  USERID: "user_id",
  PASSWD: "password",
  USRTYPE: "usr_type",
};

// ---------------------------------------------------------------------------------------
// CU02: PUT /admin/users/{id}
// ---------------------------------------------------------------------------------------

// The edit form. `user_id` is the protected key USRIDIN, taken from the route and sent as
// the {id} path segment, not in the body. The current password is never shown, because
// only its argon2 hash is stored (E-21): the password field starts empty behind a "Change
// password" switch. Switched off, the password is left out of the request and stays
// unchanged; switched on, it is required, as the COBOL empty edit requires the field.
export const userUpdateSchema = z
  .object({
    user_id: z
      .string({ required_error: USER_UPDATE_MESSAGES.userIdEmpty })
      .max(WIDTH.userId)
      .regex(ASCII_TEXT),
    first_name: z
      .string({ required_error: USER_UPDATE_MESSAGES.firstNameEmpty })
      .max(WIDTH.firstName)
      .regex(STORABLE_TEXT),
    last_name: z
      .string({ required_error: USER_UPDATE_MESSAGES.lastNameEmpty })
      .max(WIDTH.lastName)
      .regex(STORABLE_TEXT),
    change_password: z.boolean().default(false),
    password: z.string().max(WIDTH.password).regex(ASCII_TEXT).optional(),
    // Any non-blank ASCII value, as for the add form: COUSR02C edits only for blank.
    usr_type: z
      .string({ required_error: USER_UPDATE_MESSAGES.userTypeEmpty })
      .max(WIDTH.userType)
      .regex(ASCII_TEXT),
  })
  .superRefine((values, ctx) => {
    rejectBlank(ctx, values, "user_id", WIDTH.userId, USER_UPDATE_MESSAGES.userIdEmpty);
    rejectBlank(ctx, values, "first_name", WIDTH.firstName, USER_UPDATE_MESSAGES.firstNameEmpty);
    rejectBlank(ctx, values, "last_name", WIDTH.lastName, USER_UPDATE_MESSAGES.lastNameEmpty);
    // A password left out stands for the stored one and passes the edit (E-21).
    if (values.change_password) {
      rejectBlank(ctx, values, "password", WIDTH.password, USER_UPDATE_MESSAGES.passwordEmpty);
    }
    rejectBlank(ctx, values, "usr_type", WIDTH.userType, USER_UPDATE_MESSAGES.userTypeEmpty);
  }) satisfies z.ZodType<components["schemas"]["UserUpdateRequest"]>;

export type UserUpdateFormInput = z.input<typeof userUpdateSchema>;
export type UserUpdateFormValues = z.infer<typeof userUpdateSchema>;

// The PUT body of a validated edit form. Without the switch the `password` key is omitted,
// the API's "unchanged" (E-21); `null` and the empty string are never sent for it, since
// the empty string is a password the user left blank. With the switch on, the password
// goes exactly as typed, and a field never typed into is that blank password.
export function toUserUpdateRequest(
  values: UserUpdateFormValues,
): components["schemas"]["UserUpdateRequest"] {
  const { first_name, last_name, usr_type } = values;
  if (!values.change_password) {
    return { first_name, last_name, usr_type };
  }
  return { first_name, last_name, password: values.password ?? "", usr_type };
}

export const userUpdateBmsFieldMap: Record<
  components["schemas"]["UserUpdateField"],
  keyof UserUpdateFormValues
> = {
  USRIDIN: "user_id",
  FNAME: "first_name",
  LNAME: "last_name",
  PASSWD: "password",
  USRTYPE: "usr_type",
};

// ---------------------------------------------------------------------------------------
// CU03: DELETE /admin/users/{id}
// ---------------------------------------------------------------------------------------

export const userDeleteKeySchema = z
  .object({
    user_id: z
      .string({ required_error: USER_DELETE_MESSAGES.userIdEmpty })
      .max(WIDTH.userId)
      .regex(ASCII_TEXT),
  })
  .superRefine((values, ctx) => {
    rejectBlank(ctx, values, "user_id", WIDTH.userId, USER_DELETE_MESSAGES.userIdEmpty);
  }) satisfies z.ZodType<{
  user_id: paths["/admin/users/{id}"]["delete"]["parameters"]["path"]["id"];
}>;

export type UserDeleteKeyValues = z.infer<typeof userDeleteKeySchema>;

// The delete dialog holds only the key, so FNAME, where COUSR03C puts the cursor when the
// read for update or the delete fails (V-COUSR03C-005, V-COUSR03C-007), lands on it too.
export const userDeleteBmsFieldMap: Record<
  components["schemas"]["UserDeleteField"],
  keyof UserDeleteKeyValues
> = {
  USRIDIN: "user_id",
  FNAME: "user_id",
};
