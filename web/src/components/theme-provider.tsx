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

// Reading sessionStorage throws where the browser blocks storage (sandboxed
// frames, some privacy settings); the default theme applies then, as it does
// for a missing or unrecognised stored value.
function readStoredTheme(fallback: Theme): Theme {
  try {
    const stored = window.sessionStorage.getItem(THEME_STORAGE_KEY);
    return isTheme(stored) ? stored : fallback;
  } catch {
    return fallback;
  }
}

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
  const [theme, setThemeState] = useState<Theme>(() =>
    readStoredTheme(defaultTheme),
  );

  // The theme class is applied after React commits the provider's render and
  // before the browser paints that commit, so the first frame React renders,
  // and every later theme change, already shows the chosen theme's tokens. It
  // does not govern what the document paints before the bundle runs and React
  // mounts, when the :root (light) defaults of index.css apply. index.css
  // switches the tokens on the `dark` class of the root element; `light` is
  // set for symmetry and selects the :root defaults.
  useLayoutEffect(() => {
    const root = window.document.documentElement;
    root.classList.remove("light", "dark");
    root.classList.add(theme);
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
