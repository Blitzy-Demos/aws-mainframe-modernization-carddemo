/**
 * Query hooks of the admin transaction type screens over the Db2 master
 * `transaction_types`: the CTLI list (COTRTLIC, map COTRTLI) with its inline
 * description update and delete, and the CTTU add and update (COTRTUPC, map
 * COTRTUP).
 *
 * | Hook | Request | Screen action |
 * |---|---|---|
 * | `useTranTypes` | `GET /admin/transaction-types` | CTLI ENTER, F7, F8; CTTU read of the type being edited |
 * | `useTranTypeCreateMutation` | `POST /admin/transaction-types` | CTTU F5 on a new code |
 * | `useTranTypeUpdateMutation` | `PUT /admin/transaction-types/{id}` | CTTU F5 on a code on file; CTLI `U` row update with `origin: "list"` |
 * | `useTranTypeDeleteMutation` | `DELETE /admin/transaction-types/{id}` | CTLI `D` row delete, confirmed with F10; CTTU F4 delete, confirmed with F4, with `origin: "form"` |
 *
 * - Every endpoint is admin-only. The writes reach the Db2 master alone, so
 *   reports show an edit only after `batch seed --only tranextr`.
 * - The API has no single-type read: the CTTU edit page lists with the
 *   `type_code` filter and takes the matching row.
 * - COTRTUP F6 has no endpoint of its own: its add is the F5 add on a new
 *   code, `useTranTypeCreateMutation`.
 * - Failures reach the caller as the client's `ApiError`, unchanged; the COBOL
 *   texts of a success (`message`, and `info` on an inline update) are data.
 */
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

/**
 * The list filters and paging of `GET /admin/transaction-types`: `type_code`
 * (`TRTYPE`), `description` (`TRDESC`), `start_key`, `direction` and
 * `page_size` (seven rows, the COTRTLI screen, when absent).
 */
export type TranTypeListParams = NonNullable<
  paths["/admin/transaction-types"]["get"]["parameters"]["query"]
>;

/**
 * The `origin` query value of `PUT /admin/transaction-types/{id}`: `"list"`
 * selects the COTRTLIC inline-update edits and texts; leaving it out selects
 * the COTRTUPC maintenance edits and texts.
 */
export type TranTypeUpdateOrigin = NonNullable<
  NonNullable<
    paths["/admin/transaction-types/{id}"]["put"]["parameters"]["query"]
  >["origin"]
>;

/**
 * The `origin` query value of `DELETE /admin/transaction-types/{id}`:
 * `"form"` selects the COTRTUPC F4 delete with its key edits and texts;
 * leaving it out selects the COTRTLIC row delete and its texts.
 */
export type TranTypeDeleteOrigin = NonNullable<
  NonNullable<
    paths["/admin/transaction-types/{id}"]["delete"]["parameters"]["query"]
  >["origin"]
>;

/** One transaction type: a COTRTLI row and the COTRTUP details. */
export type TranType = components["schemas"]["TranTypeResponse"];

/** The CTTU add, `TRTYPCD` and `TRTYDSC` as entered. */
export type TranTypeCreateBody = components["schemas"]["TranTypeCreateRequest"];

/** The new description, `TRTYDSC` (CTTU) or a row's `TRTYPDn` (CTLI). */
export type TranTypeUpdateBody = components["schemas"]["TranTypeUpdateRequest"];

/** The answer of every write: the stored type, the COBOL text, the info line. */
export type TranTypeMessage = components["schemas"]["TranTypeMessageResponse"];

/** The variables of `useTranTypeUpdateMutation`. */
export type TranTypeUpdateVariables = {
  /** The type code `{id}`, as listed or as fetched for the edit page. */
  typeCode: string;
  body: TranTypeUpdateBody;
  /** `"list"` for the CTLI inline update; absent for the CTTU screen. */
  origin?: TranTypeUpdateOrigin;
};

/** The variables of `useTranTypeDeleteMutation`. */
export type TranTypeDeleteVariables = {
  /** The type code `{id}`, as listed or as fetched for the edit page. */
  typeCode: string;
  /** `"form"` for the CTTU F4 delete; absent for the CTLI row delete. */
  origin?: TranTypeDeleteOrigin;
};

const ALL = ["admin-transaction-types"] as const;

/** Query keys; every write invalidates `all`. */
export const tranTypeKeys = {
  all: ALL,
  lists: () => [...ALL, "list"] as const,
  list: (params: TranTypeListParams) => [...ALL, "list", params] as const,
};

/**
 * One page of transaction types in ascending code order, with `next_key` and
 * `prev_key` for F8 and F7 and the COTRTLIC page-boundary text in `message`.
 * The previous page stays on screen while the next one loads.
 */
export function useTranTypes(params: TranTypeListParams = {}) {
  return useQuery({
    queryKey: tranTypeKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/admin/transaction-types", {
          params: { query: params },
          signal,
        }),
      ),
    placeholderData: keepPreviousData,
  });
}

/** Adds a type (CTTU); the server answers 201 with the commit text. */
export function useTranTypeCreateMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: TranTypeCreateBody): Promise<TranTypeMessage> =>
      unwrap(await api.POST("/admin/transaction-types", { body })),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: tranTypeKeys.all }),
  });
}

/**
 * Updates a type's description. The query string carries `origin=list` only
 * when `origin` is given, because the server picks the program's edits and
 * texts by its presence (one endpoint serves CTLI and CTTU).
 */
export function useTranTypeUpdateMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      typeCode,
      body,
      origin,
    }: TranTypeUpdateVariables): Promise<TranTypeMessage> =>
      unwrap(
        await api.PUT("/admin/transaction-types/{id}", {
          params: {
            path: { id: typeCode },
            query: origin === undefined ? {} : { origin },
          },
          body,
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: tranTypeKeys.all }),
  });
}

/**
 * Deletes a type. Without `origin` it is the CTLI row delete, whose answer
 * carries no type; with `origin: "form"` it is the CTTU F4 delete, whose
 * answer carries the deleted type and `Delete successful.`. The query string
 * carries `origin` only when given, as for the update.
 */
export function useTranTypeDeleteMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      typeCode,
      origin,
    }: TranTypeDeleteVariables): Promise<TranTypeMessage> =>
      unwrap(
        await api.DELETE("/admin/transaction-types/{id}", {
          params: {
            path: { id: typeCode },
            query: origin === undefined ? {} : { origin },
          },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: tranTypeKeys.all }),
  });
}
