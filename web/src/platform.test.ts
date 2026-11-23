import assert from "node:assert/strict";
import { test } from "node:test";
import { detectSystem, isIOS } from "./platform.ts";

const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const WIN_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const LINUX_UA = "Mozilla/5.0 (X11; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";
const ANDROID_UA = "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36";
const IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const CROS_UA = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

test("the three desktop systems, from navigator.platform or userAgentData's names", () => {
  assert.equal(detectSystem("MacIntel", MAC_UA, 0), "mac");
  assert.equal(detectSystem("macOS", MAC_UA, 0), "mac");
  assert.equal(detectSystem("Win32", WIN_UA, 0), "windows");
  assert.equal(detectSystem("Windows", WIN_UA, 0), "windows");
  assert.equal(detectSystem("Linux x86_64", LINUX_UA, 0), "linux");
  assert.equal(detectSystem("Linux aarch64", LINUX_UA, 0), "linux");
});

test("with no platform, the user agent decides", () => {
  assert.equal(detectSystem("", MAC_UA, 0), "mac");
  assert.equal(detectSystem("", WIN_UA, 0), "windows");
  assert.equal(detectSystem("", LINUX_UA, 0), "linux");
});

test("phones, tablets and ChromeOS get no system", () => {
  assert.equal(detectSystem("Linux armv8l", ANDROID_UA, 5), undefined);
  assert.equal(detectSystem("iPhone", IPHONE_UA, 5), undefined);
  // An iPad asking for the desktop site: a Mac's platform and user agent, with a touch screen.
  assert.equal(detectSystem("MacIntel", MAC_UA, 5), undefined);
  assert.equal(detectSystem("Linux x86_64", CROS_UA, 0), undefined);
  assert.equal(detectSystem("", "", 0), undefined);
});

test("isIOS identifies iPhone and iPad, including desktop-mode iPadOS", () => {
  assert.equal(isIOS("iPhone", IPHONE_UA, 5), true);
  assert.equal(isIOS("MacIntel", MAC_UA, 5), true);
  assert.equal(isIOS("MacIntel", MAC_UA, 0), false);
  assert.equal(isIOS("Linux armv8l", ANDROID_UA, 5), false);
  assert.equal(isIOS("Win32", WIN_UA, 0), false);
});
