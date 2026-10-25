import { test } from "node:test";
import assert from "node:assert/strict";
import { formatTicks } from "../src/live/format.ts";

test("ticks show the step's decimals", () => {
  assert.deepEqual(formatTicks([0, 0.25, 0.5], 0.25), ["0.00", "0.25", "0.50"]);
  assert.deepEqual(formatTicks([40000, 42000], 2000), ["40000", "42000"]);
});

test("large ticks keep neighbours apart as exponents", () => {
  assert.deepEqual(formatTicks([478e6, 479e6, 480e6], 1e6), ["4.78e+8", "4.79e+8", "4.80e+8"]);
  assert.deepEqual(formatTicks([2e6, 4e6], 2e6), ["2.0e+6", "4.0e+6"]);
});
