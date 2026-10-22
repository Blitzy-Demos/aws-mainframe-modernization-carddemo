import { useMemo, type MouseEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { usePfKeys, type PfKeyHandlers, type PfKeyItem, type PfKeyOptions } from "@/lib/pfkeys";

/**
 * One key of a map's row 24 as a screen passes it: a `keyRowItems(KEY_ROWS.<MAP>)` item (or a
 * label the screen adds for a key the map does not label) with its action and visibility.
 */
export type PfKeyBarEntry = PfKeyItem & {
  /**
   * The key's action, run by a click on its button and by the key itself. A key other than
   * ENTER without one is not served. ENTER without one submits the screen's form.
   */
  readonly onPress?: () => void;
  /**
   * Whether the label is hidden; when omitted, the field's `DRK` flag decides. Screens pass
   * `hidden: false` once the program brightens a dark label (`F5=Save` on COACTUP).
   */
  readonly hidden?: boolean;
};

/** Props of {@link PfKeyBar}. */
export type PfKeyBarProps = {
  /** The map's row-24 keys in field order, each key at most once. */
  keys: readonly PfKeyBarEntry[];
  /** `id` of the screen's `<form>`, which the ENTER button and the Enter key submit. */
  formId?: string;
  /** Settings of the keyboard binding, forwarded to `usePfKeys` unchanged. */
  options?: PfKeyOptions;
};

/** Submits the form whose `id` is `formId`, as a click on its submit button would. */
function submitForm(formId: string | undefined): void {
  const form = formId === undefined ? null : document.getElementById(formId);
  if (form instanceof HTMLFormElement) {
    form.requestSubmit();
  }
}

/**
 * The keyboard actions of `keys`, hidden keys included: a hidden label does not stop the key,
 * because the program still receives that AID and decides what it does (COACTUPC treats an
 * inapplicable PF5 or PF12 as ENTER, `app/cbl/COACTUPC.cbl:905-916`; COCRDUPC likewise,
 * `app/cbl/COCRDUPC.cbl:413-424`).
 */
function pfKeyHandlers(keys: readonly PfKeyBarEntry[], formId: string | undefined): PfKeyHandlers {
  const handlers: PfKeyHandlers = {};
  for (const entry of keys) {
    if (entry.key === "ENTER") {
      handlers.ENTER =
        entry.onPress ??
        (() => {
          submitForm(formId);
        });
    } else if (entry.onPress !== undefined) {
      handlers[entry.key] = entry.onPress;
    }
  }
  return handlers;
}

/**
 * The click handler of a PF-key button. It calls `onPress` with no argument, so an action such as
 * a form reset never receives the click event as its own argument.
 */
function pressHandler(onPress: (() => void) | undefined): (() => void) | undefined {
  return onPress === undefined
    ? undefined
    : () => {
        onPress();
      };
}

/**
 * The click handler of the ENTER button. With an action, the click runs it instead of submitting,
 * exactly as the Enter key does; without one, the click submits the form natively.
 */
function enterHandler(
  onPress: (() => void) | undefined,
): ((event: MouseEvent<HTMLButtonElement>) => void) | undefined {
  return onPress === undefined
    ? undefined
    : (event) => {
        event.preventDefault();
        onPress();
      };
}

/**
 * The map's row-24 function-key labels as buttons, and the screen's keyboard binding.
 *
 * Each label is a `Button` whose text, and so whose accessible name, is the map's label
 * (`F3=Exit`). ENTER is the submit button of the screen's form (`form={formId}`), so a click or
 * an Enter typed in a field submits it; every other key is a plain button, so none becomes the
 * form's default button. A hidden label keeps its button with the HTML `hidden` attribute, which
 * removes it from view and from the accessibility tree; a hidden ENTER button still serves as the
 * form's default button on maps that label no ENTER key.
 *
 * This is the only caller of `usePfKeys` on a screen: a second caller would bind a second window
 * listener and run every key's action twice. Browsers keep some F-keys (F5 reloads, F12 opens the
 * developer tools, AAP 0.4.4), so every action is reachable through its button as well.
 */
export function PfKeyBar({ keys, formId, options }: PfKeyBarProps): ReactNode {
  const handlers = useMemo(() => pfKeyHandlers(keys, formId), [keys, formId]);
  usePfKeys(handlers, options);

  return (
    <div role="group" aria-label="Function keys" className="flex flex-wrap gap-2">
      {keys.map((entry) =>
        entry.key === "ENTER" ? (
          <Button
            key={entry.key}
            type="submit"
            variant="default"
            size="sm"
            form={formId}
            hidden={entry.hidden ?? entry.dark}
            onClick={enterHandler(entry.onPress)}
          >
            {entry.label}
          </Button>
        ) : (
          <Button
            key={entry.key}
            type="button"
            variant="outline"
            size="sm"
            hidden={entry.hidden ?? entry.dark}
            onClick={pressHandler(entry.onPress)}
          >
            {entry.label}
          </Button>
        ),
      )}
    </div>
  );
}
