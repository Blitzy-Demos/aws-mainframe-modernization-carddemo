// Tests of the single error display (lib/api-errors.ts): the text each kind
// of failure is toasted with, the unknown-outcome note a write caller passes,
// which is shown only when the request got no usable answer, and its
// forwarding through applyApiError. The toast is Sonner's, spied on.
import type { UseFormReturn } from "react-hook-form";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError, RequestTimeoutError } from "@/api/client";
import type { components } from "@/api/schema";

import {
  applyApiError,
  noUsableAnswer,
  showApiError,
  UNEXPECTED_ERROR,
  type ApiErrorOptions,
  type BmsFieldMap,
} from "./api-errors";

type ErrorBody = components["schemas"]["ErrorBody"];

// A form of two fields, the CB00 entry.
type Values = { account_id: string; confirm: string };

const fieldMap: BmsFieldMap<"ACTIDIN" | "CONFIRM", Values> = {
  ACTIDIN: "account_id",
  CONFIRM: "confirm",
};

const NOTE = "The payment may have been made. Check before paying again.";
const WITH_NOTE: ApiErrorOptions = { unknownOutcome: NOTE };

// The client's text for a request past its deadline (api/client.ts).
const DEADLINE_TEXT = new RequestTimeoutError().message;

const REFUSED = "Account ID NOT found...";

function body(message: string, fields: ErrorBody["fields"] = []): ErrorBody {
  return { code: "NOT_FOUND", message, fields };
}

beforeEach(() => {
  vi.spyOn(toast, "error");
});

afterEach(() => {
  toast.dismiss();
  vi.restoreAllMocks();
});

describe("showApiError", () => {
  const CASES: readonly {
    failure: string;
    error: unknown;
    options?: ApiErrorOptions;
    status: number | null;
    shown: readonly unknown[];
  }[] = [
    {
      failure: "the client's deadline",
      error: new RequestTimeoutError(),
      status: null,
      shown: [DEADLINE_TEXT],
    },
    {
      failure: "a network failure",
      error: new TypeError("Failed to fetch"),
      status: null,
      shown: [UNEXPECTED_ERROR],
    },
    {
      failure: "a 404 with its text",
      error: new ApiError(404, body(REFUSED)),
      status: 404,
      shown: [REFUSED],
    },
    {
      failure: "a 500 with a blank text",
      error: new ApiError(500, body("   ")),
      status: 500,
      shown: [UNEXPECTED_ERROR],
    },
    {
      failure: "the client's deadline, with the note",
      error: new RequestTimeoutError(),
      options: WITH_NOTE,
      status: null,
      shown: [DEADLINE_TEXT, { description: NOTE, duration: Infinity }],
    },
    {
      failure: "a network failure, with the note",
      error: new TypeError("Failed to fetch"),
      options: WITH_NOTE,
      status: null,
      shown: [UNEXPECTED_ERROR, { description: NOTE, duration: Infinity }],
    },
    {
      failure: "a 201 whose body could not be read, with the note",
      error: new SyntaxError("Unexpected end of JSON input"),
      options: WITH_NOTE,
      status: null,
      shown: [UNEXPECTED_ERROR, { description: NOTE, duration: Infinity }],
    },
    {
      failure: "a 201 without a body, with the note",
      error: new ApiError(201, body(UNEXPECTED_ERROR)),
      options: WITH_NOTE,
      status: 201,
      shown: [UNEXPECTED_ERROR, { description: NOTE, duration: Infinity }],
    },
    {
      failure: "a 404, with the note",
      error: new ApiError(404, body(REFUSED)),
      options: WITH_NOTE,
      status: 404,
      shown: [REFUSED],
    },
    {
      failure: "a 500, with the note",
      error: new ApiError(500, body("Unable to Update Account...")),
      options: WITH_NOTE,
      status: 500,
      shown: ["Unable to Update Account..."],
    },
    {
      failure: "the client's deadline, with a blank note",
      error: new RequestTimeoutError(),
      options: { unknownOutcome: "  " },
      status: null,
      shown: [DEADLINE_TEXT],
    },
    {
      failure: "a 401 that ended the session, with the note",
      error: new ApiError(401, body("Authentication required"), true),
      options: WITH_NOTE,
      status: 401,
      shown: ["Authentication required", { id: "session-ended" }],
    },
  ];

  it.each(CASES)(
    "toasts $failure as one error",
    ({ error, options, status, shown }) => {
      const result = showApiError(error, options);

      expect(vi.mocked(toast.error).mock.calls).toEqual([shown]);
      expect(result).toEqual({ status, message: shown[0] });
    },
  );
});

describe("noUsableAnswer", () => {
  it.each([
    { failure: "the client's deadline", error: new RequestTimeoutError(), none: true },
    { failure: "a network failure", error: new TypeError("Failed to fetch"), none: true },
    { failure: "a 201 without a body", error: new ApiError(201, body("")), none: true },
    { failure: "a 400", error: new ApiError(400, body(REFUSED)), none: false },
    { failure: "a 503", error: new ApiError(503, body("")), none: false },
  ])("is $none for $failure", ({ error, none }) => {
    expect(noUsableAnswer(error)).toBe(none);
  });
});

describe("applyApiError", () => {
  // The two form methods applyApiError calls, recorded.
  function stubForm() {
    const setError = vi.fn();
    const setFocus = vi.fn();
    const form: Pick<UseFormReturn<Values>, "setError" | "setFocus"> = {
      setError,
      setFocus,
    };
    return { form, setError, setFocus };
  }

  it("passes the note to the toast of a request past the deadline and marks no field", () => {
    const { form, setError, setFocus } = stubForm();

    const applied = applyApiError(
      new RequestTimeoutError(),
      form,
      fieldMap,
      WITH_NOTE,
    );

    expect(vi.mocked(toast.error).mock.calls).toEqual([
      [DEADLINE_TEXT, { description: NOTE, duration: Infinity }],
    ]);
    expect(applied).toEqual({ status: null, message: DEADLINE_TEXT, fields: [] });
    expect(setError).not.toHaveBeenCalled();
    expect(setFocus).not.toHaveBeenCalled();
  });

  it("still marks and focuses the fields of a refusal given the note, which it does not show", () => {
    const { form, setError, setFocus } = stubForm();
    const error = new ApiError(
      404,
      body(REFUSED, [{ field: "ACTIDIN", message: REFUSED }]),
    );

    const applied = applyApiError(error, form, fieldMap, WITH_NOTE);

    expect(vi.mocked(toast.error).mock.calls).toEqual([[REFUSED]]);
    expect(applied).toEqual({
      status: 404,
      message: REFUSED,
      fields: ["account_id"],
    });
    expect(setError).toHaveBeenCalledWith("account_id", {
      type: "server",
      message: REFUSED,
    });
    expect(setFocus).toHaveBeenCalledWith("account_id");
  });
});
