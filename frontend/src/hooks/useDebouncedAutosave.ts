import { useCallback, useRef } from "react";

// Debounced autosave shared by the repo-settings and app-settings JsonForms.
// Skips the no-op onChange JsonForms fires on load (via a JSON baseline seeded
// with `seed`), debounces the write, and reports success/failure through
// `onError`. `cancel` drops a pending write (e.g. before deleting the file).
//
// Per-form differences are options: `onChange` runs synchronously on a real
// change (the app form applies the theme immediately); `onSaved` runs after a
// successful write (the app form refreshes tool resolution); `rollbackOnError`
// restores the baseline so a fixed value can save again (the app form wants this;
// the repo form does not).
export function useDebouncedAutosave<T>(opts: {
  save: (data: T) => Promise<void>;
  onError: (msg: string | null) => void;
  delayMs?: number;
  rollbackOnError?: boolean;
  onChange?: (data: T) => void;
  onSaved?: () => void;
}) {
  const { delayMs = 400, rollbackOnError = false } = opts;
  const lastSavedRef = useRef<string>("");
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Read callbacks through refs so `schedule` stays stable across renders.
  const optsRef = useRef(opts);
  optsRef.current = opts;

  // Seed the baseline so JsonForms' initial onChange (same data) is a no-op.
  const seed = useCallback((data: unknown) => {
    lastSavedRef.current = JSON.stringify(data);
  }, []);

  const cancel = useCallback(() => {
    if (timerRef.current) clearTimeout(timerRef.current);
  }, []);

  const schedule = useCallback((data: T) => {
    const serialized = JSON.stringify(data);
    if (serialized === lastSavedRef.current) return;
    const prev = lastSavedRef.current;
    lastSavedRef.current = serialized;
    optsRef.current.onChange?.(data);
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(async () => {
      try {
        await optsRef.current.save(data);
        optsRef.current.onError(null);
        optsRef.current.onSaved?.();
      } catch (e) {
        if (rollbackOnError) lastSavedRef.current = prev; // let a fixed value save again
        optsRef.current.onError(String(e));
      }
    }, delayMs);
  }, [delayMs, rollbackOnError]);

  return { seed, cancel, schedule };
}
