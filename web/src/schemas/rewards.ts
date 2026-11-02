/**
 * The account lookup of the rewards screen: the `{account_id}` of
 * `GET /rewards/balance/{account_id}` and `GET /rewards/history/{account_id}`.
 *
 * Rewards have no COBOL program, map or audit row. The key is the same
 * `ACCT-ID PIC 9(11)` the account view looks up [app/cpy/CVACT01Y.cpy:L5],
 * entered in a field shaped like `ACCTSID` [app/bms/COACTVW.bms], so it gets
 * the CAVW edit and its `COACTVWC` texts rather than texts of its own: blank,
 * spaces or `*` is 'No input received', and anything but 11 digits, or 11
 * zeros, is 'Account Filter must  be a non-zero 11 digit number'.
 *
 * The server is narrower in what it reports and wider in what it takes: a key
 * of 1 to 11 digits is read as an account number, and any other key, like an
 * unknown account, is answered 404 'Account not found' with no `fields`. The
 * form asks for all 11 digits, as the account view does, so one account
 * number is entered the same way on every screen. Values are never trimmed.
 */
import { z } from "zod";

import type { paths } from "@/api/schema";

import { accountKeySchema } from "./accounts";

/** The path parameter both reward endpoints take. */
type RewardsAccountId =
  paths["/rewards/balance/{account_id}"]["get"]["parameters"]["path"]["account_id"];

/**
 * The account number form of the rewards screen.
 *
 * COACTVWC 2210-EDIT-ACCOUNT — V-COACTVWC-001, V-COACTVWC-002, through the
 * account view's own field schema. It is shared rather than rebuilt from
 * `ACCOUNT_KEY_MESSAGES`, so the two lookups of the one key cannot drift
 * apart.
 */
export const rewardsAccountSchema = z.object({
  account_id: accountKeySchema.shape.account_id,
}) satisfies z.ZodType<{ account_id: RewardsAccountId }>;

/** The values of the rewards account number form. */
export type RewardsAccountValues = z.infer<typeof rewardsAccountSchema>;

/**
 * The form field of each field a reward endpoint's error names: none, since
 * the reward endpoints publish no field enum and answer with no `fields`. It
 * exists so the page passes a map to `applyApiError` as every other form does.
 */
export const rewardsAccountBmsFieldMap: Record<
  never,
  keyof RewardsAccountValues
> = {};
