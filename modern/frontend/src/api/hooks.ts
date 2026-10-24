import { useMutation, useQuery } from "@tanstack/react-query";
import { api } from "./client";
import type { components, paths } from "./schema";

/*
 * One TanStack Query hook per API operation, the only way a screen reaches the API. Every hook is
 * keyed by its operation's `operationId`, and every request and response is typed by the
 * generated `paths`.
 *
 * The two menus are queries, loaded when their screen mounts, as COMEN01C and COADM01C send their
 * option list on first entry. Every other operation is a mutation the screen fires on a key press
 * (ENTER, F5, F7/F8, F10, PF4), because each program reads or writes only when an AID key arrives;
 * a screen whose program reads on first entry fires its mutation from an effect.
 *
 * Variables carry only the `path`, `query` and `body` an operation takes, typed inline by indexed
 * access into the generated types, never by a hand-written payload type (Generated API Types
 * Only). Values pass through unchanged: dates stay as generated, amounts stay strings, and the
 * opaque server values (`card_ref`, `page_key`, `first_key`, `last_key`, `auth_key`, `state`) are
 * forwarded exactly as a response carried them.
 *
 * Nothing here logs, stores or copies a request or response. Data lives only in TanStack's
 * in-memory cache under the hook's own key, so the full card number that endpoint 8 discloses to
 * an admin reaches no other key or sink (Cardholder Data Redaction); the one other store,
 * `failureStatus`, holds only the HTTP status of a thrown failure. Identity travels only as the
 * bearer token `client.ts` adds; no hook sends the signed-on user's ID or type.
 */

declare module "@tanstack/react-query" {
  /**
   * Every query and mutation error is typed as the API's `ScreenError` body, which the message
   * row shows. That is the body the generated error responses of every operation declare, with
   * two kinds of exception:
   * - A `ScreenError` with optional members beside `message` and `field`: `coactvwc_view`
   *   (endpoint 4) answers 404 and 500 with an `AccountViewError`, which may add the `account`
   *   and `customer` blocks, and `coactupc_fetch` (endpoint 5) with an `AccountUpdateViewError`,
   *   which may add `state` and `details`. Its message reads as a `ScreenError`'s; COACTVW's
   *   `isViewError` and COACTUP's `isFetchError` guards narrow the thrown body before they read
   *   the members beside it.
   * - A page in place of the `ScreenError`: `cotrtlic_list` (endpoint 24) answers a
   *   `TranTypeListResponse` on 400 and 500, and `cotrtlic_list_actions` (endpoint 25) a
   *   `TranTypeListActionResult` on 400 and 500 and always on 404; their 401, 403 and 422 are
   *   `ScreenError`s. Such a page can lack a string `message`, so a consumer of those two hooks
   *   narrows the thrown body before it reads it as a `ScreenError`, as COTRTLI's `isListPage` and
   *   `isActionResult` guards do.
   *
   * The three failures without a body carry a string `message` too: the network `TypeError` and
   * the `TimeoutError` `DOMException` of the request deadline, both of which `client.ts` gives its
   * one fixed `NO_ANSWER_MESSAGE` whatever the browser, and the status `Error` of `settle`, which
   * names the HTTP status. `errorStatus` gives the HTTP status of every failure `settle` threw.
   */
  interface Register {
    defaultError: components["schemas"]["ScreenError"];
  }
}

/**
 * The HTTP status of each failure `settle` threw, keyed by the thrown value itself: the error
 * body, or the status `Error`. Kept beside the value rather than on it, so the generated body
 * reaches the screen unchanged, with no key its schema does not declare; weakly held, so an
 * entry lives no longer than the error it describes.
 */
const failureStatus = new WeakMap<object, number>();

/**
 * Resolves an openapi-fetch call to its success body, or throws its failure.
 *
 * A failure whose body is a JSON object throws that object unchanged: the body the operation's
 * generated error response declares for the status, the `ScreenError` with the program's message
 * and, for endpoints 4 and 5, the members `Register` names beside it, or, for endpoints 24 and 25,
 * the COTRTLIC page `Register` names, which COTRTLI tells apart with its `isListPage` and
 * `isActionResult` guards; the dev proxy's 502 while the API cannot be
 * reached is such an object too, whose `message` names CARDDEMO_API_BIND. Any other failure has
 * no object to throw: openapi-fetch hands over an empty or non-JSON error body as its text, and a
 * declared-empty one as `undefined`. Such a failure, and a success without a body, reject with an
 * `Error` naming the HTTP status, so the screen always reads a string `message` and never reads
 * `undefined` as data. Either way the response's status is recorded for `errorStatus`.
 */
