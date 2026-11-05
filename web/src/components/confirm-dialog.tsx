// The application's single Y/N confirmation. It replaces the 3270 CONFIRM
// field of COTRN02 (CT02) and COBIL00 (CB00) and the confirming key of a
// pending action: COUSR03 F5=Delete (CU03), the COTRTLI delete (CTLI),
// COPAU01 F5=Mark/Remove Fraud (CPVD) and the reward-rule soft delete. Pages
// render this component and never compose AlertDialog themselves.
//
// The dialog is controlled and has no trigger: the page opens it from its own
// Button, and focus returns to that button on close unless the page has
// moved it while the dialog closed (an error's field, say). A button that has
// left the document by then, such as the Delete of a deleted record's row,
// hands focus to its nearest focusable ancestor still in the document (the
// data table), never to another record's button. The action reports the
// choice through onConfirm, in which the page starts its request (with
// `confirm: "Y"` where the resource carries a confirmation); Radix then
// closes the dialog through onOpenChange(false). The page shows the outcome
// through lib/api-errors, and a focus move that outcome makes goes through
// afterClose, described below. Cancel and Escape stand for the COBOL `N`:
// onOpenChange(false) without onConfirm.
//
// While the dialog is on the page, its closing animation included, Radix
// hides the rest of the page from assistive technology (aria-hidden), so
// focus moved there would sit inside hidden content. The answer to the
// request can arrive in that time, so onConfirm receives `afterClose`: a
// focus move the answer makes (onto the field an error names, or the first
// field of a cleared form) is handed to it and runs once the dialog has left
// the page, or at once when it already has. The focus that move places
// stands instead of the opener's; a move that places none, such as an error
// naming no field, leaves focus to the opener or, once the opener has left
// the document, to its nearest focusable ancestor still in it.
//
// Keyboard, Escape and outside-click handling is the Radix AlertDialog's own
// and is not overridden: focus is trapped inside, Cancel takes focus on open,
// Tab moves between Cancel and the action, Enter activates the focused button,
// Escape closes (pending included, since the caller owns `open`) and an
// outside click is ignored. Focus placement is adjusted only where Radix would
// leave it on the body, as described in the component: on an open while
// pending, on close, and on a press of the overlay. The content also carries
// aria-modal, the alertdialog semantics the Radix content does not set.
//
// Every visible word comes from the caller, so the buttons' accessible names
// are exactly the labels docs/parity/function-map.md lists for the
// dialog-action rows. The action keeps the library's default Button variant
// for every confirmation: the component mapping names no variant and the
// props carry none, so an add and a delete confirm alike.
import {
  useCallback,
  useEffect,
  useRef,
  type JSX,
  type ReactNode,
} from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

/**
 * Runs `move`, a focus move of the page's, once the dialog has left the page
 * and the page is no longer hidden from assistive technology, or at once when
 * no dialog is on the page. Moves handed over while the dialog closes run in
 * the order given.
 */
export type AfterClose = (move: () => void) => void;

export type ConfirmDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The dialog's heading and its accessible name. */
  title: string;
  /** What confirming does, for example the record about to be deleted. */
  description: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  /**
   * The action's choice. A focus move the request's answer makes goes through
   * `afterClose`, which keeps it until the closing dialog has left the page.
   */
  onConfirm: (afterClose: AfterClose) => void;
  /**
   * While the caller's request runs: content busy, and both buttons disabled
   * while the dialog is open.
   */
  pending?: boolean;
};

// The ancestors of an element that can take focus (a tabindex, such as the
// data table's or the main region's -1), nearest first.
function focusableAncestors(element: HTMLElement): HTMLElement[] {
  const found: HTMLElement[] = [];
  for (
    let node = element.parentElement;
    node !== null;
    node = node.parentElement
  ) {
    if (node.hasAttribute("tabindex")) {
      found.push(node);
    }
  }
  return found;
}

