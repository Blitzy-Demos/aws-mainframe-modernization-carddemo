// The request deadline and caller cancellation of the typed client
// (api/client.ts). The network is a stub that treats its signal as the
// browser does: an abort rejects a pending answer with the abort reason and
// errors a body still being received with the same reason. Fake timers stand
// in for the wait to the deadline.
import type { Middleware } from "openapi-fetch";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  type Mock,
  vi,
} from "vitest";

import {
  api,
  REQUEST_TIMEOUT_MS,
  RequestTimeoutError,
  unwrap,
} from "@/api/client";

type Network = (input: Request, init?: RequestInit) => Promise<Response>;

const ACCOUNT_ID = "00000000001";

const ACCOUNT_BODY = { acct_id: ACCOUNT_ID, active_status: "Y" };

// Calls `listener` with the signal's reason once it is aborted, at once when
// it already is.
function onAbort(
  signal: AbortSignal | null | undefined,
  listener: (reason: unknown) => void,
): void {
  if (signal === null || signal === undefined) {
    return;
  }
  if (signal.aborted) {
    listener(signal.reason);
    return;
  }
  signal.addEventListener("abort", () => listener(signal.reason), {
    once: true,
  });
}

// No status line ever arrives.
const stalledHeaders: Network = (_input, init) =>
  new Promise<Response>((_resolve, reject) => onAbort(init?.signal, reject));

