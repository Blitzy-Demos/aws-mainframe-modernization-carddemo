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
import { Link, NavLink, useLocation, useResolvedPath } from "react-router";

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
  userMenu,
  type MenuEntry,
  type MenuSection,
} from "@/nav/menu";

// NavLink's own `end` test, so the highlight (`isActive`) and NavLink's
// aria-current="page" always agree: the location's pathname equals the
// resolved target, compared case-insensitively as NavLink does by default.
// useMatch is not used: its `end` pattern also accepts trailing slashes
// ("/cards/") and matches the decoded pathname, where NavLink marks no current
// page. The menu routes are plain ASCII paths, which the router's location
// encoding leaves unchanged.
function useIsCurrentRoute(route: string): boolean {
  const target = useResolvedPath(route).pathname;
  const { pathname } = useLocation();
  return pathname.toLowerCase() === target.toLowerCase();
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

// Several options share a route (Account View and Account Update both open
// the dashboard lookup, E-61), so more than one entry can be current at once,
// each with its own aria-current.
//
// The generated tooltip shows only in the icon rail. In the expanded sidebar
// the generated `truncate` style cuts the longest option texts, such as
// "Transaction Type Maintenance (Db2)", which the function map fixes and the
// generated width cannot hold. A clipped label therefore also gets its
// tooltip there, so the full option text stays readable without restyling
// the generated button. `hidden` overrides the generated condition, which
// SidebarMenuButton spreads the tooltip props over.
function SidebarNavItem({ entry }: { entry: MenuEntry }): JSX.Element {
  const active = useIsCurrentRoute(entry.route);
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
        <NavLink to={entry.route} end>
          <entry.icon aria-hidden="true" />
          <span ref={labelRef}>{entry.label}</span>
        </NavLink>
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
}: {
  label: string;
  entries: readonly MenuEntry[];
}): JSX.Element {
  const labelId = useId();

  return (
    <SidebarGroup>
      <SidebarGroupLabel id={labelId}>{label}</SidebarGroupLabel>
      <SidebarGroupContent>
        <SidebarMenu aria-labelledby={labelId}>
          {entries.map((entry) => (
            <SidebarNavItem key={entry.id} entry={entry} />
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}

export function AppSidebar(): JSX.Element {
  const session = useSession();

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
        <SidebarNavGroup label={MENU_SECTION_LABELS.user} entries={userMenu} />
        {isAdmin(session) && (
          <SidebarNavGroup
            label={MENU_SECTION_LABELS.admin}
            entries={adminMenu}
          />
        )}
      </SidebarContent>
    </Sidebar>
  );
}

/** The nav/menu.ts entries of each menu section, in COBOL option order. */
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
