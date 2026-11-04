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

describe("takeSessionEndCause", () => {
  it("hands out once the cause recorded when clearSession ended a session", async () => {
    const { clearSession, setSession, takeSessionEndCause } =
      await loadSession();
    setSession(sessionExpiringIn(60_000));
    const cause = new Error("Authentication required");

    clearSession(cause);

    expect(takeSessionEndCause()).toBe(cause);
    expect(takeSessionEndCause()).toBeUndefined();
  });

  it("records no cause when no session was present, keeping the one recorded", async () => {
    const { clearSession, setSession, takeSessionEndCause } =
      await loadSession();
    clearSession(new Error("signed out already"));
    expect(takeSessionEndCause()).toBeUndefined();
    setSession(sessionExpiringIn(60_000));
    const cause = new Error("Authentication required");
    clearSession(cause);

    clearSession(new Error("signed out already"));
    clearSession();

    expect(takeSessionEndCause()).toBe(cause);
  });

  it("is reset by a sign-on", async () => {
    const { clearSession, setSession, takeSessionEndCause } =
      await loadSession();
    setSession(sessionExpiringIn(60_000));
    clearSession(new Error("Authentication required"));

    setSession(sessionExpiringIn(120_000));

    expect(takeSessionEndCause()).toBeUndefined();
  });

  it.each<{ end: string; run: (module: SessionModule) => void }>([
    {
      end: "a sign-off",
      run: ({ clearSession }) => {
        clearSession();
      },
    },
    {
      end: "the expiry timer",
      run: () => {
        vi.advanceTimersByTime(60_000);
      },
    },
    {
      end: "getSession after the expiry",
      run: ({ getSession }) => {
        vi.setSystemTime(NOW.getTime() + 60_000);
        getSession();
      },
    },
    {
      end: "a resync with a store another page emptied",
      run: () => {
        sessionStorage.removeItem(KEY);
        window.dispatchEvent(
          new PageTransitionEvent("pageshow", { persisted: true }),
        );
      },
    },
  ])("is undefined after $end", async ({ run }) => {
    const session = await loadSession();
    session.setSession(sessionExpiringIn(60_000));

    run(session);

    expect(session.getSession()).toBeNull();
    expect(session.takeSessionEndCause()).toBeUndefined();
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

// Each test loads a fresh module instance, and the instances of earlier tests
// keep their window listeners, so they react to the events dispatched here as
// well. The assertions therefore read only the instance under test.
describe("resync with sessionStorage", () => {
  const USER = sessionExpiringIn(60_000, {
    token: "user.session.token",
    user_id: "USER0001",
    usr_type: "U",
    landing: "/",
  });

  // What another document of the tab left in the store after signing on.
  const OTHER = sessionExpiringIn(120_000, { token: "other.session.token" });

  function restoredFromCache(): Event {
    return new PageTransitionEvent("pageshow", { persisted: true });
  }

  it.each<{
    event: string;
    outcome: string;
    make: () => Event;
    ends: boolean;
  }>([
    {
      event: "a pageshow restoring the page from the back/forward cache",
      outcome: "ends",
      make: restoredFromCache,
      ends: true,
    },
    {
      event: "a pageshow of a fresh load",
      outcome: "keeps",
      make: () => new PageTransitionEvent("pageshow", { persisted: false }),
      ends: false,
    },
    {
      event: "a storage event for the session key",
      outcome: "ends",
      make: () =>
        new StorageEvent("storage", { key: KEY, storageArea: sessionStorage }),
      ends: true,
    },
    {
      event: "a storage event for clear()",
      outcome: "ends",
      make: () =>
        new StorageEvent("storage", { key: null, storageArea: sessionStorage }),
      ends: true,
    },
    {
      event: "a storage event for another key",
      outcome: "keeps",
      make: () =>
        new StorageEvent("storage", {
          key: "carddemo.theme",
          storageArea: sessionStorage,
        }),
      ends: false,
    },
    {
      event: "a storage event for another storage area",
      outcome: "keeps",
      make: () => new StorageEvent("storage", { key: KEY, storageArea: null }),
      ends: false,
    },
  ])(
    "$event $outcome a session another page of the tab signed off",
    async ({ make, ends }) => {
      const { getSession, setSession, subscribe } = await loadSession();
      setSession(USER);
      const listener = vi.fn();
      subscribe(listener);
      // The later page's sign-off, which this page's memory never saw.
      sessionStorage.removeItem(KEY);

      window.dispatchEvent(make());

      expect(getSession()).toEqual(ends ? null : USER);
      expect(listener).toHaveBeenCalledTimes(ends ? 1 : 0);
      expect(vi.getTimerCount()).toBe(ends ? 0 : 1);
    },
  );

  it.each<{ store: string; arrange: () => void; storedAfter: unknown }>([
    {
      store: "holds no session",
      arrange: () => {
        sessionStorage.removeItem(KEY);
      },
      storedAfter: null,
    },
    {
      store: "holds another page's live session",
      arrange: () => {
        sessionStorage.setItem(KEY, JSON.stringify(OTHER));
      },
      storedAfter: OTHER,
    },
    {
      store: "holds text that is not a session",
      arrange: () => {
        sessionStorage.setItem(KEY, "{not json");
      },
      storedAfter: null,
    },
  ])(
    "ends the in-memory session when the store $store, leaving the store as the other page wrote it",
    async ({ arrange, storedAfter }) => {
      const { getSession, setSession, subscribe } = await loadSession();
      setSession(USER);
      const listener = vi.fn();
      subscribe(listener);
      arrange();

      window.dispatchEvent(restoredFromCache());

      expect(getSession()).toBeNull();
      expect(listener).toHaveBeenCalledOnce();
      expect(vi.getTimerCount()).toBe(0);
      expect(storedValue()).toEqual(storedAfter);
    },
  );

  it.each<{
    store: string;
    signOn: (setSession: (s: Session) => void) => void;
  }>([
    {
      store: "still holds its token",
      signOn: (setSession) => {
        setSession(USER);
      },
    },
    {
      store: "cannot be read",
      signOn: (setSession) => {
        setSession(USER);
        vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
          throw new DOMException("storage is blocked", "SecurityError");
        });
      },
    },
    {
      store: "never received it, because the write was rejected",
      signOn: (setSession) => {
        vi.spyOn(console, "warn").mockImplementation(() => undefined);
        vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
          throw new DOMException("quota exceeded", "QuotaExceededError");
        });
        setSession(USER);
      },
    },
  ])("keeps the session when the store $store", async ({ signOn }) => {
    const { getSession, setSession, subscribe } = await loadSession();
    signOn(setSession);
    const listener = vi.fn();
    subscribe(listener);

    window.dispatchEvent(restoredFromCache());

    expect(getSession()).toEqual(USER);
    expect(listener).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(1);
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
