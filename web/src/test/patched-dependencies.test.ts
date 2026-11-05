// Regression checks for the pnpm patches under web/patches, which
// web/package.json lists in `pnpm.patchedDependencies`. Each backports an upstream
// security fix into a package that the pinned stack reaches only through a pinned
// package, at its locked version:
// - minimatch 10.2.6 (shadcn → ts-morph → @ts-morph/common), GHSA-g3hv-75wg-m249:
//   bounded `**/..` preprocessing at optimizationLevel 2 (minimatch 11.0.1).
// - deepmerge 4.3.1 (shadcn and @shadcn/registry), GHSA-gcqx-w48m-w2h4: prototype
//   poisoning of merged objects, which no release fixes.
// - @vitest/mocker 3.2.7 (vitest), GHSA-82fw-gwwq-j7x9 and GHSA-qxg6-89mg-rqv8:
//   redirect mocks outside the dev server's file-serving allowlist, and code
//   injection through manual mocks (@vitest/mocker 4.1.11 and 4.1.12).
// - tinypool 1.1.1 (vitest), GHSA-5gmw-xhrv-c9v3 and GHSA-85c8-ppgw-ccpr: worker and
//   run() options read through a polluted Object.prototype (tinypool 2.1.1, 2.1.2).
//
// Each package is resolved the way its dependent resolves it, from the real path of
// the dependent's package.json, so the checks run the installed, patched files.
//
// The file runs in the configured jsdom environment, because the shared setup file
// (src/test/setup.ts) needs a DOM. Every check that needs a plain Node runtime (a
// Vite config, a worker pool, a generated module to evaluate) or pollutes
// Object.prototype runs in a child `node` process, so the Vitest worker is never
// polluted.
import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterAll, describe, expect, it } from "vitest";

type Require = ReturnType<typeof createRequire>;

// jsdom replaces the global URL, which fileURLToPath does not accept, so the path
// is derived from the module's URL string.
const WEB_ROOT = join(dirname(fileURLToPath(import.meta.url)), "../..");

/** A require bound to the real location of `packageJson`, as that package resolves. */
function requireFrom(packageJson: string): Require {
  return createRequire(realpathSync(packageJson));
}

/** The real directory of package `name` as `from` resolves it. */
function packageDir(from: Require, name: string): string {
  return dirname(realpathSync(from.resolve(`${name}/package.json`)));
}

const SHADCN_PACKAGE_JSON = realpathSync(
  join(WEB_ROOT, "node_modules/shadcn/package.json"),
);
const shadcnRequire = requireFrom(SHADCN_PACKAGE_JSON);
const vitestRequire = requireFrom(
  join(WEB_ROOT, "node_modules/vitest/package.json"),
);
const tsMorphCommonRequire = requireFrom(
  requireFrom(shadcnRequire.resolve("ts-morph/package.json")).resolve(
    "@ts-morph/common/package.json",
  ),
);
// @shadcn/registry exports no package.json, so it is found where Node's lookup
// finds it: beside shadcn in shadcn's own node_modules.
const registryRequire = requireFrom(
  join(dirname(SHADCN_PACKAGE_JSON), "../@shadcn/registry/package.json"),
);

const MINIMATCH_DIR = packageDir(tsMorphCommonRequire, "minimatch");
const DEEPMERGE_DIR = packageDir(shadcnRequire, "deepmerge");
const MOCKER_DIR = packageDir(vitestRequire, "@vitest/mocker");
const TINYPOOL_DIR = packageDir(vitestRequire, "tinypool");

// Scratch files for the child processes, removed once the file is done.
const WORK_DIR = realpathSync(mkdtempSync(join(tmpdir(), "patched-deps-")));
afterAll(() => {
  rmSync(WORK_DIR, { recursive: true, force: true });
});

/**
 * Runs `source` as an ES module in a child `node` process with `args` and returns
 * what it prints on stdout, parsed as JSON. A failing child throws with its stderr.
 */
function runNodeModule(name: string, source: string, args: string[]): unknown {
  const script = join(WORK_DIR, name);
  writeFileSync(script, source);
  const stdout = execFileSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    timeout: 60_000,
  });
  return JSON.parse(stdout);
}

