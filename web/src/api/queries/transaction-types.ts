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
 *   A write that fails without a usable answer, or with a 5xx, may still have
 *   been stored (`mayHaveWritten`), so the types on screen are read again.
 * - A type code in the path of /admin/transaction-types/{id} travels as its
 *   key segment (lib/key-segment.ts), so the codes `.` and `..`, which
 *   MNTTRDB2 can store and a URL would drop, still address their type. The
 *   `typeCode` of each mutation is the stored code.
 */
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { ApiError, api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";
import { encodeKeySegment } from "@/lib/key-segment";

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

/**
 * Query keys. Every write invalidates `all` once it succeeds, and once it
 * fails in a way that may follow a stored write (`mayHaveWritten`). Each such
 * invalidation reads the lists on screen again, the edit page's read of its
 * type included, except a successful delete's, which leaves that read to its
 * caller.
 */
export const tranTypeKeys = {
  all: ALL,
  lists: () => [...ALL, "list"] as const,
  list: (params: TranTypeListParams) => [...ALL, "list", params] as const,
};

/**
 * Whether a failed write may have been stored all the same: no usable answer
 * arrived (the client's deadline, a network failure, a success body that
 * could not be read, or a 2xx without one), or the answer is a 5xx, which the
 * Vite proxy also sends when the API's connection ends after its commit.
 * Every 4xx refuses the write before anything is stored: an edit (400), no
 * valid session (401), no admin (403), no type with the code (404), a
 * duplicate code or a referencing category (409).
 *
 * Each write hook's onError then invalidates `all` without returning the
 * reads: TanStack Query awaits that callback before the caller's onError, so
 * the error shown would otherwise wait for the reads.
 */
function mayHaveWritten(error: Error): boolean {
  if (!(error instanceof ApiError)) {
    return true;
  }
  return (error.status >= 200 && error.status < 300) || error.status >= 500;
}

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

/**
 * Adds a type (CTTU); the server answers 201 with the commit text. A failure
 * that may follow the insert (`mayHaveWritten`) reads the lists again.
 */
export function useTranTypeCreateMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: TranTypeCreateBody): Promise<TranTypeMessage> =>
      unwrap(await api.POST("/admin/transaction-types", { body })),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: tranTypeKeys.all }),
    onError: (error) => {
      if (mayHaveWritten(error)) {
        void queryClient.invalidateQueries({ queryKey: tranTypeKeys.all });
      }
    },
  });
}

/**
 * Updates a type's description. The query string carries `origin=list` only
 * when `origin` is given, because the server picks the program's edits and
 * texts by its presence (one endpoint serves CTLI and CTTU). A failure that
 * may follow the update (`mayHaveWritten`) reads the type and the lists
 * again; the CTTU form keeps what the user typed when the type read back is
 * unchanged.
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
            path: { id: encodeKeySegment(typeCode) },
            query: origin === undefined ? {} : { origin },
          },
          body,
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: tranTypeKeys.all }),
    onError: (error) => {
      if (mayHaveWritten(error)) {
        void queryClient.invalidateQueries({ queryKey: tranTypeKeys.all });
      }
    },
  });
}

/**
 * Deletes a type. Without `origin` it is the CTLI row delete, whose answer
 * carries no type; with `origin: "form"` it is the CTTU F4 delete, whose
 * answer carries the deleted type and `Delete successful.`. The query string
 * carries `origin` only when given, as for the update.
 *
 * A success marks every cached list stale and reads none of them again: the
 * caller reads the page it shows next. The CTLI list returns to its first
 * page and the CTTU form leaves for its key entry, so a page the screen is
 * about to discard is never requested, and a list shown later is read anew.
 * A failure that may follow the delete (`mayHaveWritten`) leaves the screen
 * where it is, so the page and the type it shows are read again at once.
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
            path: { id: encodeKeySegment(typeCode) },
            query: origin === undefined ? {} : { origin },
          },
        }),
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: tranTypeKeys.all,
        refetchType: "none",
      }),
    onError: (error) => {
      if (mayHaveWritten(error)) {
        void queryClient.invalidateQueries({ queryKey: tranTypeKeys.all });
      }
    },
  });
}
