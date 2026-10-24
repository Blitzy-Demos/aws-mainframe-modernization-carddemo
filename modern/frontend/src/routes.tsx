import type { ComponentType, ReactNode } from "react";
import {
  matchPath,
  Navigate,
  Outlet,
  replace,
  type RouteObject,
  useLoaderData,
  useLocation,
  useNavigate,
  useNavigation,
} from "react-router";
import { RequireSignon, refusedTarget } from "@/auth/RequireSignon";
import { getSession, isAdmin } from "@/auth/session";
import type { PfKeyBarEntry } from "@/components/screen/PfKeyBar";
import { ScreenShell, TransferPendingContext } from "@/components/screen/ScreenShell";

/**
 * Route table of the 21 CardDemo screens, one per BMS map (AAP 0.3.5).
 *
 * `SCREENS` is the registry. It holds each map's route, the README transaction that ran it, the
 * COBOL program that served it (README.md:269-294; `modern/audit/screens.md`) and the guard in
 * front of it. `PATHS`, `PROGRAM_ROUTES` and `routes` are derived from it, so every route is
 * written once.
 *
 * Screens import this module for their header values and for the paths they navigate to, so it
 * never imports a screen statically. Each screen's module is imported by its route's `loader`,
 * once the route's guard admits the visitor (`screenLoader`). A screen that cannot be loaded shows
 * `ScreenUnavailable`, the root route's error boundary, which the main chunk carries.
 */

/**
 * The guard in front of a screen:
 * - `none`: reachable without a session (sign-on only);
 * - `signon`: needs a stored session, else the route's loader, before it imports the screen, and
 *   `RequireSignon`, while the screen shows, send the user to `/signon`;
 * - `admin`: also needs user type `A`, else the route's loader and `RequireSignon admin` send the
 *   user to `/menu`.
 *
 * The guards only hide screens. The API takes identity from the verified token alone, and its
 * `AdminUser` extractor answers 403 on every admin endpoint; that is the control (AAP 0.5.1).
 */
export type Gate = "none" | "signon" | "admin";

/** Map name → route, README transaction, COBOL program and guard of each screen. */
export const SCREENS = {
  COSGN00: {
    path: "/signon",
    transaction: "CC00",
    program: "COSGN00C",
    gate: "none",
  },
  COMEN01: {
    path: "/menu",
    transaction: "CM00",
    program: "COMEN01C",
    gate: "signon",
  },
  COADM01: {
    path: "/admin",
    transaction: "CA00",
    program: "COADM01C",
    gate: "admin",
  },
  COACTVW: {
    path: "/accounts/view",
    transaction: "CAVW",
    program: "COACTVWC",
    gate: "signon",
  },
  COACTUP: {
    path: "/accounts/update",
    transaction: "CAUP",
    program: "COACTUPC",
    gate: "signon",
  },
  COCRDLI: {
    path: "/cards",
    transaction: "CCLI",
    program: "COCRDLIC",
    gate: "signon",
  },
  COCRDSL: {
    path: "/cards/view",
    transaction: "CCDL",
    program: "COCRDSLC",
    gate: "signon",
  },
  COCRDUP: {
    path: "/cards/update",
    transaction: "CCUP",
    program: "COCRDUPC",
    gate: "signon",
  },
  COTRN00: {
    path: "/transactions",
    transaction: "CT00",
    program: "COTRN00C",
    gate: "signon",
  },
  COTRN01: {
    path: "/transactions/view",
    transaction: "CT01",
    program: "COTRN01C",
    gate: "signon",
  },
  COTRN02: {
    path: "/transactions/add",
    transaction: "CT02",
    program: "COTRN02C",
    gate: "signon",
  },
  CORPT00: {
    path: "/reports",
    transaction: "CR00",
    program: "CORPT00C",
    gate: "signon",
  },
  COBIL00: {
    path: "/bill-payment",
    transaction: "CB00",
    program: "COBIL00C",
    gate: "signon",
  },
  COUSR00: {
    path: "/users",
    transaction: "CU00",
    program: "COUSR00C",
    gate: "admin",
  },
  COUSR01: {
    path: "/users/add",
    transaction: "CU01",
    program: "COUSR01C",
    gate: "admin",
  },
  COUSR02: {
    path: "/users/update",
    transaction: "CU02",
    program: "COUSR02C",
    gate: "admin",
  },
  COUSR03: {
    path: "/users/delete",
    transaction: "CU03",
    program: "COUSR03C",
    gate: "admin",
  },
  COPAU00: {
    path: "/authorizations",
    transaction: "CPVS",
    program: "COPAUS0C",
    gate: "signon",
  },
  COPAU01: {
    path: "/authorizations/detail",
    transaction: "CPVD",
    program: "COPAUS1C",
    gate: "signon",
  },
  COTRTLI: {
    path: "/transaction-types",
    transaction: "CTLI",
    program: "COTRTLIC",
    gate: "admin",
  },
  COTRTUP: {
    path: "/transaction-types/maintain",
    transaction: "CTTU",
    program: "COTRTUPC",
    gate: "admin",
  },
} as const satisfies Record<
  string,
  { path: string; transaction: string; program: string; gate: Gate }
