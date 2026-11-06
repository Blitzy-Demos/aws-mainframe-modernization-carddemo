// What a page route shows in place of its page: the skeleton while the page's
// chunk downloads, and the error panel when the page cannot be shown.
//
// - PageFallback is the loading state. A page route shows it on the document
//   load (its hydrateFallbackElement, routes/lazy-page.tsx), the app shell
//   shows it in the page content while the router downloads the next page
//   (components/app-layout.tsx), and Redirect shows it until the screen it
//   redirects to has arrived.
// - PageLoadError is the errorElement: in the shell's page content for every
//   signed-on route (App.tsx), with the sidebar and header in place, and as a
//   page of its own, with its own main landmark, for /login. It names the
//   page that failed through PageHeader, whose crumbs and heading come from
//   the routes the failed navigation matched, and offers a retry: "Try again"
//   re-runs the failed route's loader, which downloads the page again, and
//   "Reload page" loads the whole application again, the way back from a
//   deployment that replaced the chunks the running application names.
import { useEffect, useRef, useState, type JSX } from "react";
import { LoaderCircle } from "lucide-react";
import {
  Navigate,
  useRevalidator,
  useRouteError,
  type NavigateProps,
} from "react-router";

import { PageHeader } from "@/components/page-header";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

/** The outline of a page heading and its content, shown while it loads. */
export function PageFallback(): JSX.Element {
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label="Loading"
      className="flex flex-col gap-4"
    >
      <Skeleton className="h-8 w-48" />
      <Skeleton className="h-4 w-72" />
      <Skeleton className="h-32 w-full" />
    </div>
  );
}

/**
 * A redirect, the route element that sends the visitor on to another screen
 * (the guards' redirects, the unknown-path route). The router commits the
 * destination once its page's chunk has arrived, and the skeleton stands in
 * until then for the screen the redirect leaves.
 */
export function Redirect(props: NavigateProps): JSX.Element {
  return (
    <>
      <Navigate {...props} />
      <PageFallback />
    </>
  );
}

export type PageLoadErrorProps = {
  /**
   * Renders the panel as a page of its own, inside its own main landmark,
   * for a route outside the app shell (/login). Inside the shell the panel
   * sits in the shell's page content, its single main landmark.
   */
  standalone?: boolean;
};

function hasLostFocus(): boolean {
  const active = document.activeElement;
  return active === null || active === document.body;
}

/**
 * The errorElement of the page routes: the failed page's header, an alert
 * that it could not be loaded, and the "Try again" and "Reload page" buttons.
 * "Try again" stays disabled while its retry runs.
 */
export function PageLoadError({
  standalone = false,
}: PageLoadErrorProps): JSX.Element {
  const error = useRouteError();
  const revalidator = useRevalidator();
  const retrying = revalidator.state === "loading";
  const panel = useRef<HTMLDivElement>(null);
  const tryAgain = useRef<HTMLButtonElement>(null);
  const retried = useRef(false);

  // A retry that fails again hands this element a new error. The alert is
  // then a new node, so assistive technology announces it again.
  const [shown, setShown] = useState({ error, attempt: 0 });
  if (shown.error !== error) {
    setShown({ error, attempt: shown.attempt + 1 });
  }

  // The router hands the error to this element rather than reporting it, so
  // its cause still reaches the console. Disabling the focused "Try again"
  // for the retry drops the focus to the body; a failed retry gives it back.
  useEffect(() => {
    console.error("The page could not be loaded.", error);
    if (retried.current && hasLostFocus()) {
      tryAgain.current?.focus();
    }
  }, [error]);

  // A retry that succeeds replaces this panel by the page at the same
  // location, so the shell's route-change handling does not run, and focus
  // goes to the page content as it would on a route change, unless the page
  // has placed it (autoFocus applies before this cleanup, a mount-time focus
  // after it). The standalone panel's own main leaves with it, and the page
  // takes the focus. StrictMode's extra cleanup on mount, and a navigation
  // away, which moves the focus itself, leave it alone.
  useEffect(() => {
    const main = panel.current?.closest("main") ?? null;
    return () => {
      if (retried.current && main?.isConnected === true && hasLostFocus()) {
        main.focus({ preventScroll: true });
      }
    };
  }, []);

  const retry = (): void => {
    retried.current = true;
    void revalidator.revalidate();
  };

  const content = (
    <>
      <PageHeader />
      <div ref={panel} className="flex flex-col items-start gap-4">
        <p key={shown.attempt} role="alert">
          This page could not be loaded. Check your network connection, then try
          again.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            ref={tryAgain}
            type="button"
            disabled={retrying}
            onClick={retry}
          >
            {retrying ? (
              <LoaderCircle
                aria-hidden="true"
                className="motion-safe:animate-spin"
              />
            ) : null}
            Try again
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              window.location.reload();
            }}
          >
            Reload page
          </Button>
        </div>
      </div>
    </>
  );

  // The standalone layout is LoginPage's, so the panel stands where the
  // sign-on card would.
  return standalone ? (
    <main className="flex min-h-svh flex-col items-center justify-center gap-6 p-6">
      <div className="flex w-full max-w-sm flex-col gap-6">{content}</div>
    </main>
  ) : (
    content
  );
}