describe("the patches are installed", () => {
  it("web/package.json patches exactly the four packages, each with its patch file", () => {
    const manifest = JSON.parse(
      readFileSync(join(WEB_ROOT, "package.json"), "utf8"),
    ) as { pnpm?: { patchedDependencies?: Record<string, string> } };
    const patched = manifest.pnpm?.patchedDependencies ?? {};
    expect(patched).toEqual({
      "minimatch@10.2.6": "patches/minimatch@10.2.6.patch",
      "deepmerge@4.3.1": "patches/deepmerge@4.3.1.patch",
      "@vitest/mocker@3.2.7": "patches/@vitest__mocker@3.2.7.patch",
      "tinypool@1.1.1": "patches/tinypool@1.1.1.patch",
    });
    for (const patch of Object.values(patched)) {
      expect(readFileSync(join(WEB_ROOT, patch), "utf8")).toMatch(
        /^diff --git /,
      );
    }
  });

  it.each([
    {
      dir: MINIMATCH_DIR,
      version: "10.2.6",
      file: "dist/commonjs/index.js",
      fix: "globParts.length >= braceExpandMax",
    },
    {
      dir: MINIMATCH_DIR,
      version: "10.2.6",
      file: "dist/esm/index.js",
      fix: "globParts.length >= braceExpandMax",
    },
    {
      dir: DEEPMERGE_DIR,
      version: "4.3.1",
      file: "dist/cjs.js",
      fix: "keyIsUnsafe(key) || propertyIsUnsafe(target, key)",
    },
    {
      dir: DEEPMERGE_DIR,
      version: "4.3.1",
      file: "dist/umd.js",
      fix: "keyIsUnsafe(key) || propertyIsUnsafe(target, key)",
    },
    {
      dir: DEEPMERGE_DIR,
      version: "4.3.1",
      file: "index.js",
      fix: "keyIsUnsafe(key) || propertyIsUnsafe(target, key)",
    },
    {
      dir: MOCKER_DIR,
      version: "3.2.7",
      file: "dist/node.js",
      fix: "isFileLoadingAllowed(server.config, redirect)",
    },
    {
      dir: MOCKER_DIR,
      version: "3.2.7",
      file: "dist/chunk-utils.js",
      fix: "getFactoryModule(${JSON.stringify(moduleUrl)})",
    },
    {
      dir: TINYPOOL_DIR,
      version: "1.1.1",
      file: "dist/index.js",
      fix: "= withNullPrototype(options);",
    },
  ])(
    "$file of the installed $version package holds the backport",
    ({ dir, version, file, fix }) => {
      expect(dir).toContain("_patch_hash=");
      const manifest = JSON.parse(
        readFileSync(join(dir, "package.json"), "utf8"),
      ) as { version: string };
      expect(manifest.version).toBe(version);
      expect(readFileSync(join(dir, file), "utf8")).toContain(fix);
    },
  );

  it("@shadcn/registry resolves the same patched deepmerge as shadcn", () => {
    expect(packageDir(registryRequire, "deepmerge")).toBe(DEEPMERGE_DIR);
  });
});

interface MinimatchInstance {
  readonly set: readonly unknown[];
  match(path: string): boolean;
}

interface MinimatchModule {
  readonly Minimatch: new (
    pattern: string,
    options: { optimizationLevel: number; braceExpandMax?: number },
  ) => MinimatchInstance;
}

