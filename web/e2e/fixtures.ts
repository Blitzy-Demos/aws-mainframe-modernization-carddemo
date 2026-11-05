// Shared support for the Playwright specs: the `test` and `expect` every spec
// imports, the function-map contract (docs/parity/function-map.md) parsed into
// typed rows, the SPA route table, the seeded fixture keys and users, the
// COBOL message texts the specs assert, the database fixtures the specs set
// through `docker compose exec postgres psql`, the API helpers and the UI
// helpers that locate controls by role and accessible name.
//
// Every database fixture is restorable: it takes a checkpoint of the rows it
// may change before it writes, and the spec that uses it restores the
// checkpoint in afterAll. Amounts, points and identifiers stay strings from
// end to end; any arithmetic on them is done by PostgreSQL.
//
// The psql helper runs inside the Compose project the environment names
// (COMPOSE_PROJECT_NAME and COMPOSE_FILE, when set) from the repository root,
// so a spec reaches the same PostgreSQL the API under test uses. Every call is
// bounded per statement (lock wait and run time) and as a whole command inside
// the container, and runs as a session named for that call alone. A failed
// call settles only once its session has been reaped and can no longer write,
// or rejects reporting it unconfirmed, which blocks checkpoint restores. A
// restore also waits for the calls still in flight, so SQL a timed-out test
// left running cannot land after the restore.
import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import {
  expect,
  test,
  type APIRequestContext,
  type Locator,
  type Page,
  type Request,
} from "@playwright/test";
import { z } from "zod";

import type { components } from "../src/api/schema";

export { expect, test };

type TransactionAddRequest = components["schemas"]["TransactionAddRequest"];
type TransactionAddResponse = components["schemas"]["TransactionAddResponse"];
type ErrorBody = components["schemas"]["ErrorBody"];

// ---------------------------------------------------------------------------
// Repository paths
// ---------------------------------------------------------------------------

const REPO_ROOT_URL = new URL("../../", import.meta.url);

/** The repository root (the Cargo workspace root), with a trailing slash. */
export const repoRoot: string = fileURLToPath(REPO_ROOT_URL);

/** The absolute path of `relative`, a repository-root-relative path. */
export function repoPath(relative: string): string {
  return fileURLToPath(new URL(relative, REPO_ROOT_URL));
}

function readRepoText(relative: string): string {
  return readFileSync(repoPath(relative), "utf8");
}

// ---------------------------------------------------------------------------
// psql
// ---------------------------------------------------------------------------

const execFileAsync = promisify(execFile);

// PostgreSQL's bounds on each statement a psql() call sends: the longest wait
// for a lock, and the longest run, lock wait included. They reach the psql
// inside the container through PGOPTIONS, which adds no output line. The
// fixtures' statements are small, so only a blocked or runaway statement
// meets them.
const PSQL_LOCK_TIMEOUT = "15s";
const PSQL_STATEMENT_TIMEOUT = "30s";

// How often a running statement checks that its psql is still connected (also
// through PGOPTIONS), so a backend whose psql was killed aborts its statement,
// and with it the rest of the call's SQL, within this interval.
const PSQL_CONNECTION_CHECK_INTERVAL = "500ms";

// The bound on one whole psql() command, in seconds, applied inside the
// container: `timeout` kills psql with SIGKILL once it has run this long,
// however many statements its SQL holds. It exceeds PSQL_STATEMENT_TIMEOUT, so
// a single statement's own error normally arrives first.
const PSQL_COMMAND_TIMEOUT_S = 45;

// The client's backstop on one `docker compose exec` child, in milliseconds:
// the in-container bound plus room for a slow docker start. It stays below the
// 120 s Playwright test timeout.
const PSQL_CHILD_TIMEOUT_MS = 75_000;

// The bounds of the reap that follows a failed call (reapSession): one
// attempt's in-container and client bounds, how long it waits for each
// signalled backend to exit, and how many attempts it makes, a second apart,
// before it reports the session unconfirmed.
const REAP_COMMAND_TIMEOUT_S = 12;
const REAP_CHILD_TIMEOUT_MS = 20_000;
const REAP_TERMINATE_WAIT_MS = 5_000;
const REAP_ATTEMPTS = 3;
const REAP_RETRY_DELAY_MS = 1_000;

// The psql() calls not yet settled. A checkpoint restore waits for those in
// flight when it starts, so a call a timed-out test left running cannot write
// after the restore.
const psqlInFlight = new Set<Promise<unknown>>();

// The sessions of failed psql() calls whose end no reap could confirm. A
// checkpoint restore refuses to run while any remains (confirmSessionsEnded).
const unconfirmedSessions = new Set<string>();

// The last number given to a psql session of this process.
let sessionSequence = 0;

// A new application_name for one psql session: carddemo-e2e-<pid>-<n> for a
// psql() call, carddemo-e2e-<pid>-reap-<n> for a reap. Each Playwright worker
// is its own process, so no two sessions share a name, and every name stays
// far below PostgreSQL's 63-byte limit.
function sessionTag(kind: "call" | "reap"): string {
  sessionSequence += 1;
  return kind === "call"
    ? `carddemo-e2e-${process.pid}-${sessionSequence}`
    : `carddemo-e2e-${process.pid}-reap-${sessionSequence}`;
}

// Whether `error` is execFile's report of a child it killed at its timeout.
// A maxBuffer overflow, the only other kill, reports a RangeError without
// `killed`.
function killedAtTimeout(error: unknown): boolean {
  return typeof error === "object" && error !== null && "killed" in error && error.killed === true;
}

// The signal that ended the child, as execFile reports it.
function exitSignal(error: unknown): string {
  if (typeof error === "object" && error !== null && "signal" in error) {
    return String(error.signal);
  }
  return "unknown signal";
}

// The exit status of a child that ran and failed, as execFile reports it.
function exitStatus(error: unknown): number | undefined {
  if (typeof error === "object" && error !== null && "code" in error && typeof error.code === "number") {
    return error.code;
  }
  return undefined;
}

// Text a child-process failure carries: the stderr of the failed command, or
// the error itself when the command could not start.
function describeExecFailure(error: unknown): string {
  if (typeof error === "object" && error !== null) {
    const stderr = "stderr" in error ? error.stderr : undefined;
    if (typeof stderr === "string" && stderr.trim() !== "") {
      return stderr.trim();
    }
    if ("message" in error && typeof error.message === "string") {
      return error.message;
    }
  }
  return String(error);
}

// What ended a failed psql child bounded by `commandTimeoutS` inside the
// container and `childTimeoutMs` by the client: the client's kill, the
// in-container kill (`timeout` exits 137 after SIGKILL, 124 after another
// signal), else psql's own error text.
function describePsqlFailure(error: unknown, commandTimeoutS: number, childTimeoutMs: number): string {
  if (killedAtTimeout(error)) {
    return `psql exceeded ${childTimeoutMs} ms and its docker child was killed with ${exitSignal(error)}`;
  }
  const status = exitStatus(error);
  if (status === 124 || status === 137) {
    return `psql ran past its ${commandTimeoutS} s bound and was killed inside the container (exit ${status})`;
  }
  return `psql failed: ${describeExecFailure(error)}`;
}

// Runs psql in the Compose `postgres` service from the repository root as the
// session `tag`, with each of `commands` sent by its own -c, so each runs and
// commits on its own, and returns the output without the final newline.
// `timeout --foreground` kills psql after `commandTimeoutS` and itself stays
// alive to reap it: without --foreground it would also kill itself, leaving
// psql to the container's PID 1, the postmaster, which takes any child's
// SIGKILL for a crashed backend and restarts the whole server.
async function execPsql(
  tag: string,
  commandTimeoutS: number,
  childTimeoutMs: number,
  commands: readonly string[],
): Promise<string> {
  const { stdout } = await execFileAsync(
    "docker",
    [
      "compose",
      "exec",
      "-T",
      "-e",
      `PGAPPNAME=${tag}`,
      "-e",
      `PGOPTIONS=-c lock_timeout=${PSQL_LOCK_TIMEOUT} -c statement_timeout=${PSQL_STATEMENT_TIMEOUT} -c client_connection_check_interval=${PSQL_CONNECTION_CHECK_INTERVAL}`,
      "postgres",
      "timeout",
      "--foreground",
      "-s",
      "KILL",
      String(commandTimeoutS),
      "psql",
      "-U",
      "carddemo",
      "-d",
      "carddemo",
      "-v",
      "ON_ERROR_STOP=1",
      "-tA",
      ...commands.flatMap((command) => ["-c", command]),
    ],
    {
      cwd: repoRoot,
      maxBuffer: 64 * 1024 * 1024,
      timeout: childTimeoutMs,
      killSignal: "SIGKILL",
    },
  );
  return stdout.endsWith("\n") ? stdout.slice(0, -1) : stdout;
}

// A reap's outcome: the session is gone, after `found` of its backends were
// still open and terminated, or its end is unconfirmed for `reason`.
type Reap = { ended: true; found: number } | { ended: false; reason: string };

// Ends every backend of the session `tag` and confirms none remains, through
// a separate bounded psql session that is neither in flight nor reaped itself.
// An attempt signals each backend named `tag`, waiting REAP_TERMINATE_WAIT_MS
// for it to exit, then counts those left by a second -c, whose own
// transaction reads pg_stat_activity afresh. Up to REAP_ATTEMPTS attempts run.
async function reapSession(tag: string): Promise<Reap> {
  const name = sqlLiteral(tag);
  const terminate = `SELECT count(*)
                       FROM (SELECT pg_terminate_backend(pid, ${REAP_TERMINATE_WAIT_MS})
                               FROM pg_stat_activity
                              WHERE application_name = ${name}) AS signalled`;
  const remaining = `SELECT count(*) FROM pg_stat_activity WHERE application_name = ${name}`;
  let found = 0;
  let reason = "no reap attempt ran";
  for (let attempt = 1; attempt <= REAP_ATTEMPTS; attempt += 1) {
    if (attempt > 1) {
      await delay(REAP_RETRY_DELAY_MS);
    }
    let out: string;
    try {
      out = await execPsql(sessionTag("reap"), REAP_COMMAND_TIMEOUT_S, REAP_CHILD_TIMEOUT_MS, [terminate, remaining]);
    } catch (error) {
      reason = `the reap failed: ${describePsqlFailure(error, REAP_COMMAND_TIMEOUT_S, REAP_CHILD_TIMEOUT_MS)}`;
      continue;
    }
    const counts = /^([0-9]+)\n([0-9]+)$/.exec(out);
    if (counts === null) {
      reason = `the reap printed ${JSON.stringify(out)}, not two counts`;
      continue;
    }
    found += Number(counts[1]);
    if (counts[2] === "0") {
      return { ended: true, found };
    }
    reason = `${counts[2]} of its backends still open`;
  }
  return { ended: false, reason };
}

