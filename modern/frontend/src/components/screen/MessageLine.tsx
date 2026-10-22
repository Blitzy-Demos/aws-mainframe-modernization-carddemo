import type { ReactNode } from "react";
import { cn } from "cn";
import { Alert, AlertDescription } from "@/components/ui/alert";

/**
 * Which message row of a map a {@link MessageLine} renders.
 *
 * - `error`: the `ERRMSG` field every map declares at `POS=(23,1)`, `LENGTH=78` (80 on COCRDSL and
 *   COCRDUP), `ATTRB=(ASKIP,BRT,FSET)`, `COLOR=RED` (for example `app/bms/COSGN00.bms:197-200`).
 * - `info`: the `INFOMSG` field, `COLOR=NEUTRAL`, `HILIGHT=OFF`, which only COACTUP, COACTVW,
 *   COCRDLI, COCRDSL, COCRDUP, COTRTLI and COTRTUP declare.
 */
export type MessageKind = "error" | "info";

/** Props of {@link MessageLine}. */
export type MessageLineProps = {
  /**
   * The message to show, as the screen received it: the server's message when there is one,
   * otherwise the screen's own validation message. Absent, empty or all spaces shows no box.
   */
  text?: string;
  /** Which of the map's message rows this is. */
  kind: MessageKind;
};

/**
 * One message row of a map, rendered as a shadcn/ui `Alert`: `destructive` for `ERRMSG` (RED),
 * `default` for `INFOMSG` (NEUTRAL). Placement on the screen is the caller's.
 *
 * The `Alert` is rendered even when there is no message, with a transparent border so no empty box
 * shows, because a live region must exist before its first message for screen readers to announce
 * it. `aria-live="polite"` overrides the assertive default of the `Alert`'s `role="alert"`, so a
 * message that follows another is announced without interrupting. `data-message-kind` tells the
 * error row from the information row on maps that carry both.
 *
 * The text is shown verbatim, spacing included, and is never composed, formatted or logged here:
 * the server owns every message, and any card number in one already arrives redacted.
 */
export function MessageLine({ text, kind }: MessageLineProps): ReactNode {
  const isError = kind === "error";
  return (
    <Alert
      variant={isError ? "destructive" : "default"}
      aria-live="polite"
      data-message-kind={kind}
      className={cn(!text?.trim() && "border-transparent")}
    >
      {/*
       * `min-h-5` is one `text-sm` line, so the row keeps its height while empty and the rows
       * below do not move when a message arrives. ERRMSG's `BRT` maps to `font-semibold`; its
       * colour is the destructive variant's. INFOMSG is NEUTRAL, which maps to the foreground
       * token, not to the `muted-foreground` that `AlertDescription` applies by default (the token
       * of the BLUE labels), so the information text inherits the default variant's
       * `text-card-foreground`.
       */}
      <AlertDescription
        className={cn(
          "min-h-5 whitespace-pre-wrap break-words",
          isError ? "font-semibold" : "text-inherit",
        )}
      >
        {text ?? ""}
      </AlertDescription>
    </Alert>
  );
}
