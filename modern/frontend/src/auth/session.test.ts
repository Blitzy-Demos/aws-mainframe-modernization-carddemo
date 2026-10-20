import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  SESSION_KEY,
  type StoredSession,
  clearSession,
  getSession,
  getToken,
  isAdmin,
  setSession,
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
  ])("never throws and holds no session when $why", ({ install }) => {
    install();

    expect(() => setSession(TOKEN, "USER0001", "U")).not.toThrow();
    expect(getSession()).toBeNull();
    expect(getToken()).toBeNull();
    expect(() => clearSession()).not.toThrow();
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
