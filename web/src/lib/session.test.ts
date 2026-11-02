// Tests of the signed-on session store (lib/session.ts): the sessionStorage
// mirror, rehydration on module load, expiry through getSession and through
// the expiry timer, change notification, and the useSession binding.
//
// The module rehydrates when it loads, so every test loads a fresh instance
// after arranging storage. Fake timers are installed first, so both the clock
// the module reads and the timers it schedules are under the test's control.
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Session } from "./session";

type SessionModule = typeof import("./session");

const KEY = "carddemo.session";

const NOW = new Date("2022-07-01T12:00:00Z");

// setTimeout's largest delay, which the module caps each wait at.
const MAX_TIMER_DELAY_MS = 2_147_483_647;

const THIRTY_DAYS_MS = 30 * 24 * 60 * 60 * 1000;

function sessionExpiringIn(
  ms: number,
  overrides: Partial<Session> = {},
): Session {
  return {
    token: "header.payload.signature",
    user_id: "ADMIN001",
    usr_type: "A",
    expires_at: new Date(NOW.getTime() + ms).toISOString(),
    landing: "/admin",
    ...overrides,
  };
}

async function loadSession(): Promise<SessionModule> {
  vi.resetModules();
  return import("./session");
}

function storedValue(): unknown {
  const text = sessionStorage.getItem(KEY);
  return text === null ? null : JSON.parse(text);
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("SESSION_STORAGE_KEY", () => {
  it("is the single storage key", async () => {
    const { SESSION_STORAGE_KEY } = await loadSession();
    expect(SESSION_STORAGE_KEY).toBe(KEY);
  });
});

describe("setSession", () => {
  it("mirrors exactly the session fields to sessionStorage and keeps a frozen copy", async () => {
    const { getSession, setSession } = await loadSession();
    const login = {
      ...sessionExpiringIn(60_000),
      extra: "not a session field",
    };

    setSession(login);

    const expected = sessionExpiringIn(60_000);
    expect(storedValue()).toEqual(expected);
    const current = getSession();
    expect(current).toEqual(expected);
    expect(current).not.toBe(login);
    expect(Object.isFrozen(current)).toBe(true);
  });

  it("replaces an earlier session", async () => {
    const { getSession, setSession } = await loadSession();
    setSession(sessionExpiringIn(60_000));
    const user = sessionExpiringIn(120_000, {
      user_id: "USER0001",
      usr_type: "U",
      landing: "/",
    });

    setSession(user);

    expect(getSession()).toEqual(user);
    expect(storedValue()).toEqual(user);
  });

  it("keeps the in-memory session and drops the stale stored one when storage rejects the write", async () => {
    sessionStorage.setItem(KEY, JSON.stringify(sessionExpiringIn(60_000)));
    const { getSession, setSession } = await loadSession();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("quota exceeded", "QuotaExceededError");
    });
    const user = sessionExpiringIn(120_000, {
      user_id: "USER0001",
      usr_type: "U",
    });

    setSession(user);

    expect(getSession()).toEqual(user);
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(warn).toHaveBeenCalledOnce();
  });
});

