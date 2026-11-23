// Trailing-edge debounce. The timer exists only while a call is pending.

export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  cancel(): void;
  flush(): void;
  pending(): boolean;
}

export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number): Debounced<A> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let last: A | null = null;

  const run = () => {
    timer = null;
    const args = last;
    last = null;
    if (args) fn(...args);
  };

  const d = ((...args: A) => {
    last = args;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(run, ms);
  }) as Debounced<A>;

  d.cancel = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    last = null;
  };
  d.flush = () => {
    if (timer === null) return;
    clearTimeout(timer);
    run();
  };
  d.pending = () => timer !== null;
  return d;
}
