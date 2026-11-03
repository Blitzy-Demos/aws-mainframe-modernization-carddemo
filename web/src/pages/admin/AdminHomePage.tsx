// CA00 admin menu (COADM01C, map COADM01) at /admin, where COSGN00C sends a
// user of type 'A' after sign-on [app/cbl/COSGN00C.cbl:L230-240]. The route
// sits inside RequireAdmin; the API's 403 on /admin/** decides access.
//
// - Each COADM02Y option is a link named by its option text, in COBOL order,
//   followed by the Reward Rules entry; nav/menu.ts supplies every label,
//   route and icon (F-COADM01-001 to F-COADM01-006). Choosing a link replaces
//   typing the option number and pressing ENTER [app/cbl/COADM01C.cbl:L98-99],
//   so there is no OPTION field, and the option-number and invalid-key
//   messages have no counterpart (F-COADM01-008, E-56).
// - F3=Exit, the sign-off XCTL to COSGN00C [app/cbl/COADM01C.cbl:L100-102],
//   is the app-shell user menu's "Log out" item (F-COADM01-007), which
//   app-layout renders; this page adds no sign-off control of its own.
// - The heading and breadcrumb come from the route handle App.tsx declares.
import type { JSX } from "react";
import { Link } from "react-router";

import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { adminMenu } from "@/nav/menu";

/** The map title [app/bms/COADM01.bms:L79], naming the menu landmark. */
const MENU_LANDMARK_LABEL = "Admin Menu";

export default function AdminHomePage(): JSX.Element {
  // BLITZY [A11Y]: the links keep the generated Button focus ring, the
  // built-in `ring-ring/50` token of index.css, about 1.5:1 against the card
  // background, below the WCAG AA 3:1 non-text minimum. Kept as the shadcn
  // token set defines it; flagged for designer review.
  return (
    <div className="flex flex-col gap-6">
      <PageHeader />
      {/* A landmark distinct from the sidebar's "Main" navigation, holding
          the options as a list: one column at md, two at lg. */}
      <nav aria-label={MENU_LANDMARK_LABEL}>
        <ul className="grid gap-4 lg:grid-cols-2">
          {adminMenu.map((entry) => (
            <li key={entry.id}>
              <Card className="relative h-full">
                <CardContent className="flex items-center gap-3">
                  {/* Outside the link, so the link's accessible name is the
                      option text alone and the link variant's padding for an
                      svg child does not apply. */}
                  <entry.icon
                    aria-hidden="true"
                    className="size-5 shrink-0 text-muted-foreground"
                  />
                  {/* The link fills the whole card through its ::after box, so
                      the card is the click target rather than the text line
                      alone. The padding and height reset align the text with
                      the icon, and wrapping keeps the longest option text
                      inside a lg column beside the expanded sidebar. */}
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
      </nav>
    </div>
  );
}
