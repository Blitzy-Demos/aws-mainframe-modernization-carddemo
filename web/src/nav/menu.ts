/**
 * The single navigation definition of the SPA: the CM00 main menu (COMEN01C,
 * options from COMEN02Y), the CA00 admin menu (COADM01C, options from
 * COADM02Y) and the two reward entries. The sidebar, the dashboard and admin
 * landing menu lists, and the route table read their options from here, and
 * the sidebar reads from menuForUserType which sections a user type sees; no
 * other module declares a menu option or decides a section's visibility.
 *
 * Labels are the copybook `PIC X(35)` option texts, right-trimmed and without
 * the `NN. ` prefix that BUILD-MENU-OPTIONS prepends on the 3270 screen
 * [app/cbl/COMEN01C.cbl:L269-272] [app/cbl/COADM01C.cbl:L236-239]. Entries keep
 * the COBOL option order. Each route is the `route=` destination of the
 * option's row in docs/parity/function-map.md.
 *
 * Which entry the sidebar marks as the current page is decided by each
 * entry's `currentRoutes`, not by its link target: several options share a
 * link target (E-61), while a route has at most one current item.
 */
import type { LucideIcon } from "lucide-react";
import {
  Award,
  ChartBar,
  CreditCard,
  Eye,
  Gift,
  List,
  Pencil,
  Plus,
  Receipt,
  Search,
  Settings,
  ShieldAlert,
  SquarePen,
  Tags,
  UserCog,
  UserMinus,
  UserPlus,
  Users,
  Wallet,
} from "lucide-react";

export type MenuSection = "user" | "admin";

export type MenuEntry = {
  id: string;
  label: string;
  program: string | null;
  /** The link target, which E-61 lets several options share. */
  route: string;
  /**
   * The route patterns, in the path syntax of App.tsx's route table, on which
   * this entry is the sidebar's current item: the routes of the option's own
   * screen, and the detail and edit routes reached from it that no other
   * option opens. Each pattern belongs to one entry at most. Of options
   * sharing a link target, the first in option order owns the target
   * (Account View `/`, Credit Card List `/cards`, Transaction List
   * `/transactions`, User List `/admin/users`); the others own only the
   * routes that carry their key. The sidebar marks the entry whose pattern
   * the router ranks first for the location, so the static
   * `/transactions/new` outranks `/transactions/:tranId`. A route no pattern
   * names, such as the `/admin` landing, has no current item.
   */
  currentRoutes: readonly string[];
  section: MenuSection;
  icon: LucideIcon;
};

/** Sidebar group titles: "Card management" for CM00, "Administration" for CA00. */
export const MENU_SECTION_LABELS: Readonly<Record<MenuSection, string>> = {
  user: "Card management",
  admin: "Administration",
};

/**
 * CARDDEMO-MAIN-MENU-OPTIONS [app/cpy/COMEN02Y.cpy:L19-90] plus Rewards. Every
 * COMEN02Y option carries user type 'U'. Options whose program first asks for
 * a record key link to the route whose controls supply that key (E-61): the
 * dashboard account lookup on `/`, the card list filters on `/cards` and the
 * transaction list on `/transactions`.
 */
