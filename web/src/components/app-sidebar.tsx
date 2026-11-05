// The application sidebar: the CM00 main menu (COMEN01C, options from
// COMEN02Y) and the CA00 admin menu (COADM01C, options from COADM02Y) as
// navigation links, the web replacement for typing an option number and
// pressing ENTER [app/cbl/COMEN01C.cbl:L94-95] [app/cbl/COADM01C.cbl:L98-99].
// The file also supplies MenuOptionList, the same options as the list of
// option cards the dashboard and /admin landing pages show.
//
// Every label, route and icon comes from nav/menu.ts, so the link names and
// destinations are the function-map rows' `control=link "<name>"` and
// `route=` values; nothing about an option is written here.
//
// "Card management" shows for every signed-on user, because the COBOL user
// programs check no user type. "Administration" shows for user type 'A' only.
// That choice shapes navigation alone: RequireAdmin and the API's 403 decide
// access.
//
// One link at most is the current page, the entry whose nav/menu.ts
// `currentRoutes` pattern matches the location; its link alone carries
// aria-current="page" and the generated active style.
//
// The sidebar collapses to an icon rail (AAP 0.5.4.4, driven by app-layout's
// open state). Each link keeps its label span in the DOM there, so its
// accessible name stays the label while the generated styles hide the text
// and the tooltip shows it on hover or focus. No SidebarRail is rendered: the
// header's SidebarTrigger is the single "Toggle Sidebar" control. The user
// menu and sign-out live in app-layout's header.
import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type JSX,
  type RefObject,
} from "react";
import { WalletCards } from "lucide-react";
import { Link, matchRoutes, useLocation } from "react-router";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { isAdmin, useSession } from "@/lib/session";
import { cn } from "@/lib/utils";
import {
  MENU_SECTION_LABELS,
  adminMenu,
  menu,
  userMenu,
  type MenuEntry,
  type MenuSection,
} from "@/nav/menu";

// Each nav/menu.ts `currentRoutes` pattern as a route of its own, carrying the
// entry that owns it. Built once, as the menu is static.
type CurrentRoute = { path: string; entry: MenuEntry };

const CURRENT_ROUTES: CurrentRoute[] = menu.flatMap((entry) =>
  entry.currentRoutes.map((path) => ({ path, entry })),
);

// The id of the entry that is the current page at the location, or null on a
// route no entry owns, such as the /admin landing. matchRoutes ranks the
// patterns as the router ranks App.tsx's route table, so the static
// /transactions/new picks Transaction Add rather than Transaction View's
// /transactions/:tranId, and like the router it ignores case and a trailing
// slash. Read once for the whole sidebar, so no two links can both be current.
function useCurrentEntryId(): string | null {
  const { pathname } = useLocation();
  return matchRoutes(CURRENT_ROUTES, pathname)?.at(-1)?.route.entry.id ?? null;
}

