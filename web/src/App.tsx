// The SPA's route table, replacing the CICS XCTL and RETURN TRANSID navigation
// between the 21 BMS maps (AAP 0.5.4.1). Every route docs/parity/function-map.md
// names resolves here, plus the reward screens; no other route exists.
//
// - /login is the only public route. After sign-on LoginPage navigates to the
//   login response's `landing`: /admin for user type A and / for U, the
//   COSGN00C XCTL targets COADM01C and COMEN01C [app/cbl/COSGN00C.cbl:L230-240].
// - Every other route sits under RequireAuth and the AppLayout shell. The user
//   functions are open to both user types, because the COBOL programs check
//   none; /admin/** also sits under RequireAdmin. Both guards only shape the
//   UI: the server's 401 and 403 decide access. Each guard route comes from
//   guardRoute, which marks its handle with the access the guard requires, so
//   the page title stops at a guard the session does not pass.
// - Each page is loaded through React.lazy, one chunk per route (AAP 0.10.3),
//   which its route's loader downloads before the router commits the
//   navigation (routes/lazy-page.tsx). The loaders only download page code and
//   fetch no data: data is fetched by the pages' TanStack Query hooks.
// - A page that cannot be loaded or rendered shows PageLoadError: inside the
//   shell for the signed-on routes, as a page of its own for /login.
//
// Breadcrumbs (the PF3 replacement, AAP 0.5.4.4): PageHeader builds the trail
// from the RouteHandle of every matched route, the last crumb being the page
// itself and its heading. A route with an index page therefore carries that
// page's handle and its index child carries none, so no crumb links to the
// page it sits on. Crumbs are the COMEN02Y and COADM02Y option texts that
// nav/menu.ts holds; titles are the BMS screen titles. Pages that render their
// map title themselves pass the same text to PageHeader.
import type { JSX } from "react";
import {
  createBrowserRouter,
  RouterProvider,
  type RouteObject,
} from "react-router";

import { AppLayout } from "@/components/app-layout";
import type { RouteHandle } from "@/components/page-header";
import { MENU_SECTION_LABELS, menu } from "@/nav/menu";
import { AppRoot } from "@/routes/app-root";
import { guardRoute } from "@/routes/guards";
import { inShellErrorBoundary, lazyPage, pageRoute } from "@/routes/lazy-page";
import { PageLoadError, Redirect } from "@/routes/page-fallbacks";

const LoginPage = lazyPage(() => import("@/pages/auth/LoginPage"));
const DashboardPage = lazyPage(() => import("@/pages/dashboard/DashboardPage"));
const AdminHomePage = lazyPage(() => import("@/pages/admin/AdminHomePage"));
const AccountViewPage = lazyPage(
  () => import("@/pages/accounts/AccountViewPage"),
);
const AccountEditPage = lazyPage(
  () => import("@/pages/accounts/AccountEditPage"),
);
const CardListPage = lazyPage(() => import("@/pages/cards/CardListPage"));
const CardViewPage = lazyPage(() => import("@/pages/cards/CardViewPage"));
const CardEditPage = lazyPage(() => import("@/pages/cards/CardEditPage"));
const TransactionListPage = lazyPage(
  () => import("@/pages/transactions/TransactionListPage"),
);
const TransactionViewPage = lazyPage(
  () => import("@/pages/transactions/TransactionViewPage"),
);
const TransactionAddPage = lazyPage(
  () => import("@/pages/transactions/TransactionAddPage"),
);
const TransactionReportPage = lazyPage(
  () => import("@/pages/reports/TransactionReportPage"),
);
const BillPaymentPage = lazyPage(
  () => import("@/pages/payments/BillPaymentPage"),
);
const AuthorizationSummaryPage = lazyPage(
  () => import("@/pages/authorizations/AuthorizationSummaryPage"),
);
const AuthorizationDetailPage = lazyPage(
  () => import("@/pages/authorizations/AuthorizationDetailPage"),
);
const RewardsPage = lazyPage(() => import("@/pages/rewards/RewardsPage"));
const UserListPage = lazyPage(() => import("@/pages/admin/users/UserListPage"));
const UserAddPage = lazyPage(() => import("@/pages/admin/users/UserAddPage"));
const UserEditPage = lazyPage(() => import("@/pages/admin/users/UserEditPage"));
const TranTypeListPage = lazyPage(
  () => import("@/pages/admin/transaction-types/TranTypeListPage"),
);
const TranTypeFormPage = lazyPage(
  () => import("@/pages/admin/transaction-types/TranTypeFormPage"),
);
const RewardRuleListPage = lazyPage(
  () => import("@/pages/admin/reward-rules/RewardRuleListPage"),
);
const RewardRuleFormPage = lazyPage(
  () => import("@/pages/admin/reward-rules/RewardRuleFormPage"),
);

