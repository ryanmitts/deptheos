import { createRoot } from "react-dom/client";
import { Agentation } from "agentation";

/**
 * Mounts the dev-only Agentation visual feedback overlay.
 * Tree-shaken completely from production builds via `if (import.meta.env.DEV)`.
 * Syncs with the local Agentation MCP server on http://localhost:4747 by default;
 * override with `PUBLIC_AGENTATION_ENDPOINT`.
 */
export function mountAgentation(): void {
  if (document.getElementById("deptheos-agentation-root")) return;
  const host = document.createElement("div");
  host.id = "deptheos-agentation-root";
  document.body.append(host);
  const isLocal = location.hostname === "localhost" || location.hostname === "127.0.0.1";
  const endpoint = import.meta.env.PUBLIC_AGENTATION_ENDPOINT ?? (isLocal ? "http://localhost:4747" : undefined);
  createRoot(host).render(endpoint ? <Agentation endpoint={endpoint} /> : <Agentation />);
}
