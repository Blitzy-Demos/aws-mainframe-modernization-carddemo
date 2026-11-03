/**
 * Card query hooks: the CCLI card list (`GET /cards`, COCRDLIC, map COCRDLI),
 * the CCDL card view (`GET /cards/{id}?account_id=`, COCRDSLC, map COCRDSL)
 * and the CCUP card update (`PUT /cards/{id}`, COCRDUPC, map COCRDUP), for
 * CardListPage, CardViewPage and CardEditPage.
 *
 * Every failure reaches the page as the ApiError `unwrap` raises, unchanged:
 * the page shows its COBOL `message` and maps its `fields` (`CardListField`,
 * `CardViewField`, `CardUpdateField`) through lib/api-errors.ts. Keys, card
 * and account numbers, expiry parts and the version token stay strings as the
 * server sends them.
 */
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { ApiError, api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

/**
 * The CCLI filters and page parameters: `account_id` (`ACCTSIDI X(11)`),
 * `card_number` (`CARDSIDI X(16)`), and `start_key`, `direction` and
 * `page_size` (7 rows, the `COCRDLI` screen, when omitted).
 */
export type CardListParams = NonNullable<
  paths["/cards"]["get"]["parameters"]["query"]
>;

/** A card as COCRDSL shows it, with its version token and no CVV. */
export type Card = components["schemas"]["CardResponse"];

/**
 * The CCDL answer: the card's fields and token, and `message`, the COCRDSLC
 * information line of a found card ('   Displaying requested details').
 */
export type CardViewResult = components["schemas"]["CardViewResponse"];

/** One `COCRDLI` row: `ACCTNOn`, `CRDNUMn`, `CRDSTSn`. */
export type CardSummary = components["schemas"]["CardSummary"];

/** The `COCRDUP` input fields plus the `version` of the card as fetched. */
export type CardUpdateBody = components["schemas"]["CardUpdateRequest"];

/** The card as it now stands and the COCRDUPC information text. */
export type CardUpdateResult = components["schemas"]["CardUpdateResponse"];

const all = ["cards"] as const;

/** Query keys of the card screens, from the widest prefix to one entry. */
export const cardKeys = {
  all,
  lists: () => [...all, "list"] as const,
  list: (params: CardListParams) => [...all, "list", params] as const,
  details: () => [...all, "detail"] as const,
  detail: (cardNumber: string, accountId: string) =>
    [...all, "detail", cardNumber, accountId] as const,
};

/**
 * One page of the card list (CCLI). With both filters blank COCRDLIC lists
 * every card; a filter its edits reject answers 400 at `ACCTSID` or
 * `CARDSID`. The page shown stays on screen while the next one loads, as the
 * 3270 screen did between PF7/PF8 presses.
 */
export function useCards(params: CardListParams = {}) {
  return useQuery({
    queryKey: cardKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(await api.GET("/cards", { params: { query: params }, signal })),
    placeholderData: keepPreviousData,
  });
}

/**
 * One card (CCDL), by its number and the account entered with it: COCRDSLC
 * edits both keys, so the query waits until both are non-empty. It resolves to
 * the card with its COCRDSLC information line in `message`.
 */
export function useCard(cardNumber: string, accountId: string) {
  return useQuery({
    queryKey: cardKeys.detail(cardNumber, accountId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/cards/{id}", {
          params: {
            path: { id: cardNumber },
            query: { account_id: accountId },
          },
          signal,
        }),
      ),
    enabled: cardNumber !== "" && accountId !== "",
  });
}

/**
 * The card update (CCUP). The body carries `account_id` and the `version`
 * `useCard` returned, sent unchanged; without `version` the server answers its
 * no-message 400 (E-24).
 *
 * A success refreshes every card query, because a new status or name shows on
 * the list as well as the detail. A 409 ('Record changed by some one else.
 * Please review', COCRDUPC 9300-CHECK-CHANGE-IN-REC) refreshes this card's
 * detail whatever account number it was read with, so the page holds the
 * current record and its new `version`. Both refreshes are returned, so the mutation settles only
 * once the active card queries have reloaded: the 409 then reaches the page
 * with the current record already in place, as COCRDUPC redisplays the
 * changed record with its message, and a form reset on the reloaded record
 * cannot clear the field error the page sets afterwards.
 */
export function useCardUpdateMutation(cardNumber: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: CardUpdateBody) =>
      unwrap(
        await api.PUT("/cards/{id}", {
          params: { path: { id: cardNumber } },
          body,
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: cardKeys.all }),
    onError: (error) => {
      if (error instanceof ApiError && error.status === 409) {
        return queryClient.invalidateQueries({
          queryKey: [...cardKeys.details(), cardNumber],
        });
      }
      return undefined;
    },
  });
}
