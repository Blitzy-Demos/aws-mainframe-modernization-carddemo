// The page header every screen renders at the top of its content: the
// breadcrumb trail of the matched routes, the page's single h1, an optional
// description and the page's action buttons.
//
// The trail is built from the `handle` of each matched route (App.tsx types
// every handle as RouteHandle), so no route or label is written here. Layout
// routes without a handle (guards, the app layout) contribute no crumb. Every
// crumb but the last links back up the route hierarchy, the breadcrumb half of
// the 3270 PF3 exit; the page's own "Back" link is the BackLink below, which
// goes in `actions`.
//
// useMatches() needs a data router: App.tsx uses createBrowserRouter, and tests
// that render a page use createMemoryRouter.
//
// shadcn/ui has no heading component, so the title is a semantic h1 styled with
// Tailwind type-scale utilities. Pages render no other h1.
import { Fragment, type JSX, type ReactNode } from "react";
import { ArrowLeft } from "lucide-react";
import {
  Link,
  useLocation,
  useMatches,
  type To,
  type UIMatch,
} from "react-router";

import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import {
  isRouteAccess,
  sessionAdmits,
  useSession,
  type RouteAccess,
  type Session,
} from "@/lib/session";

/**
 * The `handle` of a route that appears in the breadcrumb trail.
 *
 * - `crumb`: the crumb's label, and the page heading when `title` is absent.
 * - `title`: the page heading, when it differs from the crumb.
 * - `to`: the crumb's link target, when it is not the matched pathname (for
 *   example a parent that only groups routes and renders no page itself).
 */
export type RouteHandle = { title?: string; crumb: string; to?: string };

/**
 * Whether a route `handle` (typed `unknown` by react-router) is a RouteHandle:
 * a non-null object whose `crumb` is a string and whose `title` and `to` are
 * strings or absent.
 */
export function isRouteHandle(value: unknown): value is RouteHandle {
  if (
    typeof value !== "object" ||
    value === null ||
    !("crumb" in value) ||
    typeof value.crumb !== "string"
  ) {
    return false;
  }
  const title = "title" in value ? value.title : undefined;
  const to = "to" in value ? value.to : undefined;
  return (
    (title === undefined || typeof title === "string") &&
    (to === undefined || typeof to === "string")
  );
}

/**
 * The mark a guard route's `handle` carries (routes/guards.tsx guardRoute):
 * the access its guard requires. It sits beside the route's RouteHandle
 * fields when the route has a crumb, and alone when it has none.
 */
export type AccessHandle = { access: RouteAccess };

/** Whether a route `handle` carries an AccessHandle mark. */
export function isAccessHandle(value: unknown): value is AccessHandle {
  return (
    typeof value === "object" &&
    value !== null &&
    "access" in value &&
    isRouteAccess(value.access)
  );
}

// The matched routes the guards let `session` reach: those above the first
// route whose AccessHandle the session does not satisfy. That route's guard
// renders a redirect in place of everything below it, so none of them is the
// page shown.
function admittedMatches(
  matches: readonly UIMatch[],
  session: Session | null,
): readonly UIMatch[] {
  const refused = matches.findIndex(
    (match) =>
      isAccessHandle(match.handle) &&
      !sessionAdmits(session, match.handle.access),
  );
  return refused === -1 ? matches : matches.slice(0, refused);
}

export type PageHeaderProps = {
  /** Overrides the heading taken from the leaf route's handle. */
  title?: string;
  description?: string;
  /** Page-level buttons and links, such as "Edit", "Add" or "Back". */
  actions?: ReactNode;
};

type Crumb = {
  id: string;
  /** The route's matched pathname, decoded as react-router matched it. */
  pathname: string;
  handle: RouteHandle;
};

// null, undefined and booleans render nothing, so they get no actions
// container that would only add an empty flex item.
function rendersContent(node: ReactNode): boolean {
  return node !== undefined && node !== null && typeof node !== "boolean";
}

// The breadcrumb trail of the matched routes: every match whose handle is a
// RouteHandle, outermost first, so the last crumb is the page itself.
function routeCrumbs(matches: readonly UIMatch[]): Crumb[] {
  return matches.flatMap((match): Crumb[] =>
    isRouteHandle(match.handle)
      ? [{ id: match.id, pathname: match.pathname, handle: match.handle }]
      : [],
  );
}

function nonEmptySegments(pathname: string): string[] {
  return pathname.split("/").filter((segment) => segment !== "");
}

