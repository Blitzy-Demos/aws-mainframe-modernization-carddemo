// Vitest setup file (vite.config.ts `test.setupFiles`), loaded before every
// test file in jsdom. Vitest globals are off, so Testing Library registers no
// automatic cleanup and this file does it.
//
// The stubs below fill browser APIs jsdom does not implement. Each is
// installed only when jsdom lacks the API, so a jsdom release that adds one
// keeps its own implementation.
import { cleanup } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach } from "vitest";

// sessionStorage holds the signed-on session (lib/session.ts) and the theme
// (components/theme-provider.tsx); clearing it keeps tests independent. The
// persistent stores are deliberately left alone: the no-persistent-preferences
// test asserts they stay empty, and wiping them here would hide a leak from an
// earlier test in the same file.
//
// Sonner keeps its toasts in module state, and an unmounted Toaster's timers
// never retire them, so the Toaster the next test mounts would replay them and
// that test could find an earlier test's toast. Dismissing them all once
// cleanup() has unmounted every Toaster leaves no subscriber to update.
afterEach(() => {
  cleanup();
  toast.dismiss();
  sessionStorage.clear();
});

// Consumers: hooks/use-mobile.ts (a change listener on the max-width 767px
// query) and components/app-layout.tsx (the min-width 1024px query drives the
// controlled SidebarProvider). Every query reports no match, so the shell
// starts as the icon rail on every run. The query text is never parsed.
if (typeof window.matchMedia !== "function") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    configurable: true,
    value: (query: string): MediaQueryList => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }),
  });
}

// Consumers: Radix use-size, popper (dropdown menu, select content) and
// tooltip. jsdom performs no layout, so there is never a size to report.
if (typeof globalThis.ResizeObserver === "undefined") {
  class ResizeObserverStub implements ResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = ResizeObserverStub;
}

// Consumers: Radix Select and menus scroll the active item into view on open.
if (typeof Element.prototype.scrollIntoView !== "function") {
  Element.prototype.scrollIntoView = () => undefined;
}

// Consumers: the Radix Select trigger checks and releases pointer capture on
// pointerdown.
if (typeof Element.prototype.hasPointerCapture !== "function") {
  Element.prototype.hasPointerCapture = () => false;
}

if (typeof Element.prototype.releasePointerCapture !== "function") {
  Element.prototype.releasePointerCapture = () => undefined;
}