const menuLabels: ReadonlyMap<string, string> = new Map(
  menu.map((entry) => [entry.id, entry.label]),
);

// The label of the nav/menu.ts entry `id`, so a crumb and its sidebar link
// always read alike. An id missing from the menu is a programming error, and
// it fails as soon as this module loads.
function menuLabel(id: string): string {
  const label = menuLabels.get(id);
  if (label === undefined) {
    throw new Error(`nav/menu.ts has no menu entry ${id}`);
  }
  return label;
}

const transactionTypeMaintenance = {
  title: "Maintain Transaction Type",
  crumb: menuLabel("COADM02Y-06"),
} satisfies RouteHandle;

const appRoutes: RouteObject[] = [
  {
    path: "/login",
    ...pageRoute(LoginPage, "public"),
    // Outside the shell, so the error panel is a page of its own.
    errorElement: <PageLoadError standalone />,
    handle: { title: "Sign on", crumb: "Sign on" } satisfies RouteHandle,
  },
  {
    ...guardRoute("signed-on"),
    children: [
      {
        // The shell is pathless, so `to` names the crumb target outright. Its
        // title heads the index page, the CM00 main menu [app/bms/COMEN01.bms:L79].
        element: <AppLayout />,
        handle: {
          title: "Main Menu",
          crumb: "Home",
          to: "/",
        } satisfies RouteHandle,
        // Every child sits in one pathless route whose errorElement renders in
        // the shell's page content, so a page that fails to load or render
        // leaves the sidebar and header in place.
        children: inShellErrorBoundary([
          { index: true, ...pageRoute(DashboardPage, "signed-on") },
          {
            // CAVW; the account number is entered on the dashboard (E-61).
            path: "accounts/:accountId",
            handle: {
              title: "View Account",
              crumb: menuLabel("COMEN02Y-01"),
            } satisfies RouteHandle,
            children: [
              { index: true, ...pageRoute(AccountViewPage, "signed-on") },
              {
                // CAUP
                path: "edit",
                ...pageRoute(AccountEditPage, "signed-on"),
                handle: {
                  title: "Update Account",
                  crumb: menuLabel("COMEN02Y-02"),
                } satisfies RouteHandle,
              },
            ],
          },
          {
            // CCLI. The card view and update are siblings rather than a
            // nested view > update trail: both take the account number from
            // the `account_id` search parameter (E-55), which a crumb, linking
            // to its route's pathname alone, would drop. The update page's
            // Back link returns to the view with it.
            path: "cards",
            handle: {
              title: "List Credit Cards",
              crumb: menuLabel("COMEN02Y-03"),
            } satisfies RouteHandle,
            children: [
              { index: true, ...pageRoute(CardListPage, "signed-on") },
              {
                // CCDL
                path: ":cardNumber",
                ...pageRoute(CardViewPage, "signed-on"),
                handle: {
                  title: "View Credit Card Detail",
                  crumb: menuLabel("COMEN02Y-04"),
                } satisfies RouteHandle,
              },
              {
                // CCUP
                path: ":cardNumber/edit",
                ...pageRoute(CardEditPage, "signed-on"),
                handle: {
                  title: "Update Credit Card Details",
                  crumb: menuLabel("COMEN02Y-05"),
                } satisfies RouteHandle,
              },
            ],
          },
          {
            // CT00
            path: "transactions",
            handle: {
              title: "List Transactions",
              crumb: menuLabel("COMEN02Y-06"),
            } satisfies RouteHandle,
            children: [
              { index: true, ...pageRoute(TransactionListPage, "signed-on") },
              {
                // CT02; the static segment outranks :tranId.
                path: "new",
                ...pageRoute(TransactionAddPage, "signed-on"),
                handle: {
                  title: "Add Transaction",
                  crumb: menuLabel("COMEN02Y-08"),
                } satisfies RouteHandle,
              },
              {
                // CT01
                path: ":tranId",
                ...pageRoute(TransactionViewPage, "signed-on"),
                handle: {
                  title: "View Transaction",
                  crumb: menuLabel("COMEN02Y-07"),
                } satisfies RouteHandle,
              },
            ],
          },
          {
            // CR00
            path: "reports/transactions",
            ...pageRoute(TransactionReportPage, "signed-on"),
            handle: {
              title: "Transaction Reports",
              crumb: menuLabel("COMEN02Y-09"),
            } satisfies RouteHandle,
          },
          {
            // CB00
            path: "payments",
            ...pageRoute(BillPaymentPage, "signed-on"),
            handle: {
              title: "Bill Payment",
              crumb: menuLabel("COMEN02Y-10"),
            } satisfies RouteHandle,
          },
          {
            // CPVS
            path: "authorizations",
            handle: {
              title: "View Authorizations",
              crumb: menuLabel("COMEN02Y-11"),
            } satisfies RouteHandle,
            children: [
              {
                index: true,
                ...pageRoute(AuthorizationSummaryPage, "signed-on"),
              },
              {
                // CPVD; no menu option opens it, so its crumb is the map title.
                path: ":authId",
                ...pageRoute(AuthorizationDetailPage, "signed-on"),
                handle: {
                  title: "View Authorization Details",
                  crumb: "View Authorization Details",
                } satisfies RouteHandle,
              },
            ],
          },
          {
            path: "rewards",
            ...pageRoute(RewardsPage, "signed-on"),
            handle: {
              title: "Rewards",
              crumb: menuLabel("REWARDS"),
            } satisfies RouteHandle,
          },
          {
            // CA00 [app/bms/COADM01.bms:L79]
            path: "admin",
            ...guardRoute("admin", {
              title: "Admin Menu",
              crumb: MENU_SECTION_LABELS.admin,
            }),
            children: [
              { index: true, ...pageRoute(AdminHomePage, "admin") },
              {
                // CU00, with the CU03 delete dialog on the list.
                path: "users",
                handle: {
                  title: "List Users",
                  crumb: menuLabel("COADM02Y-01"),
                } satisfies RouteHandle,
                children: [
                  { index: true, ...pageRoute(UserListPage, "admin") },
                  {
                    // CU01
                    path: "new",
                    ...pageRoute(UserAddPage, "admin"),
                    handle: {
                      title: "Add User",
                      crumb: menuLabel("COADM02Y-02"),
                    } satisfies RouteHandle,
                  },
                  {
                    // CU02
                    path: ":userId/edit",
                    ...pageRoute(UserEditPage, "admin"),
                    handle: {
                      title: "Update User",
                      crumb: menuLabel("COADM02Y-03"),
                    } satisfies RouteHandle,
                  },
                ],
              },
              {
                // CTLI
                path: "transaction-types",
                handle: {
                  title: "Maintain Transaction Type",
                  crumb: menuLabel("COADM02Y-05"),
                } satisfies RouteHandle,
                children: [
                  { index: true, ...pageRoute(TranTypeListPage, "admin") },
                  {
                    // CTTU key entry and add
                    path: "new",
                    ...pageRoute(TranTypeFormPage, "admin"),
                    handle: transactionTypeMaintenance,
                  },
                  {
                    // CTTU update and delete
                    path: ":code/edit",
                    ...pageRoute(TranTypeFormPage, "admin"),
                    handle: transactionTypeMaintenance,
                  },
                ],
              },
              {
                path: "reward-rules",
                handle: {
                  title: "Reward Rules",
                  crumb: menuLabel("REWARD-RULES"),
                } satisfies RouteHandle,
                children: [
                  { index: true, ...pageRoute(RewardRuleListPage, "admin") },
                  {
                    path: "new",
                    ...pageRoute(RewardRuleFormPage, "admin"),
                    handle: {
                      title: "New reward rule",
                      crumb: "New reward rule",
                    } satisfies RouteHandle,
                  },
                  {
                    path: ":ruleId/edit",
                    ...pageRoute(RewardRuleFormPage, "admin"),
                    handle: {
                      title: "Edit reward rule",
                      crumb: "Edit reward rule",
                    } satisfies RouteHandle,
                  },
                ],
              },
            ],
          },
        ]),
      },
    ],
  },
  // An unknown path goes to the main menu, and RequireAuth sends a visitor
  // without a session on to /login.
  { path: "*", element: <Redirect to="/" replace /> },
];

// The root route: AppRoot sets the document title from the routes the guards
// admit, and keeps each history entry's scroll position in memory, restoring
// it on Back and Forward and opening every other screen at its top.
const routes: RouteObject[] = [{ element: <AppRoot />, children: appRoutes }];

const router = createBrowserRouter(routes);

export default function App(): JSX.Element {
  return <RouterProvider router={router} />;
}
