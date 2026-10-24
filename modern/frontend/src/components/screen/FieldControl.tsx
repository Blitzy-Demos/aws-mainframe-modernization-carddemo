import type { ComponentProps, ReactNode } from "react";
import { FormControl, useFormField } from "@/components/ui/form";

/**
 * Props of {@link FieldControl}: those of `FormControl`, whose `aria-describedby` it computes,
 * and what the field's `FormItem` renders to describe the input.
 */
export type FieldControlProps = Omit<ComponentProps<typeof FormControl>, "aria-describedby"> & {
  /** The field's `FormItem` renders a `FormDescription`, such as a map hint like `(Y/N)`. */
  described?: boolean;
  /**
   * Ids of rendered elements outside the `FormItem` that describe the field, such as one hint
   * shared by several inputs. They are named in the order given, after the description.
   */
  hintIds?: readonly string[];
  /**
   * The item's `FormMessage` renders text the screen passes as its children, for a mark the
   * screen keeps outside react-hook-form. A react-hook-form error of the field takes precedence,
   * as it does in `FormMessage`.
   */
  messaged?: boolean;
};

/**
 * The `FormControl` of a map field, naming in `aria-describedby` only the elements the screen
 * renders, so every id an input references resolves.
 *
 * The generated `FormControl` names the item's `FormDescription` id whether or not one is
 * rendered, and adds its `FormMessage` id whenever the field holds an error, though `FormMessage`
 * renders nothing for an empty message. On a map field without a hint, or with an error of no
 * text, those ids would resolve to nothing. This control names, in order, the description when
 * `described`, the `hintIds`, and the message while `FormMessage` shows text: the field's error
 * message when it holds one, otherwise the children `messaged` declares. With none of them the
 * attribute is left out.
 *
 * The value is passed to `FormControl` itself, whose props replace its own value. The Radix
 * `Slot` it renders joins a child's `aria-describedby` to the slot's, so an `aria-describedby` on
 * the child `Input` could add ids but never drop one. `id`, `aria-invalid` and the slotted child
 * stay as `FormControl` and the child set them.
 *
 * The contract with the screen: the same `FormItem` renders `<FormMessage className="sr-only" />`
 * (the map's message row is its one visible message), a `FormDescription` when `described` is
 * set, and every element `hintIds` names. `src/components/ui/form.tsx` stays as the shadcn CLI
 * generated it; this component is the one place that corrects its references for all screens.
 */
export function FieldControl({
  described = false,
  hintIds = [],
  messaged = false,
  ...props
}: FieldControlProps): ReactNode {
  const { error, formDescriptionId, formMessageId } = useFormField();
  // `FormMessage` shows the error's message when the field holds one, otherwise its children.
  const shown = error !== undefined ? String(error.message ?? "") !== "" : messaged;
  const ids = [described ? formDescriptionId : "", ...hintIds, shown ? formMessageId : ""].filter(
    (id) => id !== "",
  );
  return <FormControl {...props} aria-describedby={ids.length > 0 ? ids.join(" ") : undefined} />;
}