// Reaps again every session in unconfirmedSessions; a session confirmed ended
// leaves the set. Rejects, naming each session left, while any remains, since
// its SQL might still write.
async function confirmSessionsEnded(): Promise<void> {
  const open: string[] = [];
  for (const tag of [...unconfirmedSessions]) {
    const reap = await reapSession(tag);
    if (reap.ended) {
      unconfirmedSessions.delete(tag);
    } else {
      open.push(`${tag} (${reap.reason})`);
    }
  }
  if (open.length > 0) {
    throw new Error(
      `checkpoint restore refused: the session of a failed psql() call is not confirmed ended and might still write: ${open.join("; ")}`,
    );
  }
}

/**
 * Runs `sql` with psql in the Compose `postgres` service, exactly as
 * `docker compose exec -T -e PGAPPNAME=<session> -e PGOPTIONS=<bounds> postgres
 * timeout --foreground -s KILL <PSQL_COMMAND_TIMEOUT_S> psql -U carddemo
 * -d carddemo -v ON_ERROR_STOP=1 -tA -c <sql>` from the repository root, and
 * returns its unaligned, tuples-only output without the final newline. Rows
 * are separated by newlines and columns by `|`. A failing statement rejects
 * with psql's error text.
 *
 * Bounds: PostgreSQL fails a statement that waits PSQL_LOCK_TIMEOUT for a lock
 * or runs PSQL_STATEMENT_TIMEOUT; `timeout` kills psql once the whole command
 * has run PSQL_COMMAND_TIMEOUT_S, and its backend then aborts within
 * PSQL_CONNECTION_CHECK_INTERVAL; the client kills a docker child still
 * running after PSQL_CHILD_TIMEOUT_MS. Each call runs as its own session
 * (sessionTag), and a failed call settles only after a reap has ended that
 * session's backends and confirmed none remains, so its SQL can no longer
 * write. When no reap can confirm that, the call rejects saying so, and every
 * checkpoint restore is refused until a later reap does. Until the call
 * settles it is in flight, and a checkpoint restore that starts meanwhile
 * waits for it.
 */
export async function psql(sql: string): Promise<string> {
  const call = runPsql(sql);
  psqlInFlight.add(call);
  try {
    return await call;
  } finally {
    psqlInFlight.delete(call);
  }
}

async function runPsql(sql: string): Promise<string> {
  const tag = sessionTag("call");
  try {
    return await execPsql(tag, PSQL_COMMAND_TIMEOUT_S, PSQL_CHILD_TIMEOUT_MS, [sql]);
  } catch (error) {
    const failure = describePsqlFailure(error, PSQL_COMMAND_TIMEOUT_S, PSQL_CHILD_TIMEOUT_MS);
    const reap = await reapSession(tag);
    if (reap.ended) {
      const ended = reap.found > 0 ? `\nits session ${tag} was still open and has been terminated` : "";
      throw new Error(`${failure}${ended}\nSQL: ${sql}`);
    }
    unconfirmedSessions.add(tag);
    throw new Error(
      `${failure}\nits session ${tag} is not confirmed ended (${reap.reason}), so checkpoint restores are refused until it is\nSQL: ${sql}`,
    );
  }
}

/**
 * `value` as an SQL string literal. NUL cannot be stored in a PostgreSQL text
 * value, so it is refused instead of being cut off.
 */
export function sqlLiteral(value: string): string {
  if (value.includes("\0")) {
    throw new Error("an SQL literal cannot hold a NUL character");
  }
  return `'${value.replaceAll("'", "''")}'`;
}

/** `value` when it holds only ASCII digits (exactly `width` when given). */
export function assertDigits(value: string, width?: number): string {
  const pattern = width === undefined ? /^[0-9]+$/ : new RegExp(`^[0-9]{${width}}$`);
  if (!pattern.test(value)) {
    throw new Error(
      `expected ${width === undefined ? "a digit string" : `${width} digits`}, got ${JSON.stringify(value)}`,
    );
  }
  return value;
}

const IDENTIFIER = /^[a-z_][a-z0-9_]*$/;

// ---------------------------------------------------------------------------
// Checkpoints
// ---------------------------------------------------------------------------

/**
 * The rows of one table a checkpoint covers: those matching `where`, an SQL
 * condition over the table's own columns. Rows that match it when the
 * checkpoint is restored but did not when it was taken are deleted; rows that
 * matched then are put back with every column as it was.
 */
export type RowScope = { table: string; where: string };

export type Checkpoint = {
  /**
   * Puts every covered row back as it was when the checkpoint was taken, in
   * one database transaction, after the psql() calls in flight when it starts
   * have settled and every failed call's session is confirmed ended; while
   * one is not, it rejects naming it. A call after a successful restore does
   * nothing; concurrent calls share one attempt; a failed attempt rejects, and
   * a later call retries it.
   */
  restore(): Promise<void>;
};

type ScopeSnapshot = {
  table: string;
  where: string;
  columns: readonly string[];
  key: readonly string[];
  rows: string;
};

// The column list, the primary key and the matching rows of one scope, read
// in one statement. The rows are a jsonb array, whose text has no newline, so
// the three fields come back on one line; the rows are last because their
// text may hold `|`.
async function snapshotScope(scope: RowScope): Promise<ScopeSnapshot> {
  if (!IDENTIFIER.test(scope.table)) {
    throw new Error(`not a table name: ${scope.table}`);
  }
  const relation = sqlLiteral(`public.${scope.table}`);
  const out = await psql(
    `SELECT
       (SELECT string_agg(quote_ident(a.attname), ',' ORDER BY a.attnum)
          FROM pg_attribute a
         WHERE a.attrelid = ${relation}::regclass AND a.attnum > 0 AND NOT a.attisdropped)
     || '|' ||
       (SELECT string_agg(quote_ident(a.attname), ',' ORDER BY k.ord)
          FROM pg_index i
          CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord)
          JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
         WHERE i.indrelid = ${relation}::regclass AND i.indisprimary)
     || '|' ||
       (SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text
          FROM public.${scope.table} t
         WHERE (${scope.where}))`,
  );
  const first = out.indexOf("|");
  const second = out.indexOf("|", first + 1);
  if (first <= 0 || second <= first + 1) {
    throw new Error(`table ${scope.table} has no primary key to restore by`);
  }
  return {
    table: scope.table,
    where: scope.where,
    columns: out.slice(0, first).split(","),
    key: out.slice(first + 1, second).split(","),
    rows: out.slice(second + 1),
  };
}

function restoreSql(snapshots: readonly ScopeSnapshot[]): string {
  const statements: string[] = ["BEGIN"];
  // Children before parents: a row created under a covered parent goes first.
  for (const s of [...snapshots].reverse()) {
    const key = s.key.join(", ");
    statements.push(
      `DELETE FROM public.${s.table}
        WHERE (${s.where})
          AND (${key}) NOT IN (
                SELECT ${key} FROM jsonb_populate_recordset(NULL::public.${s.table}, ${sqlLiteral(s.rows)}))`,
    );
  }
  // Parents before children, every column back as it was.
  for (const s of snapshots) {
    const columns = s.columns.join(", ");
    const others = s.columns.filter((column) => !s.key.includes(column));
    const conflict =
      others.length === 0
        ? "DO NOTHING"
        : `DO UPDATE SET (${others.join(", ")}) = ROW(${others.map((c) => `EXCLUDED.${c}`).join(", ")})`;
    statements.push(
      `INSERT INTO public.${s.table} (${columns}) OVERRIDING SYSTEM VALUE
       SELECT ${columns} FROM jsonb_populate_recordset(NULL::public.${s.table}, ${sqlLiteral(s.rows)})
       ON CONFLICT (${s.key.join(", ")}) ${conflict}`,
    );
  }
  statements.push("COMMIT");
  return statements.join(";\n");
}

/**
 * Takes a checkpoint of the rows `scopes` cover, listed parents first. Its
 * restore deletes, children first, the covered rows that did not exist, then
 * writes back, parents first, every covered row that did. Identity sequences
 * are not rewound, as after a rolled-back insert.
 */
export async function checkpoint(scopes: readonly RowScope[]): Promise<Checkpoint> {
  const snapshots: ScopeSnapshot[] = [];
  for (const scope of scopes) {
    snapshots.push(await snapshotScope(scope));
  }
  let restored = false;
  let attempt: Promise<void> | undefined;
  // One restore attempt. The psql() calls in flight are read before its own
  // call starts, and waited for (each is bounded and settles only once its
  // session can no longer write), so a call a timed-out test left running
  // cannot write after the restore. The attempt then refuses to run while a
  // failed call's session is still unconfirmed. The restore SQL is one
  // transaction, so a failed attempt changes nothing and a later call can
  // retry it.
  const restoreOnce = async (): Promise<void> => {
    try {
      await Promise.allSettled([...psqlInFlight]);
      await confirmSessionsEnded();
      await psql(restoreSql(snapshots));
      restored = true;
    } finally {
      attempt = undefined;
    }
  };
  return {
    async restore(): Promise<void> {
      if (restored) {
        return;
      }
      attempt ??= restoreOnce();
      await attempt;
    },
  };
}

// ---------------------------------------------------------------------------
// Fixture keys and users
// ---------------------------------------------------------------------------

/**
 * Keys of the seeded fixture corpus. `assertFixtureKeys` re-reads them from
 * app/data/ASCII, the files the seed loads.
 */
