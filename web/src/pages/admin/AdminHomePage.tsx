// CA00 admin menu (COADM01C, map COADM01) at /admin, where COSGN00C sends a
// user of type 'A' after sign-on [app/cbl/COSGN00C.cbl:L230-240]. The route
// sits inside RequireAdmin; the API's 403 on /admin/** decides access.
//
// - Each COADM02Y option is a link named by its option text, in COBOL order,
//   followed by the Reward Rules entry; the shared MenuOptionList
//   (components/app-sidebar.tsx) renders them, every label, route and icon
//   from nav/menu.ts (F-COADM01-001 to F-COADM01-006). Choosing a link
//   replaces typing the option number and pressing ENTER
//   [app/cbl/COADM01C.cbl:L98-99], so there is no OPTION field, and the
//   option-number and invalid-key messages have no counterpart
//   (F-COADM01-008, E-56).
// - F3=Exit, the sign-off XCTL to COSGN00C [app/cbl/COADM01C.cbl:L100-102],
//   is the app-shell user menu's "Log out" item (F-COADM01-007), which
//   app-layout renders; this page adds no sign-off control of its own.
// - The heading and breadcrumb come from the route handle App.tsx declares.
import type { JSX } from "react";

import { MenuOptionList } from "@/components/app-sidebar";
import { PageHeader } from "@/components/page-header";

// The map title [app/bms/COADM01.bms:L79], naming the menu landmark.
const MENU_LANDMARK_LABEL = "Admin Menu";

export default function AdminHomePage(): JSX.Element {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader />
      {/* A landmark distinct from the sidebar's "Main" navigation, holding
          the options as a list: one column at md, two at lg. */}
      <nav aria-label={MENU_LANDMARK_LABEL}>
        <MenuOptionList section="admin" className="lg:grid-cols-2" />
      </nav>
    </div>
  );
}