// Where focus returns on close: the opener while it is in the document, else
// the nearest of the ancestors recorded at open that still is, else none (the
// Radix default stands). Read when focus is returned, not at close, because
// a page's focus move may remove the opener's row in between.
function returnTarget(
  opener: HTMLElement | null,
  ancestors: readonly HTMLElement[],
): HTMLElement | undefined {
  return opener?.isConnected
    ? opener
    : ancestors.find((ancestor) => ancestor.isConnected);
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  cancelLabel = "Cancel",
  onConfirm,
  pending = false,
}: ConfirmDialogProps): JSX.Element {
  // Radix restores focus on close only to an AlertDialogTrigger and, with
  // none, leaves it on the body, so a keyboard user would restart from the top
  // of the page. The element focused when the dialog opens (the page's own
  // button) is kept and refocused instead, the way the focus scope restores
  // it. An opener that has left the document, such as the row of a deleted
  // record, cannot be: its focusable ancestors are recorded at open, because
  // a removed element no longer reaches them, and the nearest one still in
  // the document takes focus. Only with none left does the Radix default
  // stand.
  const openerRef = useRef<HTMLElement | null>(null);
  const openerAncestorsRef = useRef<HTMLElement[]>([]);

  // Whether the content is on the page, and the page behind it hidden: from
  // the content's open autofocus to its close autofocus, which Radix fires
  // once the closing animation has removed the content and released the
  // page. The moves afterClose receives in that time wait in the queue. One
  // function serves every confirmation, so a page may keep it past a render.
  const presentRef = useRef(false);
  const queuedMovesRef = useRef<(() => void)[]>([]);
  const afterClose = useCallback<AfterClose>((move) => {
    if (presentRef.current) {
      queuedMovesRef.current.push(move);
    } else {
      move();
    }
  }, []);

  // `open` as last committed, for the overlay check below, which runs a task
  // after the blur that scheduled it and must not act on a closing dialog.
  const openRef = useRef(open);
  useEffect(() => {
    openRef.current = open;
  }, [open]);

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent
        aria-modal="true"
        aria-busy={pending}
        onOpenAutoFocus={(event) => {
          presentRef.current = true;
          const opener =
            document.activeElement instanceof HTMLElement
              ? document.activeElement
              : null;
          openerRef.current = opener;
          openerAncestorsRef.current =
            opener === null ? [] : focusableAncestors(opener);
          // Cancel takes focus on open. Opened while pending, Cancel is
          // disabled and cannot, which would leave focus on the body outside
          // the trapped dialog; the content takes it instead, as the Radix
          // focus scope does when nothing inside is tabbable.
          if (pending && event.currentTarget instanceof HTMLElement) {
            event.preventDefault();
            event.currentTarget.focus({ preventScroll: true });
          }
        }}
        onCloseAutoFocus={(event) => {
          const opener = openerRef.current;
          const ancestors = openerAncestorsRef.current;
          openerRef.current = null;
          openerAncestorsRef.current = [];
          presentRef.current = false;
          // This runs once the close animation has removed the content and
          // the page is visible again. A request the action sent may have
          // answered meanwhile and handed its focus move to afterClose: onto
          // the field an error names (lib/api-errors) or the first field of
          // a cleared form. Those moves run now and their focus stands. A
          // move may focus on a later task (react-hook-form's setFocus does,
          // on its own timer), so whether one placed focus is read on the
          // task after theirs: with focus still on the body, where the
          // removed dialog left it, the opener takes it or, once its row has
          // gone, its nearest focusable ancestor still in the document.
          const moves = queuedMovesRef.current.splice(0);
          if (moves.length > 0) {
            event.preventDefault();
            try {
              for (const move of moves) {
                move();
              }
            } finally {
              setTimeout(() => {
                const placed = document.activeElement;
                const returned = returnTarget(opener, ancestors);
                if (
                  (placed === null || placed === document.body) &&
                  returned !== undefined
                ) {
                  returned.focus({ preventScroll: true });
                }
              }, 0);
            }
            return;
          }
          // With no move handed over, focus the page placed itself on one of
          // its own controls stands. Focus on the body was still in the
          // removed dialog, and only then is the opener refocused or, once
          // its row has gone, its nearest focusable ancestor still in the
          // document.
          const active = document.activeElement;
          if (
            active instanceof HTMLElement &&
            active !== document.body &&
            active.isConnected
          ) {
            event.preventDefault();
            return;
          }
          const target = returnTarget(opener, ancestors);
          if (target !== undefined) {
            event.preventDefault();
            target.focus({ preventScroll: true });
          }
        }}
        onBlur={(event) => {
          // A press on the overlay keeps the dialog open but, the overlay
          // taking no focus, moves focus to the body, and the Radix focus
          // trap ignores a blur with no element receiving focus. Once the
          // press has settled, on the next task, the element that lost focus
          // takes it back, or the content when that element can no longer
          // hold it. A switch to another window keeps focus on its element,
          // and a closing dialog hands focus on in onCloseAutoFocus, so
          // neither is touched.
          if (event.relatedTarget !== null || !openRef.current) {
            return;
          }
          const content = event.currentTarget;
          const lost = event.target;
          setTimeout(() => {
            const active = document.activeElement;
            if (
              !openRef.current ||
              !content.isConnected ||
              (active !== null && active !== document.body)
            ) {
              return;
            }
            const target =
              lost instanceof HTMLElement &&
              lost.isConnected &&
              content.contains(lost) &&
              !lost.matches(":disabled")
                ? lost
                : content;
            target.focus({ preventScroll: true });
          }, 0);
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          {/* A div in place of the default p, so a description may hold block
              content (such as the fields of the user about to be deleted)
              without invalid nesting. Radix still links it to the dialog as
              its accessible description. */}
          <AlertDialogDescription asChild>
            <div>{description}</div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        {/* The buttons are disabled while the caller's request runs only as
            long as the dialog is open, so the button pressed keeps focus while
            the dialog closes: a disabled one would drop it to the body for the
            whole closing animation. A press of the action in that time is
            ignored instead, so the choice is reported once. */}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending && open}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction
            disabled={pending && open}
            onClick={(event) => {
              if (!open) {
                event.preventDefault();
                return;
              }
              onConfirm(afterClose);
            }}
          >
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
