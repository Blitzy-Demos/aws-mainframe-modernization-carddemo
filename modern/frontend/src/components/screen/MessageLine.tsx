import type { ReactNode } from "react";
import { cn } from "cn";
import { Alert, AlertDescription } from "@/components/ui/alert";

/**
 * The colour of the message a {@link MessageLine} shows, which picks its `Alert` variant. The
 * colour only: whether the text is bright is the row's `bright`.
 *
 * - `error`: RED, the colour of the `ERRMSG` field every map declares at `POS=(23,1)`, `LENGTH=78`
 *   (80 on COCRDSL and COCRDUP), `ATTRB=(ASKIP,BRT,FSET)`, `COLOR=RED` (for example
 *   `app/bms/COSGN00.bms:197-200`).
 * - `info`: the foreground, for the `INFOMSG` field (`COLOR=NEUTRAL`, `HILIGHT=OFF`), which only
 *   COACTUP, COACTVW, COCRDLI, COCRDSL, COCRDUP, COTRTLI and COTRTUP declare, and for a success
 *   text a program shows in `ERRMSG` after recolouring the field GREEN or NEUTRAL.
 */
export type MessageKind = "error" | "info";

/** Props of {@link MessageLine}. */
export type MessageLineProps = {
  /**
   * The message to show, as the screen received it: the server's message when there is one,
   * otherwise the screen's own validation message. Absent, empty or all spaces shows no box.
   */
  text?: string;
  /** The message's colour, which sets the `Alert` variant. */
  kind: MessageKind;
  /**
   * Whether the row's BMS field shows bright (`BRT`), which maps to `font-semibold` (AAP 0.4.3).
   * It follows the field's attribute, not `kind`: a program that recolours a bright field keeps
   * it bright.
   */
  bright: boolean;
};

/**
 * One message row of a map, rendered as a shadcn/ui `Alert`. `kind` gives the colour: the
 * `destructive` variant for RED, the `default` variant for the foreground. `bright` gives the
 * emphasis: `font-semibold` for a `BRT` field, the normal weight otherwise. Placement on the screen
 * is the caller's.
 *
 * The `Alert` is rendered even when there is no message, with a transparent border so no empty box
 * shows, because a live region must exist before its first message for screen readers to announce
 * it. `aria-live="polite"` overrides the assertive default of the `Alert`'s `role="alert"`, so a
 * message that follows another is announced without interrupting. `data-message-kind` names the
 * row's colour, which tells the error row from the information row on maps that carry both.
 * `data-message-empty` marks a row that shows no message, so `src/index.css` can keep its border
 * invisible in forced-colors mode, which overrides `transparent`.
 *
 * The text is shown verbatim, spacing included, and is never composed, formatted or logged here:
 * the server owns every message, and any card number in one already arrives redacted.
 */
export function MessageLine({ text, kind, bright }: MessageLineProps): ReactNode {
  const isError = kind === "error";
  const isEmpty = !text?.trim();
  return (
    <Alert
      variant={isError ? "destructive" : "default"}
      aria-live="polite"
      data-message-kind={kind}
      data-message-empty={isEmpty || undefined}
      className={cn(
        // RED is the `--destructive` token itself (AAP 0.4.3); the destructive variant gives the
        // description that token at 90 percent, which this replaces in the merge.
        isError && "*:data-[slot=alert-description]:text-destructive",
        isEmpty && "border-transparent",
      )}
    >
      {/*
       * `min-h-5` is one `text-sm` line, so the row keeps its height while empty and the rows
       * below do not move when a message arrives. An error's colour is the `Alert`'s
       * `--destructive`. NEUTRAL and GREEN map to the foreground token, not to the
       * `muted-foreground` that `AlertDescription` applies by default (the token of the BLUE
       * labels), so the information text inherits the default variant's `text-card-foreground`.
       * `BRT` maps to `font-semibold` whatever the colour.
       */}
      <AlertDescription
        className={cn(
          "min-h-5 whitespace-pre-wrap break-words",
          !isError && "text-inherit",
          bright && "font-semibold",
        )}
      >
        {text ?? ""}
      </AlertDescription>
    </Alert>
  );
}
