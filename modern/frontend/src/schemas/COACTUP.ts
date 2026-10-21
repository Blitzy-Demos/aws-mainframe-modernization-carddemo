import { z } from "zod";

/**
 * Client-side form schemas of map COACTUP (`CACTUPA`): the account update screen of
 * transaction CAUP, program COACTUPC, route `/accounts/update`.
 *
 * Each schema repeats only the map `LENGTH` of the fields the screen sends, the 43 input
 * rows `modern/audit/screens.md` audits as Layer `both` (COACTUP-013, -016, -019, -021,
 * -023, -026, -029, -031, -033, -036, -039, -041, -043, -046, -049, -052, -055, -059, -062,
 * -064, -066, -069, -071, -073, -076, -081, -083, -085, -087, -090, -092, -095, -098, -101,
 * -104, -105, -106, -109, -112, -113, -114, -117 and -120). The map declares no `NUM`,
 * `VALIDN` or `PICIN`, and a length rejection carries zod's own message because no row
 * records one.
 *
 * Neither schema repeats a COACTUPC edit. `modern/audit/online/COACTUPC.md` has no `both`
 * row: the account edit of the fetch (COACTUPC-045, COACTUPC-046), the 24 field edits of
 * the validation in their order (COACTUPC-015 to COACTUPC-038, with the yes/no, amount,
 * name, FICO, state, ZIP, phone and SSN edits of COACTUPC-048 to COACTUPC-066 and the
 * state/ZIP cross-field edit COACTUPC-067), the date-part edits of `CSUTLDPY`
 * (COACTUPC-117 to COACTUPC-131) and the change check (COACTUPC-040 to COACTUPC-044) are
 * all Layer `server`. The endpoint composes each message from the field name, picks the
 * first failure in program order and sets the state and the info line, so a client-side
 * check would show a message the program never sends at that point. Calendar validity is `HALTED(H5)` and server-owned (`online/CSUTLDTC.md`, whose
 * rows are all `server`).
 *
 * The date, phone and SSN parts and the five amounts are raw screen text, never typed
 * values: COACTUPC edits them as screen parts and assembles the typed dates itself (the
 * raw-edit-input exception of Typed Dates and Timestamps), and money stays the string the
 * operator typed (R1). The left zero fill of the `JUSTIFY=(RIGHT)` date and phone parts
 * (COACTUPC-008) is applied by the screen when it builds the request; the schema validates
 * the text as typed.
 */

/** Map `LENGTH` of each field the screen sends (`app/bms/COACTUP.bms`), for `maxLength`. */
export const COACTUP_LENGTHS = {
  /** `ACCTSID`, screens.md COACTUP-013 (initial cursor, `IC`). */
  acctId: 11,
  /** `ACSTTUS`, screens.md COACTUP-016. */
  activeStatus: 1,
  /** `OPNYEAR`, screens.md COACTUP-019. */
  openYear: 4,
  /** `OPNMON`, screens.md COACTUP-021. */
  openMonth: 2,
  /** `OPNDAY`, screens.md COACTUP-023. */
  openDay: 2,
  /** `ACRDLIM`, screens.md COACTUP-026. */
  creditLimit: 15,
  /** `EXPYEAR`, screens.md COACTUP-029. */
  expiryYear: 4,
  /** `EXPMON`, screens.md COACTUP-031. */
  expiryMonth: 2,
  /** `EXPDAY`, screens.md COACTUP-033. */
  expiryDay: 2,
  /** `ACSHLIM`, screens.md COACTUP-036. */
  cashCreditLimit: 15,
  /** `RISYEAR`, screens.md COACTUP-039. */
  reissueYear: 4,
  /** `RISMON`, screens.md COACTUP-041. */
  reissueMonth: 2,
  /** `RISDAY`, screens.md COACTUP-043. */
  reissueDay: 2,
  /** `ACURBAL`, screens.md COACTUP-046. */
  currentBalance: 15,
  /** `ACRCYCR`, screens.md COACTUP-049. */
  currentCycleCredit: 15,
  /** `AADDGRP`, screens.md COACTUP-052. */
  groupId: 10,
  /** `ACRCYDB`, screens.md COACTUP-055. */
  currentCycleDebit: 15,
  /** `ACSTNUM`, screens.md COACTUP-059. */
  custId: 9,
  /** `ACTSSN1`, screens.md COACTUP-062. */
  ssnPart1: 3,
  /** `ACTSSN2`, screens.md COACTUP-064. */
  ssnPart2: 2,
  /** `ACTSSN3`, screens.md COACTUP-066. */
  ssnPart3: 4,
  /** `DOBYEAR`, screens.md COACTUP-069. */
  dobYear: 4,
  /** `DOBMON`, screens.md COACTUP-071. */
  dobMonth: 2,
  /** `DOBDAY`, screens.md COACTUP-073. */
  dobDay: 2,
  /** `ACSTFCO`, screens.md COACTUP-076. */
  ficoScore: 3,
  /** `ACSFNAM`, screens.md COACTUP-081. */
  firstName: 25,
  /** `ACSMNAM`, screens.md COACTUP-083. */
  middleName: 25,
  /** `ACSLNAM`, screens.md COACTUP-085. */
  lastName: 25,
  /** `ACSADL1`, screens.md COACTUP-087. */
  addressLine1: 50,
  /** `ACSSTTE`, screens.md COACTUP-090. */
  state: 2,
  /** `ACSADL2`, screens.md COACTUP-092. */
  addressLine2: 50,
  /** `ACSZIPC`, screens.md COACTUP-095. */
  zip: 5,
  /** `ACSCITY`, screens.md COACTUP-098. */
  city: 50,
  /** `ACSCTRY`, screens.md COACTUP-101. */
  country: 3,
  /** `ACSPH1A`, screens.md COACTUP-104. */
  phone1Area: 3,
  /** `ACSPH1B`, screens.md COACTUP-105. */
  phone1Prefix: 3,
  /** `ACSPH1C`, screens.md COACTUP-106. */
  phone1Line: 4,
  /** `ACSGOVT`, screens.md COACTUP-109. */
  govtId: 20,
  /** `ACSPH2A`, screens.md COACTUP-112. */
  phone2Area: 3,
  /** `ACSPH2B`, screens.md COACTUP-113. */
  phone2Prefix: 3,
  /** `ACSPH2C`, screens.md COACTUP-114. */
  phone2Line: 4,
  /** `ACSEFTC`, screens.md COACTUP-117. */
  eftAccountId: 10,
  /** `ACSPFLG`, screens.md COACTUP-120. */
  primaryCardHolder: 1,
} as const;

