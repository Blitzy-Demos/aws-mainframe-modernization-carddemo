import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from "vitest";
import {
  SESSION_KEY,
  type StoredSession,
  clearSession,
  getSession,
  getSessionSnapshot,
  getToken,
  isAdmin,
  setSession,
  subscribeSession,
} from "./session";

/**
 * Unit tests of the browser session store. The "unit" Vitest project runs in Node, where
 * `sessionStorage` does not exist, so each test installs an in-memory `Storage` of its own.
 * Token values are fake and are never logged.
 */

const TOKEN = "test-token-not-a-real-jwt";
const OTHER_TOKEN = "test-token-earlier-sign-on";

/** An in-memory `Storage` with the browser's string-only semantics. */
function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear() {
      items.clear();
    },
    getItem(key) {
      return items.get(key) ?? null;
    },
    key(index) {
      return [...items.keys()][index] ?? null;
    },
    removeItem(key) {
      items.delete(key);
    },
    setItem(key, value) {
      items.set(key, String(value));
    },
  };
}

let store: Storage;

beforeEach(() => {
  store = memoryStorage();
  vi.stubGlobal("sessionStorage", store);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("setSession and getSession", () => {
  it("round-trips the token, user ID and user type, and stores nothing else", () => {
    setSession(TOKEN, "USER0001", "U");

    expect(getSession()).toEqual({ token: TOKEN, userId: "USER0001", userType: "U" });
    expect(getToken()).toBe(TOKEN);
    expect(store.length).toBe(1);
    expect(JSON.parse(store.getItem(SESSION_KEY) ?? "")).toEqual({
      token: TOKEN,
      userId: "USER0001",
      userType: "U",
    });
  });

  it("replaces an earlier sign-on's session", () => {
    setSession(OTHER_TOKEN, "USER0001", "U");
    setSession(TOKEN, "ADMIN001", "A");

    expect(getSession()).toEqual({ token: TOKEN, userId: "ADMIN001", userType: "A" });
  });

  it("leaves no session, earlier or new, when the write fails", () => {
    setSession(OTHER_TOKEN, "USER0001", "U");
    vi.spyOn(store, "setItem").mockImplementation(() => {
      throw new DOMException("quota exceeded", "QuotaExceededError");
    });

    expect(() => setSession(TOKEN, "ADMIN001", "A")).not.toThrow();
    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
  });
});

describe("getSession", () => {
  it("returns null when nothing is stored", () => {
    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
  });

  it.each([
    { stored: "not json", why: "is not JSON" },
    { stored: '{"token":', why: "is truncated JSON" },
    { stored: "null", why: "is JSON null" },
    { stored: '"text"', why: "is a JSON string" },
    { stored: "[]", why: "is a JSON array" },
    { stored: '{"userId":"USER0001","userType":"U"}', why: "lacks the token" },
    { stored: '{"token":"test-token","userType":"U"}', why: "lacks the user ID" },
    { stored: '{"token":"test-token","userId":"USER0001"}', why: "lacks the user type" },
    { stored: '{"token":"","userId":"USER0001","userType":"U"}', why: "has an empty token" },
    { stored: '{"token":"test-token","userId":"","userType":"U"}', why: "has an empty user ID" },
    {
      stored: '{"token":"test-token","userId":"USER0001","userType":""}',
      why: "has an empty user type",
    },
    { stored: '{"token":7,"userId":"USER0001","userType":"U"}', why: "has a numeric token" },
  ])("returns null when the stored value $why", ({ stored }) => {
    store.setItem(SESSION_KEY, stored);

    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
  });

  it("returns null when reading storage throws", () => {
    vi.spyOn(store, "getItem").mockImplementation(() => {
      throw new DOMException("storage blocked", "SecurityError");
    });

    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
  });

  it("reads afresh, so a session removed elsewhere is gone at once", () => {
    setSession(TOKEN, "USER0001", "U");
    store.removeItem(SESSION_KEY);

    expect(getSession()).toBeNull();
  });
});

describe("clearSession", () => {
  it("ends the session and leaves other keys in place", () => {
    store.setItem("unrelated", "kept");
    setSession(TOKEN, "USER0001", "U");

    clearSession();

    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
    expect(store.getItem(SESSION_KEY)).toBeNull();
    expect(store.getItem("unrelated")).toBe("kept");
  });

  it("does not throw when removing the item throws", () => {
    vi.spyOn(store, "removeItem").mockImplementation(() => {
      throw new DOMException("storage blocked", "SecurityError");
    });

    expect(() => clearSession()).not.toThrow();
  });
});

describe("subscribeSession and getSessionSnapshot", () => {
  /** Registers `listener` for the current test only, and returns its remover. */
  function subscribe(listener: () => void): () => void {
    const unsubscribe = subscribeSession(listener);
    onTestFinished(unsubscribe);
    return unsubscribe;
  }

  it("notifies every listener once per setSession and clearSession, until it unsubscribes", () => {
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = subscribe(first);
    subscribe(second);
    // The same function registered again is held once, so it is still called once per change.
    subscribe(second);

    setSession(TOKEN, "USER0001", "U");
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);

    clearSession();
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(2);

    unsubscribeFirst();
    setSession(TOKEN, "ADMIN001", "A");
    clearSession();
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(4);
  });

  it("notifies after the change, so a listener reads the new session", () => {
    const seen: Array<StoredSession | null> = [];
    subscribe(() => {
      seen.push(getSession());
    });

    setSession(TOKEN, "ADMIN001", "A");
    clearSession();

    expect(seen).toEqual([{ token: TOKEN, userId: "ADMIN001", userType: "A" }, null]);
  });

  it("returns the identical snapshot until the session changes", () => {
    expect(getSessionSnapshot()).toBeNull();
    expect(getSessionSnapshot()).toBe(getSessionSnapshot());

    setSession(TOKEN, "USER0001", "U");
    const signedOn = getSessionSnapshot();
    expect(signedOn).toBe(store.getItem(SESSION_KEY));
    expect(signedOn).not.toBeNull();
    expect(getSessionSnapshot()).toBe(signedOn);

    setSession(OTHER_TOKEN, "USER0001", "U");
    expect(getSessionSnapshot()).not.toBe(signedOn);

    clearSession();
    expect(getSessionSnapshot()).toBeNull();
  });

  it("returns a null snapshot when reading storage throws", () => {
    setSession(TOKEN, "USER0001", "U");
    vi.spyOn(store, "getItem").mockImplementation(() => {
      throw new DOMException("storage blocked", "SecurityError");
    });

    expect(() => getSessionSnapshot()).not.toThrow();
    expect(getSessionSnapshot()).toBeNull();
  });

  it("lets a throwing listener stop neither the other listeners nor the caller", () => {
    const failing = vi.fn(() => {
      throw new Error("listener failure");
    });
    const after = vi.fn();
    subscribe(failing);
    subscribe(after);

    expect(() => setSession(TOKEN, "USER0001", "U")).not.toThrow();
    expect(getSession()).toEqual({ token: TOKEN, userId: "USER0001", userType: "U" });
    expect(() => clearSession()).not.toThrow();
    expect(getSession()).toBeNull();
    expect(failing).toHaveBeenCalledTimes(2);
    expect(after).toHaveBeenCalledTimes(2);
  });

  it.each([
    { method: "setItem", name: "setSession", change: () => setSession(TOKEN, "USER0001", "U") },
    { method: "removeItem", name: "setSession", change: () => setSession(TOKEN, "USER0001", "U") },
    { method: "removeItem", name: "clearSession", change: () => clearSession() },
  ] as const)("still notifies when $method throws during $name", ({ method, change }) => {
    const listener = vi.fn();
    subscribe(listener);
    vi.spyOn(store, method).mockImplementation(() => {
      throw new DOMException("storage blocked", "SecurityError");
    });

    expect(change).not.toThrow();
    expect(listener).toHaveBeenCalledTimes(1);
  });
});

describe("without reachable sessionStorage", () => {
  it.each([
    {
      why: "it does not exist",
      install: () => {
        vi.stubGlobal("sessionStorage", undefined);
      },
    },
    {
      why: "reaching it throws",
      install: () => {
        Object.defineProperty(globalThis, "sessionStorage", {
          configurable: true,
          get(): Storage {
            throw new DOMException("storage blocked", "SecurityError");
          },
        });
      },
    },
  ])("never throws, holds no session and still notifies when $why", ({ install }) => {
    const listener = vi.fn();
    onTestFinished(subscribeSession(listener));
    install();

    expect(() => setSession(TOKEN, "USER0001", "U")).not.toThrow();
    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
    expect(getSessionSnapshot()).toBeNull();
    expect(() => clearSession()).not.toThrow();
    expect(listener).toHaveBeenCalledTimes(2);
  });
});

describe("isAdmin", () => {
  const session = (userType: string): StoredSession => ({
    token: TOKEN,
    userId: "ADMIN001",
    userType,
  });

  it.each([
    { name: "user type A", value: session("A"), expected: true },
    { name: "user type U", value: session("U"), expected: false },
    { name: "lower-case a", value: session("a"), expected: false },
    { name: "any other type", value: session("X"), expected: false },
    { name: "no session", value: null, expected: false },
  ])("is $expected for $name", ({ value, expected }) => {
    expect(isAdmin(value)).toBe(expected);
  });
});
