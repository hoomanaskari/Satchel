// Keyset paging over `list_clips`. Plain class; it reports every change
// through `onChange` so a reactive owner can mirror the state.

import { buildQuery, cursorOf, MAX_LIMIT, type Filters } from "./query";
import type { ClipId, ClipSummary, Query } from "./types";

export type FetchPage = (q: Query) => Promise<ClipSummary[]>;

/** Load the next page when the rendered window ends this close to the tail. */
const PREFETCH_MARGIN = 10;

export class Pager {
  items: ClipSummary[] = [];
  hasMore = false;
  loading = false;
  error: string | null = null;
  filters: Filters = { text: "", kind: "all", pinboardId: null };

  /** Bumped by every reset; responses from older generations are dropped. */
  private gen = 0;
  private moreInFlight = false;

  constructor(
    private readonly fetch: FetchPage,
    readonly pageSize = 50,
    private readonly onChange: () => void = () => {},
  ) {}

  /** Load the first page for `filters`. Resolves false if a newer reset superseded it. */
  reset(filters: Filters, keep = 0): Promise<boolean> {
    this.filters = { ...filters };
    const limit = Math.min(MAX_LIMIT, Math.max(this.pageSize, keep));
    return this.load(limit, true);
  }

  /** Reload the same filters, keeping as many rows as are already loaded. */
  refresh(): Promise<boolean> {
    return this.reset(this.filters, this.items.length);
  }

  async loadMore(): Promise<void> {
    if (!this.hasMore || this.loading || this.moreInFlight) return;
    await this.load(this.pageSize, false);
  }

  wantsMore(renderedEnd: number): boolean {
    return this.hasMore && !this.loading && renderedEnd >= this.items.length - PREFETCH_MARGIN;
  }

  remove(ids: ClipId[]): void {
    const drop = new Set(ids);
    this.items = this.items.filter((c) => !drop.has(c.id));
    this.onChange();
  }

  patch(id: ClipId, change: Partial<ClipSummary>): void {
    this.items = this.items.map((c) => (c.id === id ? { ...c, ...change } : c));
    this.onChange();
  }

  private async load(limit: number, fresh: boolean): Promise<boolean> {
    const gen = fresh ? ++this.gen : this.gen;
    const last = this.items[this.items.length - 1];
    const before = !fresh && last ? cursorOf(last) : null;
    const query = buildQuery(this.filters, limit, before);
    this.loading = true;
    if (!fresh) this.moreInFlight = true;
    this.onChange();
    try {
      const page = await this.fetch(query);
      if (gen !== this.gen) return false;
      if (fresh) {
        this.items = page;
      } else {
        const seen = new Set(this.items.map((c) => c.id));
        this.items = this.items.concat(page.filter((c) => !seen.has(c.id)));
      }
      this.hasMore = page.length >= query.limit;
      this.error = null;
      return true;
    } catch (e) {
      if (gen !== this.gen) return false;
      this.error = e instanceof Error ? e.message : String(e);
      this.hasMore = false;
      if (fresh) this.items = [];
      return true;
    } finally {
      if (!fresh) this.moreInFlight = false;
      if (gen === this.gen) {
        this.loading = false;
        this.onChange();
      }
    }
  }
}
