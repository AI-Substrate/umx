// Lock age check (plan 159, security option b). pnpm 10.34.9 applies minimumReleaseAge only when it resolves; a frozen
// install of an existing lock skips it (headless install, pnpm issue 10438). So this checks the lock itself: every
// package entry that is new or changed against a base lock (all entries without one) must have been published at least
// minimumReleaseAge minutes ago (apps/ui/pnpm-workspace.yaml), by the `time` field of the registry packument. The
// registry is npm_config_registry when set (a user-level proxy), else https://registry.npmjs.org/; it is never printed.
// Fails closed: an unreachable registry, a missing time[version] or an unparseable date fails, never skips. The only
// exception is an exact name@version listed in minimumReleaseAgeExclude with a reason comment on its line.
// Usage: node scripts/check-lock-ages.mjs [--base <old lockfile>] [lockfile]. Exit 1 on any finding.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";

const args = process.argv.slice(2);
const baseAt = args.indexOf("--base");
const basePath = baseAt >= 0 ? args.splice(baseAt, 2)[1] : null;
const lockPath = args[0] ?? fileURLToPath(new URL("../pnpm-lock.yaml", import.meta.url));
const registry = (process.env.npm_config_registry || "https://registry.npmjs.org/").replace(/\/+$/, "");
const bad = [];

/** Package entries of a v9 lockfile: key -> the entry's lines, so any change to an entry shows. */
function entries(text) {
  const out = new Map();
  let inPackages = false;
  let key = null;
  for (const line of text.split("\n")) {
    if (line && !line.startsWith(" ")) {
      inPackages = line === "packages:";
      key = null;
    } else if (inPackages && /^ {2}\S.*:$/.test(line)) {
      key = line.trim().replace(/:$/, "").replace(/^'(.*)'$/, "$1");
      out.set(key, "");
    } else if (inPackages && key !== null && line) {
      out.set(key, `${out.get(key)}${line}\n`);
    }
  }
  return out;
}

const workspace = readFileSync(new URL("../pnpm-workspace.yaml", import.meta.url), "utf8").split("\n");
const ageLine = workspace.map((l) => /^minimumReleaseAge:\s*(\d+)\s*(#.*)?$/.exec(l)).find(Boolean);
const minutes = ageLine ? Number(ageLine[1]) : null;
if (minutes === null) bad.push("pnpm-workspace.yaml has no minimumReleaseAge");
const excluded = new Set();
const at = workspace.findIndex((l) => /^minimumReleaseAgeExclude:/.test(l));
for (let i = at + 1; at >= 0 && i < workspace.length && (workspace[i] === "" || /^\s/.test(workspace[i])); i += 1) {
  const item = /^\s+-\s+'?"?([^\s'"#]+)'?"?\s+#\s*\S/.exec(workspace[i]);
  if (item) excluded.add(item[1]);
}

const lock = entries(readFileSync(lockPath, "utf8"));
const base = basePath ? entries(readFileSync(basePath, "utf8")) : new Map();
const todo = [...lock].filter(([key, body]) => base.get(key) !== body).map(([key]) => key);

const byName = new Map();
for (const key of todo) {
  const m = /^((?:@[^/@]+\/)?[^/@]+)@(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]*)?)$/.exec(key);
  if (!m) {
    bad.push(`${key}: cannot read a name and an exact version from the key`);
    continue;
  }
  if (!byName.has(m[1])) byName.set(m[1], []);
  byName.get(m[1]).push(m[2]);
}

async function packumentTime(name) {
  const url = `${registry}/${name.replace("/", "%2F")}`;
  let last = "";
  for (let attempt = 0; attempt < 4; attempt += 1) {
    if (attempt > 0) await sleep(1000 * 2 ** attempt);
    try {
      const r = await fetch(url, { headers: { accept: "application/json" } });
      if (r.ok) return (await r.json()).time ?? {};
      last = `HTTP ${r.status}`;
      if (r.status < 500) break;
    } catch (e) {
      last = e?.cause?.code ?? e?.name ?? "fetch failed";
    }
  }
  throw new Error(`registry unreachable for ${name} (${last})`);
}

const now = Date.now();
let checked = 0;
let excepted = 0;
const names = [...byName.keys()];
async function worker() {
  for (let name = names.shift(); name !== undefined; name = names.shift()) {
    let time;
    try {
      time = await packumentTime(name);
    } catch (e) {
      for (const v of byName.get(name)) bad.push(`${name}@${v}: ${e.message}`);
      continue;
    }
    for (const v of byName.get(name)) {
      checked += 1;
      const published = Date.parse(time[v] ?? "");
      if (time[v] === undefined) bad.push(`${name}@${v}: the registry gives no publish time`);
      else if (Number.isNaN(published)) bad.push(`${name}@${v}: publish time ${JSON.stringify(time[v])} is not a date`);
      else if (minutes !== null && now - published < minutes * 60_000) {
        if (excluded.has(`${name}@${v}`)) excepted += 1;
        else bad.push(`${name}@${v}: published ${time[v]}, younger than ${minutes} minutes`);
      }
    }
  }
}
await Promise.all(Array.from({ length: 8 }, worker));

console.log(
  `lock ages: ${lock.size} entries, ${todo.length} new or changed against ${basePath ? "the base lock" : "nothing (full lock)"}; ` +
    `${checked} checked against the registry's publish times; minimumReleaseAge ${minutes ?? "missing"}; excepted ${excepted}`,
);
for (const b of bad.slice(0, 30)) console.log(`FAIL ${b}`);
if (bad.length > 0) {
  console.log(`check-lock-ages: FAILED (${bad.length} findings)`);
  process.exit(1);
}
console.log("check-lock-ages: OK");
