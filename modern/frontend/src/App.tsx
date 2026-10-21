import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, RouterProvider } from "react-router";
import { routes } from "@/routes";

/**
 * Server state for every screen.
 *
 * Nothing retries, so a server `ScreenError` reaches the message row at once, as the 3270 message
 * row showed the program's message. Nothing refetches on window focus, because a COBOL screen
 * re-reads its data only when a key is pressed.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: false, refetchOnWindowFocus: false },
    mutations: { retry: false },
  },
});

/** Data mode, because the route table loads each screen through its route's `lazy`. */
const router = createBrowserRouter(routes);

/** The application root: query client and router, nothing else. */
export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
