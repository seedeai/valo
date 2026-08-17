/**
 * One-shot handoff from a docs widget into the playground.
 *
 * The playground only accepts a catalog id in the URL. Edits made in the docs
 * editor are not a catalog entry, so they travel through sessionStorage and
 * are consumed once the playground mounts. A module-level copy survives React
 * StrictMode's remount, which would otherwise take and drop the stash before
 * the lasting mount runs.
 */

const KEY = 'valo.playground.draft';

type Draft = { readonly demo: string; readonly source: string };

let lastTaken: Draft | undefined;

export function stashPlaygroundDraft(demo: string, source: string): void {
  lastTaken = undefined;
  sessionStorage.setItem(KEY, JSON.stringify({ demo, source } satisfies Draft));
}

export function takePlaygroundDraft(demo: string): string | undefined {
  if (lastTaken?.demo === demo) return lastTaken.source;

  const raw = sessionStorage.getItem(KEY);
  sessionStorage.removeItem(KEY);
  if (!raw) return;

  try {
    const parsed = JSON.parse(raw) as Draft;
    if (parsed.demo !== demo || typeof parsed.source !== 'string') return;
    lastTaken = parsed;
    return parsed.source;
  } catch {
    return undefined;
  }
}