// The link target of a crumb without `to`: its route's matched pathname as
// the current URL spells it. A matched pathname is decoded (react-router
// decodes each URL segment before matching and keeps a decoded "/" as "%2F"),
// so a link to it would send a key holding a space, "#", "?" or "%" to
// another route. Decoding maps segments one to one, so the match spans as
// many leading segments of the location's pathname, which the router keeps
// as the URL has it; following the link therefore matches the same params.
// Re-encoding the decoded text instead would turn a kept "%2F" into "%252F".
// The matched pathname of a route above the page never ends in "/", so
// neither does the target, whatever trailing slash the current URL has.
function urlPathPrefix(
  matchedPathname: string,
  locationPathname: string,
): string {
  const depth = nonEmptySegments(matchedPathname).length;
  return `/${nonEmptySegments(locationPathname).slice(0, depth).join("/")}`;
}

/**
 * The heading the matched routes give the current page: the last RouteHandle's
 * `title`, else its `crumb`, among the routes the guards let the session
 * reach; undefined when none of them has a handle. A guard that refuses the
 * session redirects after the router has committed its matches, so the
 * routes from it down never title the page: a regular user on /admin/** gets
 * the shell's title, and a visitor without a session none until /login.
 * PageHeader renders it as the h1 unless its `title` prop overrides it, the
 * app shell announces it on every route change, and AppRoot puts it in the
 * document title.
 */
export function usePageTitle(): string | undefined {
  const matches = useMatches();
  const session = useSession();
  const leaf = routeCrumbs(admittedMatches(matches, session)).at(-1)?.handle;
  return leaf?.title ?? leaf?.crumb;
}

/**
 * The page's PF3 "Back" link to `to`: an outline link button led by an
 * ArrowLeft icon. It is the one form of the control on every page, so Back
 * looks the same on every screen, and pages render it in PageHeader
 * `actions`, unless a form's key order places Back after its other buttons.
 * The accessible name is exactly "Back", the function map's name.
 */
export function BackLink({ to }: { to: To }): JSX.Element {
  return (
    <Button variant="outline" asChild>
      <Link to={to}>
        <ArrowLeft aria-hidden="true" />
        Back
      </Link>
    </Button>
  );
}

export function PageHeader({
  title,
  description,
  actions,
}: PageHeaderProps): JSX.Element {
  const crumbs = routeCrumbs(useMatches());
  const locationPathname = useLocation().pathname;
  const pageTitle = usePageTitle();
  const heading = title ?? pageTitle;
  const hasHeading = heading !== undefined && heading !== "";
  const hasDescription = description !== undefined && description !== "";
  const hasActions = rendersContent(actions);

  // BLITZY [A11Y]: the crumb links keep the browser focus outline in the
  // built-in `outline-ring/50` colour of index.css, about 1.5:1 against the
  // light background, below the WCAG AA 3:1 non-text minimum. Kept as the
  // shadcn token set defines it; flagged for designer review.
  return (
    <header className="flex flex-col gap-2">
      {crumbs.length > 0 ? (
        <Breadcrumb>
          <BreadcrumbList>
            {crumbs.map(({ id, pathname, handle }, index) =>
              index < crumbs.length - 1 ? (
                <Fragment key={id}>
                  <BreadcrumbItem>
                    <BreadcrumbLink asChild>
                      <Link
                        to={
                          handle.to ?? urlPathPrefix(pathname, locationPathname)
                        }
                      >
                        {handle.crumb}
                      </Link>
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                  <BreadcrumbSeparator />
                </Fragment>
              ) : (
                <BreadcrumbItem key={id}>
                  <BreadcrumbPage>{handle.crumb}</BreadcrumbPage>
                </BreadcrumbItem>
              ),
            )}
          </BreadcrumbList>
        </Breadcrumb>
      ) : null}
      {hasHeading || hasDescription || hasActions ? (
        <div className="flex flex-col gap-2 lg:flex-row lg:items-start lg:justify-between">
          {hasHeading || hasDescription ? (
            <div className="flex flex-col gap-1">
              {hasHeading ? (
                <h1 className="text-2xl font-semibold tracking-tight">
                  {heading}
                </h1>
              ) : null}
              {hasDescription ? (
                <p className="text-sm text-muted-foreground">{description}</p>
              ) : null}
            </div>
          ) : null}
          {hasActions ? (
            <div className="flex flex-wrap items-center gap-2">{actions}</div>
          ) : null}
        </div>
      ) : null}
    </header>
  );
}
