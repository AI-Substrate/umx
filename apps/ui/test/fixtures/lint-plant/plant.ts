// A planted breach of the import boundary. test/lint-plant.test.ts lints this file and expects every line below
// to fail; normal lint ignores test/fixtures.
import { execFile } from "node:child_process";
import { connect } from "node:net";
import { readFileSync } from "node:fs";

export const breach = [execFile, connect, readFileSync, import("node:fs/promises")];