// Whether the element's text overflows its box, measured again whenever the
// box resizes, the sidebar's expand and collapse transitions included.
function useIsClipped(ref: RefObject<HTMLElement | null>): boolean {
  const [clipped, setClipped] = useState(false);

  useLayoutEffect(() => {
    const element = ref.current;
    if (element === null) {
      return;
    }
    const measure = (): void => {
      setClipped(element.scrollWidth > element.clientWidth);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [ref]);

  return clipped;
}

// `active` is the sidebar's one current-page decision, so the generated
// highlight (data-active) and aria-current="page" always agree. The link is a
// Link, not a NavLink: NavLink would set aria-current from its own target,
// which several options share (Account View and Account Update both open the
// dashboard lookup, E-61).
//
// The generated tooltip shows only in the icon rail. In the expanded sidebar
// the generated `truncate` style cuts the longest option texts, such as
// "Transaction Type Maintenance (Db2)", which the function map fixes and the
// generated width cannot hold. A clipped label therefore also gets its
// tooltip there, so the full option text stays readable without restyling
// the generated button. `hidden` overrides the generated condition, which
// SidebarMenuButton spreads the tooltip props over.
function SidebarNavItem({
  entry,
  active,
}: {
  entry: MenuEntry;
  active: boolean;
}): JSX.Element {
  const labelRef = useRef<HTMLSpanElement>(null);
  const clipped = useIsClipped(labelRef);
  const { isMobile, state } = useSidebar();

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={active}
        tooltip={{
          children: entry.label,
          hidden: isMobile || (state !== "collapsed" && !clipped),
        }}
      >
        <Link to={entry.route} aria-current={active ? "page" : undefined}>
          <entry.icon aria-hidden="true" />
          <span ref={labelRef}>{entry.label}</span>
        </Link>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

// One menu section. The group label names the link list for assistive
// technology; in the icon rail the label is only made transparent, so the
// name remains.
function SidebarNavGroup({
  label,
  entries,
  currentId,
}: {
  label: string;
  entries: readonly MenuEntry[];
  currentId: string | null;
}): JSX.Element {
  const labelId = useId();

  return (
    <SidebarGroup>
      <SidebarGroupLabel id={labelId}>{label}</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu aria-labelledby={labelId}>
          {entries.map((entry) => (
            <SidebarNavItem
              key={entry.id}
              entry={entry}
              active={entry.id === currentId}
            />
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

export function AppSidebar(): JSX.Element {
  const session = useSession();
  const currentId = useCurrentEntryId();

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        {/* Brand row, not a link, so it adds no Tab stop. In the icon rail the
            row's overflow clipping leaves only the icon, centred on the menu
            icons below, without an arbitrary-value variant. */}
        <div className="flex h-8 items-center gap-2 overflow-hidden px-2 text-sm font-semibold">
          <WalletCards aria-hidden="true" className="size-4 shrink-0" />
          <span className="truncate">CardDemo</span>
        </div>
      </SidebarHeader>
      {/* The generated SidebarContent renders a div; the navigation role on it
          keeps the groups its direct children while making the sidebar a named
          landmark distinct from the breadcrumb's. */}
      <SidebarContent role="navigation" aria-label="Main">
        <SidebarNavGroup
          label={MENU_SECTION_LABELS.user}
          entries={userMenu}
          currentId={currentId}
        />
        {isAdmin(session) && (
          <SidebarNavGroup
            label={MENU_SECTION_LABELS.admin}
            entries={adminMenu}
            currentId={currentId}
          />
        )}
      </SidebarContent>
    </Sidebar>
  );
}

// The nav/menu.ts entries of each menu section, in COBOL option order.
const MENU_OPTIONS: Readonly<Record<MenuSection, readonly MenuEntry[]>> = {
  user: userMenu,
  admin: adminMenu,
};

/**
 * The CM00 or CA00 option list the menu landing pages show: section "user"
 * on the dashboard, "admin" on /admin. Each option's label, route and icon
 * comes from nav/menu.ts, and choosing its link replaces typing the option
 * number and pressing ENTER [app/cbl/COMEN01C.cbl:L94-95]
 * [app/cbl/COADM01C.cbl:L98-99]. The page supplies the navigation landmark
 * and the column classes; this renders only the list.
 */
export function MenuOptionList({
  section,
  className,
}: {
  section: MenuSection;
  className?: string;
}): JSX.Element {
  // BLITZY [A11Y]: the links keep the generated Button focus ring, the
  // built-in `ring-ring/50` token of index.css, about 1.5:1 against the card
  // background, below the WCAG AA 3:1 non-text minimum. Kept as the shadcn
  // token set defines it; flagged for designer review.
  return (
    <ul className={cn("grid gap-4", className)}>
      {MENU_OPTIONS[section].map((entry) => (
        <li key={entry.id}>
          <Card className="relative h-full">
            <CardContent className="flex items-center gap-3">
              {/* Outside the link, so the link's accessible name is the
                  option text alone and the link variant's padding for an svg
                  child does not apply. */}
              <entry.icon
                aria-hidden="true"
                className="size-5 shrink-0 text-muted-foreground"
              />
              {/* The link fills the whole card through its ::after box, so
                  the card is the click target rather than the text line
                  alone. The padding and height reset align the text with the
                  icon, and wrapping keeps the longest option text inside its
                  column, a lg column beside the expanded sidebar included. */}
              <Button
                variant="link"
                asChild
                className="h-auto justify-start p-0 text-left whitespace-normal after:absolute after:inset-0"
              >
                <Link to={entry.route}>{entry.label}</Link>
              </Button>
            </CardContent>
          </Card>
        </li>
      ))}
    </ul>
  );
}