async function settle<Data>(
  pending: Promise<{ data?: Data; error?: unknown; response: Response }>,
): Promise<Data> {
  const { data, error, response } = await pending;
  if (typeof error === "object" && error !== null) {
    failureStatus.set(error, response.status);
    throw error;
  }
  if (data === undefined) {
    const failure = new Error(`HTTP ${response.status} ${response.statusText}`.trimEnd());
    failureStatus.set(failure, response.status);
    throw failure;
  }
  return data;
}

/**
 * The HTTP status of the response a hook's error came from, for a screen whose program reacts to
 * the outcome class and not only to the message: COCRDSLC marks both inputs on a not-found read
 * (404) but only the account on a file error (500). `undefined` for anything `settle` did not
 * throw, such as the network `TypeError` or the request deadline's `TimeoutError` of `client.ts`,
 * when no response arrived.
 */
export function errorStatus(error: unknown): number | undefined {
  return typeof error === "object" && error !== null ? failureStatus.get(error) : undefined;
}

/** Endpoint 1, CC00 / COSGN00C: ENTER signs on and returns the token and the next route. */
export function useCosgn00cSignon() {
  return useMutation({
    mutationKey: ["cosgn00c_signon"],
    mutationFn: (vars: { body: components["schemas"]["SignonRequest"] }) =>
      settle(api.POST("/api/v1/signon", { body: vars.body })),
  });
}

/** Endpoint 2, CM00 / COMEN01C: the main menu's option table, loaded when the screen mounts. */
export function useComen01cMainMenu() {
  return useQuery({
    queryKey: ["comen01c_main_menu"],
    queryFn: ({ signal }) => settle(api.GET("/api/v1/menus/main", { signal })),
  });
}

/** Endpoint 3, CA00 / COADM01C: the admin menu's option table, loaded when the screen mounts. */
export function useCoadm01cAdminMenu() {
  return useQuery({
    queryKey: ["coadm01c_admin_menu"],
    queryFn: ({ signal }) => settle(api.GET("/api/v1/menus/admin", { signal })),
  });
}

/**
 * Endpoint 4, CAVW / COACTVWC: ENTER shows the account and its customer. A 404 or 500 may carry
 * a block beside its message (`AccountViewError`).
 */
export function useCoactvwcView() {
  return useMutation({
    mutationKey: ["coactvwc_view"],
    mutationFn: (vars: {
      path: paths["/api/v1/accounts/{acct_id}/view"]["get"]["parameters"]["path"];
    }) => settle(api.GET("/api/v1/accounts/{acct_id}/view", { params: { path: vars.path } })),
  });
}

/**
 * Endpoint 5, CAUP / COACTUPC: ENTER fetches the account for update, with its snapshot. A 404 or
 * 500 may carry state `S` and the details beside its message (`AccountUpdateViewError`).
 */
export function useCoactupcFetch() {
  return useMutation({
    mutationKey: ["coactupc_fetch"],
    mutationFn: (vars: {
      path: paths["/api/v1/accounts/{acct_id}"]["get"]["parameters"]["path"];
    }) => settle(api.GET("/api/v1/accounts/{acct_id}", { params: { path: vars.path } })),
  });
}

/** Endpoint 6, CAUP / COACTUPC: ENTER validates the changes, F5 saves them. */
export function useCoactupcSave() {
  return useMutation({
    mutationKey: ["coactupc_save"],
    mutationFn: (vars: {
      path: paths["/api/v1/accounts/{acct_id}"]["put"]["parameters"]["path"];
      body: components["schemas"]["AccountUpdateRequest"];
    }) =>
      settle(
        api.PUT("/api/v1/accounts/{acct_id}", { params: { path: vars.path }, body: vars.body }),
      ),
  });
}

/**
 * Endpoint 7, CCLI / COCRDLIC: ENTER applies the account and card filters, F7/F8 page with the
 * previous response's paging state echoed unchanged. The query is copied field by field and the
 * page-size parameter is never sent, so the server's default, the map's 7 rows, decides the page
 * (Bounded Pagination).
 */