export const userMenu: readonly MenuEntry[] = [
  /** Option 1 [app/cpy/COMEN02Y.cpy:L25-29], F-COMEN01-001. */
  {
    id: "COMEN02Y-01",
    label: "Account View",
    program: "COACTVWC",
    route: "/",
    currentRoutes: ["/", "/accounts/:accountId"],
    section: "user",
    icon: Eye,
  },
  /** Option 2 [app/cpy/COMEN02Y.cpy:L31-35], F-COMEN01-002. */
  {
    id: "COMEN02Y-02",
    label: "Account Update",
    program: "COACTUPC",
    route: "/",
    currentRoutes: ["/accounts/:accountId/edit"],
    section: "user",
    icon: Pencil,
  },
  /** Option 3 [app/cpy/COMEN02Y.cpy:L37-41], F-COMEN01-003. */
  {
    id: "COMEN02Y-03",
    label: "Credit Card List",
    program: "COCRDLIC",
    route: "/cards",
    currentRoutes: ["/cards"],
    section: "user",
    icon: CreditCard,
  },
  /** Option 4 [app/cpy/COMEN02Y.cpy:L43-47], F-COMEN01-004. */
  {
    id: "COMEN02Y-04",
    label: "Credit Card View",
    program: "COCRDSLC",
    route: "/cards",
    currentRoutes: ["/cards/:cardNumber"],
    section: "user",
    icon: Search,
  },
  /** Option 5 [app/cpy/COMEN02Y.cpy:L49-53], F-COMEN01-005. */
  {
    id: "COMEN02Y-05",
    label: "Credit Card Update",
    program: "COCRDUPC",
    route: "/cards",
    currentRoutes: ["/cards/:cardNumber/edit"],
    section: "user",
    icon: SquarePen,
  },
  /** Option 6 [app/cpy/COMEN02Y.cpy:L55-59], F-COMEN01-006. */
  {
    id: "COMEN02Y-06",
    label: "Transaction List",
    program: "COTRN00C",
    route: "/transactions",
    currentRoutes: ["/transactions"],
    section: "user",
    icon: List,
  },
  /** Option 7 [app/cpy/COMEN02Y.cpy:L61-65], F-COMEN01-007. */
  {
    id: "COMEN02Y-07",
    label: "Transaction View",
    program: "COTRN01C",
    route: "/transactions",
    currentRoutes: ["/transactions/:tranId"],
    section: "user",
    icon: Receipt,
  },
  /**
   * Option 8 [app/cpy/COMEN02Y.cpy:L67-72], F-COMEN01-008. The live literal is
   * L70; the "(Admin Only)" text on L69 is a comment line.
   */
  {
    id: "COMEN02Y-08",
    label: "Transaction Add",
    program: "COTRN02C",
    route: "/transactions/new",
    currentRoutes: ["/transactions/new"],
    section: "user",
    icon: Plus,
  },
  /** Option 9 [app/cpy/COMEN02Y.cpy:L74-78], F-COMEN01-009. */
  {
    id: "COMEN02Y-09",
    label: "Transaction Reports",
    program: "CORPT00C",
    route: "/reports/transactions",
    currentRoutes: ["/reports/transactions"],
    section: "user",
    icon: ChartBar,
  },
  /** Option 10 [app/cpy/COMEN02Y.cpy:L80-84], F-COMEN01-010. */
  {
    id: "COMEN02Y-10",
    label: "Bill Payment",
    program: "COBIL00C",
    route: "/payments",
    currentRoutes: ["/payments"],
    section: "user",
    icon: Wallet,
  },
  /** Option 11 [app/cpy/COMEN02Y.cpy:L86-90], F-COMEN01-011. */
  {
    id: "COMEN02Y-11",
    label: "Pending Authorization View",
    program: "COPAUS0C",
    route: "/authorizations",
    currentRoutes: ["/authorizations", "/authorizations/:authId"],
    section: "user",
    icon: ShieldAlert,
  },
  /** Reward balance and history; new, no COBOL program and no F-ID. */
  {
    id: "REWARDS",
    label: "Rewards",
    program: null,
    route: "/rewards",
    currentRoutes: ["/rewards"],
    section: "user",
    icon: Gift,
  },
];

/**
 * CARDDEMO-ADMIN-MENU-OPTIONS [app/cpy/COADM02Y.cpy:L19-53] plus Reward Rules.
 * COADM02Y carries no user-type byte; the section as a whole is shown to user
 * type 'A' only. User Update and User Delete need a user ID first, so they
 * link to the user list, whose row actions supply it (E-61).
 */
