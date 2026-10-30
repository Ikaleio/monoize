import { useCallback } from "react";
import { useSearchParams } from "react-router-dom";

/**
 * One string-valued search param as React state. Writes use `replace`, the param is
 * removed when it equals `defaultValue`, and unknown values read as `defaultValue`.
 *
 * Data-router writes land asynchronously, so a text input bound directly to this value
 * drops fast keystrokes. Text inputs MUST hold a local draft and commit it on debounce
 * or submit; use this hook directly only for discrete controls such as tabs and selects.
 */
export function useSearchParamState<T extends string>(
  key: string,
  defaultValue: T,
  allowed?: readonly T[],
): [T, (value: T) => void] {
  const [searchParams, setSearchParams] = useSearchParams();
  const raw = searchParams.get(key);
  const value =
    raw !== null && (!allowed || allowed.includes(raw as T)) ? (raw as T) : defaultValue;

  const setValue = useCallback(
    (next: T) => {
      setSearchParams(
        (current) => {
          const params = new URLSearchParams(current);
          if (next === defaultValue) {
            params.delete(key);
          } else {
            params.set(key, next);
          }
          return params;
        },
        { replace: true },
      );
    },
    [defaultValue, key, setSearchParams],
  );

  return [value, setValue];
}
