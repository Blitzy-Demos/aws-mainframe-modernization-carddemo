// Tests of nav/menu.ts.
//
// The current-item patterns (`currentRoutes`) decide the one sidebar link
// marked as the current page: no pattern belongs to two entries, and every
// link target belongs to the first option linking there, so of the options
// sharing a target (E-61) one alone owns it.
//
// menuForUserType decides the sidebar sections: the user section for every
// user type, the admin section after it for the exact code A alone, as the
// same group objects on every call.
import { describe, expect, it } from "vitest";

import {
  MENU_SECTION_LABELS,
  adminMenu,
  menu,
  menuForUserType,
  userMenu,
  type MenuEntry,
  type MenuSection,
} from "./menu";

interface SectionsCase {
  userType: string | null | undefined;
  sections: MenuSection[];
}

const ENTRIES: Readonly<Record<MenuSection, readonly MenuEntry[]>> = {
  user: userMenu,
  admin: adminMenu,
};

describe("menuForUserType", () => {
  it.each<SectionsCase>([
    { userType: "A", sections: ["user", "admin"] },
    { userType: "U", sections: ["user"] },
    { userType: "X", sections: ["user"] },
    // The code is compared exactly, as isAdmin compares it.
    { userType: "a", sections: ["user"] },
    { userType: "", sections: ["user"] },
    { userType: null, sections: ["user"] },
    { userType: undefined, sections: ["user"] },
  ])("gives user type $userType the sections $sections", ({
    userType,
    sections,
  }) => {
    const groups = menuForUserType(userType);

    expect(groups.map((group) => group.section)).toEqual(sections);
    for (const group of groups) {
      expect(group.label).toBe(MENU_SECTION_LABELS[group.section]);
      expect(group.entries).toBe(ENTRIES[group.section]);
    }
    expect(menuForUserType(userType)).toBe(groups);
  });

  it("shares the user section object between the user types", () => {
    expect(menuForUserType("A")[0]).toBe(menuForUserType("U")[0]);
  });
});

describe("menu currentRoutes", () => {
  it("gives each route pattern to one entry at most", () => {
    const owners = new Map<string, string[]>();
    for (const entry of menu) {
      for (const pattern of entry.currentRoutes) {
        owners.set(pattern, [...(owners.get(pattern) ?? []), entry.id]);
      }
    }
    const shared = [...owners].filter(([, ids]) => ids.length > 1);
    expect(shared).toEqual([]);
  });

  it("gives each link target to the first option linking there alone", () => {
    const targets = [...new Set(menu.map((entry) => entry.route))];
    const owners = targets.map((route) => ({
      route,
      owners: menu
        .filter((entry) => entry.currentRoutes.includes(route))
        .map((entry) => entry.id),
    }));
    expect(owners).toEqual(
      targets.map((route) => ({
        route,
        owners: [menu.find((entry) => entry.route === route)?.id],
      })),
    );
  });
});
