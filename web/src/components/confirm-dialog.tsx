// The application's single Y/N confirmation. It replaces the 3270 CONFIRM
// field of COTRN02 (CT02) and COBIL00 (CB00) and the confirming key of a
// pending action: COUSR03 F5=Delete (CU03), the COTRTLI delete (CTLI),
// COPAU01 F5=Mark/Remove Fraud (CPVD) and the reward-rule soft delete. Pages
// render this component and never compose AlertDialog themselves.
//
// The dialog is controlled and has no trigger: the page opens it from its own
// Button, and focus returns to that button on close. The action only
// reports the choice through onConfirm, after which Radix closes the dialog
// through onOpenChange(false); the page then sends its request (with
// `confirm: "Y"` where the resource carries a confirmation) and shows the
// outcome through lib/api-errors. Cancel and Escape stand for the COBOL `N`:
// onOpenChange(false) without onConfirm.
//
// Keyboard, Escape and outside-click handling is the Radix AlertDialog's own
// and is not overridden: focus is trapped inside, Cancel takes focus on open,
// Tab moves between Cancel and the action, Enter activates the focused button,
// Escape closes (pending included, since the caller owns `open`) and an
// outside click is ignored. Focus placement is adjusted only where Radix would
// leave it on the body, as described in the component.
//
// Every visible word comes from the caller, so the buttons' accessible names
// are exactly the labels docs/parity/function-map.md lists for the
// dialog-action rows. The action keeps the library's default Button variant
// for every confirmation: the component mapping names no variant and the
// props carry none, so an add and a delete confirm alike.
import { useRef, type JSX, type ReactNode } from "react";

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

export type ConfirmDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The dialog's heading and its accessible name. */
  title: string;
  /** What confirming does, for example the record about to be deleted. */
  description: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  onConfirm: () => void;
  /** While the caller's request runs: both buttons disabled, content busy. */
  pending?: boolean;
};

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
  // record, keeps the Radix default.
  const openerRef = useRef<HTMLElement | null>(null);

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent
        aria-busy={pending}
        onOpenAutoFocus={(event) => {
          openerRef.current =
            document.activeElement instanceof HTMLElement
              ? document.activeElement
              : null;
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
          openerRef.current = null;
          if (opener?.isConnected) {
            event.preventDefault();
            opener.focus({ preventScroll: true });
          }
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
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction disabled={pending} onClick={() => onConfirm()}>
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