export const adminMenu: readonly MenuEntry[] = [
  /** Option 1 [app/cpy/COADM02Y.cpy:L26-29], F-COADM01-001. */
  {
    id: "COADM02Y-01",
    label: "User List (Security)",
    program: "COUSR00C",
    route: "/admin/users",
    currentRoutes: ["/admin/users"],
    section: "admin",
    icon: Users,
  },
  /** Option 2 [app/cpy/COADM02Y.cpy:L31-34], F-COADM01-002. */
  {
    id: "COADM02Y-02",
    label: "User Add (Security)",
    program: "COUSR01C",
    route: "/admin/users/new",
    currentRoutes: ["/admin/users/new"],
    section: "admin",
    icon: UserPlus,
  },
  /** Option 3 [app/cpy/COADM02Y.cpy:L36-39], F-COADM01-003. */
  {
    id: "COADM02Y-03",
    label: "User Update (Security)",
    program: "COUSR02C",
    route: "/admin/users",
    currentRoutes: ["/admin/users/:userId/edit"],
    section: "admin",
    icon: UserCog,
  },
  /**
   * Option 4 [app/cpy/COADM02Y.cpy:L41-44], F-COADM01-004. The delete is a
   * confirm dialog on the user list (E-58), whose route User List owns, so
   * no route makes this entry current.
   */
  {
    id: "COADM02Y-04",
    label: "User Delete (Security)",
    program: "COUSR03C",
    route: "/admin/users",
    currentRoutes: [],
    section: "admin",
    icon: UserMinus,
  },
  /** Option 5 [app/cpy/COADM02Y.cpy:L46-49], F-COADM01-005. */
  {
    id: "COADM02Y-05",
    label: "Transaction Type List/Update (Db2)",
    program: "COTRTLIC",
    route: "/admin/transaction-types",
    currentRoutes: ["/admin/transaction-types"],
    section: "admin",
    icon: Tags,
  },
  /** Option 6 [app/cpy/COADM02Y.cpy:L50-53], F-COADM01-006. */
  {
    id: "COADM02Y-06",
    label: "Transaction Type Maintenance (Db2)",
    program: "COTRTUPC",
    route: "/admin/transaction-types/new",
    currentRoutes: ["/admin/transaction-types/new", "/admin/transaction-types/:code/edit"],
    section: "admin",
    icon: Settings,
  },
  /** Reward rule management; new, no COBOL program and no F-ID. */
  {
    id: "REWARD-RULES",
    label: "Reward Rules",
    program: null,
    route: "/admin/reward-rules",
    currentRoutes: ["/admin/reward-rules", "/admin/reward-rules/new", "/admin/reward-rules/:ruleId/edit"],
    section: "admin",
    icon: Award,
  },
];

/** Every entry, the user section first, each section in COBOL option order. */
export const menu: readonly MenuEntry[] = [...userMenu, ...adminMenu];

/** One sidebar section: its title and its entries in COBOL option order. */
export type MenuGroup = {
  section: MenuSection;
  label: string;
  entries: readonly MenuEntry[];
};

const userGroup: MenuGroup = {
  section: "user",
  label: MENU_SECTION_LABELS.user,
  entries: userMenu,
};

const adminGroup: MenuGroup = {
  section: "admin",
  label: MENU_SECTION_LABELS.admin,
  entries: adminMenu,
};

// Built once so consumers receive stable references across renders.
const userGroups: readonly MenuGroup[] = [userGroup];
const adminGroups: readonly MenuGroup[] = [userGroup, adminGroup];

/**
 * The menu sections a signed-on user sees, in sidebar order, and the single
 * definition of that rule, which the sidebar renders: "Card management"
 * always, then "Administration" for user type 'A' only. Only that exact code
 * is an administrator, as `isAdmin` in lib/session.ts reads it, so any other
 * code, and a missing one, gets the user section alone. The section decides
 * visibility, not the COBOL option user-type byte: every COMEN02Y option is
 * 'U', so the COMEN01C admin-only rejection [app/cbl/COMEN01C.cbl:L136-143]
 * is never reached and has no counterpart. This shapes the UI only; the
 * API's 403 on `/admin/**` decides access.
 */
export function menuForUserType(
  userType: string | null | undefined,
): readonly MenuGroup[] {
  return userType === "A" ? adminGroups : userGroups;
}
