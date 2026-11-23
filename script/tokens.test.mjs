// Tests for the token generator's checks: `node --test script/`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { validate } from "./tokens.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tokens = () => JSON.parse(readFileSync(join(root, "design/tokens.json"), "utf8"));

test("the tokens file is valid", () => {
  validate(tokens());
});

test("chrome over the photo is one value in both modes", () => {
  for (const name of ["hud", "hud-border", "hud-text", "viewer"]) {
    const t = tokens();
    t.color[name].dark = "#000000";
    t.color[name].light = "#ffffff";
    assert.throws(() => validate(t), new RegExp(`color.${name}.light and .dark must be equal`));
  }
});

test("a color carries an alpha only as #rrggbbaa", () => {
  const t = tokens();
  t.color.hud.light = t.color.hud.dark = "#121418d";
  assert.throws(() => validate(t), /must be #rrggbb or #rrggbbaa/);
  const ok = tokens();
  ok.color.hud.light = ok.color.hud.dark = "#12141880";
  validate(ok);
});

test("the hud tokens are there", () => {
  const t = tokens();
  delete t.color["hud-text"];
  assert.throws(() => validate(t), /color.hud-text is missing/);
});