describe("clearSession", () => {
  it("removes the stored session and notifies only when a session was present", async () => {
    const { clearSession, getSession, setSession, subscribe } =
      await loadSession();
    const listener = vi.fn();
    subscribe(listener);
    setSession(sessionExpiringIn(60_000));
    expect(listener).toHaveBeenCalledTimes(1);

    clearSession();

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(listener).toHaveBeenCalledTimes(2);

    clearSession();

    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("cancels the expiry timer", async () => {
    const { clearSession, setSession } = await loadSession();
    setSession(sessionExpiringIn(60_000));
    expect(vi.getTimerCount()).toBe(1);

    clearSession();

    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("getSession", () => {
  it("returns null and clears storage once the session has expired", async () => {
    const { getSession, setSession, subscribe } = await loadSession();
    setSession(sessionExpiringIn(1_000));
    const listener = vi.fn();
    subscribe(listener);
    // The clock moves without running timers, so getSession does the clearing.
    vi.setSystemTime(NOW.getTime() + 1_000);

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(listener).toHaveBeenCalledOnce();
  });

  it("treats a session that is already expired when set as signed out", async () => {
    const { getSession, setSession } = await loadSession();

    setSession(sessionExpiringIn(-1));

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
  });
});

describe("subscribe", () => {
  it("stops notifying a listener once it unsubscribes", async () => {
    const { clearSession, setSession, subscribe } = await loadSession();
    const listener = vi.fn();
    const unsubscribe = subscribe(listener);
    setSession(sessionExpiringIn(60_000));

    unsubscribe();
    clearSession();
    setSession(sessionExpiringIn(60_000));

    expect(listener).toHaveBeenCalledOnce();
  });
});

describe("rehydration on module load", () => {
  it("restores a live stored session with only its session fields", async () => {
    const live = sessionExpiringIn(60_000);
    sessionStorage.setItem(KEY, JSON.stringify({ ...live, extra: "dropped" }));

    const { getSession } = await loadSession();

    const current = getSession();
    expect(current).toEqual(live);
    expect(Object.isFrozen(current)).toBe(true);
  });

  it("clears a restored session when its expiry timer fires", async () => {
    sessionStorage.setItem(KEY, JSON.stringify(sessionExpiringIn(1_000)));
    const { getSession, subscribe } = await loadSession();
    const listener = vi.fn();
    subscribe(listener);
    expect(vi.getTimerCount()).toBe(1);

    vi.advanceTimersByTime(1_000);

    expect(listener).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(getSession()).toBeNull();
  });

  const { landing: _omitted, ...withoutLanding } = sessionExpiringIn(60_000);

  it.each<{ name: string; text: string }>([
    { name: "text that is not JSON", text: "{not json" },
    { name: "JSON null", text: "null" },
    {
      name: "a JSON value that is not an object",
      text: JSON.stringify("token"),
    },
    { name: "a missing field", text: JSON.stringify(withoutLanding) },
    {
      name: "a field that is not a string",
      text: JSON.stringify({ ...sessionExpiringIn(60_000), token: 42 }),
    },
    { name: "an expired session", text: JSON.stringify(sessionExpiringIn(0)) },
    {
      name: "an unparsable expiry",
      text: JSON.stringify(sessionExpiringIn(60_000, { expires_at: "never" })),
    },
  ])("starts signed out and removes $name", async ({ text }) => {
    sessionStorage.setItem(KEY, text);

    const { getSession } = await loadSession();

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("starts signed out when the browser blocks storage", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("storage is blocked", "SecurityError");
    });

    const { getSession } = await loadSession();

    expect(getSession()).toBeNull();
  });
});

describe("expiry timer", () => {
  it("waits in steps of the longest timer delay for a distant expiry", async () => {
    const { getSession, setSession } = await loadSession();
    const schedule = vi.spyOn(globalThis, "setTimeout");
    setSession(sessionExpiringIn(THIRTY_DAYS_MS));

    // A longer delay would overflow and fire at once, again and again.
    expect(schedule).toHaveBeenLastCalledWith(
      expect.any(Function),
      MAX_TIMER_DELAY_MS,
    );

    vi.advanceTimersByTime(MAX_TIMER_DELAY_MS);

    expect(getSession()).not.toBeNull();
    expect(vi.getTimerCount()).toBe(1);

    vi.advanceTimersByTime(THIRTY_DAYS_MS - MAX_TIMER_DELAY_MS);

    expect(getSession()).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("isAdmin", () => {
  it.each<{ usrType: string; admin: boolean }>([
    { usrType: "A", admin: true },
    { usrType: "U", admin: false },
    { usrType: "a", admin: false },
    { usrType: "", admin: false },
    { usrType: "A ", admin: false },
  ])("is $admin for usr_type '$usrType'", async ({ usrType, admin }) => {
    const { isAdmin } = await loadSession();
    expect(isAdmin(sessionExpiringIn(60_000, { usr_type: usrType }))).toBe(
      admin,
    );
  });

  it("is false without a session", async () => {
    const { isAdmin } = await loadSession();
    expect(isAdmin(null)).toBe(false);
  });
});

describe("useSession", () => {
  it("re-renders on sign-on and sign-off with a stable snapshot in between", async () => {
    const { clearSession, setSession, useSession } = await loadSession();
    const { result, rerender } = renderHook(() => useSession());
    expect(result.current).toBeNull();

    act(() => {
      setSession(sessionExpiringIn(60_000));
    });

    const signedOn = result.current;
    expect(signedOn).toEqual(sessionExpiringIn(60_000));
    rerender();
    expect(result.current).toBe(signedOn);

    act(() => {
      clearSession();
    });

    expect(result.current).toBeNull();
  });

  it("notifies subscribers when the expiry timer fires", async () => {
    const { setSession, useSession } = await loadSession();
    const { result } = renderHook(() => useSession());
    act(() => {
      setSession(sessionExpiringIn(60_000));
    });

    act(() => {
      vi.advanceTimersByTime(59_999);
    });

    expect(result.current).not.toBeNull();

    act(() => {
      vi.advanceTimersByTime(1);
    });

    expect(result.current).toBeNull();
    expect(sessionStorage.getItem(KEY)).toBeNull();
  });
});
