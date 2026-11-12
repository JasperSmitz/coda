// Loading a browse answer: loading / error (with retry) / data. A newer
// load wins over an older one still in flight. `refresh` changes reload
// quietly (the old data stays up), e.g. while the library scan grows it.
// `key` names the load a ready or error result came from: on the render
// where the key changes, the old result is still returned (the new load
// starts in an effect), so callers that must not act on it compare keys.

import { useCallback, useEffect, useRef, useState } from "react";
import { clearBrowseCache, toBrowseError } from "../lib/browse";
import type { BrowseError } from "../lib/types";

export type Load<T> =
  | { state: "loading"; data?: undefined; error?: undefined }
  | { state: "error"; error: BrowseError; data?: undefined; key: string }
  | { state: "ready"; data: T; error?: undefined; key: string };

export function useLoad<T>(
  load: () => Promise<T>,
  key: string,
  refresh: unknown = null,
): Load<T> & { retry: () => void } {
  const [result, setResult] = useState<Load<T>>({ state: "loading" });
  const [attempt, setAttempt] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const shownKey = useRef<string | null>(null);

  useEffect(() => {
    let current = true;
    const quiet = shownKey.current === key;
    shownKey.current = key;
    if (!quiet) setResult({ state: "loading" });
    loadRef.current().then(
      (data) => current && setResult({ state: "ready", data, key }),
      (error: unknown) => {
        // A quiet refresh that fails keeps what is shown.
        if (current && !quiet) setResult({ state: "error", error: toBrowseError(error), key });
      },
    );
    return () => {
      current = false;
    };
  }, [key, refresh, attempt]);

  const retry = useCallback(() => {
    clearBrowseCache();
    shownKey.current = null;
    setAttempt((n) => n + 1);
  }, []);

  return { ...result, retry };
}
