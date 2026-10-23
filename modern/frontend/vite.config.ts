import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { createLogger, type HttpProxy, type Logger } from "vite";
import { defineConfig } from "vitest/config";

/**
 * Vite dev server, build and Vitest configuration.
 *
 * Configuration comes from the environment only: export `modern/.env` first
 * (`set -a; . ./.env; set +a`); nothing here loads it. No variable is exposed to the client.
 */

/** Address the API binds; an unset or empty value falls back to the API's own default. */
const apiBind = process.env.CARDDEMO_API_BIND || "127.0.0.1:8080";

/** The origin the dev proxy forwards `/api` requests to. */
const apiTarget = `http://${apiBind}`;

/**
 * The body of the dev proxy's 502 while the API cannot be reached, shaped as the API's
 * `ScreenError` so that the screen shows it on its message row. It names the variable only.
 */
const PROXY_ERROR_BODY = JSON.stringify({
  message: "The dev proxy cannot reach the API at CARDDEMO_API_BIND",
});

/**
 * Answers a request the dev proxy could not forward, because nothing listens at
 * CARDDEMO_API_BIND or the connection to the API failed.
 *
 * Vite calls `configure` before it adds its own error listener, so this one runs first. It logs
 * one line naming the variable, the target and the error code, never a header, a body or a
 * token, and, while the response is unsent, answers 502 with the JSON body above. Vite's own
 * listener then finds the response ended and, instead of answering an empty 500, only logs its
 * line naming the request URL. That line goes through `redactingLogger`, so it prints the
 * request path without the query string.
 */
function answerUnreachableApi(proxy: HttpProxy.Server): void {
  proxy.on("error", (error, _request, response) => {
    const code = "code" in error && typeof error.code === "string" ? error.code : "no error code";
    console.error(`dev proxy: CARDDEMO_API_BIND target ${apiTarget} unreachable (${code})`);
    // A WebSocket upgrade hands over its socket rather than a response, which takes no body.
    if (
      !("req" in response) ||
      response.headersSent ||
      response.writableEnded ||
      response.destroyed
    ) {
      return;
    }
    response.writeHead(502, {
      "Content-Type": "application/json",
      "Content-Length": Buffer.byteLength(PROXY_ERROR_BODY),
    });
    response.end(PROXY_ERROR_BODY);
  });
}

/**
 * Removes the query string and fragment from every `/api` URL in a dev server log message,
 * keeping the path.
 *
 * Query values carry card numbers (`card_num` on the card list, view and edit calls), and
 * the Cardholder Data Redaction rule keeps them out of every log. No path segment carries a
 * protected value. A URL ends at whitespace or at an ANSI escape, so the colour reset that
 * follows it on a terminal survives. Vite also passes some errors as `Error` objects despite
 * its `string` type; they name no request URL and pass unchanged.
 */
function withoutApiQuery(message: string): string {
  return typeof message === "string"
    ? message.replace(/(\/api(?:\/[^\s?#\u001b]*)?)[?#][^\s\u001b]*/g, "$1")
    : message;
}

/**
 * Vite's default logger with every message passed through `withoutApiQuery` before it prints,
 * so that no line the dev server logs, its proxy error line included, carries a query value.
 * The methods are replaced on Vite's own logger object, so its `hasWarned` flag still tracks
 * what was printed.
 */
function redactingLogger(): Logger {
  const logger = createLogger();
  const info = logger.info.bind(logger);
  const warn = logger.warn.bind(logger);
  const warnOnce = logger.warnOnce.bind(logger);
  const error = logger.error.bind(logger);
  logger.info = (message, options) => info(withoutApiQuery(message), options);
  logger.warn = (message, options) => warn(withoutApiQuery(message), options);
  logger.warnOnce = (message, options) => warnOnce(withoutApiQuery(message), options);
  logger.error = (message, options) => error(withoutApiQuery(message), options);
  return logger;
}

const DEFAULT_WEB_PORT = 5173;

/** A TCP port given as decimal digits in 1..65535, or the default for anything else. */
function webPort(value: string | undefined): number {
  if (value === undefined || !/^[0-9]{1,5}$/.test(value)) {
    return DEFAULT_WEB_PORT;
  }
  const port = Number.parseInt(value, 10);
  return port >= 1 && port <= 65535 ? port : DEFAULT_WEB_PORT;
}

export default defineConfig({
  plugins: [react(), tailwindcss()],
  customLogger: redactingLogger(),
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    host: "127.0.0.1",
    // From CARDDEMO_WEB_PORT so concurrent local stacks keep their own port; defaults to 5173.
    port: webPort(process.env.CARDDEMO_WEB_PORT),
    strictPort: true,
    proxy: {
      // Matches the /api path segment only, so /api-docs and other /api* paths stay with Vite.
      "^/api(?:[/?]|$)": {
        target: apiTarget,
        configure: answerUnreachableApi,
      },
    },
  },
  test: {
    globals: true,
    // Keeps it.each `$title` names whole (default cut: 40 chars), so every test name is distinct.
    chaiConfig: { truncateThreshold: 0 },
    projects: [
      {
        extends: true,
        test: {
          name: "unit",
          environment: "node",
          include: ["src/**/*.test.ts"],
        },
      },
      {
        extends: true,
        test: {
          name: "dom",
          environment: "jsdom",
          include: ["src/**/*.test.tsx"],
          // A screen render test drives a whole map through several requests in jsdom. On a
          // shared, loaded host they run 1-5 s each and the heaviest (COACTUP) 3-16 s, past
          // Vitest's 5 s default, so the budget is widened rather than lost to timeouts; a hung
          // test still fails.
          testTimeout: 30_000,
        },
      },
    ],
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/components/ui/**",
        "src/api/schema.d.ts",
        "src/**/*.test.ts",
        "src/**/*.test.tsx",
      ],
      thresholds: {
        lines: 70,
      },
      reporter: ["text", "json-summary"],
      reportsDirectory: "coverage",
    },
  },
});