// The upstream preprocess-limits cases of the fix, at optimizationLevel 2.
describe("minimatch 10.2.6 bounds **/.. preprocessing (GHSA-g3hv-75wg-m249)", () => {
  const optimizationLevel = 2;
  const thirtySplits = Array(30).fill("**/../a/b").join("/");
  const builds: readonly [string, () => Promise<MinimatchModule>][] = [
    [
      "commonjs, which ts-morph loads",
      async () => tsMorphCommonRequire("minimatch") as MinimatchModule,
    ],
    [
      "esm",
      async () =>
        (await import(
          pathToFileURL(join(MINIMATCH_DIR, "dist/esm/index.js")).href
        )) as MinimatchModule,
    ],
  ];

  it.each(builds)(
    "%s: **/.. splits stop at braceExpandMax",
    async (_build, load) => {
      const { Minimatch } = await load();
      const mm = new Minimatch(thirtySplits, {
        optimizationLevel,
        braceExpandMax: 64,
      });
      expect(mm.set).toHaveLength(64);
    },
  );

  it.each(builds)(
    "%s: **/.. splits below the max still match",
    async (_build, load) => {
      const { Minimatch } = await load();
      const mm = new Minimatch("**/../a/b/**/../c/d", { optimizationLevel });
      expect(mm.set).toHaveLength(4);
      expect(mm.match("../a/b/../c/d")).toBe(true);
      expect(mm.match("x/a/b/y/c/d")).toBe(true);
    },
  );

  it.each(builds)(
    "%s: sets above 1000 patterns are not deduped and match the same paths",
    async (_build, load) => {
      const { Minimatch } = await load();
      expect(
        new Minimatch("{x,*}/{0..9}", { optimizationLevel }).set,
      ).toHaveLength(10);
      const large = new Minimatch("{x,*}/{0..999}", { optimizationLevel });
      expect(large.set).toHaveLength(2000);
      expect(large.match("x/5")).toBe(true);
      expect(large.match("y/999")).toBe(true);
      expect(large.match(".y/5")).toBe(false);
    },
  );

  // Unbounded, 30 splits would double the set 30 times and then compare every pair;
  // the default braceExpandMax of 100 000 ends it within the test timeout.
  it(
    "the default braceExpandMax bounds 30 splits to 100 000 sets",
    async () => {
      const { Minimatch } = await builds[0][1]();
      expect(
        new Minimatch(thirtySplits, { optimizationLevel }).set,
      ).toHaveLength(100_000);
    },
    120_000,
  );
});

interface DeepMerge {
  (target: unknown, source: unknown): object;
  all(objects: readonly unknown[]): object;
}

// Fresh hostile payloads, as JSON.parse makes them: `__proto__` is an own key.
const protoPayload = (): unknown =>
  JSON.parse('{"__proto__":{"polluted":true},"kept":1}');
const constructorPayload = (): unknown =>
  JSON.parse(
    '{"constructor":{"prototype":{"polluted":true}},"prototype":{"polluted":true},"kept":1}',
  );

/** `object[key]`, which must itself be an object. */
function member(object: object, key: string): object {
  const value: unknown = Reflect.get(object, key);
  expect(value).toBeTypeOf("object");
  return value as object;
}

describe("deepmerge 4.3.1 never poisons a prototype (GHSA-gcqx-w48m-w2h4)", () => {
  const builds: readonly [string, DeepMerge][] = [
    [
      "dist/cjs.js, which shadcn imports",
      shadcnRequire("deepmerge") as DeepMerge,
    ],
    ["dist/umd.js", shadcnRequire("deepmerge/dist/umd.js") as DeepMerge],
  ];
  // Each case returns the merged objects whose prototypes it checks; the last one
  // holds the payload's ordinary `kept` key.
  const cases: readonly [string, (deepmerge: DeepMerge) => object[]][] = [
    ["__proto__ into {}", (dm) => [dm({}, protoPayload())]],
    [
      "__proto__ into a null-prototype target",
      (dm) => [dm(Object.create(null), protoPayload())],
    ],
    ["__proto__ owned by the target", (dm) => [dm(protoPayload(), {})]],
    ["__proto__ on both sides", (dm) => [dm(protoPayload(), protoPayload())]],
    [
      "nested __proto__",
      (dm) => {
        const merged = dm({ nested: {} }, { nested: protoPayload() });
        return [merged, member(merged, "nested")];
      },
    ],
    [
      "nested __proto__ into a null-prototype value",
      (dm) => {
        const merged = dm(
          { nested: Object.create(null) },
          { nested: protoPayload() },
        );
        return [merged, member(merged, "nested")];
      },
    ],
    [
      "__proto__ through deepmerge.all",
      (dm) => [dm.all([protoPayload(), Object.create(null), protoPayload()])],
    ],
    ["constructor and prototype into {}", (dm) => [dm({}, constructorPayload())]],
    [
      "constructor and prototype into a null-prototype target",
      (dm) => [dm(Object.create(null), constructorPayload())],
    ],
    [
      "constructor and prototype owned by the target",
      (dm) => [dm(constructorPayload(), {})],
    ],
  ];
  const table: [string, string, DeepMerge, (deepmerge: DeepMerge) => object[]][] =
    builds.flatMap(([build, deepmerge]) =>
      cases.map(
        ([name, merge]): [
          string,
          string,
          DeepMerge,
          (deepmerge: DeepMerge) => object[],
        ] => [build, name, deepmerge, merge],
      ),
    );

  it.each(table)("%s: %s", (_build, _name, deepmerge, merge) => {
    const objects = merge(deepmerge);
    for (const merged of objects) {
      expect(Object.getPrototypeOf(merged)).toBe(Object.prototype);
      expect("polluted" in merged).toBe(false);
      for (const key of ["__proto__", "constructor", "prototype"]) {
        expect(Object.hasOwn(merged, key)).toBe(false);
      }
    }
    expect(Reflect.get(objects[objects.length - 1], "kept")).toBe(1);
    expect(Object.hasOwn(Object.prototype, "polluted")).toBe(false);
    expect(Reflect.get({}, "polluted")).toBeUndefined();
  });

  it.each(builds)("%s: an ordinary merge is unchanged", (_build, deepmerge) => {
    const symbol = Symbol("kept");
    const merged = deepmerge(
      { list: [1], nested: { a: 1 }, same: "target", [symbol]: "symbol" },
      { list: [2], nested: { b: 2 }, same: "source", extra: "x" },
    );
    expect(merged).toEqual({
      list: [1, 2],
      nested: { a: 1, b: 2 },
      same: "source",
      extra: "x",
      [symbol]: "symbol",
    });
    expect(Object.keys(merged)).toEqual(["list", "nested", "same", "extra"]);
    expect(Object.getOwnPropertyDescriptor(merged, "extra")).toEqual({
      value: "x",
      writable: true,
      enumerable: true,
      configurable: true,
    });
  });
});

