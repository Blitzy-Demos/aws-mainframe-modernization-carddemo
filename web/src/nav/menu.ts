/**
 * The single navigation definition of the SPA: the CM00 main menu (COMEN01C,
 * options from COMEN02Y), the CA00 admin menu (COADM01C, options from
 * COADM02Y) and the two reward entries. The sidebar, the dashboard and admin
 * landing menu lists, and the route table read their options from here; no
 * other module declares a menu option.
 *
 * Labels are the copybook `PIC X(35)` option texts, right-trimmed and without
 * the `NN. ` prefix that BUILD-MENU-OPTIONS prepends on the 3270 screen
 * [app/cbl/COMEN01C.cbl:L269-272] [app/cbl/COADM01C.cbl:L236-239]. Entries keep
 * the COBOL option order. Each route is the `route=` destination of the
 * option's row in docs/parity/function-map.md.
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
  route: string;
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
    section: "user",
    icon: Eye,
  },
  /** Option 2 [app/cpy/COMEN02Y.cpy:L31-35], F-COMEN01-002. */
  {
    id: "COMEN02Y-02",
    label: "Account Update",
    program: "COACTUPC",
    route: "/",
    section: "user",
    icon: Pencil,
  },
  /** Option 3 [app/cpy/COMEN02Y.cpy:L37-41], F-COMEN01-003. */
  {
    id: "COMEN02Y-03",
    label: "Credit Card List",
    program: "COCRDLIC",
    route: "/cards",
    section: "user",
    icon: CreditCard,
  },
  /** Option 4 [app/cpy/COMEN02Y.cpy:L43-47], F-COMEN01-004. */
  {
    id: "COMEN02Y-04",
    label: "Credit Card View",
    program: "COCRDSLC",
    route: "/cards",
    section: "user",
    icon: Search,
  },
  /** Option 5 [app/cpy/COMEN02Y.cpy:L49-53], F-COMEN01-005. */
  {
    id: "COMEN02Y-05",
    label: "Credit Card Update",
    program: "COCRDUPC",
    route: "/cards",
    section: "user",
    icon: SquarePen,
  },
  /** Option 6 [app/cpy/COMEN02Y.cpy:L55-59], F-COMEN01-006. */
  {
    id: "COMEN02Y-06",
    label: "Transaction List",
    program: "COTRN00C",
    route: "/transactions",
    section: "user",
    icon: List,
  },
  /** Option 7 [app/cpy/COMEN02Y.cpy:L61-65], F-COMEN01-007. */
  {
    id: "COMEN02Y-07",
    label: "Transaction View",
    program: "COTRN01C",
    route: "/transactions",
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
    section: "user",
    icon: Plus,
  },
  /** Option 9 [app/cpy/COMEN02Y.cpy:L74-78], F-COMEN01-009. */
  {
    id: "COMEN02Y-09",
    label: "Transaction Reports",
    program: "CORPT00C",
    route: "/reports/transactions",
    section: "user",
    icon: ChartBar,
  },
  /** Option 10 [app/cpy/COMEN02Y.cpy:L80-84], F-COMEN01-010. */
  {
    id: "COMEN02Y-10",
    label: "Bill Payment",
    program: "COBIL00C",
    route: "/payments",
    section: "user",
    icon: Wallet,
  },
  /** Option 11 [app/cpy/COMEN02Y.cpy:L86-90], F-COMEN01-011. */
  {
    id: "COMEN02Y-11",
    label: "Pending Authorization View",
    program: "COPAUS0C",
    route: "/authorizations",
    section: "user",
    icon: ShieldAlert,
  },
  /** Reward balance and history; new, no COBOL program and no F-ID. */
  {
    id: "REWARDS",
    label: "Rewards",
    program: null,
    route: "/rewards",
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
    section: "admin",
    icon: Users,
  },
  /** Option 2 [app/cpy/COADM02Y.cpy:L31-34], F-COADM01-002. */
  {
    id: "COADM02Y-02",
    label: "User Add (Security)",
    program: "COUSR01C",
    route: "/admin/users/new",
    section: "admin",
    icon: UserPlus,
  },
  /** Option 3 [app/cpy/COADM02Y.cpy:L36-39], F-COADM01-003. */
  {
    id: "COADM02Y-03",
    label: "User Update (Security)",
    program: "COUSR02C",
    route: "/admin/users",
    section: "admin",
    icon: UserCog,
  },
  /** Option 4 [app/cpy/COADM02Y.cpy:L41-44], F-COADM01-004. */
  {
    id: "COADM02Y-04",
    label: "User Delete (Security)",
    program: "COUSR03C",
    route: "/admin/users",
    section: "admin",
    icon: UserMinus,
  },
  /** Option 5 [app/cpy/COADM02Y.cpy:L46-49], F-COADM01-005. */
  {
    id: "COADM02Y-05",
    label: "Transaction Type List/Update (Db2)",
    program: "COTRTLIC",
    route: "/admin/transaction-types",
    section: "admin",
    icon: Tags,
  },
  /** Option 6 [app/cpy/COADM02Y.cpy:L50-53], F-COADM01-006. */
  {
    id: "COADM02Y-06",
    label: "Transaction Type Maintenance (Db2)",
    program: "COTRTUPC",
    route: "/admin/transaction-types/new",
    section: "admin",
    icon: Settings,
  },
  /** Reward rule management; new, no COBOL program and no F-ID. */
  {
    id: "REWARD-RULES",
    label: "Reward Rules",
    program: null,
    route: "/admin/reward-rules",
    section: "admin",
    icon: Award,
  },
];

/** Every entry, the user section first, each section in COBOL option order. */
export const menu: readonly MenuEntry[] = [...userMenu, ...adminMenu];

type MenuGroup = {
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
 * The menu sections a signed-on user sees: "Card management" always, then
 * "Administration" for user type 'A' only. The section decides visibility,
 * not the COBOL option user-type byte: every COMEN02Y option is 'U', so the
 * COMEN01C admin-only rejection [app/cbl/COMEN01C.cbl:L136-143] is never
 * reached and has no counterpart. This shapes the UI only; the API's 403 on
 * `/admin/**` decides access.
 */
export function menuForUserType(userType: "A" | "U" | null | undefined): readonly {
  section: MenuSection;
  label: string;
  entries: readonly MenuEntry[];
}[] {
  return userType === "A" ? adminGroups : userGroups;
}
