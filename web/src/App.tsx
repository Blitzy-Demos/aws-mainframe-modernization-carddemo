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
//   UI: the server's 401 and 403 decide access.
// - Each page is loaded through React.lazy, one chunk per route (AAP 0.10.3).
//   Data is fetched by the pages' TanStack Query hooks, never by route loaders.
//
// Breadcrumbs (the PF3 replacement, AAP 0.5.4.4): PageHeader builds the trail
// from the RouteHandle of every matched route, the last crumb being the page
// itself and its heading. A route with an index page therefore carries that
// page's handle and its index child carries none, so no crumb links to the
// page it sits on. Crumbs are the COMEN02Y and COADM02Y option texts that
// nav/menu.ts holds; titles are the BMS screen titles. Pages that render their
// map title themselves pass the same text to PageHeader.
import {
  lazy,
  Suspense,
  type ComponentType,
  type JSX,
  type LazyExoticComponent,
} from "react";
import {
  createBrowserRouter,
  Navigate,
  RouterProvider,
  type RouteObject,
} from "react-router";

import { AppLayout } from "@/components/app-layout";
import type { RouteHandle } from "@/components/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { MENU_SECTION_LABELS, menu } from "@/nav/menu";
import { RequireAdmin, RequireAuth } from "@/routes/guards";

const LoginPage = lazy(() => import("@/pages/auth/LoginPage"));
const DashboardPage = lazy(() => import("@/pages/dashboard/DashboardPage"));
const AdminHomePage = lazy(() => import("@/pages/admin/AdminHomePage"));
const AccountViewPage = lazy(() => import("@/pages/accounts/AccountViewPage"));
const AccountEditPage = lazy(() => import("@/pages/accounts/AccountEditPage"));
const CardListPage = lazy(() => import("@/pages/cards/CardListPage"));
const CardViewPage = lazy(() => import("@/pages/cards/CardViewPage"));
const CardEditPage = lazy(() => import("@/pages/cards/CardEditPage"));
const TransactionListPage = lazy(
  () => import("@/pages/transactions/TransactionListPage"),
);
const TransactionViewPage = lazy(
  () => import("@/pages/transactions/TransactionViewPage"),
);
const TransactionAddPage = lazy(
  () => import("@/pages/transactions/TransactionAddPage"),
);
const TransactionReportPage = lazy(
  () => import("@/pages/reports/TransactionReportPage"),
);
const BillPaymentPage = lazy(() => import("@/pages/payments/BillPaymentPage"));
const AuthorizationSummaryPage = lazy(
  () => import("@/pages/authorizations/AuthorizationSummaryPage"),
);
const AuthorizationDetailPage = lazy(
  () => import("@/pages/authorizations/AuthorizationDetailPage"),
);
const RewardsPage = lazy(() => import("@/pages/rewards/RewardsPage"));
const UserListPage = lazy(() => import("@/pages/admin/users/UserListPage"));
const UserAddPage = lazy(() => import("@/pages/admin/users/UserAddPage"));
const UserEditPage = lazy(() => import("@/pages/admin/users/UserEditPage"));
const TranTypeListPage = lazy(
  () => import("@/pages/admin/transaction-types/TranTypeListPage"),
);
const TranTypeFormPage = lazy(
  () => import("@/pages/admin/transaction-types/TranTypeFormPage"),
);
const RewardRuleListPage = lazy(
  () => import("@/pages/admin/reward-rules/RewardRuleListPage"),
);
const RewardRuleFormPage = lazy(
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

// Shown while a page's chunk loads: the outline of a heading and its content.
function PageFallback(): JSX.Element {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label="Loading"
      className="flex flex-col gap-4"
    >
      <Skeleton className="h-8 w-48" />
      <Skeleton className="h-4 w-72" />
      <Skeleton className="h-32 w-full" />
    </div>
  );
}

// The route element of a lazy page, inside its own Suspense boundary.
function page(Component: LazyExoticComponent<ComponentType>): JSX.Element {
  return (
    <Suspense fallback={<PageFallback />}>
      <Component />
    </Suspense>
  );
}

const transactionTypeMaintenance = {
  title: "Maintain Transaction Type",
  crumb: menuLabel("COADM02Y-06"),
} satisfies RouteHandle;