// The child process for @vitest/mocker. argv: the patched dist/node.js and Vite's
// node entry, as file URLs, and the scratch directory. It prints:
// - manual: what a module generated by createManualModuleSource does when imported,
//   given a hostile URL and hostile export names;
// - redirects: for each redirect target registered through the interceptor's
//   `vitest:interceptor:register` event, the events the plugin sent and what its
//   `load` hook then serves for the mocked id.
// String.raw keeps the backslash of the Windows-style traversal literal.
const MOCKER_CHILD = String.raw`
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const [mockerNodeUrl, viteUrl, workDir] = process.argv.slice(2);
const { createManualModuleSource, interceptorPlugin } = await import(mockerNodeUrl);
const { resolveConfig } = await import(viteUrl);

const url = 'x"); globalThis.INJECTED_BY_URL = 1; void("';
const names = [
  "plain",
  "b = 1; globalThis.INJECTED_BY_NAME = 1; export const c",
  'd"]; globalThis.INJECTED_BY_KEY = 1; //',
  "default",
];
const moduleFile = join(workDir, "manual-mock.mjs");
writeFileSync(moduleFile, createManualModuleSource(url, names));
const factory = Object.fromEntries(names.map((name, index) => [name, index]));
let seenUrl = null;
globalThis.__vitest_mocker__ = {
  getFactoryModule(requested) {
    seenUrl = requested;
    return factory;
  },
};
const namespace = await import(pathToFileURL(moduleFile).href);
const manual = {
  seenUrl,
  injected: ["INJECTED_BY_URL", "INJECTED_BY_NAME", "INJECTED_BY_KEY"].filter(
    (key) => key in globalThis,
  ),
  exports: Object.fromEntries(Object.entries(namespace)),
};

const root = join(workDir, "root");
mkdirSync(root);
writeFileSync(join(root, "inroot.js"), "export const marker = 'in-root-redirect-ok'\n");
writeFileSync(join(workDir, "secret.txt"), "outside-the-root\n");
const config = await resolveConfig(
  { root, configFile: false, logLevel: "silent", server: { fs: { allow: [root] } } },
  "serve",
);
const redirects = {};
for (const redirect of ["traversal:../secret.txt", "traversal:..\\secret.txt", "traversal:inroot.js"]) {
  const plugin = interceptorPlugin();
  const handlers = new Map();
  const sent = [];
  plugin.configureServer({
    config,
    ws: {
      on: (event, handler) => handlers.set(event, handler),
      send: (event) => sent.push(event),
    },
  });
  handlers.get("vitest:interceptor:register")({
    type: "redirect",
    raw: "",
    id: "/mock",
    url: "/mock",
    redirect,
  });
  redirects[redirect] = { sent, served: (await plugin.load.handler("/mock")) ?? null };
}
process.stdout.write(JSON.stringify({ manual, redirects }));
`;

