// Tests of ConfirmDialog (components/confirm-dialog.tsx), the application's
// single Y/N confirmation: the focus move a page hands to `afterClose` waits
// until the dialog has left the page and then stands, or runs at once once
// the dialog has gone; a move that places no focus, Cancel and Escape return
// focus to the opener; a press of the overlay leaves the open dialog holding
// focus; the content is a modal alertdialog; and through the closing
// animation the pressed action keeps focus, ignores further presses and holds
// back a move handed over meanwhile.
//
// jsdom runs no CSS animation, so Radix removes the content as soon as the
// dialog closes, and its close autofocus follows on the next task. The
// closing tests give the dialog an exit animation of their own.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type JSX } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConfirmDialog, type AfterClose } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type User = ReturnType<typeof userEvent.setup>;

const TITLE = "Delete the record";

type PageProps = {
  onConfirm?: (afterClose: AfterClose) => void;
  pending?: boolean;
};

// A page as the form pages are: its own button opens the dialog, a confirm
// starts its request, which stays pending, and a field elsewhere on the page
// is where an answer moves the cursor. `pending` is a request already running.
function Page({ onConfirm = () => undefined, pending = false }: PageProps): JSX.Element {
  const [open, setOpen] = useState(false);
  const [running, setRunning] = useState(pending);
  return (
    <>
      <Button type="button" onClick={() => setOpen(true)}>
        Delete
      </Button>
      <Input aria-label="Field" />
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={TITLE}
        description="The record will be deleted."
        confirmLabel="Confirm"
        onConfirm={(afterClose) => {
          setRunning(true);
          onConfirm(afterClose);
        }}
        pending={running}
      />
    </>
  );
}

const DIALOG_PARTS =
  '[data-slot="alert-dialog-content"], [data-slot="alert-dialog-overlay"]';
const EXIT_ANIMATION = "exit";

// Radix keeps a closing dialog on the page until its exit animation ends, and
// jsdom runs no animation, so the dialog would leave at once. This gives the
// content and the overlay an exit animation while they are closed, as the
// generated classes do, and returns the function that ends it.
function animateDialogExit(): () => void {
  const computed = window.getComputedStyle.bind(window);
  vi.spyOn(window, "getComputedStyle").mockImplementation((element, pseudo) => {
    const styles = computed(element, pseudo);
    if (!element.matches(DIALOG_PARTS)) {
      return styles;
    }
    return new Proxy(styles, {
      get(target, property) {
        if (property === "animationName") {
          return element.getAttribute("data-state") === "closed"
            ? EXIT_ANIMATION
            : "none";
        }
        const value: unknown = Reflect.get(target, property, target);
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
  });
  return () => {
    act(() => {
      for (const part of document.querySelectorAll(DIALOG_PARTS)) {
        const end = new Event("animationend");
        Object.defineProperty(end, "animationName", { value: EXIT_ANIMATION });
        part.dispatchEvent(end);
      }
    });
  };
}

/** The dialog content while it is on the page, closing included. */
function dialogContent(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-slot="alert-dialog-content"]');
}

function renderPage(props: PageProps = {}): User {
  const user = userEvent.setup();
  render(<Page {...props} />);
  return user;
}

function opener(): HTMLElement {
  return screen.getByRole("button", { name: "Delete" });
}

function field(): HTMLElement {
  return screen.getByRole("textbox", { name: "Field" });
}

/** Opens the dialog from its opener, which then holds the page's focus. */
async function openDialog(user: User): Promise<HTMLElement> {
  await user.click(opener());
  return screen.findByRole("alertdialog", { name: TITLE });
}

/** Lets the timer tasks queued so far run. */
async function nextTask(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => {
      setTimeout(resolve, 0);
    });
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("ConfirmDialog focus handoff", () => {
  it.each<{ name: string; focus: (target: HTMLElement) => void }>([
    { name: "at once", focus: (target) => target.focus() },
    {
      // react-hook-form's setFocus focuses on a timer of its own.
      name: "on a later timer",
      focus: (target) => {
        setTimeout(() => target.focus(), 0);
      },
    },
  ])(
    "runs a move handed over while the dialog is shown once it has left, and its focus $name stands",
    async ({ focus }) => {
      let shownWhenMoved: boolean | null = null;
      const move = vi.fn(() => {
        shownWhenMoved = screen.queryByRole("alertdialog") !== null;
        // By label: a shown dialog hides the page from role queries.
        focus(screen.getByLabelText("Field"));
      });
      const user = renderPage({ onConfirm: (afterClose) => afterClose(move) });

      const dialog = await openDialog(user);
      await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

      await waitFor(() => {
        expect(document.activeElement).toBe(field());
      });
      expect(move).toHaveBeenCalledTimes(1);
      expect(shownWhenMoved).toBe(false);
      // The opener does not take focus back afterwards.
      await nextTask();
      await nextTask();
      expect(document.activeElement).toBe(field());
    },
  );

  it("runs a move handed over after the dialog has left at once", async () => {
    const kept: AfterClose[] = [];
    const user = renderPage({ onConfirm: (afterClose) => kept.push(afterClose) });

    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(document.activeElement).toBe(opener());
    });

    const move = vi.fn(() => field().focus());
    expect(kept).toHaveLength(1);
    act(() => {
      for (const afterClose of kept) {
        afterClose(move);
      }
    });
    expect(move).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(field());
  });

  it("returns focus to the opener when the move handed over places none", async () => {
    // An error that names no field: its toast is shown and the cursor stays.
    const move = vi.fn(() => undefined);
    const user = renderPage({ onConfirm: (afterClose) => afterClose(move) });

    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: "Confirm" }));

    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(document.activeElement).toBe(opener());
    });
    expect(move).toHaveBeenCalledTimes(1);
  });

  it.each<{ how: string; dismiss: (user: User, dialog: HTMLElement) => Promise<void> }>([
    {
      how: "Cancel",
      dismiss: (user, dialog) =>
        user.click(within(dialog).getByRole("button", { name: "Cancel" })),
    },
    { how: "Escape", dismiss: (user) => user.keyboard("{Escape}") },
  ])("returns focus to the opener on $how, without onConfirm", async ({ dismiss }) => {
    const onConfirm = vi.fn();
    const user = renderPage({ onConfirm });

    const dialog = await openDialog(user);
    expect(document.activeElement).toBe(
      within(dialog).getByRole("button", { name: "Cancel" }),
    );
    await dismiss(user, dialog);

    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(document.activeElement).toBe(opener());
    });
    expect(onConfirm).not.toHaveBeenCalled();
  });
});

