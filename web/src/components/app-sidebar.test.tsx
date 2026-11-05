// Tests of the sidebar (components/app-sidebar.tsx): which menu sections a
// user type sees, and the current-page marking.
//
// "Card management" shows for every user type and "Administration" after it
// for type A alone, each section listing its nav/menu.ts entries in option
// order.
//
// On every route at most one link carries aria-current="page": the entry
// whose nav/menu.ts `currentRoutes` pattern the router ranks first for the
// location, whatever link targets the options share (E-61). The generated
// active style (data-active) marks that same link and no other. Those cases
// sign on an administrator, so both menu sections render.
import { render, screen, within } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AppSidebar } from "@/components/app-sidebar";
import { SidebarProvider } from "@/components/ui/sidebar";
import { clearSession, setSession, type Session } from "@/lib/session";
import { adminMenu, userMenu } from "@/nav/menu";

interface CurrentCase {
  pathname: string;
  current: string | null;
}

// A sidebar link list: its accessible name and its link names in order.
interface Section {
  name: string | null;
  links: (string | null)[];
}

interface SectionsCase {
  usrType: string;
  sections: Section[];
}

const CARD_MANAGEMENT: Section = {
  name: "Card management",
  links: userMenu.map((entry) => entry.label),
};

const ADMINISTRATION: Section = {
  name: "Administration",
  links: adminMenu.map((entry) => entry.label),
};

// The sidebar alone at `pathname`, under the provider app-layout gives it.
function renderSidebar(pathname: string): void {
  const router = createMemoryRouter(
    [
      {
        path: "*",
        element: (
          <SidebarProvider>
            <AppSidebar />
          </SidebarProvider>
        ),
      },
    ],
    { initialEntries: [pathname] },
  );
  render(<RouterProvider router={router} />);
}

// Every link of the sidebar's navigation landmark, in DOM order.
function sidebarLinks(): HTMLElement[] {
  return within(screen.getByRole("navigation", { name: "Main" })).getAllByRole(
    "link",
  );
}

// Every link list of the sidebar's navigation landmark, in DOM order, named
// through its aria-labelledby group label as assistive technology names it.
function sidebarSections(): Section[] {
  return within(screen.getByRole("navigation", { name: "Main" }))
    .getAllByRole("list")
    .map((list) => ({
      name:
        document.getElementById(list.getAttribute("aria-labelledby") ?? "")
          ?.textContent ?? null,
      links: within(list)
        .getAllByRole("link")
        .map((link) => link.textContent),
    }));
}

describe("AppSidebar sections", () => {
  // session.ts keeps the session in a module variable with an expiry timer,
  // which only clearSession resets.
  afterEach(() => {
    clearSession();
  });

  it.each<SectionsCase>([
    { usrType: "A", sections: [CARD_MANAGEMENT, ADMINISTRATION] },
    { usrType: "U", sections: [CARD_MANAGEMENT] },
    // Only the exact code A is an administrator.
    { usrType: "X", sections: [CARD_MANAGEMENT] },
  ])("shows user type $usrType its sections in order", ({
    usrType,
    sections,
  }) => {
    setSession({
      token: "test-token",
      user_id: "TESTUSER",
      usr_type: usrType,
      landing: "/",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    });
    renderSidebar("/");

    expect(sidebarSections()).toEqual(sections);
  });
});

describe("AppSidebar current page", () => {
  beforeEach(() => {
    const session: Session = {
      token: "test-token",
      user_id: "ADMIN001",
      usr_type: "A",
      landing: "/admin",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
    };
    setSession(session);
  });

  // session.ts keeps the session in a module variable with an expiry timer,
  // which only clearSession resets.
  afterEach(() => {
    clearSession();
  });

  it.each<CurrentCase>([
    { pathname: "/", current: "Account View" },
    { pathname: "/accounts/1", current: "Account View" },
    { pathname: "/accounts/1/edit", current: "Account Update" },
    { pathname: "/cards", current: "Credit Card List" },
    { pathname: "/cards/4111", current: "Credit Card View" },
    { pathname: "/cards/4111/edit", current: "Credit Card Update" },
    { pathname: "/transactions", current: "Transaction List" },
    { pathname: "/transactions/0000000000683580", current: "Transaction View" },
    { pathname: "/transactions/new", current: "Transaction Add" },
    // The router ignores case and a trailing slash, and so does the marking.
    { pathname: "/Transactions/New/", current: "Transaction Add" },
    { pathname: "/reports/transactions", current: "Transaction Reports" },
    { pathname: "/payments", current: "Bill Payment" },
    { pathname: "/authorizations", current: "Pending Authorization View" },
    { pathname: "/authorizations/2", current: "Pending Authorization View" },
    { pathname: "/rewards", current: "Rewards" },
    // No option links to the admin landing, so no link is its current page.
    { pathname: "/admin", current: null },
    { pathname: "/admin/users", current: "User List (Security)" },
    { pathname: "/admin/users/new", current: "User Add (Security)" },
    {
      pathname: "/admin/users/USER0001/edit",
      current: "User Update (Security)",
    },
    {
      pathname: "/admin/transaction-types",
      current: "Transaction Type List/Update (Db2)",
    },
    {
      pathname: "/admin/transaction-types/new",
      current: "Transaction Type Maintenance (Db2)",
    },
    {
      pathname: "/admin/transaction-types/01/edit",
      current: "Transaction Type Maintenance (Db2)",
    },
    { pathname: "/admin/reward-rules", current: "Reward Rules" },
    { pathname: "/admin/reward-rules/new", current: "Reward Rules" },
    { pathname: "/admin/reward-rules/3/edit", current: "Reward Rules" },
  ])("marks $current as the current page on $pathname", ({
    pathname,
    current,
  }) => {
    renderSidebar(pathname);
    const links = sidebarLinks();

    const marked = links
      .filter((link) => link.getAttribute("aria-current") === "page")
      .map((link) => link.textContent);
    expect(marked).toEqual(current === null ? [] : [current]);

    expect(
      links.map((link) => [
        link.textContent,
        link.getAttribute("data-active") === "true",
      ]),
    ).toEqual(
      links.map((link) => [
        link.textContent,
        link.getAttribute("aria-current") === "page",
      ]),
    );
  });
});