export function useCocrdlicList() {
  return useMutation({
    mutationKey: ["cocrdlic_list"],
    mutationFn: (vars: { query: paths["/api/v1/cards"]["get"]["parameters"]["query"] }) =>
      settle(
        api.GET("/api/v1/cards", {
          params: {
            query: {
              acct_id: vars.query?.acct_id,
              card_num: vars.query?.card_num,
              direction: vars.query?.direction,
              page_no: vars.query?.page_no,
              first_key: vars.query?.first_key,
              last_key: vars.query?.last_key,
              has_next_page: vars.query?.has_next_page,
              last_page_shown: vars.query?.last_page_shown,
              reread: vars.query?.reread,
            },
          },
        }),
      ),
  });
}

/**
 * Endpoint 8, CCDL / COCRDSLC: ENTER shows one card, named by the typed number or by the
 * `card_ref` of a list row. The only operation that may return a full card number (to an admin).
 */
export function useCocrdslcView() {
  return useMutation({
    mutationKey: ["cocrdslc_view"],
    mutationFn: (vars: { query: paths["/api/v1/cards/view"]["get"]["parameters"]["query"] }) =>
      settle(api.GET("/api/v1/cards/view", { params: { query: vars.query } })),
  });
}

/** Endpoint 9, CCUP / COCRDUPC: ENTER fetches the card for update, with its snapshot. */
export function useCocrdupcFetch() {
  return useMutation({
    mutationKey: ["cocrdupc_fetch"],
    mutationFn: (vars: { query: paths["/api/v1/cards/edit"]["get"]["parameters"]["query"] }) =>
      settle(api.GET("/api/v1/cards/edit", { params: { query: vars.query } })),
  });
}

/** Endpoint 10, CCUP / COCRDUPC: ENTER validates the changes, F5 saves them. */
export function useCocrdupcSave() {
  return useMutation({
    mutationKey: ["cocrdupc_save"],
    mutationFn: (vars: {
      path: paths["/api/v1/cards/{card_ref}"]["put"]["parameters"]["path"];
      body: components["schemas"]["CardUpdateRequest"];
    }) =>
      settle(api.PUT("/api/v1/cards/{card_ref}", { params: { path: vars.path }, body: vars.body })),
  });
}

/**
 * Endpoint 11, CT00 / COTRN00C: ENTER searches from the start ID, F7/F8 page with the previous
 * response's paging state echoed unchanged. The query is copied field by field and the page-size
 * parameter is never sent, so the server's default, the map's 10 rows, decides the page (Bounded
 * Pagination).
 */
export function useCotrn00cList() {
  return useMutation({
    mutationKey: ["cotrn00c_list"],
    mutationFn: (vars: { query: paths["/api/v1/transactions"]["get"]["parameters"]["query"] }) =>
      settle(
        api.GET("/api/v1/transactions", {
          params: {
            query: {
              start_tran_id: vars.query?.start_tran_id,
              direction: vars.query?.direction,
              entry_key: vars.query?.entry_key,
              first_key: vars.query?.first_key,
              last_key: vars.query?.last_key,
              page_no: vars.query?.page_no,
              has_next_page: vars.query?.has_next_page,
            },
          },
        }),
      ),
  });
}

/** Endpoint 12, CT01 / COTRN01C: ENTER shows one transaction. */
export function useCotrn01cView() {
  return useMutation({
    mutationKey: ["cotrn01c_view"],
    mutationFn: (vars: {
      path: paths["/api/v1/transactions/{tran_id}"]["get"]["parameters"]["path"];
    }) => settle(api.GET("/api/v1/transactions/{tran_id}", { params: { path: vars.path } })),
  });
}

/** Endpoint 13, CT02 / COTRN02C: ENTER validates or adds, F5 copies the last transaction. */
export function useCotrn02cAdd() {
  return useMutation({
    mutationKey: ["cotrn02c_add"],
    mutationFn: (vars: { body: components["schemas"]["TranAddRequest"] }) =>
      settle(api.POST("/api/v1/transactions", { body: vars.body })),
  });
}

