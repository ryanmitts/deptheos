// Where Chrome is, for the scripts that drive it (puppeteer-core brings no browser of its own):
// $CHROME, or the usual install path on Windows, macOS or Linux.

import { existsSync } from "node:fs";

export function findChrome(): string {
  const chrome =
    process.env.CHROME ??
    [
      "C:/Program Files/Google/Chrome/Application/chrome.exe",
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/usr/bin/google-chrome",
    ].find(existsSync);
  if (!chrome) throw new Error("Chrome not found; set CHROME");
  return chrome;
}