// The status line and the start of the body arrive; the rest never does.
const stalledBody: Network = async (_input, init) => {
  const body = new ReadableStream<Uint8Array>({
    start(stream) {
      stream.enqueue(new TextEncoder().encode('{"acct_id":'));
      onAbort(init?.signal, (reason) => stream.error(reason));
    },
  });
  return new Response(body, {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
};

const STALLS = [
  ["no status line", stalledHeaders],
  ["a body that never ends", stalledBody],
] as const;

const NETWORK_FAILURE = new TypeError("network error");

// The status line and the start of the body arrive, then the connection
// drops.
const droppedBody: Network = async () => {
  let started = false;
  const body = new ReadableStream<Uint8Array>({
    pull(stream) {
      if (!started) {
        started = true;
        stream.enqueue(new TextEncoder().encode('{"acct_id":'));
        return;
      }
      stream.error(NETWORK_FAILURE);
    },
  });
  return new Response(body, {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
};

const ACCOUNT_JSON = JSON.stringify(ACCOUNT_BODY);

const NOT_FOUND_BODY = {
  code: "NOT_FOUND",
  message: "Account ID NOT found...",
  fields: [],
};

// Answers that end, each read by openapi-fetch its own way, and one it never
// reads: the status, the success body and the error body each yields.
const COMPLETE = [
  [
    "a JSON body with its Content-Length, read by response.json()",
    () =>
      new Response(ACCOUNT_JSON, {
        status: 200,
        headers: {
          "Content-Type": "application/json",
          "Content-Length": String(
            new TextEncoder().encode(ACCOUNT_JSON).byteLength,
          ),
        },
      }),
    { status: 200, data: ACCOUNT_BODY, error: undefined },
  ],
  [
    "a JSON body without a Content-Length, read by response.text()",
    () =>
      new Response(ACCOUNT_JSON, {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    { status: 200, data: ACCOUNT_BODY, error: undefined },
  ],
  [
    "an error body, read by response.text()",
    () =>
      new Response(JSON.stringify(NOT_FOUND_BODY), {
        status: 404,
        statusText: "Not Found",
        headers: { "Content-Type": "application/json" },
      }),
    { status: 404, data: undefined, error: NOT_FOUND_BODY },
  ],
  [
    "an empty 200 with Content-Length 0, never read",
    () =>
      new Response("", {
        status: 200,
        headers: { "Content-Length": "0" },
      }),
    { status: 200, data: undefined, error: undefined },
  ],
] as const;

// A read, and a write that must never be sent twice.
const CALLS = [
  [
    "GET /accounts/{id}",
    () => api.GET("/accounts/{id}", { params: { path: { id: ACCOUNT_ID } } }),
  ],
  [
    "POST /payments",
    () =>
      api.POST("/payments", {
        body: { account_id: ACCOUNT_ID, confirm: "Y" },
      }),
  ],
] as const;

// The caller's abort reason: none, so the default AbortError, or its own.
const ABORTS = [
  ["the default AbortError", () => undefined],
  ["the caller's own reason", () => new Error("Screen left")],
] as const;

type Outcome = { settled: boolean; error?: unknown };

// The outcome of a call, readable while the call is still pending.
function track(call: Promise<unknown>): Outcome {
  const outcome: Outcome = { settled: false };
  call.then(
    () => {
      outcome.settled = true;
    },
    (error: unknown) => {
      outcome.settled = true;
      outcome.error = error;
    },
  );
  return outcome;
}

let network: Mock<Network>;

beforeEach(() => {
  vi.useFakeTimers();
  network = vi.fn<Network>();
  vi.stubGlobal("fetch", network);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe.each(STALLS)("an answer with %s", (_stall, stall) => {
  it.each(CALLS)(
    "rejects %s with RequestTimeoutError at the deadline and sends it once",
    async (_call, call) => {
      network.mockImplementation(stall);
      const outcome = track(call());

      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
      expect(outcome.settled).toBe(false);

      await vi.advanceTimersByTimeAsync(1);
      expect(outcome.error).toBeInstanceOf(RequestTimeoutError);

      await vi.advanceTimersByTimeAsync(5 * REQUEST_TIMEOUT_MS);
      expect(network).toHaveBeenCalledTimes(1);
    },
  );

  it.each(ABORTS)(
    "rejects a call its caller aborts after 1 s with %s",
    async (_abort, reasonOf) => {
      network.mockImplementation(stall);
      const caller = new AbortController();
      const outcome = track(
        api.GET("/accounts/{id}", {
          params: { path: { id: ACCOUNT_ID } },
          signal: caller.signal,
        }),
      );

      await vi.advanceTimersByTimeAsync(1000);
      expect(outcome.settled).toBe(false);
      caller.abort(reasonOf());
      await vi.advanceTimersByTimeAsync(0);

      expect(outcome.settled).toBe(true);
      expect(outcome.error).toBe(caller.signal.reason);
      expect(outcome.error).not.toBeInstanceOf(RequestTimeoutError);
      expect(outcome.error).toHaveProperty(
        "name",
        reasonOf() === undefined ? "AbortError" : "Error",
      );

      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS);
      expect(network).toHaveBeenCalledTimes(1);
    },
  );
});

describe("a call whose signal is aborted before it starts", () => {
  it("rejects at once with the signal's reason", async () => {
    network.mockImplementation(stalledHeaders);
    const caller = new AbortController();
    caller.abort();

    const outcome = track(
      api.GET("/accounts/{id}", {
        params: { path: { id: ACCOUNT_ID } },
        signal: caller.signal,
      }),
    );
    await vi.advanceTimersByTimeAsync(0);

    expect(outcome.error).toBe(caller.signal.reason);
    expect(outcome.error).toHaveProperty("name", "AbortError");
  });
});

describe("an answer whose body fails mid-stream", () => {
  it("rejects with the network error and leaves no deadline armed", async () => {
    network.mockImplementation(droppedBody);
    const outcome = track(
      api.GET("/accounts/{id}", { params: { path: { id: ACCOUNT_ID } } }),
    );
    await vi.advanceTimersByTimeAsync(0);

    expect(outcome.error).toBe(NETWORK_FAILURE);
    expect(vi.getTimerCount()).toBe(0);

    await vi.advanceTimersByTimeAsync(5 * REQUEST_TIMEOUT_MS);
    const [, init] = network.mock.calls[0];
    expect(init?.signal?.aborted).toBe(false);
    expect(network).toHaveBeenCalledTimes(1);
  });
});

describe("a body its reader cancels", () => {
  it("disarms the deadline and passes the reason on to the network", async () => {
    const reason = new Error("Screen left");
    let cancelledWith: unknown;
    network.mockImplementation(async () => {
      const body = new ReadableStream<Uint8Array>({
        start(stream) {
          stream.enqueue(new TextEncoder().encode('{"acct_id":'));
        },
        cancel(cancelReason: unknown) {
          cancelledWith = cancelReason;
        },
      });
      return new Response(body, {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });

    const { data } = await api.GET("/accounts/{id}", {
      params: { path: { id: ACCOUNT_ID } },
      parseAs: "stream",
    });
    expect(vi.getTimerCount()).toBe(1);

    await data?.cancel(reason);

    expect(cancelledWith).toBe(reason);
    expect(vi.getTimerCount()).toBe(0);
    const [, init] = network.mock.calls[0];
    await vi.advanceTimersByTimeAsync(5 * REQUEST_TIMEOUT_MS);
    expect(init?.signal?.aborted).toBe(false);
  });
});

describe("a complete answer", () => {
  let seen: Request | undefined;
  const recorder: Middleware = {
    onRequest({ request }) {
      seen = request;
      return undefined;
    },
  };

  beforeEach(() => {
    seen = undefined;
    api.use(recorder);
  });

  afterEach(() => {
    api.eject(recorder);
  });

  it("resolves unchanged on the middleware's Request, sent once", async () => {
    network.mockImplementation(
      async () =>
        new Response(JSON.stringify(ACCOUNT_BODY), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );

    const data = unwrap(
      await api.GET("/accounts/{id}", { params: { path: { id: ACCOUNT_ID } } }),
    );

    expect(data).toEqual(ACCOUNT_BODY);
    expect(network).toHaveBeenCalledTimes(1);
    const [input, init] = network.mock.calls[0];
    expect(seen).toBeInstanceOf(Request);
    expect(input).toBe(seen);
    expect(init?.signal?.aborted).toBe(false);

    await vi.advanceTimersByTimeAsync(5 * REQUEST_TIMEOUT_MS);
    expect(network).toHaveBeenCalledTimes(1);
  });

  it.each(COMPLETE)(
    "disarms the deadline and the caller's abort once the answer ends: %s",
    async (_answer, answer, expected) => {
      let answered = new Response();
      network.mockImplementation(async () => {
        answered = answer();
        return answered;
      });
      const caller = new AbortController();

      const result = await api.GET("/accounts/{id}", {
        params: { path: { id: ACCOUNT_ID } },
        signal: caller.signal,
      });

      expect(result.response.status).toBe(expected.status);
      expect(result.data).toEqual(expected.data);
      expect(result.error).toEqual(expected.error);
      expect(result.response.statusText).toBe(answered.statusText);
      expect([...result.response.headers]).toEqual([...answered.headers]);
      expect(vi.getTimerCount()).toBe(0);

      const [, init] = network.mock.calls[0];
      await vi.advanceTimersByTimeAsync(5 * REQUEST_TIMEOUT_MS);
      expect(init?.signal?.aborted).toBe(false);
      caller.abort();
      expect(init?.signal?.aborted).toBe(false);
      expect(network).toHaveBeenCalledTimes(1);
    },
  );
});
