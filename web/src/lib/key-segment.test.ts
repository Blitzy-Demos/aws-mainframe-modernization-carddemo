// Tests of the key transport (lib/key-segment.ts): a stored key that is `.`
// or `..` after zero or more `~` travels behind one more `~`, every other key
// travels as stored, and the segment decodes back to the key, so the URL
// never holds a dot segment and no two keys share a segment. The pairs are
// the ones the server's KeySegment decodes (crates/api/src/dto/mod.rs).
import { describe, expect, it } from "vitest";

import {
  decodeKeySegment,
  encodeKeySegment,
  keyPathSegment,
} from "./key-segment";

interface KeyCase {
  key: string;
  segment: string;
}

const KEY_CASES: KeyCase[] = [
  // Dot segments, and keys that are a dot segment after `~`s.
  { key: ".", segment: "~." },
  { key: "..", segment: "~.." },
  { key: "~.", segment: "~~." },
  { key: "~..", segment: "~~.." },
  { key: "~~~~~~~.", segment: "~~~~~~~~." },
  { key: "~~~~~~..", segment: "~~~~~~~.." },
  // Every other key, unchanged.
  { key: "USER0001", segment: "USER0001" },
  { key: "...", segment: "..." },
  { key: ".a", segment: ".a" },
  { key: "~A", segment: "~A" },
  { key: "~", segment: "~" },
  { key: "~.~.", segment: "~.~." },
  { key: "%2e", segment: "%2e" },
  { key: "", segment: "" },
];

describe("key segments", () => {
  it.each(KEY_CASES)(
    "the key '$key' travels as '$segment' and decodes back",
    ({ key, segment }) => {
      expect(encodeKeySegment(key)).toBe(segment);
      expect(decodeKeySegment(segment)).toBe(key);
      expect(decodeKeySegment(encodeKeySegment(key))).toBe(key);
    },
  );

  it("gives every key its own segment, none of them a dot segment", () => {
    const segments = KEY_CASES.map(({ key }) => encodeKeySegment(key));
    expect(new Set(segments).size).toBe(KEY_CASES.length);
    expect(segments.filter((segment) => /^\.\.?$/.test(segment))).toEqual([]);
  });

  it.each([
    { key: ".", path: "/admin/users/~./edit" },
    { key: "..", path: "/admin/users/~../edit" },
    { key: "USER 01", path: "/admin/users/USER%2001/edit" },
    { key: "%2e", path: "/admin/users/%252e/edit" },
  ])(
    "links the key '$key' through a path the URL keeps as written",
    ({ key, path }) => {
      const href = `/admin/users/${keyPathSegment(key)}/edit`;
      expect(href).toBe(path);
      expect(new URL(href, "http://localhost").pathname).toBe(path);
    },
  );
});