const routes: RouteObject[] = [
  {
    path: "/login",
    element: page(LoginPage),
    handle: { title: "Sign on", crumb: "Sign on" } satisfies RouteHandle,
  },
  {
    element: <RequireAuth />,
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
        children: [
          { index: true, element: page(DashboardPage) },
          {
            // CAVW; the account number is entered on the dashboard (E-61).
            path: "accounts/:accountId",
            handle: {
              title: "View Account",
              crumb: menuLabel("COMEN02Y-01"),
            } satisfies RouteHandle,
            children: [
              { index: true, element: page(AccountViewPage) },
              {
                // CAUP
                path: "edit",
                element: page(AccountEditPage),
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
              { index: true, element: page(CardListPage) },
              {
                // CCDL
                path: ":cardNumber",
                element: page(CardViewPage),
                handle: {
                  title: "View Credit Card Detail",
                  crumb: menuLabel("COMEN02Y-04"),
                } satisfies RouteHandle,
              },
              {
                // CCUP
                path: ":cardNumber/edit",
                element: page(CardEditPage),
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
              { index: true, element: page(TransactionListPage) },
              {
                // CT02; the static segment outranks :tranId.
                path: "new",
                element: page(TransactionAddPage),
                handle: {
                  title: "Add Transaction",
                  crumb: menuLabel("COMEN02Y-08"),
                } satisfies RouteHandle,
              },
              {
                // CT01
                path: ":tranId",
                element: page(TransactionViewPage),
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
            element: page(TransactionReportPage),
            handle: {
              title: "Transaction Reports",
              crumb: menuLabel("COMEN02Y-09"),
            } satisfies RouteHandle,
          },
          {
            // CB00
            path: "payments",
            element: page(BillPaymentPage),
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
              { index: true, element: page(AuthorizationSummaryPage) },
              {
                // CPVD; no menu option opens it, so its crumb is the map title.
                path: ":authId",
                element: page(AuthorizationDetailPage),
                handle: {
                  title: "View Authorization Details",
                  crumb: "View Authorization Details",
                } satisfies RouteHandle,
              },
            ],
          },
          {
            path: "rewards",
            element: page(RewardsPage),
            handle: {
              title: "Rewards",
              crumb: menuLabel("REWARDS"),
            } satisfies RouteHandle,
          },
          {
            // CA00 [app/bms/COADM01.bms:L79]
            path: "admin",
            element: <RequireAdmin />,
            handle: {
              title: "Admin Menu",
              crumb: MENU_SECTION_LABELS.admin,
            } satisfies RouteHandle,
            children: [
              { index: true, element: page(AdminHomePage) },
              {
                // CU00, with the CU03 delete dialog on the list.
                path: "users",
                handle: {
                  title: "List Users",
                  crumb: menuLabel("COADM02Y-01"),
                } satisfies RouteHandle,
                children: [
                  { index: true, element: page(UserListPage) },
                  {
                    // CU01
                    path: "new",
                    element: page(UserAddPage),
                    handle: {
                      title: "Add User",
                      crumb: menuLabel("COADM02Y-02"),
                    } satisfies RouteHandle,
                  },
                  {
                    // CU02
                    path: ":userId/edit",
                    element: page(UserEditPage),
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
                  { index: true, element: page(TranTypeListPage) },
                  {
                    // CTTU key entry and add
                    path: "new",
                    element: page(TranTypeFormPage),
                    handle: transactionTypeMaintenance,
                  },
                  {
                    // CTTU update and delete
                    path: ":code/edit",
                    element: page(TranTypeFormPage),
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
                  { index: true, element: page(RewardRuleListPage) },
                  {
                    path: "new",
                    element: page(RewardRuleFormPage),
                    handle: {
                      title: "New reward rule",
                      crumb: "New reward rule",
                    } satisfies RouteHandle,
                  },
                  {
                    path: ":ruleId/edit",
                    element: page(RewardRuleFormPage),
                    handle: {
                      title: "Edit reward rule",
                      crumb: "Edit reward rule",
                    } satisfies RouteHandle,
                  },
                ],
              },
            ],
          },
        ],
      },
    ],
  },
  // An unknown path goes to the main menu, and RequireAuth sends a visitor
  // without a session on to /login.
  { path: "*", element: <Navigate to="/" replace /> },
];

const router = createBrowserRouter(routes);

export default function App(): JSX.Element {
  return <RouterProvider router={router} />;
}