>;

/** The map name of a screen, such as `COSGN00`. */
export type ScreenName = keyof typeof SCREENS;

/**
 * Map name → route path; `PATHS.COMEN01` is `/menu`.
 *
 * `Object.fromEntries` types its result with a string index. The entries are built from every
 * key of `SCREENS`, so the result holds exactly the literal paths the cast names.
 */
export const PATHS = Object.freeze(
  Object.fromEntries(Object.entries(SCREENS).map(([name, screen]) => [name, screen.path])),
) as { readonly [K in ScreenName]: (typeof SCREENS)[K]["path"] };

/**
 * COBOL program → route path, for all 21 programs.
 *
 * The menus look up the program of each `MenuResponse` option here, because COMEN01C and
 * COADM01C transfer to the option's program (`COMEN01C-014`, `COMEN01C-018`, `COADM01C-017`).
 * A program the table lacks reads as `undefined`, and the menu screen shows the server's
 * message for it rather than navigating.
 */
export const PROGRAM_ROUTES: Readonly<Record<string, string>> = Object.freeze(
  Object.fromEntries(Object.values(SCREENS).map((screen) => [screen.program, screen.path])),
);

/** The component of one loaded screen module, as its route's loader returns it. */
type ScreenModule = { Component: ComponentType };

/** Loads one screen module and names the component its route renders. */
type ScreenLoader = () => Promise<ScreenModule>;

/**
 * One module import per screen. `@/screens/<MAP>` exports the function component `<MAP>`.
 *
 * Every specifier is a string literal, so Vite emits one chunk per screen. A template string or a
 * glob would also match the screens' `*.test.tsx` files. `satisfies` rejects a missing or an extra
 * map name.
 */