export const FIXTURE_KEYS = {
  /** ACCT-ID of acctdata.txt record 1. */
  account: "00000000001",
  /** XREF-CARD-NUM of account 1 (cardxref.txt record 49). */
  card: "9680294154603697",
  /** XREF-CUST-ID of account 1 (cardxref.txt record 49). */
  customer: "000000001",
  /** ACCT-ID of acctdata.txt record 2. */
  otherAccount: "00000000002",
  /** XREF-CARD-NUM of account 2 (cardxref.txt record 3). */
  otherCard: "0923877193247330",
  /** An 11-digit account number no acctdata.txt record holds. */
  missingAccount: "99999999999",
  /** Transaction type 01 Purchase (trantype.txt record 1). */
  tranType: "01",
  /** Category 0001 Regular Sales Draft of type 01 (trancatg.txt record 1). */
  tranCategory: "0001",
  /**
   * ACCT-ID of acctdata.txt record 50, the account every accrual-producing
   * step credits, so the reward balance of account 1 stays as the pinned
   * balance test sets it.
   */
  rewardAccount: "00000000050",
  /** XREF-CARD-NUM of account 50 (cardxref.txt record 1). */
  rewardCard: "0500024453765740",
  /**
   * ACCT-ID of acctdata.txt record 37, an account POSTTRAN credits no reward
   * points: every 01/0001 posting of its card is rejected as an overlimit
   * transaction (P-CBTRN02C-027, -052, -053, -063 and -064) and its one
   * posted record is a 03/0001 credit (P-CBTRN02C-189). It has no reward
   * balance row and no accrual, so the API answers the zero balance and an
   * empty history.
   */
  zeroRewardAccount: "00000000037",
  /**
   * Type 07 Adjustment (trantype.txt record 7) and its category 0001 Sales
   * draft credit adjustment (trancatg.txt record 18): a pair no dailytran.txt
   * record and no seed reward rule uses, for the rules the specs create.
   * reward_rules has no foreign key to the type tables
   * (migrations/0005_rewards.sql), and this pair exists in both anyway.
   */
  ruleTestTypeCode: "07",
  ruleTestCategoryCode: "0001",
} as const;

// CVACT03Y CARD-XREF-RECORD: XREF-CARD-NUM X(16), XREF-CUST-ID 9(09),
// XREF-ACCT-ID 9(11).
type XrefRecord = { card: string; customer: string; account: string };

function readXref(): XrefRecord[] {
  return readRepoText("app/data/ASCII/cardxref.txt")
    .split(/\r?\n/)
    .filter((line) => line.length >= 36)
    .map((line) => ({
      card: line.slice(0, 16),
      customer: line.slice(16, 25),
      account: line.slice(25, 36),
    }));
}

// The non-blank lines of a fixture file.
function fixtureLines(relative: string): string[] {
  return readRepoText(relative)
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "");
}

/**
 * Asserts that FIXTURE_KEYS agree with the fixture corpus: the accounts are
 * ACCT-IDs of acctdata.txt (CVACT01Y ACCT-ID 9(11) at offset 0), the missing
 * account is none, and each card and customer is the cross-reference of its
 * account in cardxref.txt. The rule-test type is a TRAN-TYPE of trantype.txt
 * (CVTRA03Y TRAN-TYPE X(02) at offset 0), its category a TRAN-TYPE-CD and
 * TRAN-CAT-CD key of trancatg.txt (CVTRA04Y, offsets 0 and 2), and no
 * dailytran.txt record (CVTRA06Y DALYTRAN-TYPE-CD and DALYTRAN-CAT-CD at
 * offsets 16 and 18) nor the two seed rules (01/0001 and 04/0003) use the
 * pair, so a rule the specs create matches no fixture posting.
 */
export function assertFixtureKeys(): void {
  const accounts = new Set(
    fixtureLines("app/data/ASCII/acctdata.txt").map((line) => line.slice(0, 11)),
  );
  const xref = readXref();
  const xrefOf = (account: string): XrefRecord | undefined =>
    xref.find((record) => record.account === account);
  expect(accounts.has(FIXTURE_KEYS.account), "account in acctdata.txt").toBe(true);
  expect(accounts.has(FIXTURE_KEYS.otherAccount), "other account in acctdata.txt").toBe(true);
  expect(accounts.has(FIXTURE_KEYS.rewardAccount), "reward account in acctdata.txt").toBe(true);
  expect(accounts.has(FIXTURE_KEYS.zeroRewardAccount), "zero-reward account in acctdata.txt").toBe(true);
  expect(accounts.has(FIXTURE_KEYS.missingAccount), "missing account absent").toBe(false);
  expect(xrefOf(FIXTURE_KEYS.account)).toEqual({
    card: FIXTURE_KEYS.card,
    customer: FIXTURE_KEYS.customer,
    account: FIXTURE_KEYS.account,
  });
  expect(xrefOf(FIXTURE_KEYS.otherAccount)?.card).toBe(FIXTURE_KEYS.otherCard);
  expect(xrefOf(FIXTURE_KEYS.rewardAccount)?.card).toBe(FIXTURE_KEYS.rewardCard);

  const ruleTestPair = `${FIXTURE_KEYS.ruleTestTypeCode}${FIXTURE_KEYS.ruleTestCategoryCode}`;
  const types = fixtureLines("app/data/ASCII/trantype.txt").map((line) => line.slice(0, 2));
  const categories = fixtureLines("app/data/ASCII/trancatg.txt").map((line) => line.slice(0, 6));
  const postings = fixtureLines("app/data/ASCII/dailytran.txt").map((line) => line.slice(16, 22));
  expect(types, "rule-test type in trantype.txt").toContain(FIXTURE_KEYS.ruleTestTypeCode);
  expect(categories, "rule-test category in trancatg.txt").toContain(ruleTestPair);
  expect(postings, "no dailytran.txt record of the rule-test pair").not.toContain(ruleTestPair);
  expect(
    [`${FIXTURE_KEYS.tranType}${FIXTURE_KEYS.tranCategory}`, "040003"],
    "the rule-test pair is no seed rule's",
  ).not.toContain(ruleTestPair);
}

export type Role = "admin" | "user";

/**
 * The two DUSRSECJ users the request names [app/jcl/DUSRSECJ.jcl:L35, L40]
 * and the route COSGN00C's XCTL split lands each on: COADM01C for type A,
 * COMEN01C for type U [app/cbl/COSGN00C.cbl:L230-240].
 */
export const USERS: Readonly<
  Record<Role, { userId: string; password: string; usrType: "A" | "U"; landing: string }>
> = {
  admin: { userId: "ADMIN001", password: "PASSWORD", usrType: "A", landing: "/admin" },
  user: { userId: "USER0001", password: "PASSWORD", usrType: "U", landing: "/" },
};

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/**
 * The message texts the specs assert, byte-identical to the constants of the
 * same name in crates/domain/src/messages.rs (`assertMessagesMatchCatalog`
 * compares them). Pages render them in toasts or field errors, where the
 * browser collapses runs of spaces, which Playwright's text matching does too.
 */
export const MESSAGES = {
  CCDA_MSG_THANK_YOU: "Thank you for using CardDemo application...      ",
  PLEASE_ENTER_USER_ID: "Please enter User ID ...",
  PLEASE_ENTER_PASSWORD: "Please enter Password ...",
  WRONG_PASSWORD_TRY_AGAIN: "Wrong Password. Try again ...",
  USER_NOT_FOUND_TRY_AGAIN: "User not found. Try again ...",
  NO_INPUT_RECEIVED: "No input received",
  ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT: "Account Filter must  be a non-zero 11 digit number",
  NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED:
    "No change detected with respect to values fetched.",
  CHANGES_COMMITTED_TO_DATABASE: "Changes committed to database",
  MUST_BE_SUPPLIED: " must be supplied.",
  IS_NOT_VALID: " is not valid",
  FIELD_CREDIT_LIMIT: "Credit Limit",
  ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT: "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER",
  CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT: "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
  CARD_NAME_NOT_PROVIDED: "Card name not provided",
  TRAN_ID_MUST_BE_NUMERIC: "Tran ID must be Numeric ...",
  ACCOUNT_OR_CARD_NUMBER_MUST_BE_ENTERED: "Account or Card Number must be entered...",
  ACCOUNT_ID_NOT_FOUND: "Account ID NOT found...",
  AMOUNT_SHOULD_BE_IN_FORMAT: "Amount should be in format -99999999.99",
  TRANSACTION_ADDED_SUCCESSFULLY: "Transaction added successfully. ",
  YOUR_TRAN_ID_IS: " Your Tran ID is ",
  SELECT_A_REPORT_TYPE_TO_PRINT_REPORT: "Select a report type to print report...",
  START_DATE_MONTH_CAN_NOT_BE_EMPTY: "Start Date - Month can NOT be empty...",
  REPORT_SUBMITTED_FOR_PRINTING: " report submitted for printing ...",
  REPORT_NAME_MONTHLY: "Monthly",
  REPORT_NAME_CUSTOM: "Custom",
  ACCT_ID_CAN_NOT_BE_EMPTY: "Acct ID can NOT be empty...",
  YOU_HAVE_NOTHING_TO_PAY: "You have nothing to pay...",
  PAYMENT_SUCCESSFUL: "Payment successful. ",
  YOUR_TRANSACTION_ID_IS: " Your Transaction ID is ",
  PLEASE_ENTER_ACCT_ID: "Please enter Acct Id...",
  ACCT_ID_MUST_BE_NUMERIC: "Acct Id must be Numeric ...",
  AUTH_MARKED_FRAUD: "AUTH MARKED FRAUD...",
  AUTH_FRAUD_REMOVED: "AUTH FRAUD REMOVED...",
  FIRST_NAME_CAN_NOT_BE_EMPTY: "First Name can NOT be empty...",
  USER_ID_ALREADY_EXIST: "User ID already exist...",
  PASSWORD_CAN_NOT_BE_EMPTY: "Password can NOT be empty...",
  PLEASE_MODIFY_TO_UPDATE: "Please modify to update ...",
  USER_LABEL: "User ",
  HAS_BEEN_ADDED: " has been added ...",
  HAS_BEEN_UPDATED: " has been updated ...",
  HAS_BEEN_DELETED: " has been deleted ...",
  MUST_BE_NUMERIC: " must be numeric.",
  CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY: " can have numbers or alphabets only.",
  FIELD_TRAN_TYPE_CODE: "Tran Type code",
  FIELD_TRANSACTION_DESC: "Transaction Desc",
  TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT: "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER",
  HIGHLIGHTED_ROW_WAS_UPDATED: "HIGHLIGHTED row was updated",
  HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE: "HIGHLIGHTED row deleted.Hit Enter to continue",
  UNEXPECTED_ERROR: "Unexpected error",
  AUTHENTICATION_REQUIRED: "Authentication required",
  ADMIN_ACCESS_REQUIRED: "Admin access required",
  REWARD_RATE_NEGATIVE: "reward_rate must not be negative",
  REWARD_MIN_EXCEEDS_MAX: "min_transaction_amount must not exceed max_transaction_amount",
  REWARD_ACTIVE_TO_BEFORE_FROM: "active_to must not precede active_from",
  ACCOUNT_NOT_FOUND: "Account not found",
} as const;

