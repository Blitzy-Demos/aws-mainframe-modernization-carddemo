// Tests of the theme provider (components/theme-provider.tsx): the stored
// theme applied to the root element when the module loads, before anything
// renders; a theme change through setTheme, which swaps the class under the
// theme-switching class that index.css turns transitions off with; and a first
// mount over a class already in place, which changes nothing.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { JSX } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Button } from "@/components/ui/button";

import {
  THEME_STORAGE_KEY,
  ThemeProvider,
  useTheme,
  type Theme,
} from "./theme-provider";

type ThemeProviderModule = typeof import("./theme-provider");

const THEME_SWITCHING_CLASS = "theme-switching";

// A fresh instance of the module, so its load-time code runs again against
// the storage the test arranged.
async function loadThemeProvider(): Promise<ThemeProviderModule> {
  vi.resetModules();
  return import("./theme-provider");
}

function classesOf(value: string): string[] {
  return value.split(/\s+/).filter((token) => token !== "");
}

// Starts recording the root element's class attribute. The returned function
// stops and gives every value the attribute took, in order, starting with the
// value it had when recording started.
function watchRootClass(): () => string[] {
  const root = document.documentElement;
  const before: string[] = [];
  const keep = (records: MutationRecord[]): void => {
    before.push(...records.map((record) => record.oldValue ?? ""));
  };
  const observer = new MutationObserver(keep);
  observer.observe(root, {
    attributeFilter: ["class"],
    attributeOldValue: true,
  });
  return () => {
    keep(observer.takeRecords());
    observer.disconnect();
    return [...before, root.className];
  };
}

function ThemeSwitch(): JSX.Element {
  const { theme, setTheme } = useTheme();
  const next: Theme = theme === "light" ? "dark" : "light";
  return (
    <Button
      onClick={() => {
        setTheme(next);
      }}
    >
      Switch to {next}
    </Button>
  );
}

afterEach(() => {
  vi.restoreAllMocks();
  document.documentElement.className = "";
});

describe("theme applied when the module loads", () => {
  it.each<{ stored: string | null; initial: string; expected: string }>([
    { stored: "dark", initial: "", expected: "dark" },
    { stored: "light", initial: "dark", expected: "light" },
    { stored: "blue", initial: "", expected: "" },
    { stored: null, initial: "", expected: "" },
  ])(
    "stored $stored turns root class $initial into $expected",
    async ({ stored, initial, expected }) => {
      if (stored !== null) {
        sessionStorage.setItem(THEME_STORAGE_KEY, stored);
      }
      document.documentElement.className = initial;

      await loadThemeProvider();

      expect(document.documentElement.className).toBe(expected);
    },
  );

  it("leaves the root element alone when sessionStorage is blocked", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });

    await expect(loadThemeProvider()).resolves.toBeDefined();

    expect(document.documentElement.className).toBe("");
  });
});

describe("ThemeProvider", () => {
  it.each<{ from: Theme; to: Theme }>([
    { from: "light", to: "dark" },
    { from: "dark", to: "light" },
  ])(
    "switches from $from to $to with transitions off during the swap",
    async ({ from, to }) => {
      // The state a load of the module leaves in a tab that chose `from`.
      sessionStorage.setItem(THEME_STORAGE_KEY, from);
      document.documentElement.className = from;
      render(
        <ThemeProvider>
          <ThemeSwitch />
        </ThemeProvider>,
      );
      const user = userEvent.setup();

      const stop = watchRootClass();
      await user.click(
        screen.getByRole("button", { name: `Switch to ${to}` }),
      );
      const values = stop();

      expect(document.documentElement.className).toBe(to);
      expect(sessionStorage.getItem(THEME_STORAGE_KEY)).toBe(to);
      const firstWithNewTheme = values
        .map(classesOf)
        .find((classes) => classes.includes(to));
      expect(firstWithNewTheme).toContain(THEME_SWITCHING_CLASS);
      expect(
        screen.getByRole("button", { name: `Switch to ${from}` }),
      ).toBeTruthy();
    },
  );

  it("changes nothing on a first mount over the stored theme's class", () => {
    sessionStorage.setItem(THEME_STORAGE_KEY, "dark");
    document.documentElement.className = "dark";

    const stop = watchRootClass();
    render(
      <ThemeProvider>
        <ThemeSwitch />
      </ThemeProvider>,
    );

    expect(stop()).toEqual(["dark"]);
    expect(
      screen.getByRole("button", { name: "Switch to light" }),
    ).toBeTruthy();
  });
});
