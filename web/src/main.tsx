// The SPA's entry module, loaded by index.html into its #root element. It owns
// the application-wide providers; App holds the route table.
//
// Provider order, outermost first:
// - ThemeProvider wraps everything, so the Toaster and every page read the one
//   light/dark theme it keeps in sessionStorage.
// - QueryClientProvider holds the single query cache the pages' TanStack Query
//   hooks share. Nothing persists it: data is fetched on navigation or action.
// - TooltipProvider gives every tooltip one context, including those outside
//   the sidebar's own provider.
// - The Toaster, the only one in the app, is App's sibling rather than part of
//   a route: a toast raised just before a navigation (sign-off, a successful
//   save) survives it, and the toast list follows the page content in document
//   and Tab order.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

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

// Mutations are never retried: a write the server may already have applied is
// sent exactly once, and its answer, success or COBOL message, is shown.
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: shouldRetryQuery,
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
    },
    mutations: {
      retry: false,
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
          <Toaster position="top-right" closeButton />
        </TooltipProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </StrictMode>,
);
