// The transport of a stored key in one URL path segment: the user ID of
// /admin/users/{id} and /admin/users/:userId/edit, and the type code of
// /admin/transaction-types/{id} and /admin/transaction-types/:code/edit.
//
// Both keys may be `.` or `..`: COUSR01C refuses only a blank user ID, and
// MNTTRDB2 stores any two characters as a type code. A URL removes those dot
// segments from its path, percent-encoded as `%2E` too (RFC 3986 §5.2.4, the
// WHATWG URL standard), so `/admin/users/..` would reach `/admin/` and
// `/admin/users/.` would reach `/admin/users/`. Such a key therefore travels
// with one more leading `~`, and so does every key that is `.` or `..` after
// one or more `~`, which keeps the escape a bijection: `.` is `~.`, `~.` is
// `~~.`, and `USER0001`, `...`, `.a` and `~A` travel as stored. The server
// undoes the escape on the same rule (crates/api/src/dto/mod.rs,
// `KeySegment`) and refuses a bare `.` or `..` segment, so the key `.` has
// the one segment `~.`.
//
// Query keys, form values and the text a page shows are the stored key; only
// the URL carries the segment.

// The escape a key that would be a dot segment travels behind.
const ESCAPE = "~";

// A key that is `.` or `..` after zero or more `~`.
const DOT_KEY = /^~*\.\.?$/;

// A segment that is `.` or `..` after one or more `~`: an escaped key.
const ESCAPED_DOT_KEY = /^~+\.\.?$/;

/**
 * The path segment of the stored `key`, before percent-encoding: `key` with
 * one more leading `~` when it is `.` or `..` after zero or more `~`, else
 * `key` unchanged. openapi-fetch percent-encodes a path parameter itself, so
 * this is the value its `path` takes.
 */
export function encodeKeySegment(key: string): string {
  return DOT_KEY.test(key) ? `${ESCAPE}${key}` : key;
}

/**
 * The stored key a percent-decoded path segment carries, as a route
 * parameter holds it: `segment` without its first `~` when it is `.` or `..`
 * after one or more `~`, else `segment` unchanged. The inverse of
 * {@link encodeKeySegment}.
 */
export function decodeKeySegment(segment: string): string {
  return ESCAPED_DOT_KEY.test(segment)
    ? segment.slice(ESCAPE.length)
    : segment;
}

/**
 * The stored `key` as a router link's path segment: its
 * {@link encodeKeySegment} segment, percent-encoded.
 */
export function keyPathSegment(key: string): string {
  return encodeURIComponent(encodeKeySegment(key));
}