// The `pub const NAME: &str = "…";` declarations of messages.rs, their string
// literal unescaped (`\"` and `\\`; the catalog uses no other escape).
function readMessageCatalog(): Map<string, string> {
  const source = readRepoText("crates/domain/src/messages.rs");
  const declaration = /^pub const ([A-Z0-9_]+): &str =\s*"((?:[^"\\]|\\.)*)";/gm;
  const catalog = new Map<string, string>();
  for (const match of source.matchAll(declaration)) {
    const [, name, literal] = match;
    if (name !== undefined && literal !== undefined) {
      catalog.set(
        name,
        literal.replace(/\\(.)/g, (_, escaped: string) => escaped),
      );
    }
  }
  return catalog;
}

/** Asserts that every MESSAGES text equals its messages.rs constant. */
export function assertMessagesMatchCatalog(): void {
  const catalog = readMessageCatalog();
  const mismatched = Object.entries(MESSAGES).filter(
    ([name, text]) => catalog.get(name) !== text,
  );
  expect(mismatched, "MESSAGES entries that differ from messages.rs").toEqual([]);
}

/** The text a page shows for `message`, its trailing padding dropped. */
export function shown(message: string): string {
  return message.trim();
}

// ---------------------------------------------------------------------------
// Function map
// ---------------------------------------------------------------------------

export type ControlRole =
  | "link"
  | "button"
  | "menuitem"
  | "form-submit"
  | "dialog-action"
  | "breadcrumb";

export type InputKind = "textbox" | "password" | "radio";

export type MapInput = { kind: InputKind; label: string };

/** A parsed UI control cell: `route=…; control=<role> "<name>"[; input=…][; target=…]`. */
export type MapControl = {
  route: string;
  role: ControlRole;
  name: string;
  inputs: readonly MapInput[];
  target: string | null;
};

export type MapRow = {
  id: string;
  map: string;
  key: string;
  action: string;
  /** null for a `—` cell: the key has no web control. */
  control: MapControl | null;
  endpoint: string;
  /** 1-based line of the row in function-map.md. */
  line: number;
};

export type FunctionMap = {
  /** The BMS maps in document order, one table each. */
  maps: readonly string[];
  rows: readonly MapRow[];
  byId: ReadonlyMap<string, MapRow>;
};

export const FUNCTION_MAP_PATH = "docs/parity/function-map.md";

const MAP_HEADER = "| ID | BMS map | Key or field | COBOL action | UI control | Endpoint |";
const MAP_SEPARATOR = /^\|(?:\s*-{3,}\s*\|){6}$/;
const MAP_ID = /^F-([A-Z][A-Z0-9]*)-(\d{3})$/;
const NO_CONTROL = "—";
const CONTROL_CELL =
  /^route=(\/\S*); control=(link|button|menuitem|form-submit|dialog-action|breadcrumb) "([^"]+)"((?:; input=(?:textbox|password|radio) "[^"]+")*)(?:; target=(\/\S*))?$/;
const INPUT_PART = /; input=(textbox|password|radio) "([^"]+)"/g;

/**
 * The F-ID pattern of the map's Conventions, as the source of a global
 * RegExp. It names no concrete ID, so the coverage scan never counts itself.
 */
export const F_ID_SOURCE = String.raw`\bF-[A-Z][A-Z0-9]*-\d{3}\b`;

// The cells of a pipe-table row; `\|` is a literal pipe inside a cell.
function splitCells(line: string, lineNumber: number): string[] {
  if (!line.startsWith("|") || !line.endsWith("|")) {
    throw new Error(`${FUNCTION_MAP_PATH}:${lineNumber}: not a table row`);
  }
  const cells: string[] = [];
  let cell = "";
  for (let i = 1; i < line.length; i += 1) {
    const ch = line.charAt(i);
    if (ch === "\\" && line.charAt(i + 1) === "|") {
      cell += "|";
      i += 1;
    } else if (ch === "|") {
      cells.push(cell.trim());
      cell = "";
    } else {
      cell += ch;
    }
  }
  return cells;
}

function parseControl(cell: string, where: string): MapControl | null {
  if (cell === NO_CONTROL) {
    return null;
  }
  const match = CONTROL_CELL.exec(cell);
  if (match === null) {
    throw new Error(`${where}: malformed UI control cell: ${cell}`);
  }
  const [, route = "", role = "", name = "", inputCells = "", target] = match;
  const inputs: MapInput[] = [];
  for (const input of inputCells.matchAll(INPUT_PART)) {
    const [, kind = "", label = ""] = input;
    inputs.push({ kind: kind as InputKind, label });
  }
  for (const path of [route, target]) {
    if (path !== undefined) {
      routeParamsOf(path, where);
    }
  }
  return {
    route,
    role: role as ControlRole,
    name,
    inputs,
    target: target ?? null,
  };
}

/**
 * Parses every row of the 21 map tables of docs/parity/function-map.md. Each
 * `## <MAP>` section holds one table under the Conventions header; the
 * "Review log" and "Escalations raised" sections are not map tables. An ID
 * must be `F-<MAP>-<nnn>` with `<MAP>` its section and BMS map cell,
 * numbered from 001 without a gap, and appear once; a UI control cell must
 * follow the Conventions grammar with route parameters from the route table.
 * Any departure throws, naming the line.
 */
export function loadFunctionMap(): FunctionMap {
  const lines = readRepoText(FUNCTION_MAP_PATH).split(/\r?\n/);
  const maps: string[] = [];
  const rows: MapRow[] = [];
  const byId = new Map<string, MapRow>();
  let section: string | null = null;
  let tableOf: string | null = null;
  const tablesBySection = new Map<string, number>();

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    const lineNumber = index + 1;
    if (line.startsWith("### Review log") || line.startsWith("## Escalations raised")) {
      break;
    }
    if (line.startsWith("## ")) {
      section = line.slice(3).trim();
      tableOf = null;
      if (section !== "Conventions") {
        if (!/^[A-Z][A-Z0-9]*$/.test(section) || tablesBySection.has(section)) {
          throw new Error(`${FUNCTION_MAP_PATH}:${lineNumber}: unexpected section ${section}`);
        }
        tablesBySection.set(section, 0);
        maps.push(section);
      }
      continue;
    }
    if (line === MAP_HEADER) {
      if (section === null || section === "Conventions") {
        throw new Error(`${FUNCTION_MAP_PATH}:${lineNumber}: map table outside a map section`);
      }
      const separator = lines[index + 1] ?? "";
      if (!MAP_SEPARATOR.test(separator)) {
        throw new Error(`${FUNCTION_MAP_PATH}:${lineNumber + 1}: missing table separator`);
      }
      tablesBySection.set(section, (tablesBySection.get(section) ?? 0) + 1);
      tableOf = section;
      index += 1;
      continue;
    }
    if (tableOf === null) {
      continue;
    }
    if (!line.startsWith("|")) {
      tableOf = null;
      continue;
    }
    const where = `${FUNCTION_MAP_PATH}:${lineNumber}`;
    const cells = splitCells(line, lineNumber);
    if (cells.length !== 6) {
      throw new Error(`${where}: expected 6 cells, found ${cells.length}`);
    }
    const [id = "", map = "", key = "", action = "", control = "", endpoint = ""] = cells;
    const idMatch = MAP_ID.exec(id);
    if (idMatch === null || idMatch[1] !== tableOf || map !== tableOf) {
      throw new Error(`${where}: ID ${id} does not belong to map ${tableOf}`);
    }
    const expectedNumber = rows.filter((row) => row.map === tableOf).length + 1;
    if (idMatch[2] !== String(expectedNumber).padStart(3, "0")) {
      throw new Error(`${where}: ${id} breaks the numbering of map ${tableOf}`);
    }
    if (byId.has(id)) {
      throw new Error(`${where}: duplicate ID ${id}`);
    }
    const row: MapRow = {
      id,
      map,
      key,
      action,
      control: parseControl(control, where),
      endpoint,
      line: lineNumber,
    };
    rows.push(row);
    byId.set(id, row);
  }

  for (const [name, count] of tablesBySection) {
    if (count !== 1) {
      throw new Error(`${FUNCTION_MAP_PATH}: map ${name} has ${count} tables, expected 1`);
    }
  }
  return { maps, rows, byId };
}

let functionMapCache: FunctionMap | undefined;

/** The parsed function map, read once per worker. */
export function functionMap(): FunctionMap {
  functionMapCache ??= loadFunctionMap();
  return functionMapCache;
}

/** The map row `id`; throws when the map does not define it. */
export function mapRow(id: string): MapRow {
  const row = functionMap().byId.get(id);
  if (row === undefined) {
    throw new Error(`${FUNCTION_MAP_PATH} defines no row ${id}`);
  }
  return row;
}

/** The UI control of map row `id`; throws for a row without one. */
export function mapControl(id: string): MapControl {
  const control = mapRow(id).control;
  if (control === null) {
    throw new Error(`${id} has no UI control`);
  }
  return control;
}

/** The `index`-th input (0-based, screen order) of map row `id`. */
export function rowInput(id: string, index: number): MapInput {
  const input = mapControl(id).inputs[index];
  if (input === undefined) {
    throw new Error(`${id} lists no input ${index + 1}`);
  }
  return input;
}

export type HttpMethod = "GET" | "POST" | "PUT" | "DELETE";

/** An API request: its method and its path with `{param}` segments, no query. */
export type Endpoint = { method: HttpMethod; path: string };