describe("@vitest/mocker 3.2.7 interceptor (GHSA-82fw-gwwq-j7x9, GHSA-qxg6-89mg-rqv8)", () => {
  const viteEntry = realpathSync(
    join(WEB_ROOT, "node_modules/vite/dist/node/index.js"),
  );

  it("generates inert manual mocks and refuses redirects outside the allowlist", () => {
    const result = runNodeModule("mocker-child.mjs", MOCKER_CHILD, [
      pathToFileURL(join(MOCKER_DIR, "dist/node.js")).href,
      pathToFileURL(viteEntry).href,
      WORK_DIR,
    ]);
    const registered = ["vitest:interceptor:register:result"];
    expect(result).toEqual({
      manual: {
        seenUrl: 'x"); globalThis.INJECTED_BY_URL = 1; void("',
        injected: [],
        exports: {
          plain: 0,
          "b = 1; globalThis.INJECTED_BY_NAME = 1; export const c": 1,
          'd"]; globalThis.INJECTED_BY_KEY = 1; //': 2,
          default: 3,
        },
      },
      redirects: {
        "traversal:../secret.txt": { sent: registered, served: null },
        "traversal:..\\secret.txt": { sent: registered, served: null },
        "traversal:inroot.js": {
          sent: registered,
          served: "export const marker = 'in-root-redirect-ok'\n",
        },
      },
    });
  }, 90_000);
});

// The child process for tinypool. argv: the patched dist/index.js as a file URL and
// the scratch directory. It pollutes Object.prototype with every option the
// advisories name, plus an inherited environment key, runs one pool per runtime,
// restores the prototype and prints per runtime what the tasks returned, whether
// the pool's options have a prototype and the runtime they keep. A polluted
// execArgv, env or filename would load fails-when-loaded.mjs, which throws.
const TINYPOOL_CHILD = String.raw`
import { writeFileSync } from "node:fs";
import { join } from "node:path";

const [tinypoolUrl, workDir] = process.argv.slice(2);
const fails = join(workDir, "fails-when-loaded.mjs");
writeFileSync(fails, 'throw new Error("a polluted option loaded fails-when-loaded.mjs");\n');
const task = join(workDir, "task.mjs");
writeFileSync(
  task,
  "export default (n) => ({ doubled: n * 2, inherited: process.env.TINYPOOL_POLLUTED ?? null });\n",
);
const { Tinypool } = await import(tinypoolUrl);

const polluted = {
  execArgv: ["--import", fails],
  env: { NODE_OPTIONS: "--import " + fails },
  filename: fails,
  runtime: "child_process",
  workerData: "polluted",
  TINYPOOL_POLLUTED: "inherited",
};
const results = {};
Object.assign(Object.prototype, polluted);
try {
  for (const runtime of ["worker_threads", "child_process"]) {
    const options = { filename: task, runtime, minThreads: 1, maxThreads: 1 };
    if (runtime === "child_process") options.env = { ...process.env };
    const pool = new Tinypool(options);
    try {
      const defaults = await pool.run(21);
      const own = await pool.run(4, { signal: new AbortController().signal });
      await pool.recycleWorkers();
      const recycled = await pool.run(5);
      results[runtime] = {
        defaults,
        own,
        recycled,
        nullPrototype: Object.getPrototypeOf(pool.options) === null,
        runtime: pool.options.runtime,
      };
    } finally {
      await pool.destroy();
    }
  }
} finally {
  for (const key of Object.keys(polluted)) delete Object.prototype[key];
}
process.stdout.write(JSON.stringify(results));
`;

describe("tinypool 1.1.1 ignores a polluted Object.prototype (GHSA-5gmw-xhrv-c9v3, GHSA-85c8-ppgw-ccpr)", () => {
  it("runs every task with the pool's own options on both runtimes", () => {
    const result = runNodeModule("tinypool-child.mjs", TINYPOOL_CHILD, [
      pathToFileURL(join(TINYPOOL_DIR, "dist/index.js")).href,
      WORK_DIR,
    ]);
    const expected = (runtime: string) => ({
      defaults: { doubled: 42, inherited: null },
      own: { doubled: 8, inherited: null },
      recycled: { doubled: 10, inherited: null },
      nullPrototype: true,
      runtime,
    });
    expect(result).toEqual({
      worker_threads: expected("worker_threads"),
      child_process: expected("child_process"),
    });
    expect(Object.hasOwn(Object.prototype, "execArgv")).toBe(false);
  }, 90_000);
});
