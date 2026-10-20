import { describe, expect, it } from "vitest";
import { COBIL00_LENGTHS, type Cobil00Values, cobil00Schema } from "./COBIL00";

/**
 * Client half of the two `both` rows of the bill-payment map `COBIL0A` in
 * `modern/audit/screens.md`: the map `LENGTH` of `ACTIDIN` (`COBIL00-013`, 11) and of `CONFIRM`
 * (`COBIL00-020`, 1). The rows record no message, so the expected messages are zod's defaults.
 * The program's own edits, the blank account (`COBIL00C-007`) and a confirmation other than `Y`,
 * `y`, `N`, `n` or blank (`COBIL00C-012`), are `server` rows the schema does not repeat, so those
 * values pass here. Every value is synthetic.
 */

/** The `success` flag of parsing `values`, with the path and message of each issue in order. */
function check(values: Cobil00Values) {
  const result = cobil00Schema.safeParse(values);
  return {
    success: result.success,
    issues: result.error?.issues.map(({ path, message }) => ({ path, message })) ?? [],
  };
}

describe("cobil00Schema", () => {
  it.each([
    { name: "a blank confirmation, which shows the balance", acctId: "12345678901", confirm: "" },
    { name: "confirmation Y", acctId: "12345678901", confirm: "Y" },
    { name: "confirmation y", acctId: "12345678901", confirm: "y" },
    { name: "confirmation N, which clears the screen", acctId: "12345678901", confirm: "N" },
    { name: "confirmation n", acctId: "12345678901", confirm: "n" },
    { name: "a blank account ID, an edit the server owns", acctId: "", confirm: "Y" },
    { name: "confirmation X, an edit the server owns", acctId: "12345678901", confirm: "X" },
  ])("passes $name unchanged", ({ acctId, confirm }) => {
    expect(cobil00Schema.safeParse({ acctId, confirm })).toEqual({
      success: true,
      data: { acctId, confirm },
    });
  });

  // audit: COBIL00-013
  it("accepts an account ID of the map's 11 characters and rejects one of 12", () => {
    expect(COBIL00_LENGTHS.acctId).toBe(11);
    expect(check({ acctId: "12345678901", confirm: "" })).toEqual({ success: true, issues: [] });
    expect(check({ acctId: "123456789012", confirm: "" })).toEqual({
      success: false,
      issues: [{ path: ["acctId"], message: "String must contain at most 11 character(s)" }],
    });
  });

  // audit: COBIL00-020
  it("accepts a confirmation of the map's 1 character and rejects one of 2", () => {
    expect(COBIL00_LENGTHS.confirm).toBe(1);
    expect(check({ acctId: "12345678901", confirm: "Y" })).toEqual({ success: true, issues: [] });
    expect(check({ acctId: "12345678901", confirm: "YY" })).toEqual({
      success: false,
      issues: [{ path: ["confirm"], message: "String must contain at most 1 character(s)" }],
    });
  });

  it("reports the account ID first when both lengths are exceeded", () => {
    expect(check({ acctId: "123456789012", confirm: "YY" })).toEqual({
      success: false,
      issues: [
        { path: ["acctId"], message: "String must contain at most 11 character(s)" },
        { path: ["confirm"], message: "String must contain at most 1 character(s)" },
      ],
    });
  });
});