const ENDPOINT_CELL = /^(GET|POST|PUT|DELETE) (\/\S*)$/;

/**
 * The request map row `id` sends: the method and path of its endpoint cell,
 * the query dropped. Throws for a row whose cell names no request (`—`).
 */
export function rowEndpoint(id: string): Endpoint {
  const cell = mapRow(id).endpoint;
  const match = ENDPOINT_CELL.exec(cell);
  if (match === null) {
    throw new Error(`${id} sends no request (endpoint cell ${JSON.stringify(cell)})`);
  }
  const [, method = "", target = ""] = match;
  return { method: method as HttpMethod, path: target.split("?")[0] ?? target };
}

/**
 * Whether `url`, a request the browser sent, addresses `path` under the dev
 * server's /api proxy: its pathname equals `/api` + `path`, each `{param}`
 * segment matching any one segment, whatever the query.
 */
export function matchesEndpoint(url: string, path: string): boolean {
  const pattern = apiPath(path.split("?")[0] ?? path)
    .split("/")
    .map((segment) => (/^\{[^/{}]+\}$/.test(segment) ? "[^/]+" : escapeRegExp(segment)))
    .join("/");
  return new RegExp(`^${pattern}$`).test(new URL(url).pathname);
}

/** Whether `request` is a call of `endpoint`. */
export function isRequestTo(request: Request, endpoint: Endpoint): boolean {
  return request.method() === endpoint.method && matchesEndpoint(request.url(), endpoint.path);
}

/** The F-IDs `source` cites, unique, in first-citation order. */
export function citedIds(source: string): string[] {
  return [...new Set(source.match(new RegExp(F_ID_SOURCE, "g")) ?? [])];
}

/**
 * The map coverage of the specs: the map IDs `navigationSource` does not
 * cite, and the IDs either source cites that the map does not define.
 */
