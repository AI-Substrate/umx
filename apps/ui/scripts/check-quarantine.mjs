// Quarantine check (plan 159, D72h, security S2/S3). Fails `just ui-check` when:
// - pnpm-workspace.yaml lacks a top-level `minimumReleaseAge` of at least 10080 minutes (7 days);
// - a minimumReleaseAgeExclude entry has no reason comment on its line (exclusions: named security fixes only);
// - package.json's packageManager is not pnpm 10.16 or later (older pnpm ignores minimumReleaseAge);
// - the running pnpm does not read the setting: argv[2] is `pnpm config get minimumReleaseAge` from the caller, and
//   must be the same number.
// Usage: node scripts/check-quarantine.mjs "$(pnpm config get minimumReleaseAge)". Exit 1 on any finding.
import { readFileSync } from "node:fs";

const MIN_AGE = 10080;
const root = new URL("../", import.meta.url);
const read = (name) => readFileSync(new URL(name, root), "utf8");
const bad = [];

let workspace = "";
try {
  workspace = read("pnpm-workspace.yaml");
} catch {
  bad.push("pnpm-workspace.yaml is missing");
}
const lines = workspace.split("\n");
const ages = lines.map((l) => /^minimumReleaseAge:\s*(\S+)\s*(#.*)?$/.exec(l)).filter(Boolean);
const age = ages.length === 1 && /^\d+$/.test(ages[0][1]) ? Number(ages[0][1]) : null;
if (ages.length !== 1) bad.push(`pnpm-workspace.yaml: expected one top-level minimumReleaseAge, found ${ages.length}`);
else if (age === null) bad.push(`pnpm-workspace.yaml: minimumReleaseAge ${ages[0][1]} is not a whole number of minutes`);
else if (age < MIN_AGE) bad.push(`pnpm-workspace.yaml: minimumReleaseAge ${age} is below ${MIN_AGE}`);

const at = lines.findIndex((l) => /^minimumReleaseAgeExclude:/.test(l));
if (at >= 0) {
  if (!/^minimumReleaseAgeExclude:\s*(#.*)?$/.test(lines[at])) {
    bad.push("pnpm-workspace.yaml: minimumReleaseAgeExclude must be a block list, one entry per line with a reason comment");
  }
  for (let i = at + 1; i < lines.length && (lines[i] === "" || /^\s/.test(lines[i])); i += 1) {
    const item = /^\s+-\s+(.+?)\s*$/.exec(lines[i]);
    if (item && !/\S\s+#\s*\S/.test(item[1])) bad.push(`pnpm-workspace.yaml: exclude ${item[1]} has no reason comment`);
  }
}

const pm = JSON.parse(read("package.json")).packageManager ?? "";
const v = /^pnpm@(\d+)\.(\d+)\.(\d+)(\+\S+)?$/.exec(pm);
if (!v || Number(v[1]) < 10 || (Number(v[1]) === 10 && Number(v[2]) < 16)) {
  bad.push(`package.json: packageManager ${JSON.stringify(pm)} is not pnpm 10.16 or later`);
}

const effective = (process.argv[2] ?? "").trim();
const named = v ? `pnpm ${v[1]}.${v[2]}.${v[3]}${v[4] ? " (with hash)" : ""}` : pm || "missing";
if (age !== null && effective !== String(age)) {
  bad.push(`the running pnpm reads minimumReleaseAge as ${JSON.stringify(effective)}, not ${age}: run ${named}`);
}

console.log(`quarantine: minimumReleaseAge ${age ?? "missing"} (at least ${MIN_AGE}), packageManager ${named}, pnpm reads ${effective || "nothing"}`);
for (const b of bad) console.log(`FAIL ${b}`);
if (bad.length > 0) {
  console.log(`check-quarantine: FAILED (${bad.length} findings)`);
  process.exit(1);
}
console.log("check-quarantine: OK");
