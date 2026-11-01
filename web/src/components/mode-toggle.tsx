// The header's light/dark switch. Choosing an item only calls setTheme; the
// ThemeProvider applies the `dark` class and keeps the choice in
// sessionStorage. Light and dark are the only themes, so there is no "System"
// item. Keyboard handling (Enter or Space opens, arrows move, Escape closes)
// is the Radix menu's own.
import type { JSX } from "react";
import { Moon, Sun } from "lucide-react";

import { useTheme } from "@/components/theme-provider";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function ModeToggle(): JSX.Element {
  const { setTheme } = useTheme();

  // BLITZY [LAYOUT]: the shadcn mode-toggle example sizes the icons 1.2rem, an
  // arbitrary value outside the Tailwind default scale; snapped to size-5
  // (1.25rem). The icon swap animates only without a reduced-motion request.
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" className="relative">
          <Sun
            aria-hidden="true"
            className="size-5 scale-100 rotate-0 motion-safe:transition-all dark:scale-0 dark:-rotate-90"
          />
          <Moon
            aria-hidden="true"
            className="absolute size-5 scale-0 rotate-90 motion-safe:transition-all dark:scale-100 dark:rotate-0"
          />
          <span className="sr-only">Toggle theme</span>
        </Button>
      </DropdownMenuTrigger>
      {/* Plain items rather than radio items, so each item's accessible name
          is exactly its label. */}
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => setTheme("light")}>
          Light
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setTheme("dark")}>
          Dark
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