export function citationReport(
  mapIds: Iterable<string>,
  navigationSource: string,
  formsSource: string,
): { uncited: string[]; unknown: string[] } {
  const defined = new Set(mapIds);
  const navigation = new Set(citedIds(navigationSource));
  const cited = new Set([...navigation, ...citedIds(formsSource)]);
  return {
    uncited: [...defined].filter((id) => !navigation.has(id)).sort(),
    unknown: [...cited].filter((id) => !defined.has(id)).sort(),
  };
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/**
 * The route parameters of the screen-domain route table (AAP 0.5.4.1). The
 * function map uses all but `ruleId`, which only the reward-rule edit route
 * carries.
 */
export const ROUTE_PARAMS = [
  "accountId",
  "cardNumber",
  "tranId",
  "authId",
  "userId",
  "code",
  "ruleId",
] as const;

export type RouteParam = (typeof ROUTE_PARAMS)[number];
export type RouteParams = Partial<Record<RouteParam, string>>;

function isRouteParam(name: string): name is RouteParam {
  return (ROUTE_PARAMS as readonly string[]).includes(name);
}

function routeParamsOf(route: string, where: string): RouteParam[] {
  if (!route.startsWith("/")) {
    throw new Error(`${where}: route ${route} is not absolute`);
  }
  return route
    .split("/")
    .filter((segment) => segment.startsWith(":"))
    .map((segment) => {
      const name = segment.slice(1);
      if (!isRouteParam(name)) {
        throw new Error(`${where}: unknown route parameter :${name} in ${route}`);
      }
      return name;
    });
}

/**
 * `route` with each `:param` segment replaced by its URL-encoded value from
 * `params`. Throws on a parameter outside the route table or one without a
 * value; values for parameters the route does not use are ignored.
 */
export function resolveRoute(route: string, params: RouteParams = {}): string {
  routeParamsOf(route, "resolveRoute");
  return route
    .split("/")
    .map((segment) => {
      if (!segment.startsWith(":")) {
        return segment;
      }
      const name = segment.slice(1) as RouteParam;
      const value = params[name];
      if (value === undefined || value === "") {
        throw new Error(`no value for :${name} in ${route}`);
      }
      return encodeURIComponent(value);
    })
    .join("/");
}

export type RouteAccess = "public" | "user" | "admin";

/** Every SPA route (AAP 0.5.4.1) and who may open it. */
export const APP_ROUTES: readonly { path: string; access: RouteAccess }[] = [
  { path: "/login", access: "public" },
  { path: "/", access: "user" },
  { path: "/accounts/:accountId", access: "user" },
  { path: "/accounts/:accountId/edit", access: "user" },
  { path: "/cards", access: "user" },
  { path: "/cards/:cardNumber", access: "user" },
  { path: "/cards/:cardNumber/edit", access: "user" },
  { path: "/transactions", access: "user" },
  { path: "/transactions/new", access: "user" },
  { path: "/transactions/:tranId", access: "user" },
  { path: "/reports/transactions", access: "user" },
  { path: "/payments", access: "user" },
  { path: "/authorizations", access: "user" },
  { path: "/authorizations/:authId", access: "user" },
  { path: "/rewards", access: "user" },
  { path: "/admin", access: "admin" },
  { path: "/admin/users", access: "admin" },
  { path: "/admin/users/new", access: "admin" },
  { path: "/admin/users/:userId/edit", access: "admin" },
  { path: "/admin/transaction-types", access: "admin" },
  { path: "/admin/transaction-types/new", access: "admin" },
  { path: "/admin/transaction-types/:code/edit", access: "admin" },
  { path: "/admin/reward-rules", access: "admin" },
  { path: "/admin/reward-rules/new", access: "admin" },
  { path: "/admin/reward-rules/:ruleId/edit", access: "admin" },
];

/** `text` with every RegExp metacharacter escaped, for a literal match. */
export function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** `path` as a URL matcher for `expect(page).toHaveURL`: path only, any query. */
export function urlPath(path: string): RegExp {
  return new RegExp(`^[a-z]+://[^/]+${escapeRegExp(path)}(?:[?#].*)?$`);
}

/**
 * `route` as a URL matcher in which each `:param` segment matches any one
 * path segment, for a control whose target key comes from the row it acts
 * on (a row action) rather than from the spec.
 */
export function routePattern(route: string): RegExp {
  routeParamsOf(route, "routePattern");
  const path = route
    .split("/")
    .map((segment) => (segment.startsWith(":") ? "[^/?#]+" : escapeRegExp(segment)))
    .join("/");
  return new RegExp(`^[a-z]+://[^/]+${path}(?:[?#].*)?$`);
}

// ---------------------------------------------------------------------------
// Processing clock
// ---------------------------------------------------------------------------

/**
 * The processing clock the API under test runs on, as this Playwright run
 * names it. CT02 accrues rewards, under the rules active at that instant,
 * and DELETE /admin/reward-rules/{id} ends a rule at it; both read
 * CARDDEMO_CLOCK_OVERRIDE (AAP 0.6.4), an RFC 3339 instant that pins the
 * clock, where only an empty value counts as unset. The API and the
 * Playwright run must therefore share the same CARDDEMO_CLOCK_OVERRIDE, both
 * unset by default; the CT02 accrual tests of rewards.spec.ts assert that
 * they do.
 */
export type ProcessingClock = {
  /** The run's CARDDEMO_CLOCK_OVERRIDE verbatim; null when unset or empty (the system clock). */
  override: string | null;
  /** The processing instant as an SQL timestamptz expression: the override, else now(). */
  instant: string;
  /** The instant as assertion messages name it. */
  label: string;
};

/** The processing clock of this run, from CARDDEMO_CLOCK_OVERRIDE (see ProcessingClock). */
export function processingClock(): ProcessingClock {
  const override = process.env.CARDDEMO_CLOCK_OVERRIDE;
  if (override === undefined || override === "") {
    return { override: null, instant: "now()", label: "now() (CARDDEMO_CLOCK_OVERRIDE unset)" };
  }
  return {
    override,
    instant: `${sqlLiteral(override)}::timestamptz`,
    label: `CARDDEMO_CLOCK_OVERRIDE ${override}`,
  };
}

/**
 * The UTC date (`YYYY-MM-DD`) of the processing instant, computed by
 * PostgreSQL: the day the reward-rule form's `active_from` must name for a
 * rule the processing clock has already started, since the form sends a
 * date as midnight UTC of that day.
 */
export async function processingDate(): Promise<string> {
  const clock = processingClock();
  const date = await psql(`SELECT to_char((${clock.instant}) AT TIME ZONE 'UTC', 'YYYY-MM-DD')`);
  if (!/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(date)) {
    throw new Error(`no UTC date for the processing instant ${clock.label} (got ${JSON.stringify(date)})`);
  }
  return date;
}

// ---------------------------------------------------------------------------
// Database fixtures
// ---------------------------------------------------------------------------

/**
 * Sets the reward balance of `accountId` to `points` earned, 0 redeemed and
 * `points` available, the CHECK invariant of reward_balances, and returns the
 * checkpoint that puts the account's balance row back as it was (or removes
 * it when the account had none).
 */
export async function setRewardBalance(accountId: string, points: string): Promise<Checkpoint> {
  const account = `${sqlLiteral(assertDigits(accountId, 11))}::bigint`;
  const value = `${sqlLiteral(assertDigits(points))}::bigint`;
  const cp = await checkpoint([{ table: "reward_balances", where: `account_id = ${account}` }]);
  await psql(
    `INSERT INTO reward_balances (account_id, total_points_earned, total_points_redeemed, available_points)
     VALUES (${account}, ${value}, 0, ${value})
     ON CONFLICT (account_id) DO UPDATE
        SET total_points_earned = EXCLUDED.total_points_earned,
            total_points_redeemed = 0,
            available_points = EXCLUDED.available_points`,
  );
  return cp;
}

export type SeededAuthorization = Checkpoint & {
  /** The new pending_auth_details identity, the `:authId` of its routes. */
  authId: string;
  /** The card the authorization was requested for. */
  cardNumber: string;
};

/**
 * Seeds one approved pending authorization for `accountId`, as COPAUA0C
 * stores one: a PAUTSUM0 summary (kept when the account has one) and a
 * PAUTDTL1 detail for the account's cross-referenced card, newest under the
 * account. `fraud` "F" seeds it already reported as fraud. The returned
 * checkpoint restores the account's summaries, details and the card's
 * auth_fraud rows.
 */
export async function seedPendingAuthorization(
  accountId: string,
  options: { fraud?: "" | "F" } = {},
): Promise<SeededAuthorization> {
  const account = `${sqlLiteral(assertDigits(accountId, 11))}::bigint`;
  const cardNumber = await psql(
    `SELECT xref_card_num FROM card_xref WHERE xref_acct_id = ${account} ORDER BY xref_card_num LIMIT 1`,
  );
  if (!/^[0-9]{16}$/.test(cardNumber)) {
    throw new Error(`account ${accountId} has no cross-referenced card`);
  }
  const card = sqlLiteral(cardNumber);
  const fraud = options.fraud ?? "";
  const cp = await checkpoint([
    { table: "pending_auth_summaries", where: `pa_acct_id = ${account}` },
    { table: "pending_auth_details", where: `pa_acct_id = ${account}` },
    { table: "auth_fraud", where: `card_num = ${card}` },
  ]);
  try {
    await psql(
      `INSERT INTO pending_auth_summaries
         (pa_acct_id, pa_cust_id, pa_auth_status, pa_account_status, pa_credit_limit,
          pa_cash_limit, pa_credit_balance, pa_cash_balance, pa_approved_auth_cnt,
          pa_declined_auth_cnt, pa_approved_auth_amt, pa_declined_auth_amt)
       SELECT x.xref_acct_id, x.xref_cust_id, '', ARRAY['', '', '', '', '']::varchar(2)[],
              a.acct_credit_limit, a.acct_cash_credit_limit, 12.34, 0, 1, 0, 12.34, 0
         FROM card_xref x JOIN accounts a ON a.account_id = x.xref_acct_id
        WHERE x.xref_card_num = ${card}
       ON CONFLICT (pa_acct_id) DO NOTHING`,
    );
    // PA-AUTH-DATE-9C = 99999 - YYDDD and PA-AUTH-TIME-9C = 999999999 -
    // HHMMSSmmm [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875]
    // for 2022-07-01 (YYDDD 22182); each seed takes the next later time of
    // that day, so the newest seed is first under the account. The insert
    // sits in a CTE so psql prints the new key alone, without the INSERT tag.
    const authId = await psql(
      `WITH slot AS (
         SELECT coalesce(min(pa_auth_time_9c), 999999999 - 120000000 + 1000) - 1000 AS time_9c
           FROM pending_auth_details
          WHERE pa_acct_id = ${account} AND pa_auth_date_9c = 99999 - 22182
       ), inserted AS (
       INSERT INTO pending_auth_details
         (pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, pa_auth_orig_date, pa_auth_orig_time,
          pa_card_num, pa_auth_type, pa_card_expiry_date, pa_message_type, pa_message_source,
          pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason, pa_processing_code,
          pa_transaction_amt, pa_approved_amt, pa_merchant_catagory_code, pa_acqr_country_code,
          pa_pos_entry_mode, pa_merchant_id, pa_merchant_name, pa_merchant_city,
          pa_merchant_state, pa_merchant_zip, pa_transaction_id, pa_match_status,
          pa_auth_fraud, pa_fraud_rpt_date)
       SELECT ${account}, 99999 - 22182, slot.time_9c, '220701',
              lpad(((999999999 - slot.time_9c) / 1000)::text, 6, '0'),
              c.card_number, 'POS', substr(c.card_expiraion_date, 6, 2) || substr(c.card_expiraion_date, 3, 2),
              '0100', 'POS', '000001', '00', '0000', 0, 12.34, 12.34, '5411', '840', 1,
              '000000000000001', 'E2E FIXTURE MERCHANT', 'SEATTLE', 'WA', '98101',
              'E2E000000000001', 'P', ${sqlLiteral(fraud)}, ${sqlLiteral(fraud === "F" ? "07/01/22" : "")}
         FROM cards c, slot
        WHERE c.card_number = ${card}
       RETURNING auth_id
       )
       SELECT auth_id FROM inserted`,
    );
    return { authId: assertDigits(authId), cardNumber, restore: () => cp.restore() };
  } catch (error) {
    await cp.restore();
    throw error;
  }
}

/** The highest TRAN-ID of the live transaction master. */
export async function latestTranId(): Promise<string> {
  const tranId = await psql("SELECT max(tran_id) FROM transaction_master");
  if (!/^[0-9]{16}$/.test(tranId)) {
    throw new Error(`the live transaction master holds no TRAN-ID (got ${JSON.stringify(tranId)})`);
  }
  return tranId;
}

/**
 * The TRAN-ID CT02 and CB00 allocate next: highest key + 1 as 9(16). The ADD
 * has no ON SIZE ERROR, so a sum that does not fit 16 digits loses its
 * high-order digit and 9999999999999999 is followed by 0000000000000000, as
 * `TranId::next` computes it. The sum is therefore reduced modulo 10^16 before
 * it is padded: `lpad` would cut a 17-digit sum on the right instead.
 *
 * An empty live master gives 0000000000000001, the first key of an empty file
 * (P-COTRN02C-022). CT02 and CB00 allocate nothing there (STARTBR NOTFND,
 * E-27), so a spec that needs a live row writes it with this key itself.
 */
export async function nextTranId(): Promise<string> {
  return assertDigits(
    await psql(
      `SELECT lpad(((coalesce(max(tran_id::numeric), 0) + 1) % 10000000000000000)::text, 16, '0')
         FROM transaction_master`,
    ),
    16,
  );
}

/**
 * A checkpoint of everything a CT02 add or a CB00 payment for `accountIds`
 * writes: the accounts, their reward balances and category balances, and
 * every transaction and accrual created after it is taken.
 */
export async function transactionWriteCheckpoint(
  accountIds: readonly string[],
): Promise<Checkpoint> {
  if (accountIds.length === 0) {
    throw new Error("a transaction checkpoint needs at least one account");
  }
  const accounts = accountIds
    .map((id) => `${sqlLiteral(assertDigits(id, 11))}::bigint`)
    .join(", ");
  const [transactionMark = "", accrualMark = ""] = (
    await psql(
      `SELECT (SELECT coalesce(max(transaction_id), 0) FROM transactions)
              || '|' || (SELECT coalesce(max(accrual_id), 0) FROM reward_accruals)`,
    )
  ).split("|");
  assertDigits(transactionMark);
  assertDigits(accrualMark);
  return checkpoint([
    { table: "accounts", where: `account_id IN (${accounts})` },
    { table: "reward_balances", where: `account_id IN (${accounts})` },
    { table: "transaction_category_balances", where: `trancat_acct_id IN (${accounts})` },
    { table: "transactions", where: `transaction_id > ${transactionMark}` },
    {
      table: "reward_accruals",
      where: `accrual_id > ${accrualMark} OR transaction_id > ${transactionMark}`,
    },
  ]);
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/** The API path under the dev server's /api proxy, the browser's only route. */
export function apiPath(path: string): string {
  return `/api${path}`;
}

// Checked decoders of the API responses the specs read, one per OpenAPI
// schema. Each is strict, so a body with a key the schema does not declare is
// rejected like a body missing one, and `satisfies` fails `tsc` when a decoder
// drifts from the generated type.

export const loginResponseSchema = z
  .object({
    expires_at: z.string(),
    landing: z.string(),
    token: z.string(),
    user_id: z.string(),
    usr_type: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["LoginResponse"]>;

export const transactionResponseSchema = z
  .object({
    tran_amt: z.string(),
    tran_card_num: z.string(),
    tran_cat_cd: z.string(),
    tran_desc: z.string(),
    tran_id: z.string(),
    tran_merchant_city: z.string(),
    tran_merchant_id: z.string(),
    tran_merchant_name: z.string(),
    tran_merchant_zip: z.string(),
    tran_orig_ts: z.string(),
    tran_proc_ts: z.string(),
    tran_source: z.string(),
    tran_type_cd: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["TransactionResponse"]>;

export const transactionAddResponseSchema = z
  .object({
    message: z.string(),
    transaction: transactionResponseSchema,
  })
  .strict() satisfies z.ZodType<components["schemas"]["TransactionAddResponse"]>;

export const errorFieldBodySchema = z
  .object({
    field: z.string(),
    message: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["ErrorFieldBody"]>;

export const errorBodySchema = z
  .object({
    code: z.string(),
    fields: z.array(errorFieldBodySchema).optional(),
    message: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["ErrorBody"]>;

export const rewardRuleResponseSchema = z
  .object({
    active_from: z.string(),
    active_to: z.string().nullable(),
    max_transaction_amount: z.string().nullable(),
    min_transaction_amount: z.string().nullable(),
    reward_rate: z.string(),
    rule_id: z.string(),
    status: z.enum(["scheduled", "active", "ended"]),
    transaction_category_cd: z.string(),
    transaction_type_cd: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["RewardRuleResponse"]>;

export const rewardBalanceResponseSchema = z
  .object({
    account_id: z.string(),
    available_points: z.string(),
    total_points_earned: z.string(),
    total_points_redeemed: z.string(),
  })
  .strict() satisfies z.ZodType<components["schemas"]["RewardBalanceResponse"]>;

/**
 * `response`'s JSON body decoded by `schema`, for an API response and a
 * browser response alike. A body that is not JSON, or that `schema` rejects,
 * throws an error that names the URL and, for a rejected body, every issue
 * `schema` reports with its path.
 */
export async function readJson<T>(
  response: { json(): Promise<unknown>; url(): string },
  schema: z.ZodType<T, z.ZodTypeDef, unknown>,
): Promise<T> {
  let body: unknown;
  try {
    body = await response.json();
  } catch (error) {
    throw new Error(`the body of ${response.url()} is not JSON`, { cause: error });
  }
  const decoded = schema.safeParse(body);
  if (!decoded.success) {
    const issues = decoded.error.issues
      .map((issue) => `${issue.path.length === 0 ? "(body)" : issue.path.join(".")}: ${issue.message}`)
      .join("; ");
    throw new Error(`the JSON body of ${response.url()} does not match its schema: ${issues}`);
  }
  return decoded.data;
}

/** The bearer header of `token`. */
export function bearer(token: string): Record<string, string> {
  return { Authorization: `Bearer ${token}` };
}

/** Signs `role` on through POST /auth/login and returns the bearer token. */
export async function apiToken(request: APIRequestContext, role: Role): Promise<string> {
  const user = USERS[role];
  const response = await request.post(apiPath("/auth/login"), {
    data: { user_id: user.userId, password: user.password },
  });
  expect(response.status(), `sign-on of ${user.userId}`).toBe(200);
  const body = await readJson(response, loginResponseSchema);
  expect(body.user_id).toBe(user.userId);
  expect(body.usr_type).toBe(user.usrType);
  expect(body.landing).toBe(user.landing);
  return body.token;
}

export type AddedTransaction = Checkpoint & {
  status: number;
  body: TransactionAddResponse | ErrorBody;
  /** The TRAN-ID of the 201 response, null otherwise. */
  tranId: string | null;
};

/**
 * A CT02 add the request accepts: the reward account (50), type 01/0001 (so
 * the retail reward rule accrues), amount +12.34 in the audited
 * `-99999999.99` form (V-COTRN02C-021), confirmed.
 */
export function transactionAddBody(
  overrides: Partial<TransactionAddRequest> = {},
): TransactionAddRequest {
  return {
    account_id: FIXTURE_KEYS.rewardAccount,
    card_number: "",
    tran_type_cd: FIXTURE_KEYS.tranType,
    tran_cat_cd: FIXTURE_KEYS.tranCategory,
    tran_source: "POS TERM",
    tran_desc: "E2E FIXTURE PURCHASE",
    tran_amt: "+00000012.34",
    tran_orig_date: "2022-07-01",
    tran_proc_date: "2022-07-01",
    tran_merchant_id: "800000001",
    tran_merchant_name: "E2E FIXTURE MERCHANT",
    tran_merchant_city: "SEATTLE",
    tran_merchant_zip: "98101",
    confirm: "Y",
    ...overrides,
  };
}

// The account a CT02 add credits rewards to: its account ID, else the account
// its card cross-references (COTRN02C reads CCXREF by card when no account is
// entered), else the reward account for a body the API rejects before any
// write.
async function accountOfAdd(body: TransactionAddRequest): Promise<string> {
  if (typeof body.account_id === "string" && /^[0-9]{11}$/.test(body.account_id)) {
    return body.account_id;
  }
  if (typeof body.card_number === "string" && /^[0-9]{16}$/.test(body.card_number)) {
    const account = await psql(
      `SELECT lpad(xref_acct_id::text, 11, '0') FROM card_xref WHERE xref_card_num = ${sqlLiteral(body.card_number)}`,
    );
    if (/^[0-9]{11}$/.test(account)) {
      return account;
    }
  }
  return FIXTURE_KEYS.rewardAccount;
}

/**
 * Sends POST /transactions with `transactionAddBody(overrides)` as the user
 * `token` names. The result carries the response and the checkpoint, taken
 * before the request, that removes the transaction, its accrual and the
 * balance change again.
 */
export async function addTransactionViaApi(
  request: APIRequestContext,
  token: string,
  overrides: Partial<TransactionAddRequest> = {},
): Promise<AddedTransaction> {
  const body = transactionAddBody(overrides);
  const cp = await transactionWriteCheckpoint([await accountOfAdd(body)]);
  try {
    const response = await request.post(apiPath("/transactions"), {
      data: body,
      headers: bearer(token),
    });
    const status = response.status();
    const parsed =
      status === 201
        ? await readJson(response, transactionAddResponseSchema)
        : await readJson(response, errorBodySchema);
    const tranId = status === 201 && "transaction" in parsed ? parsed.transaction.tran_id : null;
    return { status, body: parsed, tranId, restore: () => cp.restore() };
  } catch (error) {
    await cp.restore();
    throw error;
  }
}

export type AccrualTopUp = Checkpoint & {
  /** How many CT02 adds the top-up sent, as PostgreSQL counted them. */
  added: number;
};

/**
 * Brings the reward history of `accountId` to at least `minimum` accruals
 * (a decimal string) with CT02 adds through the API as the user `token`
 * names, each of the `transactionAddBody` kind, so the retail rule accrues
 * one row per add. PostgreSQL computes how many are missing, and the adds
 * follow the lines of `generate_series(1, n)`. The returned checkpoint, taken
 * before the first add, removes every added transaction and accrual and puts
 * the balance back.
 */
export async function topUpAccruals(
  request: APIRequestContext,
  token: string,
  accountId: string,
  minimum: string,
): Promise<AccrualTopUp> {
  const account = `${sqlLiteral(assertDigits(accountId, 11))}::bigint`;
  const missing = assertDigits(
    await psql(
      `SELECT greatest(0, ${sqlLiteral(assertDigits(minimum))}::bigint - count(*))
         FROM reward_accruals WHERE account_id = ${account}`,
    ),
  );
  const series = await psql(`SELECT generate_series(1, ${sqlLiteral(missing)}::bigint)`);
  const lines = series === "" ? [] : series.split("\n");
  const cp = await transactionWriteCheckpoint([accountId]);
  try {
    // `cp` already covers every add, so each is sent directly: a checkpoint
    // per add (addTransactionViaApi) would cost six more psql round trips.
    for (const line of lines) {
      const response = await request.post(apiPath("/transactions"), {
        data: transactionAddBody({
          account_id: accountId,
          tran_desc: `E2E REWARD HISTORY ${assertDigits(line)}`,
        }),
        headers: bearer(token),
      });
      expect(response.status(), `top-up add ${line} of ${missing}`).toBe(201);
    }
    expect(
      await psql(
        `SELECT count(*) >= ${sqlLiteral(minimum)}::bigint FROM reward_accruals WHERE account_id = ${account}`,
      ),
      `account ${accountId} holds at least ${minimum} accruals`,
    ).toBe("t");
    return { added: lines.length, restore: () => cp.restore() };
  } catch (error) {
    await cp.restore();
    throw error;
  }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/**
 * The page content: `main#main-content`, the target of the app shell's
 * "Skip to content" link. Page controls are located inside it, so the
 * sidebar's links never shadow a page link of the same name.
 */
export function content(page: Page): Locator {
  return page.locator("#main-content");
}

/** The sidebar navigation landmark "Main" (web/src/components/app-sidebar.tsx). */
export function sidebar(page: Page): Locator {
  return page.getByRole("navigation", { name: "Main", exact: true });
}

/** The open confirm dialog (web/src/components/confirm-dialog.tsx). */
export function confirmDialog(page: Page): Locator {
  return page.getByRole("alertdialog");
}

/** The sonner toast region ("Notifications" and its hotkey). */
export function notifications(page: Page): Locator {
  return page.getByRole("region", { name: /^Notifications\b/ });
}

/**
 * Asserts that `message` is shown, as a toast or as a field error (AAP
 * 0.5.4.3: a `message` is toasted, `fields[]` entries are field errors).
 */
export async function expectMessage(page: Page, message: string): Promise<void> {
  await expect(page.getByText(shown(message), { exact: true }).first()).toBeVisible();
}

/**
 * A label an input may carry: one accessible name, or the names a page may
 * give the same BMS field (its BMS caption, or the request DTO field it
 * fills).
 */
export type LabelText = string | readonly string[];

/**
 * An anchored, case-insensitive accessible-name matcher for `label`: any of
 * its names, optionally followed by the colon a BMS caption ends with.
 */
export function nameMatcher(label: LabelText): RegExp {
  const names: readonly string[] = typeof label === "string" ? [label] : label;
  if (names.length === 0) {
    throw new Error("a label needs at least one name");
  }
  return new RegExp(`^(?:${names.map(escapeRegExp).join("|")})\\s*:?$`, "i");
}

/**
 * Accessible names of inputs whose labels docs/parity/function-map.md does
 * not fix (it lists inputs only where a key needs them). Each is the BMS
 * caption of the field without its colon, or the map's labelling of the same
 * BMS field elsewhere; they are matched case-insensitively through
 * `nameMatcher`, which also accepts the caption's colon.
 */
export const UNMAPPED_LABELS = {
  /**
   * COTRN02 captions [app/bms/COTRN02.bms:L84-266]: ACTIDIN 'Enter Acct #:',
   * CARDNIN 'Card #:', TTYPCD 'Type CD:', TCATCD 'Category CD:', TRNSRC
   * 'Source:', TDESC 'Description:', TRNAMT 'Amount:', TORIGDT 'Orig Date:',
   * TPROCDT 'Proc Date:', MID 'Merchant ID:', MNAME 'Merchant Name:', MCITY
   * 'Merchant City:', MZIP 'Merchant Zip:'. The key fields also accept the
   * names the map gives the account and card elsewhere.
   */
  transactionAdd: {
    accountId: ["Enter Acct #", "Account ID"],
    cardNumber: ["Card #", "Card number"],
    typeCode: "Type CD",
    categoryCode: "Category CD",
    source: "Source",
    description: "Description",
    amount: "Amount",
    origDate: "Orig Date",
    procDate: "Proc Date",
    merchantId: "Merchant ID",
    merchantName: "Merchant Name",
    merchantCity: "Merchant City",
    merchantZip: "Merchant Zip",
  },
  /** COACTUP ACRDLIM 'Credit Limit :'. */
  accountEdit: { creditLimit: "Credit Limit" },
  /** COCRDUP CRDNAME 'Name on card :'. */
  cardEdit: { nameOnCard: "Name on card" },
  /**
   * COUSR01 and COUSR02 FNAME, LNAME, USERID, PASSWD, USRTYPE captions, and
   * the edit page's "Change password" switch in front of PASSWD (E-21).
   */
  userForm: {
    firstName: "First Name",
    lastName: "Last Name",
    userId: "User ID",
    password: "Password",
    userType: "User Type",
    changePassword: "Change password",
  },
  /** COTRTUP TRTYDSC 'Description :' and the COTRTLI row TRTYPD. */
  tranType: { description: "Description" },
  /** RewardsPage account lookup: the CAVW account key the dashboard uses. */
  rewards: { accountId: "Account number" },
  /** RewardRuleRequest fields. */
  rewardRule: {
    typeCode: "Transaction type code",
    categoryCode: "Transaction category code",
    rate: "Reward rate",
    minAmount: "Minimum transaction amount",
    maxAmount: "Maximum transaction amount",
    activeFrom: "Active from",
    activeTo: "Active to",
  },
} as const;

/**
 * The names the reward screens render (RewardsPage, RewardRuleListPage,
 * RewardRuleFormPage and the dashboard's reward card). They are new screens
 * (o8), so no function-map row names their controls; these are the names
 * their page briefs fix.
 */
export const REWARD_SCREENS = {
  rewards: {
    route: "/rewards",
    lookUp: "Look up",
    earned: "Points earned",
    redeemed: "Points redeemed",
    available: "Available points",
    historyCaption: "Reward history",
    historyColumns: ["Accrual ID", "Transaction row", "Rule ID", "Points earned", "Created at"],
    /** The history table's text in place of the rows of an empty history. */
    emptyHistory: "No reward points have been earned on this account.",
    /** HISTORY_PAGE_SIZE: the rows of one history page. */
    historyPageSize: "10",
  },
  /** The dashboard SummaryCard of the looked-up account's available points. */
  dashboardPoints: "Reward Points",
  ruleList: {
    route: "/admin/reward-rules",
    title: "Reward Rules",
    newRule: "New rule",
    caption: "Reward rules",
    columns: [
      "Rule ID",
      "Type",
      "Category",
      "Rate",
      "Min amount",
      "Max amount",
      "Active from",
      "Active to",
      "Status",
      "Actions",
    ],
    /** A null bound or end, as the list shows it. */
    none: "—",
    /** The Status badge of each server `status`. */
    active: "Active",
    scheduled: "Scheduled",
    ended: "Ended",
    edit: "Edit",
    delete: "Delete",
  },
  ruleForm: {
    newRoute: "/admin/reward-rules/new",
    newTitle: "New reward rule",
    editTitle: "Edit reward rule",
    save: "Save",
    reset: "Reset",
    back: "Back",
  },
  /** The soft-delete ConfirmDialog of the rule list. */
  deleteDialog: { title: "Delete reward rule", confirm: "Delete", cancel: "Cancel" },
} as const;

/** The table named by its caption `caption` (a DataTable's caption prop). */
export function tableByCaption(scope: Page | Locator, caption: string): Locator {
  return scope.getByRole("table", { name: caption, exact: true });
}

/** The data rows of `table`: every row but the header row. */
export function dataRows(table: Locator): Locator {
  return table.getByRole("row").filter({ hasNot: table.page().getByRole("columnheader") });
}

/**
 * The cell of `row` under the column headed `column` in `table`, found by the
 * header's position, so a spec reads a cell by its column name.
 */
export async function cellUnder(table: Locator, row: Locator, column: string): Promise<Locator> {
  await expect(table.getByRole("columnheader", { name: column, exact: true })).toBeVisible();
  const headers = (await table.getByRole("columnheader").allInnerTexts()).map((text) => text.trim());
  const index = headers.indexOf(column);
  if (index < 0) {
    throw new Error(`no column ${JSON.stringify(column)} among ${JSON.stringify(headers)}`);
  }
  return row.getByRole("cell").nth(index);
}


/** The input of a map row: a password by its label, others by role and name. */
export function mapInput(scope: Page | Locator, input: MapInput): Locator {
  if (input.kind === "password") {
    return scope.getByLabel(input.label, { exact: true });
  }
  return scope.getByRole(input.kind, { name: input.label, exact: true });
}

/**
 * The page region a control of `route` renders in: the page content of the
 * app shell, or the whole page for the public sign-on screen, which has no
 * shell.
 */
export function routeScope(page: Page, route: string): Locator {
  return route === "/login" ? page.locator("body") : content(page);
}

/**
 * The element a map control names, on the current page. Page controls are
 * located in the route's scope and dialog actions inside the open dialog;
 * row actions repeat per row, so the first match is taken.
 */
export function controlLocator(page: Page, control: MapControl): Locator {
  const name = { name: control.name, exact: true };
  const scope = routeScope(page, control.route);
  switch (control.role) {
    case "link":
      return scope.getByRole("link", name).first();
    case "button":
    case "form-submit":
      return scope.getByRole("button", name).first();
    case "dialog-action":
      return confirmDialog(page).getByRole("button", name);
    case "menuitem":
      return page.getByRole("menuitem", name);
    case "breadcrumb":
      return page
        .getByRole("navigation", { name: "breadcrumb", exact: true })
        .getByRole("link", name);
  }
}

/** The sidebar link of a menu option (an `option <n>` row or a menu entry). */
export function sidebarLink(page: Page, name: string): Locator {
  return sidebar(page).getByRole("link", { name, exact: true });
}

/** The user-menu trigger of the app shell, named by the signed-on user ID. */
export function userMenuTrigger(page: Page, role: Role): Locator {
  return page.getByRole("button", { name: USERS[role].userId, exact: true });
}

// The F-COSGN00-001 sign-on control: its route, the user-ID textbox, the
// password input and the submit button.
function signOnForm(page: Page): { route: string; userId: Locator; password: Locator; submit: Locator } {
  const control = mapControl("F-COSGN00-001");
  const userIdInput = control.inputs.find((input) => input.kind === "textbox");
  const passwordInput = control.inputs.find((input) => input.kind === "password");
  if (userIdInput === undefined || passwordInput === undefined) {
    throw new Error("F-COSGN00-001 must list the user ID textbox and the password input");
  }
  return {
    route: control.route,
    userId: mapInput(page, userIdInput),
    password: mapInput(page, passwordInput),
    submit: page.getByRole("button", { name: control.name, exact: true }),
  };
}

/** Signs `role` on through the sign-on screen and waits for its landing route. */
export async function signIn(page: Page, role: Role): Promise<void> {
  const user = USERS[role];
  const form = signOnForm(page);
  await page.goto(form.route);
  await form.userId.fill(user.userId);
  await form.password.fill(user.password);
  await form.submit.click();
  await expect(page).toHaveURL(urlPath(user.landing));
}

// Tab cannot always wrap around the document. The sonner toast list sits
// after the page content, and on losing focus it moves focus back to the
// element focused before the list, so while a toast is shown Tab loops between
// that element and the toast. tabTo therefore starts with Shift+Tab toward a
// target that precedes the focused element, and reverses once when focus comes
// back to an element it already rested on. The second rule also covers a
// focused element that was disabled or removed: focus is then on the body,
// and Chrome resumes Tab from where the element was, which document order
// cannot tell.

// Whether `target` comes before the focused element in document order.
async function precedesFocus(target: Locator): Promise<boolean> {
  return target.evaluate((element) => {
    const focused = document.activeElement;
    return (
      focused !== null &&
      focused !== element &&
      (focused.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_PRECEDING) !== 0
    );
  });
}

/**
 * Presses Tab or Shift+Tab until `target` has focus, at most `maxPresses`
 * times, so the target is proven reachable from the keyboard. Without
 * `backward` the direction is chosen by document order and reversed once if
 * focus loops; with it the direction is fixed.
 */
export async function tabTo(
  page: Page,
  target: Locator,
  options: { backward?: boolean; maxPresses?: number } = {},
): Promise<void> {
  await expect(target).toBeVisible();
  let backward = options.backward ?? (await precedesFocus(target));
  let reversible = options.backward === undefined;
  // Enough for the longest form (COACTUP) after the skip link and sidebar.
  const maxPresses = options.maxPresses ?? 200;
  // The elements focus has rested on, kept in the page so they compare by
  // identity.
  const visited = await page.evaluateHandle(() => new Set<Element>());
  try {
    for (let presses = 0; presses < maxPresses; presses += 1) {
      if (await target.evaluate((element) => element === document.activeElement)) {
        return;
      }
      const looped = await visited.evaluate((seen) => {
        const focused = document.activeElement;
        if (focused === null) {
          return false;
        }
        const again = seen.has(focused);
        seen.add(focused);
        return again;
      });
      if (looped && reversible) {
        backward = !backward;
        reversible = false;
      }
      await page.keyboard.press(backward ? "Shift+Tab" : "Tab");
    }
  } finally {
    await visited.dispose();
  }
  await expect(target, `reachable from the keyboard within ${maxPresses} presses`).toBeFocused();
}

/** Replaces the value of the focused input with `text`, typed key by key. */
export async function typeInFocused(page: Page, text: string): Promise<void> {
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("Backspace");
  if (text !== "") {
    await page.keyboard.type(text);
  }
}

/** Tabs to `field` (either direction, see `tabTo`) and types `text` into it. */
export async function typeInto(page: Page, field: Locator, text: string): Promise<void> {
  await tabTo(page, field);
  await typeInFocused(page, text);
}

/** Tabs to `control` (either direction, see `tabTo`) and presses Enter on it. */
export async function pressControl(page: Page, control: Locator): Promise<void> {
  await tabTo(page, control);
  await page.keyboard.press("Enter");
}

/**
 * Signs `role` on with the keyboard only: Tab to each F-COSGN00-001 input,
 * type, and Enter in the password field submits the form.
 */
export async function signInWithKeyboard(page: Page, role: Role): Promise<void> {
  const user = USERS[role];
  const form = signOnForm(page);
  await page.goto(form.route);
  await typeInto(page, form.userId, user.userId);
  await typeInto(page, form.password, user.password);
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(urlPath(user.landing));
}

/**
 * Signs `role` off through the user menu: the PF3 sign-off row of the
 * signed-on menu (F-COADM01-007 for an administrator, F-COMEN01-012 for a
 * user) names the menu item and the route it lands on, where the
 * CCDA-MSG-THANK-YOU text is toasted.
 */
export async function signOut(page: Page, role: Role): Promise<void> {
  const control = mapControl(role === "admin" ? "F-COADM01-007" : "F-COMEN01-012");
  await userMenuTrigger(page, role).click();
  await page.getByRole("menuitem", { name: control.name, exact: true }).click();
  await expect(page).toHaveURL(urlPath(control.target ?? "/login"));
  await expect(notifications(page).getByText(shown(MESSAGES.CCDA_MSG_THANK_YOU))).toBeVisible();
}

/** Asserts that `page` rendered a screen: its single h1 inside the page content. */
export async function expectScreen(page: Page): Promise<void> {
  await expect(content(page).getByRole("heading", { level: 1 })).toHaveCount(1);
  await expect(content(page).getByRole("heading", { level: 1 })).toBeVisible();
}
