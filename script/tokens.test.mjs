// Tests for the token generator's checks: `node --test script/`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { toDisplayP3, validate } from "./tokens.mjs";

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

test("a color in Display P3 is the same color, grays as they are", () => {
  assert.equal(toDisplayP3("#ff0000"), "#ea3323");
  assert.equal(toDisplayP3("#00ff00"), "#75fb4c");
  assert.equal(toDisplayP3("#0000ff"), "#0000f5");
  for (const gray of ["#000000", "#1a1a1a", "#808080", "#ffffff"]) assert.equal(toDisplayP3(gray), gray);
  assert.equal(toDisplayP3("#ff000080"), "#ea332380", "the alpha is kept");
});

test("the Display P3 theme is the sRGB one converted, key for key", () => {
  const read = (name) => JSON.parse(readFileSync(join(root, "crates/deptheos_gpui/themes", name), "utf8"));
  const [srgb, p3] = [read("deptheos.json"), read("deptheos-p3.json")];
  assert.equal(p3.themes.length, srgb.themes.length);
  srgb.themes.forEach((theme, i) => {
    assert.equal(p3.themes[i].name, theme.name);
    for (const [key, color] of Object.entries(theme.colors)) assert.equal(p3.themes[i].colors[key], toDisplayP3(color), key);
  });
});
