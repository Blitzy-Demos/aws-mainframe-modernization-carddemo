// Tests of the shared detail list (components/detail-list.tsx): the Y/N flag
// badge's variant for each flag value, and the list's column tracks and
// wrapping, which keep a value without spaces inside its card at `md`, with
// a key's edit texts under its value.
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import {
  DetailItem,
  DetailList,
  DetailValueSkeleton,
  FlagBadge,
} from "./detail-list";

afterEach(() => {
  cleanup();
});

describe("FlagBadge", () => {
  it.each([
    { value: "Y", variant: "secondary" },
    { value: "N", variant: "destructive" },
    { value: "X", variant: "secondary" },
  ])(
    "shows the flag $value as its letter in a $variant badge",
    ({ value, variant }) => {
      render(<FlagBadge value={value} className="py-0" />);

      const badge = screen.getByText(value);
      expect(badge.getAttribute("data-slot")).toBe("badge");
      expect(badge.getAttribute("data-variant")).toBe(variant);
      expect(badge.classList.contains("py-0")).toBe(true);
    },
  );

  it.each([{ value: "" }, { value: " " }])(
    "renders nothing for the blank flag '$value'",
    ({ value }) => {
      const { container } = render(<FlagBadge value={value} />);

      expect(container.innerHTML).toBe("");
    },
  );
});

describe("DetailList", () => {
  it("lays out one shrinkable column below lg and two from lg, with the edit texts under the value", () => {
    const { container } = render(
      <DetailList className="lg:grid-flow-col">
        <DetailItem label="Address">{"W".repeat(50)}</DetailItem>
        <DetailItem
          label="Card Number"
          errors={[{ message: "Card number not provided" }]}
        >
          0000000000000000
        </DetailItem>
      </DetailList>,
    );

    const list = container.querySelector("dl");
    if (list === null) {
      throw new Error("the values are not in a definition list");
    }
    // Explicit minmax(0, 1fr) tracks at every width, so no value's content
    // sizes a column, plus the caller's placement classes.
    for (const name of ["grid", "grid-cols-1", "lg:grid-cols-2"]) {
      expect(list.classList.contains(name)).toBe(true);
    }
    expect(list.classList.contains("lg:grid-flow-col")).toBe(true);

    const terms = Array.from(list.querySelectorAll("dt"));
    expect(terms.map((term) => term.textContent)).toEqual([
      "Address",
      "Card Number",
    ]);
    for (const term of terms) {
      // Each item may shrink below its content and each value wraps anywhere.
      expect(term.parentElement?.classList.contains("min-w-0")).toBe(true);
      const value = term.nextElementSibling;
      expect(value?.tagName).toBe("DD");
      expect(value?.classList.contains("wrap-anywhere")).toBe(true);
    }

    const [, cardTerm] = terms;
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toBe("Card number not provided");
    expect(cardTerm.nextElementSibling?.contains(alert)).toBe(true);
  });

  it("holds a value line's placeholder hidden from assistive technology", () => {
    const { container } = render(<DetailValueSkeleton className="w-48" />);

    const line = container.firstElementChild;
    expect(line?.getAttribute("aria-hidden")).toBe("true");
    expect(line?.classList.contains("h-6")).toBe(true);
    const bar = line?.querySelector('[data-slot="skeleton"]');
    expect(bar?.classList.contains("w-48")).toBe(true);
  });
});