const LOADERS = {
  COSGN00: async () => ({ Component: (await import("@/screens/COSGN00")).COSGN00 }),
  COMEN01: async () => ({ Component: (await import("@/screens/COMEN01")).COMEN01 }),
  COADM01: async () => ({ Component: (await import("@/screens/COADM01")).COADM01 }),
  COACTVW: async () => ({ Component: (await import("@/screens/COACTVW")).COACTVW }),
  COACTUP: async () => ({ Component: (await import("@/screens/COACTUP")).COACTUP }),
  COCRDLI: async () => ({ Component: (await import("@/screens/COCRDLI")).COCRDLI }),
  COCRDSL: async () => ({ Component: (await import("@/screens/COCRDSL")).COCRDSL }),
  COCRDUP: async () => ({ Component: (await import("@/screens/COCRDUP")).COCRDUP }),
  COTRN00: async () => ({ Component: (await import("@/screens/COTRN00")).COTRN00 }),
  COTRN01: async () => ({ Component: (await import("@/screens/COTRN01")).COTRN01 }),
  COTRN02: async () => ({ Component: (await import("@/screens/COTRN02")).COTRN02 }),
  CORPT00: async () => ({ Component: (await import("@/screens/CORPT00")).CORPT00 }),
  COBIL00: async () => ({ Component: (await import("@/screens/COBIL00")).COBIL00 }),
  COUSR00: async () => ({ Component: (await import("@/screens/COUSR00")).COUSR00 }),
  COUSR01: async () => ({ Component: (await import("@/screens/COUSR01")).COUSR01 }),
  COUSR02: async () => ({ Component: (await import("@/screens/COUSR02")).COUSR02 }),
  COUSR03: async () => ({ Component: (await import("@/screens/COUSR03")).COUSR03 }),
  COPAU00: async () => ({ Component: (await import("@/screens/COPAU00")).COPAU00 }),
  COPAU01: async () => ({ Component: (await import("@/screens/COPAU01")).COPAU01 }),
  COTRTLI: async () => ({ Component: (await import("@/screens/COTRTLI")).COTRTLI }),
  COTRTUP: async () => ({ Component: (await import("@/screens/COTRTUP")).COTRTUP }),
} satisfies Record<ScreenName, ScreenLoader>;

/**
 * `LOADERS` under a string index, for lookup by the keys `Object.entries(SCREENS)` yields;
 * `satisfies` above guarantees a loader for each of them.
 */
const LOADER_BY_NAME: Readonly<Record<string, ScreenLoader>> = LOADERS;

/**
 * The route `loader` of the screen `name` behind `gate`.
 *
 * It decides the gate first, through the guard's own `refusedTarget`, and redirects a refused
 * visitor before `import()` runs, so the screen's chunk is never requested. A route's `lazy` could
 * not do this: React Router starts every matched route's `lazy` with the navigation, before any
 * parent can redirect, and keeps its promise for the life of the page, a rejection included. The
 * redirect is `replace`, so the refused URL leaves the history as with `RequireSignon`'s
 * `<Navigate replace>`; a plain `redirect` from the first load would be pushed after it.
 *
 * A loader is called again whenever a navigation enters its route and keeps no earlier result, so
 * a failed import shows `ScreenUnavailable` and the next navigation to the screen imports it anew.
 */
function screenLoader(name: string, gate: Gate): ScreenLoader {
  const load = LOADER_BY_NAME[name];
  return async () => {
    const target = gate === "none" ? null : refusedTarget(gate === "admin");
    if (target !== null) {
      throw replace(target);
    }
    return load();
  };
}

/** Renders the screen component its route's loader imported. */
function LoadedScreen(): ReactNode {
  const { Component } = useLoaderData<ScreenModule>();
  return <Component />;
}

/** The routes of the screens behind `gate`, in registry order. */
function screenRoutes(gate: Gate): RouteObject[] {
  return Object.entries(SCREENS)
    .filter(([, screen]) => screen.gate === gate)
    .map(([name, screen]) => ({
      path: screen.path,
      loader: screenLoader(name, screen.gate),
      Component: LoadedScreen,
    }));
}

/** The screen title of {@link ScreenUnavailable}. */
const UNAVAILABLE_TITLE = "Screen Unavailable";

/** The error row of {@link ScreenUnavailable}; it fits the 78 characters of a map's `ERRMSG`. */
const UNAVAILABLE_MESSAGE = "Unable to load the screen. Press ENTER to try again or F3 to go back.";

/**
 * The menu F3 returns to from {@link ScreenUnavailable}: the one sign-on opens for the stored user
 * (`/admin` for user type `A`, `/menu` for any other, app/cbl/COSGN00C.cbl:230-240), or sign-on
 * itself when nobody is signed on. Read when the key is pressed, so it follows the session.
 */