/** Endpoint 14, CR00 / CORPT00C: ENTER with confirmation writes the report job deck. */
export function useCorpt00cSubmit() {
  return useMutation({
    mutationKey: ["corpt00c_submit"],
    mutationFn: (vars: { body: components["schemas"]["ReportRequest"] }) =>
      settle(api.POST("/api/v1/reports/transactions", { body: vars.body })),
  });
}

/** Endpoint 15, CB00 / COBIL00C: ENTER shows the balance, pays it on `Y`, clears on `N`. */
export function useCobil00cPay() {
  return useMutation({
    mutationKey: ["cobil00c_pay"],
    mutationFn: (vars: { body: components["schemas"]["BillPaymentRequest"] }) =>
      settle(api.POST("/api/v1/bill-payments", { body: vars.body })),
  });
}

/**
 * Endpoint 16, CU00 / COUSR00C: ENTER searches from the start user ID, F7/F8 page with the
 * previous response's paging state echoed unchanged. The query is copied field by field and the
 * page-size parameter is never sent, so the server's default, the map's 10 rows, decides the page
 * (Bounded Pagination).
 */
export function useCousr00cList() {
  return useMutation({
    mutationKey: ["cousr00c_list"],
    mutationFn: (vars: { query: paths["/api/v1/users"]["get"]["parameters"]["query"] }) =>
      settle(
        api.GET("/api/v1/users", {
          params: {
            query: {
              start_user_id: vars.query?.start_user_id,
              page_key: vars.query?.page_key,
              direction: vars.query?.direction,
              first_key: vars.query?.first_key,
              last_key: vars.query?.last_key,
              page_no: vars.query?.page_no,
              has_next_page: vars.query?.has_next_page,
            },
          },
        }),
      ),
  });
}

/** Endpoint 17, CU01 / COUSR01C: ENTER adds the user. */
export function useCousr01cAdd() {
  return useMutation({
    mutationKey: ["cousr01c_add"],
    mutationFn: (vars: { body: components["schemas"]["UserAddRequest"] }) =>
      settle(api.POST("/api/v1/users", { body: vars.body })),
  });
}

/** Endpoint 18, CU02 / COUSR02C and CU03 / COUSR03C: ENTER fetches the user, never a password. */
export function useCousr02cFetch() {
  return useMutation({
    mutationKey: ["cousr02c_fetch"],
    mutationFn: (vars: { path: paths["/api/v1/users/{user_id}"]["get"]["parameters"]["path"] }) =>
      settle(api.GET("/api/v1/users/{user_id}", { params: { path: vars.path } })),
  });
}

/** Endpoint 19, CU02 / COUSR02C: F5 updates the user; an omitted password stays unchanged. */
export function useCousr02cUpdate() {
  return useMutation({
    mutationKey: ["cousr02c_update"],
    mutationFn: (vars: {
      path: paths["/api/v1/users/{user_id}"]["put"]["parameters"]["path"];
      body: components["schemas"]["UserUpdateRequest"];
    }) =>
      settle(api.PUT("/api/v1/users/{user_id}", { params: { path: vars.path }, body: vars.body })),
  });
}

/** Endpoint 20, CU03 / COUSR03C: F5 deletes the user. */
export function useCousr03cDelete() {
  return useMutation({
    mutationKey: ["cousr03c_delete"],
    mutationFn: (vars: {
      path: paths["/api/v1/users/{user_id}"]["delete"]["parameters"]["path"];
    }) => settle(api.DELETE("/api/v1/users/{user_id}", { params: { path: vars.path } })),
  });
}

/**
 * Endpoint 21, CPVS / COPAUS0C: ENTER shows the account's summary and first page, F7/F8 page from
 * a key the screen kept. The query is copied field by field and the page-size parameter is never
 * sent, so the server's default, the map's 5 rows, decides the page (Bounded Pagination).
 */
export function useCopaus0cSummary() {
  return useMutation({
    mutationKey: ["copaus0c_summary"],
    mutationFn: (vars: {
      path: paths["/api/v1/accounts/{acct_id}/authorizations"]["get"]["parameters"]["path"];
      query: paths["/api/v1/accounts/{acct_id}/authorizations"]["get"]["parameters"]["query"];
    }) =>
      settle(
        api.GET("/api/v1/accounts/{acct_id}/authorizations", {
          params: {
            path: vars.path,
            query: {
              page_key: vars.query?.page_key,
              direction: vars.query?.direction,
            },
          },
        }),
      ),
  });
}

