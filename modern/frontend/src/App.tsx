import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createBrowserRouter, RouterProvider } from "react-router";
import { routes } from "@/routes";

/**
 * Server state for every screen.
 *
 * Nothing retries, so a server `ScreenError` reaches the message row at once, as the 3270 message
 * row showed the program's message. Nothing refetches on window focus or when the network
 * returns, because a COBOL screen re-reads its data only when a key is pressed: the menus read
 * their option table on entry, and again only when ENTER follows a load that failed.
 *
 * Every query and mutation runs whatever the browser reports about its network (`networkMode`
 * `"always"`): nothing is paused while the browser is offline, and nothing paused is sent by
 * itself when the network returns, perhaps from another screen. A request made offline goes to
 * `fetch` at once, whose network `TypeError` reaches the message row and frees the keys as any
 * other failure does; only the operator's next key sends anything again.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
      networkMode: "always",
    },
    mutations: { retry: false, networkMode: "always" },
  },
});

/**
 * Data mode, because the route table loads each screen through its route's `loader` and shows a
 * failed one through its root error boundary.
 */
const router = createBrowserRouter(routes);

/** The application root: query client and router, nothing else. */
export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
