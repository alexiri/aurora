// One helper for "every page of a list" (docs/05 list convention, FIX-117): the API pages at 50 by default and 200 at
// most, so a hook that reads one page silently drops the rest. `fetchAllPages` follows `next_cursor` up to a ceiling and
// says when it stopped there (`truncated`), so the screen can show "more not shown" instead of pretending the list is
// complete. Lists that can be longer than the ceiling should use an infinite query with "Load more" instead.

/** The largest page the API serves (`services/aurora-api/src/http/pagination.rs`). */
export const PAGE_LIMIT = 200;

/** Pages followed at most: with `PAGE_LIMIT` rows each, 10 pages cover 2000 rows. */
export const MAX_PAGES = 10;

export interface Paged<T> {
  items: T[];
  next_cursor?: string | null;
}

/**
 * Follows `next_cursor` until the list ends or `maxPages` is reached. The result is the first page's envelope (so
 * extra fields such as `group_bindings` survive) with every item and `next_cursor: null`. `truncated` is true only
 * when the server still had more at the ceiling.
 */
export async function fetchAllPages<P extends Paged<unknown>>(
  page: (cursor: string | undefined) => Promise<P>,
  maxPages: number = MAX_PAGES,
): Promise<Omit<P, "next_cursor"> & { next_cursor: null; truncated: boolean }> {
  const first = await page(undefined);
  const items: unknown[] = [...first.items];
  let cursor = first.next_cursor ?? undefined;
  let pages = 1;
  while (cursor && pages < maxPages) {
    const p = await page(cursor);
    items.push(...p.items);
    cursor = p.next_cursor ?? undefined;
    pages += 1;
  }
  return { ...first, items: items as P["items"], next_cursor: null, truncated: !!cursor };
}