describe("ConfirmDialog while shown", () => {
  it("is a modal alertdialog", async () => {
    const user = renderPage();

    const dialog = await openDialog(user);

    expect(dialog.getAttribute("aria-modal")).toBe("true");
  });

  it.each<{ name: string; pending: boolean; holder: (dialog: HTMLElement) => HTMLElement }>([
    {
      name: "Cancel, focused on open",
      pending: false,
      holder: (dialog) => within(dialog).getByRole("button", { name: "Cancel" }),
    },
    {
      // Both buttons are disabled while pending, so the content holds focus.
      name: "the content, focused on a pending open",
      pending: true,
      holder: (dialog) => dialog,
    },
  ])(
    "keeps focus on $name through a press of the overlay, and Escape still closes",
    async ({ pending, holder }) => {
      const onConfirm = vi.fn();
      const user = renderPage({ onConfirm, pending });

      const dialog = await openDialog(user);
      const focused = holder(dialog);
      expect(document.activeElement).toBe(focused);
      const overlay = document.querySelector<HTMLElement>(
        '[data-slot="alert-dialog-overlay"]',
      );
      if (overlay === null) {
        throw new Error("the dialog renders no overlay");
      }

      await user.click(overlay);
      await nextTask();

      // The press closes nothing and leaves focus inside the dialog.
      expect(screen.getByRole("alertdialog", { name: TITLE })).toBe(dialog);
      expect(document.activeElement).toBe(focused);
      expect(onConfirm).not.toHaveBeenCalled();

      await user.keyboard("{Escape}");
      await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).toBeNull();
        expect(document.activeElement).toBe(opener());
      });
    },
  );
});

describe("ConfirmDialog while closing", () => {
  it("keeps focus on the pressed action through the closing animation and reports the choice once", async () => {
    const endExit = animateDialogExit();
    const onConfirm = vi.fn();
    const user = renderPage({ onConfirm });

    const dialog = await openDialog(user);
    const confirm = within(dialog).getByRole("button", { name: "Confirm" });
    await user.click(confirm);

    // The request runs while the dialog closes: the action stays enabled and
    // focused rather than dropping focus to the body.
    expect(dialogContent()?.getAttribute("data-state")).toBe("closed");
    expect(confirm.hasAttribute("disabled")).toBe(false);
    expect(document.activeElement).toBe(confirm);
    await user.click(confirm);
    await user.keyboard("{Enter}");
    expect(onConfirm).toHaveBeenCalledTimes(1);

    endExit();
    await waitFor(() => {
      expect(dialogContent()).toBeNull();
      expect(document.activeElement).toBe(opener());
    });
  });

  it("runs a move handed over during the closing animation once it has ended", async () => {
    const endExit = animateDialogExit();
    const kept: AfterClose[] = [];
    const user = renderPage({ onConfirm: (afterClose) => kept.push(afterClose) });

    const dialog = await openDialog(user);
    const confirm = within(dialog).getByRole("button", { name: "Confirm" });
    await user.click(confirm);
    expect(dialogContent()?.getAttribute("data-state")).toBe("closed");

    // The answer arrives while the closing dialog still hides the page.
    const move = vi.fn(() => screen.getByLabelText("Field").focus());
    act(() => {
      for (const afterClose of kept) {
        afterClose(move);
      }
    });
    await nextTask();
    expect(move).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(confirm);

    endExit();
    await waitFor(() => {
      expect(dialogContent()).toBeNull();
      expect(document.activeElement).toBe(field());
    });
    expect(move).toHaveBeenCalledTimes(1);
    await nextTask();
    expect(document.activeElement).toBe(field());
  });
});
