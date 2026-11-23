// Where the site's media is served from. Pages name files under public/media/, which Vite
// serves in development. A deploy (scripts/deploy.sh) puts each set under a hash of its
// contents at images.deptheos.com, and the build rewrites the addresses (the
// `deptheosIntegration` in src/integrations/deptheos.ts, and `mediaUrl` here):
//
//   /media/gallery/hornet/tiles/0-0.jpg
//   → https://images.deptheos.com/site/gallery/hornet/<hash>/tiles/0-0.jpg
//
// A set is what one script run replaces together (a gallery folder, a figure's folder, the
// home page's files), so an address never serves two contents and may be cached for good.

/** Where a deploy's media is: the address the sets are under, and each set's hash. */
export interface MediaTarget {
  base: string;
  sets: Record<string, string>;
}

/** The path every media file's address starts with in the pages and in public/. */
export const MEDIA_ROOT = "/media/";

/** A path's set, after `/media/`: its first two folders (`gallery/hornet`), or the first for a file directly in it. */
export function setOf(rest: string): string {
  const parts = rest.split("/");
  if (parts.length < 2) throw new Error(`media file must live inside a set folder under /media/: ${rest}`);
  return parts.length >= 3 ? `${parts[0]}/${parts[1]}` : parts[0];
}

/** A media path's address: itself with no target (development), or where the target has it. */
export function resolveMedia(path: string, target: MediaTarget | null): string {
  if (!path.startsWith(MEDIA_ROOT)) throw new Error(`not a media path: ${path}`);
  if (!target) return path;
  const rest = path.slice(MEDIA_ROOT.length);
  const set = setOf(rest);
  const hash = target.sets[set];
  if (!hash) throw new Error(`no media set "${set}" for ${path}: is the file in public/media/?`);
  return `${target.base}/${set}/${hash}${rest.slice(set.length)}`;
}

/** The deploy's media target, written in by the build; absent in development and in Node. */
declare const __MEDIA__: MediaTarget | null | undefined;

/** A media path's address in the running page. */
export const mediaUrl = (path: string): string => resolveMedia(path, typeof __MEDIA__ === "undefined" ? null : (__MEDIA__ ?? null));
