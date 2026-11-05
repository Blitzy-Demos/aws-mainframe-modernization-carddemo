// Tests of the shared summary card (components/summary-card.tsx): its figure
// is the value string byte for byte on one unbroken line, at text-2xl up to
// ten characters and, past that, at the size its content box steps to.
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { SummaryCard } from "./summary-card";

afterEach(() => {
  cleanup();
});

// The fixed size of a short figure, and the content-box steps of a long one
// on the 2rem line of text-2xl.
const FIXED = "text-2xl";
const STEPPED = "text-sm leading-8 @3xs:text-xl @xs:text-2xl";
// Every type-size and line-height class a figure can carry.
const SIZE_CLASSES = [
  "text-sm",
  "text-xl",
  "text-2xl",
  "leading-8",
  "@3xs:text-xl",
  "@xs:text-2xl",
];

describe("SummaryCard", () => {
  it.each([
    { value: "2342", sizes: FIXED },
    { value: "-1020.00", sizes: FIXED },
    // Ten characters, the longest figure kept at text-2xl, and eleven.
    { value: "9999999.99", sizes: FIXED },
    { value: "99999999.99", sizes: STEPPED },
    { value: "9007199254740993", sizes: STEPPED },
    { value: "9223372036854775807", sizes: STEPPED },
    { value: "-9999999999.99", sizes: STEPPED },
  ])(
    "shows $value exactly on one line at $sizes",
    ({ value, sizes }) => {
      render(<SummaryCard title="Available points" value={value} />);

      const card = screen.getByRole("group", { name: "Available points" });
      const figure = within(card).getByText(value, { exact: true });
      expect(figure.tagName).toBe("P");
      expect(figure.textContent).toBe(value);
      expect(figure.classList.contains("whitespace-nowrap")).toBe(true);
      const expected = sizes.split(" ");
      for (const name of SIZE_CLASSES) {
        expect(figure.classList.contains(name), name).toBe(
          expected.includes(name),
        );
      }
      // The steps are container queries on the card's content box.
      const content = figure.parentElement;
      expect(content?.getAttribute("data-slot")).toBe("card-content");
      expect(content?.classList.contains("@container")).toBe(true);
    },
  );
});
