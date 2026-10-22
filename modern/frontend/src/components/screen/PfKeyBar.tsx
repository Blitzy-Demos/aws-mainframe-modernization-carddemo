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
  /**
   * Whether the key is refused for now, as the 3270 keyboard is locked while the program runs.
   * Screens set it while a request is outstanding. Its button is disabled and its key does
   * nothing: the keydown is consumed, so it neither runs the action, nor reaches the browser
   * (F5 does not reload), nor counts as an invalid key, nor submits the form.
   */
  readonly disabled?: boolean;
};

/** Props of {@link PfKeyBar}. */
export type PfKeyBarProps = {
  /** The map's row-24 keys in field order, each key at most once. */
  keys: readonly PfKeyBarEntry[];
  /** `id` of the screen's `<form>`, which the ENTER button and the Enter key submit. */
  formId?: string;
  /** Settings of the keyboard binding, forwarded to `usePfKeys`; `busy` silences `onInvalidKey`. */
  options?: PfKeyOptions;
  /**
   * Whether the screen waits for a response, rendered as `aria-busy` on the key row. While busy,
   * an F-key the screen does not serve is consumed without calling `options.onInvalidKey`, so the
   * locked keyboard shows no invalid-key message; a screen without `onInvalidKey` leaves such keys
   * to the browser, busy or not. The keys the screen serves follow their own `disabled`.
   */
  busy?: boolean;
};

/** Submits the form whose `id` is `formId`, as a click on its submit button would. */
function submitForm(formId: string | undefined): void {
  const form = formId === undefined ? null : document.getElementById(formId);
  if (form instanceof HTMLFormElement) {
    form.requestSubmit();
  }
}

/**
 * The action of a refused key: none. Bound in place of the key's own action, it still makes the
 * key one the screen takes, so `usePfKeys` consumes its keydown instead of leaving it to the
 * browser or reporting it as an invalid key.
 */
const refuseKey = (): void => undefined;

/**
 * The keyboard actions of `keys`, hidden keys included: a hidden label does not stop the key,
 * because the program still receives that AID and decides what it does (COACTUPC treats an
 * inapplicable PF5 or PF12 as ENTER, `app/cbl/COACTUPC.cbl:905-916`; COCRDUPC likewise,
 * `app/cbl/COCRDUPC.cbl:413-424`). A disabled key, ENTER included, is bound to `refuseKey`.
 */
function pfKeyHandlers(keys: readonly PfKeyBarEntry[], formId: string | undefined): PfKeyHandlers {
  const handlers: PfKeyHandlers = {};
  for (const entry of keys) {
    if (entry.disabled === true) {
      handlers[entry.key] = refuseKey;
    } else if (entry.key === "ENTER") {
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
 * a form reset never receives the click event as its own argument. A disabled key has none.
 */
function pressHandler({ onPress, disabled }: PfKeyBarEntry): (() => void) | undefined {
  return onPress === undefined || disabled === true
    ? undefined
    : () => {
        onPress();
      };
}

/**
 * The click handler of the ENTER button. With an action, the click runs it instead of submitting,
 * exactly as the Enter key does; without one, the click submits the form natively. A disabled
 * ENTER neither runs its action nor submits.
 */
function enterHandler({
  onPress,
  disabled,
}: PfKeyBarEntry): ((event: MouseEvent<HTMLButtonElement>) => void) | undefined {
  if (disabled === true) {
    return (event) => {
      event.preventDefault();
    };
  }
  return onPress === undefined
    ? undefined
    : (event) => {
        event.preventDefault();
        onPress();
      };
}

/**
 * The settings `usePfKeys` receives: `options` as the screen passes them, except that while
 * `busy` a screen's invalid-key message gives way to `refuseKey`, so an unserved F-key is still
 * consumed but shows nothing. Without `onInvalidKey` the settings stay as given, busy or not.
 */
function keyOptionsFor(options: PfKeyOptions | undefined, busy: boolean): PfKeyOptions | undefined {
  return busy && options?.onInvalidKey !== undefined
    ? { ...options, onInvalidKey: refuseKey }
    : options;
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
 * A disabled entry's button carries the HTML `disabled` attribute, shown by the `Button`'s own
 * disabled style, and its key is consumed with no action. A disabled ENTER button is a disabled
 * default button, so an Enter typed in a field of the form submits nothing either. `busy` marks
 * the key row `aria-busy` while the screen waits for a response.
 *
 * This is the only caller of `usePfKeys` on a screen: a second caller would bind a second window
 * listener and run every key's action twice. Browsers keep some F-keys (F5 reloads, F12 opens the
 * developer tools, AAP 0.4.4), so every action is reachable through its button as well.
 */
export function PfKeyBar({ keys, formId, options, busy = false }: PfKeyBarProps): ReactNode {
  const handlers = useMemo(() => pfKeyHandlers(keys, formId), [keys, formId]);
  const settings = useMemo(() => keyOptionsFor(options, busy), [options, busy]);
  usePfKeys(handlers, settings);

  return (
    <div role="group" aria-label="Function keys" aria-busy={busy} className="flex flex-wrap gap-2">
      {keys.map((entry) =>
        entry.key === "ENTER" ? (
          <Button
            key={entry.key}
            type="submit"
            variant="default"
            size="sm"
            form={formId}
            hidden={entry.hidden ?? entry.dark}
            disabled={entry.disabled}
            onClick={enterHandler(entry)}
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
            disabled={entry.disabled}
            onClick={pressHandler(entry)}
          >
            {entry.label}
          </Button>
        ),
      )}
    </div>
  );
}
