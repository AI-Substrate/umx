// Lock source check (plan 159, security S1). Fails when apps/ui/pnpm-lock.yaml has a URL on any host but
// registry.npmjs.org (a tarball, a git repo or any other resolution), an integrity that is not sha512, or a package
// entry with no integrity. Runs first in `just ui-check`; CI may call it too. Usage: node scripts/check-lock-sources.mjs
// [lockfile, default pnpm-lock.yaml beside package.json]. Exit 1 on any finding.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const ALLOWED_HOST = "registry.npmjs.org";
const path = process.argv[2] ?? fileURLToPath(new URL("../pnpm-lock.yaml", import.meta.url));
const lines = readFileSync(path, "utf8").split("\n");
const bad = [];

let section = null;
let entry = null;
let entries = 0;
const seen = new Set();
const close = () => {
  if (entry !== null && !seen.has(entry)) bad.push(`${entry}: no integrity`);
};
lines.forEach((line, i) => {
  for (const m of line.matchAll(/[a-z][a-z0-9+.-]*:\/\/([^/\s,}'"@]+@)?([^/\s,}'":]+)/gi)) {
    if (m[2].toLowerCase() !== ALLOWED_HOST) bad.push(`line ${i + 1}: URL host ${m[2]} is not ${ALLOWED_HOST}`);
  }
  for (const m of line.matchAll(/integrity: ([^,}\s]+)/g)) {
    if (!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(m[1])) bad.push(`line ${i + 1}: integrity ${m[1].slice(0, 12)}… is not sha512`);
  }
  for (const m of line.matchAll(/tarball: ([^,}\s]+)/g)) {
    if (!m[1].startsWith(`https://${ALLOWED_HOST}/`)) bad.push(`line ${i + 1}: tarball is not on https://${ALLOWED_HOST}/`);
  }
  if (line && !line.startsWith(" ")) {
    if (section === "packages") close();
    section = line.replace(/:$/, "");
    entry = null;
    return;
  }
  if (section !== "packages") return;
  if (/^ {2}\S.*:$/.test(line)) {
    close();
    entry = line.trim().replace(/:$/, "");
    entries += 1;
  } else if (entry !== null && /^ {4}resolution: \{.*\bintegrity: /.test(line)) {
    seen.add(entry);
  }
});
if (section === "packages") close();
if (entries === 0) bad.push("no package entries found");

console.log(`lock sources: ${entries} package entries; allowed host ${ALLOWED_HOST}; integrity sha512 only`);
for (const b of bad.slice(0, 20)) console.log(`FAIL ${b}`);
if (bad.length > 0) {
  console.log(`check-lock-sources: FAILED (${bad.length} findings)`);
  process.exit(1);
}
console.log("check-lock-sources: OK");
