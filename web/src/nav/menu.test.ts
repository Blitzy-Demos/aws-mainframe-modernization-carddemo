// Tests of the current-item patterns of nav/menu.ts (`currentRoutes`), which
// decide the one sidebar link marked as the current page: no pattern belongs
// to two entries, and every link target belongs to the first option linking
// there, so of the options sharing a target (E-61) one alone owns it.
import { describe, expect, it } from "vitest";

import { menu } from "./menu";

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
