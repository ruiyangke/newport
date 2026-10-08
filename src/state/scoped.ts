import { createStore, useSelector } from "@tanstack/react-store";
import { useCallback, type SetStateAction } from "react";
import { useCurrentServerScope, useServerScope, type ServerScope } from "../query/keys";

/**
 * One scoped-store factory behind the workspace and git UI stores.
 *
 * Shape per scope: `Record<scopeKey, T>`. The scope key is resolved from the
 * server scope by each slice -- stable `server.id` for UI choices (which
 * survive reconnects), ephemeral `connection` for native handles. Reads
 * share one fallback per store so selectors keep referential stability
 * for scopes without state; every write starts from a fresh
 * `getDefaults()` so one scope's `Set`s can never leak into another's.
 */
export function createScopedStore<T>(options: {
  getDefaults: () => T;
  selectScopeKey: (scope: ServerScope) => string;
}) {
  const { getDefaults, selectScopeKey } = options;
  const store = createStore<Record<string, T>>({});
  /** Read-only fallback. Never written through; writes start fresh. */
  const fallback = getDefaults();

  function read(scope: string): T {
    return store.state[scope] ?? fallback;
  }

  function set<K extends keyof T>(
    scope: string,
    key: K,
    next: SetStateAction<T[K]>,
  ) {
    store.setState((state) => {
      const page = state[scope];
      const previous = (page ?? fallback)[key];
      const value =
        typeof next === "function"
          ? (next as (value: T[K]) => T[K])(previous)
          : next;
      if (Object.is(previous, value)) return state;
      // Fresh defaults first so a missing scope never inherits another
      // scope's `Set` instances; the existing page wins where present.
      return { ...state, [scope]: { ...getDefaults(), ...page, [key]: value } };
    });
  }

  /** Several fields that must change together, as one update. */
  function patch(scope: string, partial: Partial<T>) {
    store.setState((state) => ({
      ...state,
      [scope]: { ...getDefaults(), ...state[scope], ...partial },
    }));
  }

  function retain(valid: Set<string>) {
    store.setState((state) => {
      if (Object.keys(state).every((key) => valid.has(key))) return state;
      return Object.fromEntries(
        Object.entries(state).filter(([key]) => valid.has(key)),
      );
    });
  }

  function useScoped<K extends keyof T>(id: string, key: K) {
    const scopeKey = selectScopeKey(useServerScope(id));
    const value = useSelector(store, (state) => (state[scopeKey] ?? fallback)[key]);
    const update = useCallback(
      (next: SetStateAction<T[K]>) => {
        set(scopeKey, key, next);
      },
      [scopeKey, key],
    );
    return [value, update] as const;
  }

  /**
   * The same, for components inside one server's page that are not told
   * which server it is.
   */
  function useCurrentScoped<K extends keyof T>(key: K) {
    const scopeKey = selectScopeKey(useCurrentServerScope());
    const value = useSelector(store, (state) => (state[scopeKey] ?? fallback)[key]);
    const update = useCallback(
      (next: SetStateAction<T[K]>) => {
        set(scopeKey, key, next);
      },
      [scopeKey, key],
    );
    return [value, update] as const;
  }

  return { store, read, set, patch, retain, useScoped, useCurrentScoped };
}

export interface RetainSelections {
  /** Stable server ids: UI-choice stores. */
  servers: Set<string>;
  /** Ephemeral connections: native-handle registries. */
  connections: Set<string>;
}

type RetainFn = (selections: RetainSelections) => void;

const retainFns = new Set<RetainFn>();

/** Register a retention participant; returns an unregister function. */
export function registerRetain(fn: RetainFn): () => void {
  retainFns.add(fn);
  return () => {
    retainFns.delete(fn);
  };
}

/** Run every registered retention function once. Wired up in App.tsx. */
export function retainAll(selections: RetainSelections) {
  for (const fn of retainFns) fn(selections);
}
