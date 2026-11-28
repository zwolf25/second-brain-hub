// Guards for conventions that are easy to break silently.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const mainJs = readFileSync(join(root, "src", "main.js"), "utf8");

// Errors must go through notifyError (toast), not the native alert() dialog (BACKLOG TD-16 consistency pass, 0.21.5).
test("main.js has no native alert() calls outside comments", () => {
  const offenders = mainJs
    .split("\n")
    .map((line, i) => [i + 1, line])
    .filter(([, line]) => !line.trimStart().startsWith("//") && /(^|[^\w.])alert\(/.test(line));
  assert.deepEqual(offenders, [], `use notifyError() instead: ${JSON.stringify(offenders)}`);
});