/**
 * ENTER while no account is fetched: the account number, sent as the `acct_id` path
 * parameter of endpoint 5 (`GET /api/v1/accounts/{acct_id}`). Only this field is read on
 * the fetch (COACTUPC-010); its blank, non-numeric and all-zero edits are the `server` rows
 * COACTUPC-045 and COACTUPC-046.
 */
export const coactupKeySchema = z.object({
  acctId: z.string().max(COACTUP_LENGTHS.acctId),
});

/** Values of the fetch, exactly as typed. */
export type CoactupKeyValues = z.infer<typeof coactupKeySchema>;

/**
 * The whole screen once an account is fetched: ENTER (validate) and F5 (save) of endpoint 6
 * (`PUT /api/v1/accounts/{acct_id}`), whose `fields` carry this text.
 *
 * SSN (Cardholder Data Redaction, E32): after a fetch the three parts show `***`, `**` and
 * the last four digits, never the stored SSN (COACTUPC-076). The parts are request input
 * only when the operator re-enters them, so each is optional and may be empty; omitted
 * means unchanged, and the server edits, compares and writes the stored SSN in that case
 * (COACTUPC-064). No message of this file carries an SSN value.
 */
export const coactupSchema = z.object({
  acctId: z.string().max(COACTUP_LENGTHS.acctId),
  activeStatus: z.string().max(COACTUP_LENGTHS.activeStatus),
  openYear: z.string().max(COACTUP_LENGTHS.openYear),
  openMonth: z.string().max(COACTUP_LENGTHS.openMonth),
  openDay: z.string().max(COACTUP_LENGTHS.openDay),
  creditLimit: z.string().max(COACTUP_LENGTHS.creditLimit),
  expiryYear: z.string().max(COACTUP_LENGTHS.expiryYear),
  expiryMonth: z.string().max(COACTUP_LENGTHS.expiryMonth),
  expiryDay: z.string().max(COACTUP_LENGTHS.expiryDay),
  cashCreditLimit: z.string().max(COACTUP_LENGTHS.cashCreditLimit),
  reissueYear: z.string().max(COACTUP_LENGTHS.reissueYear),
  reissueMonth: z.string().max(COACTUP_LENGTHS.reissueMonth),
  reissueDay: z.string().max(COACTUP_LENGTHS.reissueDay),
  currentBalance: z.string().max(COACTUP_LENGTHS.currentBalance),
  currentCycleCredit: z.string().max(COACTUP_LENGTHS.currentCycleCredit),
  groupId: z.string().max(COACTUP_LENGTHS.groupId),
  currentCycleDebit: z.string().max(COACTUP_LENGTHS.currentCycleDebit),
  custId: z.string().max(COACTUP_LENGTHS.custId),
  ssnPart1: z.string().max(COACTUP_LENGTHS.ssnPart1).optional(),
  ssnPart2: z.string().max(COACTUP_LENGTHS.ssnPart2).optional(),
  ssnPart3: z.string().max(COACTUP_LENGTHS.ssnPart3).optional(),
  dobYear: z.string().max(COACTUP_LENGTHS.dobYear),
  dobMonth: z.string().max(COACTUP_LENGTHS.dobMonth),
  dobDay: z.string().max(COACTUP_LENGTHS.dobDay),
  ficoScore: z.string().max(COACTUP_LENGTHS.ficoScore),
  firstName: z.string().max(COACTUP_LENGTHS.firstName),
  middleName: z.string().max(COACTUP_LENGTHS.middleName),
  lastName: z.string().max(COACTUP_LENGTHS.lastName),
  addressLine1: z.string().max(COACTUP_LENGTHS.addressLine1),
  state: z.string().max(COACTUP_LENGTHS.state),
  addressLine2: z.string().max(COACTUP_LENGTHS.addressLine2),
  zip: z.string().max(COACTUP_LENGTHS.zip),
  city: z.string().max(COACTUP_LENGTHS.city),
  country: z.string().max(COACTUP_LENGTHS.country),
  phone1Area: z.string().max(COACTUP_LENGTHS.phone1Area),
  phone1Prefix: z.string().max(COACTUP_LENGTHS.phone1Prefix),
  phone1Line: z.string().max(COACTUP_LENGTHS.phone1Line),
  govtId: z.string().max(COACTUP_LENGTHS.govtId),
  phone2Area: z.string().max(COACTUP_LENGTHS.phone2Area),
  phone2Prefix: z.string().max(COACTUP_LENGTHS.phone2Prefix),
  phone2Line: z.string().max(COACTUP_LENGTHS.phone2Line),
  eftAccountId: z.string().max(COACTUP_LENGTHS.eftAccountId),
  primaryCardHolder: z.string().max(COACTUP_LENGTHS.primaryCardHolder),
});

/** Values of the update form, exactly as typed. */
export type CoactupValues = z.infer<typeof coactupSchema>;
