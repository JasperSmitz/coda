// Lookups run a few at a time (pure: no React, no DOM; `scripts/lookupQueue.test.mjs`).
// Each key is looked up once and its answer remembered; whoever asked is
// told. Asking returns a cancel: a key nobody waits for any more leaves
// the queue before its turn (a tile scrolled away, a view unmounted).
// `drop` empties the queue (the view unmounted); `clear` also forgets the
// answers (the server changed), and answers to lookups already running
// then are ignored. A failed lookup
// is not remembered: the next ask tries again.

export interface LookupQueue<T> {
  /** Asks for `key`'s answer: `done` is called once with it (or with `null` on failure); returns a cancel. */
  request(key: string, done: (value: T | null) => void): () => void;
  /** The remembered answer, if any. */
  known(key: string): T | null | undefined;
  /** Drops what is queued and who waits (a view unmounted); answers stay remembered. */
  drop(): void;
  /** Drops what is queued, what waits and what is remembered. */
  clear(): void;
  /** Keys waiting for their turn (not running). */
  queued(): string[];
}

export function createLookupQueue<T>(
  run: (key: string) => Promise<T | null>,
  concurrency = 1,
): LookupQueue<T> {
  const answers = new Map<string, T | null>();
  const listeners = new Map<string, Set<(value: T | null) => void>>();
  let waiting: string[] = [];
  const running = new Set<string>();
  let generation = 0;

  const tell = (key: string, value: T | null) => {
    const set = listeners.get(key);
    listeners.delete(key);
    set?.forEach((f) => f(value));
  };

  const next = () => {
    while (running.size < concurrency && waiting.length > 0) {
      const key = waiting.shift()!;
      const mine = generation;
      running.add(key);
      run(key)
        .then(
          (value) => {
            if (mine !== generation) return;
            answers.set(key, value);
            tell(key, value);
          },
          () => {
            if (mine !== generation) return;
            tell(key, null);
          },
        )
        .finally(() => {
          if (mine !== generation) return;
          running.delete(key);
          next();
        });
    }
  };

  return {
    request(key, done) {
      if (answers.has(key)) {
        done(answers.get(key) ?? null);
        return () => {};
      }
      let set = listeners.get(key);
      if (!set) {
        set = new Set();
        listeners.set(key, set);
      }
      const listener = (value: T | null) => done(value);
      set.add(listener);
      if (!running.has(key) && !waiting.includes(key)) waiting.push(key);
      next();
      return () => {
        const now = listeners.get(key);
        if (!now) return;
        now.delete(listener);
        if (now.size > 0) return;
        listeners.delete(key);
        // Nobody waits: out of the queue before its turn (a running one finishes and is remembered).
        waiting = waiting.filter((k) => k !== key);
      };
    },
    known(key) {
      return answers.get(key);
    },
    drop() {
      waiting = [];
      listeners.clear();
    },
    clear() {
      generation++;
      waiting = [];
      running.clear();
      listeners.clear();
      answers.clear();
    },
    queued() {
      return [...waiting];
    },
  };
}
