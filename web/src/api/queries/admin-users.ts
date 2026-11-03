// Server state of user administration, CU00 to CU03: the COUSR00C list
// (UserListPage), the COUSR01C add (UserAddPage), the COUSR02C fetch and update
// (UserEditPage) and the COUSR03C fetch and delete (the delete dialog of
// UserListPage). Every route is admin-only: a `U` token gets the server's 403
// 'Admin access required', which reaches the page as an ApiError.
//
// - Errors are never caught here. Pages hand an ApiError to lib/api-errors.ts
//   with the UserAddField, UserUpdateField or UserDeleteField enum of the
//   operation.
// - Every value is sent as the page gives it, never trimmed or case-changed:
//   the admin programs store the user ID and the password as entered, and only
//   sign-on upper-cases its own input (E-20).
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components, paths } from "@/api/schema";

type Schemas = components["schemas"];

/**
 * The query of `GET /admin/users`: `start_key` (the `USRIDIN` search field
 * or a key a page returned), `direction` (`forward` for ENTER and PF8,
 * `backward` for PF7) and `page_size` (1 to 100; omitted, the 10 rows of the
 * `COUSR0A` screen).
 */
export type UserListParams = NonNullable<
  paths["/admin/users"]["get"]["parameters"]["query"]
>;

/**
 * One user as the list and `GET /admin/users/{id}` show it. It has no
 * password member (E-21).
 */
export type User = Schemas["UserSummary"];

/** The body of `POST /admin/users`, the five `COUSR1A` inputs. */
export type UserAddBody = Schemas["UserAddRequest"];

/**
 * The body of `PUT /admin/users/{id}`, the `COUSR2A` inputs. An absent
 * `password` stands for the current one, unchanged; `""` is a password field
 * left blank, which the server rejects (E-21).
 */
export type UserUpdateBody = Schemas["UserUpdateRequest"];

/** The answer to an add, update or delete: the user and the COUSR0nC text. */
export type UserMessage = Schemas["UserMessageResponse"];

const all = ["admin-users"] as const;

/** Query keys of user administration; `all` prefixes every one of them. */
export const userKeys = {
  all,
  lists: () => [...all, "list"] as const,
  list: (params: UserListParams) => [...all, "list", params] as const,
  detail: (userId: string) => [...all, "detail", userId] as const,
};

/**
 * CU00: one page of users in user-ID order. The previous page stays on
 * screen while the next one loads, as the 3270 screen kept its rows until
 * PF7 or PF8 answered. A page-boundary text such as
 * `You are already at the top of the page...` arrives in the page's
 * `message`, not as an error.
 */
export function useUsers(params: UserListParams = {}) {
  return useQuery({
    queryKey: userKeys.list(params),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/admin/users", { params: { query: params }, signal }),
      ),
    placeholderData: keepPreviousData,
  });
}

/**
 * CU02 and CU03 ENTER: one user, for the edit form or the delete dialog.
 * Idle until a user ID is given. The answer never carries the password
 * (E-21).
 */
export function useUser(userId: string) {
  return useQuery({
    queryKey: userKeys.detail(userId),
    queryFn: async ({ signal }) =>
      unwrap(
        await api.GET("/admin/users/{id}", {
          params: { path: { id: userId } },
          signal,
        }),
      ),
    enabled: userId !== "",
  });
}

/**
 * CU01: adds a user. The server answers 201 with the user and
 * `User <id> has been added ...`, which the page shows. The password goes out
 * exactly as entered, case kept (E-20).
 */
export function useUserAddMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: UserAddBody): Promise<UserMessage> =>
      unwrap(await api.POST("/admin/users", { body })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: userKeys.all }),
  });
}

/**
 * CU02 PF5: updates the user `userId`. The body goes out as the page built
 * it: the page leaves `password` out while the "Change password" switch is
 * off and sends `""` for an emptied field, and the two must reach the server
 * apart (E-21), so the key is never added, removed or converted here.
 *
 * When no field differs the server writes nothing and still answers 200 with
 * `Please modify to update ...`; that answer resolves as data, like
 * `User <id> has been updated ...`, and the page shows its `message`.
 */
export function useUserUpdateMutation(userId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (body: UserUpdateBody): Promise<UserMessage> =>
      unwrap(
        await api.PUT("/admin/users/{id}", {
          params: { path: { id: userId } },
          body,
        }),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: userKeys.all }),
  });
}

/**
 * CU03 PF5, the Confirm of the delete dialog: deletes the user given to
 * `mutate`. The answer carries the user as read before the delete and
 * `User <id> has been deleted ...`. The deleted user's cached detail is
 * dropped before the rest is invalidated, so no mounted view re-reads a user
 * that no longer exists.
 */
export function useUserDeleteMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (userId: string): Promise<UserMessage> =>
      unwrap(
        await api.DELETE("/admin/users/{id}", {
          params: { path: { id: userId } },
        }),
      ),
    onSuccess: (_data, userId) => {
      queryClient.removeQueries({ queryKey: userKeys.detail(userId) });
      return queryClient.invalidateQueries({ queryKey: userKeys.all });
    },
  });
}
