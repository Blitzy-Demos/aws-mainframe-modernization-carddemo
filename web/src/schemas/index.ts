/**
 * The registry of form schemas the OpenAPI conformance test
 * (`schemas.test.ts`) checks against `web/openapi/openapi.json`: for every
 * request body DTO, the zod schema its form submits with must reject a value
 * over the DTO's `maxLength`, a `pattern` violation and a missing required
 * member.
 *
 * This is not a barrel: pages import the domain modules directly
 * (`@/schemas/accounts`, …). The registry only names which schema stands for
 * which DTO or operation, so a DTO the API adds, renames or drops, or an
 * operation it removes, fails `tsc` here or the conformance test.
 *
 * Both maps use `satisfies` rather than a type annotation, so each entry keeps
 * its concrete schema type and the test can type its samples as
 * `z.input<RequestSchemas[K]>`.
 */
import type { z } from "zod";

import type { components, paths } from "@/api/schema";

import { accountKeySchema, accountUpdateSchema } from "./accounts";
import {
  userAddSchema,
  userDeleteKeySchema,
  userListFilterSchema,
  userUpdateSchema,
} from "./admin-users";
import { loginSchema } from "./auth";
import { authDetailKeySchema, authSummaryFilterSchema } from "./authorizations";
import { cardListFilterSchema, cardUpdateSchema, cardViewKeySchema } from "./cards";
import { paymentSchema } from "./payments";
import { reportRequestSchema } from "./reports";
import { rewardRuleSchema } from "./reward-rules";
import { rewardsAccountSchema } from "./rewards";
import {
  tranTypeCreateSchema,
  tranTypeListFilterSchema,
  tranTypeUpdateSchema,
} from "./transaction-types";
import {
  transactionAddFormSchema,
  transactionListFilterSchema,
  transactionViewKeySchema,
} from "./transactions";

/**
 * Every `requestBody` schema of the API's operations. `PUT /authorizations/{id}`
 * (fraud toggle), the DELETEs and `POST /auth/logout` take no body.
 */
export const REQUEST_DTO_NAMES = [
  "LoginRequest",
  "AccountUpdateRequest",
  "CardUpdateRequest",
  "TransactionAddRequest",
  "PaymentRequest",
  "UserAddRequest",
  "UserUpdateRequest",
  "TranTypeCreateRequest",
  "TranTypeUpdateRequest",
  "RewardRuleRequest",
] as const satisfies readonly (keyof components["schemas"])[];

/** The name of a request body DTO in `components["schemas"]`. */
export type RequestDtoName = (typeof REQUEST_DTO_NAMES)[number];

/**
 * The schema each request body's form submits with. Where a module layers
 * cross-field rules over the plain field schema, the layered one is
 * registered: `transactionAddFormSchema` puts COTRN02C's key-field edits
 * ahead of the data-field edits.
 */
export const requestSchemas = {
  LoginRequest: loginSchema,
  AccountUpdateRequest: accountUpdateSchema,
  CardUpdateRequest: cardUpdateSchema,
  TransactionAddRequest: transactionAddFormSchema,
  PaymentRequest: paymentSchema,
  UserAddRequest: userAddSchema,
  UserUpdateRequest: userUpdateSchema,
  TranTypeCreateRequest: tranTypeCreateSchema,
  TranTypeUpdateRequest: tranTypeUpdateSchema,
  RewardRuleRequest: rewardRuleSchema,
} as const satisfies Record<RequestDtoName, z.ZodTypeAny>;

/** The concrete schema type of each registered request body form. */
export type RequestSchemas = typeof requestSchemas;

type HttpMethod = "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace";

/**
 * `"<METHOD> <path>"` of every operation `paths` declares, in the API's
 * `{param}` path syntax and without the `/api` proxy prefix. openapi-typescript
 * writes each method a path does not serve as `?: never`, which is excluded.
 */
type OperationKey = {
  [P in keyof paths & string]: {
    [M in HttpMethod & keyof paths[P]]-?: [paths[P][M]] extends [undefined]
      ? never
      : `${Uppercase<M>} ${P}`;
  }[HttpMethod & keyof paths[P]];
}[keyof paths & string];

/**
 * The key and filter forms of operations whose input travels in the path or
 * the query string, keyed by operation. A key naming an operation the API no
 * longer declares fails `tsc`.
 */
export const queryFormSchemas = {
  "GET /cards": cardListFilterSchema,
  "GET /cards/{id}": cardViewKeySchema,
  "GET /accounts/{id}": accountKeySchema,
  "GET /transactions": transactionListFilterSchema,
  "GET /transactions/{id}": transactionViewKeySchema,
  "GET /reports/transactions": reportRequestSchema,
  "GET /authorizations": authSummaryFilterSchema,
  "GET /authorizations/{id}": authDetailKeySchema,
  "GET /admin/users": userListFilterSchema,
  "DELETE /admin/users/{id}": userDeleteKeySchema,
  "GET /admin/transaction-types": tranTypeListFilterSchema,
  "GET /rewards/balance/{account_id}": rewardsAccountSchema,
} as const satisfies Partial<Record<OperationKey, z.ZodTypeAny>>;
