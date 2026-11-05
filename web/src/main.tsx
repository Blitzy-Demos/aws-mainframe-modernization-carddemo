// The SPA's entry module, loaded by index.html into its #root element. It owns
// the application-wide providers; App holds the route table.
//
// Provider order, outermost first:
// - ThemeProvider wraps everything, so the Toaster and every page read the one
//   light/dark theme it keeps in sessionStorage.
// - QueryClientProvider holds the single query cache the pages' TanStack Query
//   hooks share. Nothing persists it; QUERY_STALE_TIME_MS states when a cached
//   entry is read again.
// - TooltipProvider gives every tooltip one context, including those outside
//   the sidebar's own provider.
// - The Toaster, the only one in the app, is App's sibling rather than part of
//   a route: a toast raised just before a navigation (sign-off, a successful
//   save) survives it, and the toast list follows the page content in document
//   and Tab order.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import {
  QueryClient,
  QueryClientProvider,
  type NetworkMode,
} from "@tanstack/react-query";

import App from "./App";
import { ApiError, RequestTimeoutError } from "@/api/client";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import "./index.css";

// How many times a failed query is sent again.
const QUERY_RETRY_LIMIT = 1;

// Whether a failed query is sent again. A 4xx is the server's final answer (a
// COBOL validation or not-found message, or the 401/403 access verdict), which
// asking again cannot change. A request that hit the client's deadline is not
// retried either: it already waited REQUEST_TIMEOUT_MS, and the client treats
// such a request as abandoned. A network failure or a 5xx is retried once.
function shouldRetryQuery(failureCount: number, error: Error): boolean {
  if (error instanceof ApiError && error.status >= 400 && error.status < 500) {
    return false;
  }
  if (error instanceof RequestTimeoutError) {
    return false;
  }
  return failureCount < QUERY_RETRY_LIMIT;
}

// How long, in milliseconds, a cached query result counts as current. It is
// zero: every screen reads its records again when it is entered or when the
// user acts on it (Enter, Previous/Next in place of PF7/PF8, an explicit
// refresh), as each COBOL screen re-reads its file on entry. Nothing pushes
// changes to the browser, and records change outside this tab's own writes:
// the batch jobs (POSTTRAN, INTCALC, TRANBKP and COMBTRAN), the CP00 queue
// consumer recording authorizations, and other signed-on users. A cached entry
// is therefore never taken as current without a read. After a write, its query
// module under src/api/queries invalidates the entries that write changed,
// which covers only this tab's writes.
//
// The cache still does three things. A screen shows the entry it already holds
// while the new read runs, observers of one key share a single in-flight
// request, and keepPreviousData keeps a list page on screen while Previous or
// Next loads the adjacent one. Reads happen only on navigation or action:
// window focus and network reconnection start none.
const QUERY_STALE_TIME_MS = 0;

// When a read or a write is sent: at once, whatever the browser reports about
// its connection. While the browser is offline the request fails as a network
// failure and is shown as `Unexpected error`, as when the server cannot be
// reached. It is never held unsent and then sent by itself once the browser is
// back online, which would apply a write the user did not repeat and start a
// read the user did not ask for.
const REQUEST_NETWORK_MODE: NetworkMode = "always";

// Mutations are never retried: a write the server may already have applied is
// sent exactly once, and its answer, success or COBOL message, is shown.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: QUERY_STALE_TIME_MS,
      retry: shouldRetryQuery,
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
      networkMode: REQUEST_NETWORK_MODE,
    },
    mutations: {
      retry: false,
      networkMode: REQUEST_NETWORK_MODE,
    },
  },
});

const container = document.getElementById("root");
if (container === null) {
  throw new Error("index.html has no #root element to mount the app into");
}

createRoot(container).render(
  <StrictMode>
    <ThemeProvider defaultTheme="light">
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <App />
          {/* Top-center, at Sonner's default offset, keeps toasts over the
              empty middle of the header: clear of the sidebar toggle, theme
              toggle and user menu at its ends, of the actions each page header
              places at the top right of the content, and of the pagination,
              row actions and form buttons that raise most toasts. A toast
              under the pointer pauses and stays, so it would take the next
              click meant for the control beneath it.
              Sonner injects its stylesheet outside the cascade layers, so its
              font-family on the toast list and its font-size on each toast
              outrank any Tailwind utility on those elements. The app's font
              and type scale therefore go one level in, where Sonner leaves
              each unset: font-sans on the toast, which only inherits the
              list's font-family, and text-sm on the content wrapper, which
              has no font-size of its own and passes it to the title. */}
          <Toaster
            position="top-center"
            closeButton
            toastOptions={{
              classNames: { toast: "font-sans", content: "text-sm" },
            }}
          />
        </TooltipProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </StrictMode>,
);