function owningMenu(): string {
  const session = getSession();
  if (session === null) {
    return PATHS.COSGN00;
  }
  return isAdmin(session) ? PATHS.COADM01 : PATHS.COMEN01;
}

/**
 * The root route's error boundary: the frame shown in place of a screen whose route failed, most
 * often because its module could not be fetched (the network dropped before the first visit), or
 * because the screen threw while rendering. It is a `ScreenShell` like every screen, headed with
 * the transaction and program of the route that failed, looked up in `SCREENS` by the location's
 * path, with the reason on the error row and two keys:
 * - ENTER navigates to the same location again, its route state included, which runs the route's
 *   loader anew; the attempt replaces the history entry rather than adding one. While it runs, the
 *   key row is busy and ENTER is refused.
 * - F3 goes to the owning menu (`owningMenu`), from where the screen can be chosen again.
 *
 * The error itself is neither shown nor logged: the message row holds only this module's text.
 */
function ScreenUnavailable(): ReactNode {
  const location = useLocation();
  const navigate = useNavigate();
  const loading = useNavigation().state !== "idle";
  const failed = Object.values(SCREENS).find(
    (screen) => matchPath(screen.path, location.pathname) !== null,
  );
  const keys: PfKeyBarEntry[] = [
    {
      key: "ENTER",
      label: "ENTER=Retry",
      dark: false,
      disabled: loading,
      onPress: () => {
        const { pathname, search, hash, state: routeState } = location;
        void navigate({ pathname, search, hash }, { replace: true, state: routeState });
      },
    },
    {
      key: "F3",
      label: "F3=Back",
      dark: false,
      onPress: () => {
        void navigate(owningMenu());
      },
    },
  ];
  return (
    <ScreenShell
      transaction={failed?.transaction ?? ""}
      program={failed?.program ?? ""}
      title={UNAVAILABLE_TITLE}
      message={{ kind: "error", text: UNAVAILABLE_MESSAGE }}
      keys={keys}
      busy={loading}
    >
      {null}
    </ScreenShell>
  );
}

/**
 * Shown while the first matched screen's module loads. Data mode warns at start-up without one;
 * the module is a local chunk, so nothing is shown meanwhile.
 */
function NoHydrateFallback(): null {
  return null;
}

/**
 * The root route's element: the matched screen, told through `ScreenShell`'s
 * {@link TransferPendingContext} whether the router is moving to another location. Every
 * navigation of this table runs at most a screen route's loader, which checks the route's guard
 * and imports the screen's module, and no route has an action, so any state but `idle` is a
 * transfer under way.
 *
 * {@link ScreenUnavailable}, the root's error boundary, renders in place of this element and so
 * outside the context; it takes the navigation state itself for its keys.
 */
export function ScreenOutlet(): ReactNode {
  const transferPending = useNavigation().state !== "idle";
  return (
    <TransferPendingContext value={transferPending}>
      <Outlet />
    </TransferPendingContext>
  );
}

/**
 * The route tree for `createBrowserRouter`. `/` and any unknown path go to sign-on, where every
 * CardDemo conversation starts (transaction CC00). The routes carry no query parameters: list
 * keys and selections travel in the route state the screens set. A route that fails, its loader
 * or its screen, shows the root's `ScreenUnavailable`. The root route renders
 * {@link ScreenOutlet}, so every screen knows when a transfer away from it is under way.
 */
export const routes: RouteObject[] = [
  {
    Component: ScreenOutlet,
    HydrateFallback: NoHydrateFallback,
    ErrorBoundary: ScreenUnavailable,
    children: [
      { path: "/", element: <Navigate to={PATHS.COSGN00} replace /> },
      ...screenRoutes("none"),
      { element: <RequireSignon />, children: screenRoutes("signon") },
      { element: <RequireSignon admin />, children: screenRoutes("admin") },
      { path: "*", element: <Navigate to={PATHS.COSGN00} replace /> },
    ],
  },
];
