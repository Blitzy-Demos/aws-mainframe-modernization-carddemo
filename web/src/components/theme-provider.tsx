// The application's single light/dark theme implementation. main.tsx wraps the
// app and the Toaster in ThemeProvider; ui/sonner.tsx and mode-toggle.tsx read
// and change the theme through useTheme.
//
// The choice is kept in sessionStorage, so it survives a reload in the same tab
// and every new browser session starts at the default. The shadcn Vite guide's
// provider keeps it across sessions instead, which is preference persistence
// this application leaves out of scope. Only the built-in shadcn light and dark
// token sets exist, so there is no "system" option.
import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
  type JSX,
  type ReactNode,
} from "react";

export type Theme = "light" | "dark";

export const THEME_STORAGE_KEY = "carddemo.theme";

type ThemeContextValue = {
  theme: Theme;
  setTheme: (theme: Theme) => void;
};

type ThemeProviderProps = {
  children: ReactNode;
  defaultTheme?: Theme;
};

// Used outside a provider, for example by the generated Toaster rendered on its
// own in a component test: light, with changes ignored.
const ThemeContext = createContext<ThemeContextValue>({
  theme: "light",
  setTheme: () => undefined,
});

function isTheme(value: string | null): value is Theme {
  return value === "light" || value === "dark";
}

// Carried by the root element while its theme class changes; index.css turns
// every CSS transition off under it.
const THEME_SWITCHING_CLASS = "theme-switching";

// The theme stored in this tab, or null for a missing or unrecognised value.
// Reading sessionStorage throws where the browser blocks storage (sandboxed
// frames, some privacy settings); that also gives null, so the caller's
// default applies.
function readStoredTheme(): Theme | null {
  try {
    const stored = window.sessionStorage.getItem(THEME_STORAGE_KEY);
    return isTheme(stored) ? stored : null;
  } catch {
    return null;
  }
}

// Whether the root element carries this theme's class and not the other's.
function hasThemeClass(root: HTMLElement, theme: Theme): boolean {
  const other: Theme = theme === "light" ? "dark" : "light";
  return root.classList.contains(theme) && !root.classList.contains(other);
}

function setThemeClass(root: HTMLElement, theme: Theme): void {
  root.classList.remove("light", "dark");
  root.classList.add(theme);
}

// Puts the theme stored in this tab on the root element, or leaves the element
// untouched when none is stored: the :root tokens of index.css are then the
// light theme, which is also the provider's default.
function applyStoredTheme(): void {
  const stored = readStoredTheme();
  if (stored !== null) {
    setThemeClass(window.document.documentElement, stored);
  }
}

// Runs once, while main.tsx's module graph is evaluated and before React
// renders anything. index.html loads that graph as a render-blocking script, so
// a reload or a direct visit in a tab that chose dark paints its first frame
// dark rather than light.
applyStoredTheme();

// A blocked or full sessionStorage only loses the choice on the next reload;
// the theme still changes for the page being shown.
function writeStoredTheme(theme: Theme): void {
  try {
    window.sessionStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch (error) {
    console.warn(
      `Theme "${theme}" applies to this page only: sessionStorage rejected ${THEME_STORAGE_KEY}.`,
      error,
    );
  }
}

export function ThemeProvider({
  children,
  defaultTheme = "light",
}: ThemeProviderProps): JSX.Element {
  const [theme, setThemeState] = useState<Theme>(
    () => readStoredTheme() ?? defaultTheme,
  );

  // Keeps the root element's class on the theme, after React commits and
  // before the browser paints that commit. index.css switches the tokens on the
  // `dark` class; `light` is set for symmetry and selects the :root defaults.
  // A class already in place, as applyStoredTheme leaves it for the first
  // mount, is not touched.
  //
  // Any other change happens under the theme-switching class, which turns CSS
  // transitions off. Reading a computed value makes the browser compute the
  // new theme's styles while they are off, so once the class goes again no
  // colour has a change left to animate. Without this, controls with colour
  // transitions would fade in from the old theme's colours over the new
  // background, unreadable for a moment.
  useLayoutEffect(() => {
    const root = window.document.documentElement;
    if (!hasThemeClass(root, theme)) {
      root.classList.add(THEME_SWITCHING_CLASS);
      try {
        setThemeClass(root, theme);
        void window.getComputedStyle(root).getPropertyValue("color");
      } finally {
        root.classList.remove(THEME_SWITCHING_CLASS);
      }
    }
    return () => {
      root.classList.remove(theme);
    };
  }, [theme]);

  const setTheme = useCallback((next: Theme): void => {
    writeStoredTheme(next);
    setThemeState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, setTheme }),
    [theme, setTheme],
  );

  return <ThemeContext value={value}>{children}</ThemeContext>;
}

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}
