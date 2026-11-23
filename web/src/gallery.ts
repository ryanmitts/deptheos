// The gallery pages: the before/after wiper, the picture's full-resolution viewer (viewer.ts).

import "./compare.ts";
import { attachViewer } from "./viewer.ts";

for (const el of document.querySelectorAll<HTMLElement>("[data-viewer]")) attachViewer(el);
