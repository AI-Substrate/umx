// The import boundary (architect R2, ws009): only lib/umx may import node:child_process or node:net, and no
// module imports node:fs outside build config, except that lib/umx/socket.ts may import lstat (file metadata, to check
// the daemon's run folder; architect, S6b) from node:fs/promises, and the gate scripts in scripts/ may read node:fs:
// readFileSync only (the lockfile and settings, plan 159). bin/lan.mjs may read its password file and read and
// replace its owner-only sessions file through the named calls below; test/lan.test.ts may make a private UMX_HOME
// for it. test/lint-plant.test.ts proves the rules fire.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

const PROCESS = ["child_process", "node:child_process"];
const NET = ["net", "node:net"];
const FS = ["fs", "node:fs", "fs/promises", "node:fs/promises"];
const BOUNDARY = "Only apps/ui/lib/umx may run processes or open sockets (architect R2).";
const NO_FS = "The UI never reads files; umx is its only source (architect R2).";
const READ_FS = "Gate scripts may read node:fs (readFileSync) only; they never write (architect, plan 159).";
const SCRIPT_FS = ["readFileSync"];

const restricted = (names, message) => names.map((name) => ({ name, message }));
const pattern = (names) => `/^(${names.map((n) => n.replace("/", "\\/")).join("|")})$/`;

function boundary(names, message, paths = restricted(names, message)) {
  return {
    "no-restricted-imports": ["error", { paths }],
    "no-restricted-syntax": [
      "error",
      { selector: `ImportExpression[source.value=${pattern(names)}]`, message },
      { selector: `CallExpression[callee.name='require'][arguments.0.value=${pattern(names)}]`, message },
    ],
  };
}

export default tseslint.config(
  { ignores: [".next/**", "node_modules/**", "test/fixtures/**", "next-env.d.ts"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  reactHooks.configs.flat["recommended-latest"],
  {
    languageOptions: { globals: { window: "readonly", document: "readonly", self: "readonly", console: "readonly", process: "readonly", URL: "readonly", Buffer: "readonly", performance: "readonly", crypto: "readonly", btoa: "readonly", fetch: "readonly", history: "readonly", getComputedStyle: "readonly", Worker: "readonly", MessageEvent: "readonly", KeyboardEvent: "readonly", HTMLElement: "readonly", HTMLInputElement: "readonly", HTMLTextAreaElement: "readonly", CanvasRenderingContext2D: "readonly", Headers: "readonly", Request: "readonly", Intl: "readonly" } },
    rules: { ...boundary([...PROCESS, ...NET, ...FS], `${BOUNDARY} ${NO_FS}`) },
  },
  {
    files: ["lib/umx/**"],
    rules: boundary(FS, NO_FS),
  },
  {
    files: ["lib/umx/socket.ts"],
    rules: boundary(FS, NO_FS, [
      ...restricted(FS.filter((n) => n !== "node:fs/promises"), NO_FS),
      { name: "node:fs/promises", allowImportNames: ["lstat"], message: `${NO_FS} socket.ts may import lstat only.` },
    ]),
  },
  {
    files: ["bin/lan.mjs"],
    rules: boundary([...PROCESS, ...NET, ...FS], `${BOUNDARY} The LAN gate may use its password and sessions files only.`, [
      ...restricted([...PROCESS, ...NET], BOUNDARY),
      {
        name: "node:fs",
        allowImportNames: ["closeSync", "constants", "fstatSync", "fsyncSync", "openSync", "readFileSync", "renameSync", "rmSync", "writeSync"],
        message: "Only the LAN gate's password and sessions files may use these.",
      },
      ...restricted(FS.filter((n) => n !== "node:fs"), NO_FS),
    ]),
  },
  {
    files: ["test/lan.test.ts"],
    rules: boundary([...PROCESS, ...NET, ...FS], `${BOUNDARY} ${NO_FS}`, [
      ...restricted([...PROCESS, ...NET, ...FS.filter((n) => n !== "node:fs")], `${BOUNDARY} ${NO_FS}`),
    ]),
  },
  {
    files: ["next.config.ts", "eslint.config.mjs", "vitest.config.ts", "postcss.config.mjs"],
    rules: { "no-restricted-imports": "off", "no-restricted-syntax": "off" },
  },
  {
    files: ["scripts/**"],
    rules: boundary([...PROCESS, ...NET, ...FS], `${BOUNDARY} ${READ_FS}`, [
      ...restricted([...PROCESS, ...NET], BOUNDARY),
      ...["fs", "node:fs"].map((name) => ({ name, allowImportNames: SCRIPT_FS, message: READ_FS })),
      ...restricted(["fs/promises", "node:fs/promises"], READ_FS),
    ]),
  },
);