/**
 * Endpoint 22, CPVD / COPAUS1C: without `direction` ENTER shows the authorization the path names;
 * `direction` `forward` is F8, the account's next authorization.
 */
export function useCopaus1cDetail() {
  return useMutation({
    mutationKey: ["copaus1c_detail"],
    mutationFn: (vars: {
      path: paths["/api/v1/authorizations/{acct_id}/{auth_key}"]["get"]["parameters"]["path"];
      query: paths["/api/v1/authorizations/{acct_id}/{auth_key}"]["get"]["parameters"]["query"];
    }) =>
      settle(
        api.GET("/api/v1/authorizations/{acct_id}/{auth_key}", {
          params: { path: vars.path, query: vars.query },
        }),
      ),
  });
}

/** Endpoint 23, CPVD / COPAUS1C and COPAUS2C: F5 marks or unmarks the authorization as fraud. */
export function useCopaus1cToggleFraud() {
  return useMutation({
    mutationKey: ["copaus1c_toggle_fraud"],
    mutationFn: (vars: {
      path: paths["/api/v1/authorizations/{acct_id}/{auth_key}/fraud"]["post"]["parameters"]["path"];
    }) =>
      settle(
        api.POST("/api/v1/authorizations/{acct_id}/{auth_key}/fraud", {
          params: { path: vars.path },
        }),
      ),
  });
}

/**
 * Endpoint 24, CTLI / COTRTLIC: the first page on entry, F7/F8 under the displayed filters. The
 * query is copied field by field and the page-size parameter is never sent, so the server's
 * default, the map's 7 rows, decides the page (Bounded Pagination).
 */
export function useCotrtlicList() {
  return useMutation({
    mutationKey: ["cotrtlic_list"],
    mutationFn: (vars: {
      query: paths["/api/v1/transaction-types"]["get"]["parameters"]["query"];
    }) =>
      settle(
        api.GET("/api/v1/transaction-types", {
          params: {
            query: {
              type_filter: vars.query?.type_filter,
              desc_filter: vars.query?.desc_filter,
              page_key: vars.query?.page_key,
              direction: vars.query?.direction,
            },
          },
        }),
      ),
  });
}

/**
 * Endpoint 25, CTLI / COTRTLIC: ENTER edits the filters, selections and descriptions and asks for
 * confirmation; F10 commits the pending update or delete. `state` is the previous response's,
 * echoed unchanged. The body is copied field by field and the page-size field is never sent, so
 * the server's default, the map's 7 rows, decides the page (Bounded Pagination).
 */
export function useCotrtlicListActions() {
  return useMutation({
    mutationKey: ["cotrtlic_list_actions"],
    mutationFn: (vars: { body: components["schemas"]["TranTypeListActionRequest"] }) =>
      settle(
        api.POST("/api/v1/transaction-types/list-actions", {
          body: {
            aid: vars.body.aid,
            type_filter: vars.body.type_filter,
            desc_filter: vars.body.desc_filter,
            rows: vars.body.rows,
            state: vars.body.state,
          },
        }),
      ),
  });
}

/** Endpoint 26, CTTU / COTRTUPC: ENTER fetches the transaction type. */
export function useCotrtupcFetch() {
  return useMutation({
    mutationKey: ["cotrtupc_fetch"],
    mutationFn: (vars: {
      path: paths["/api/v1/transaction-types/{type_code}"]["get"]["parameters"]["path"];
    }) =>
      settle(api.GET("/api/v1/transaction-types/{type_code}", { params: { path: vars.path } })),
  });
}

/** Endpoint 27, CTTU / COTRTUPC: F5 saves (updates, or inserts when absent), F4 deletes. */
export function useCotrtupcAction() {
  return useMutation({
    mutationKey: ["cotrtupc_action"],
    mutationFn: (vars: {
      path: paths["/api/v1/transaction-types/{type_code}/actions"]["post"]["parameters"]["path"];
      body: components["schemas"]["TranTypeActionRequest"];
    }) =>
      settle(
        api.POST("/api/v1/transaction-types/{type_code}/actions", {
          params: { path: vars.path },
          body: vars.body,
        }),
      ),
  });
}
