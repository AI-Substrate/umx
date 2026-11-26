import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const cwd = new URL("..", import.meta.url).pathname;
const BREACH = `import { execFile } from "node:child_process";
import { connect } from "node:net";
import { readFileSync } from "node:fs";
export const breach = [execFile, connect, readFileSync, import("node:fs/promises"), require("child_process")];
`;

async function lint(filePath: string, code = BREACH) {
  const eslint = new ESLint({ cwd });
  const [result] = await eslint.lintText(code, { filePath });
  return result.messages.filter((m) => m.ruleId === "no-restricted-imports" || m.ruleId === "no-restricted-syntax");
}

describe("the import boundary (architect R2)", () => {
  it("fails the planted fixture", async () => {
    const eslint = new ESLint({ cwd, ignore: false });
    const [result] = await eslint.lintFiles(["test/fixtures/lint-plant/plant.ts"]);
    const lines = result.messages.filter((m) => m.ruleId?.startsWith("no-restricted")).map((m) => m.line);
    expect(lines).toEqual([3, 4, 5, 7]);
  });

  it("fails every breach anywhere outside lib/umx", async () => {
    for (const file of ["app/api/graphs/route.ts", "components/x.tsx", "lib/snapshot.ts", "bin/serve.mjs"]) {
      expect((await lint(file)).map((m) => m.line)).toEqual([1, 2, 3, 4, 4]);
    }
  });

  it("lets lib/umx/socket.ts import lstat from node:fs/promises, and nothing else from fs", async () => {
    const ok = `import { connect } from "node:net";\nimport { lstat } from "node:fs/promises";\nexport const x = [connect, lstat];\n`;
    expect(await lint("lib/umx/socket.ts", ok)).toEqual([]);
    for (const code of [
      `import { readFile } from "node:fs/promises";\nexport const x = readFile;\n`,
      `import { lstat, readFile } from "node:fs/promises";\nexport const x = [lstat, readFile];\n`,
      `import * as fs from "node:fs/promises";\nexport const x = fs;\n`,
      `import { lstatSync } from "node:fs";\nexport const x = lstatSync;\n`,
      `export const x = import("node:fs/promises");\n`,
    ]) {
      expect((await lint("lib/umx/socket.ts", code)).length).toBeGreaterThan(0);
    }
    // lstat anywhere else, lib/umx/index.ts included, is still refused.
    const lstatOnly = `import { lstat } from "node:fs/promises";\nexport const x = lstat;\n`;
    for (const file of ["lib/umx/index.ts", "lib/events.ts", "app/api/events/route.ts", "components/x.tsx"]) {
      expect((await lint(file, lstatOnly)).length).toBeGreaterThan(0);
    }
  });

  it("lets lib/umx run umx but still never read files", async () => {
    const messages = await lint("lib/umx/index.ts");
    expect(messages.map((m) => m.line)).toEqual([3, 4]);
    expect(messages.every((m) => m.message.includes("never reads files"))).toBe(true);
  });

  it("lets the gate scripts read node:fs with readFileSync, and nothing else (plan 159)", async () => {
    const file = "scripts/check-x.mjs";
    for (const code of [
      `import { readFileSync } from "node:fs";\nexport const x = readFileSync;\n`,
      `import { readFileSync } from "fs";\nexport const x = readFileSync;\n`,
    ]) {
      expect(await lint(file, code)).toEqual([]);
    }
    for (const code of [
      `import { execFile } from "node:child_process";\nexport const x = execFile;\n`,
      `export const x = require("node:net");\n`,
      `import { connect } from "net";\nexport const x = connect;\n`,
      `export const x = import("child_process");\n`,
      `import { writeFileSync } from "node:fs";\nexport const x = writeFileSync;\n`,
      `import { readFileSync, rmSync } from "fs";\nexport const x = [readFileSync, rmSync];\n`,
      `import * as fs from "node:fs";\nexport const x = fs;\n`,
      `import fs from "node:fs";\nexport const x = fs;\n`,
      `import { readFile } from "node:fs/promises";\nexport const x = readFile;\n`,
      `import { writeFile } from "fs/promises";\nexport const x = writeFile;\n`,
      `export const x = require("node:fs");\n`,
      `export const x = import("node:fs/promises");\n`,
    ]) {
      expect((await lint(file, code)).length, code).toBeGreaterThan(0);
    }
  });

  it("limits the LAN password reader to its explicit read-only filesystem imports", async () => {
    const file = "bin/lan.mjs";
    expect(await lint(file, `import { openSync, fstatSync, readFileSync, closeSync, constants } from "node:fs";
export const x = [openSync, fstatSync, readFileSync, closeSync, constants];`)).toEqual([]);
    for (const code of [
      `import { writeFileSync } from "node:fs"; export const x = writeFileSync;`,
      `import * as fs from "node:fs"; export const x = fs;`,
      `export const x = import("node:fs");`,
      `import { connect } from "node:net"; export const x = connect;`,
      `import { execFile } from "node:child_process"; export const x = execFile;`,
    ]) {
      expect((await lint(file, code)).length).toBeGreaterThan(0);
    }
  });
});
