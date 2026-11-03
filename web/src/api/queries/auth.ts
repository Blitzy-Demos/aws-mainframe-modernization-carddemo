// CC00 sign-on and logout (COSGN00C, map COSGN00): the mutations behind the
// sign-on page and the logout item of the user menu.
//
// - The user ID and password go to the server exactly as typed. COSGN00C
//   upper-cases both before it reads the user [app/cbl/COSGN00C.cbl:L132-136],
//   so a value stored in lower or mixed case fails sign-on as in the COBOL
//   (E-20); changing the case here would sign such a user on.
// - A failed sign-on is a 401 carrying the COSGN00C message ('Wrong Password.
//   Try again ...', 'User not found. Try again ...'). It reaches the caller as
//   the ApiError unwrap raises, which the page shows on its fields; nothing
//   here catches or reshapes it.
// - These hooks keep no token of their own: lib/session.ts holds the session.
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { api, unwrap } from "@/api/client";
import type { components } from "@/api/schema";
import { clearSession, setSession } from "@/lib/session";

/** The key root of the auth domain's mutations. */
export const authKeys = { all: ["auth"] } as const;

/**
 * The `POST /auth/login` body: `USERIDI` and `PASSWDI` as typed, at most the
 * 8 characters of their `PIC X(8)` map fields
 * [app/cpy-bms/COSGN00.CPY:L72, L78].
 */
export type LoginBody = components["schemas"]["LoginRequest"];

/**
 * The sign-on answer: the bearer token, its expiry, the user and the landing
 * route.
 */
export type LoginResult = components["schemas"]["LoginResponse"];

/**
 * The logout answer: the `CCDA-MSG-THANK-YOU` text
 * [app/cpy/CSMSG01Y.cpy:L18-19].
 */
export type LogoutResult = components["schemas"]["MessageResponse"];

/**
 * Signs on (`POST /auth/login`). On success the answer becomes the session;
 * the caller then navigates to its `landing`, `/` for a user (COMEN01C) and
 * `/admin` for an administrator (COADM01C), the XCTL targets of COSGN00C
 * [app/cbl/COSGN00C.cbl:L230-240].
 */
export function useLoginMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: [...authKeys.all, "login"],
    mutationFn: async (body: LoginBody): Promise<LoginResult> =>
      unwrap(await api.POST("/auth/login", { body })),
    onSuccess: (response) => {
      // A session also ends on a 401 or on expiry, which clear the session
      // alone, so the queries cached for the previous user are dropped before
      // the new session starts.
      queryClient.removeQueries();
      setSession(response);
    },
  });
}

/**
 * Signs off (`POST /auth/logout`). The server keeps no session, so the
 * client's session and every cached query end whether or not the call
 * succeeds; the caller may show the thank-you `message`.
 */
export function useLogoutMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: [...authKeys.all, "logout"],
    mutationFn: async (): Promise<LogoutResult> =>
      unwrap(await api.POST("/auth/logout")),
    onSettled: () => {
      clearSession();
      queryClient.clear();
    },
  });
}
