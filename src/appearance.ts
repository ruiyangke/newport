import { useSyncExternalStore } from "react";
export type Appearance = "system" | "light" | "dark";
const changed = "porthop-appearance-changed";
const key = "porthop-appearance";
export function getAppearance(): Appearance {
  try {
    const value = localStorage.getItem(key);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}
export function applyAppearance(value = getAppearance()) {
  const dark =
    value === "dark" ||
    (value === "system" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
  window.dispatchEvent(new Event(changed));
}
export function setAppearance(value: Appearance) {
  localStorage.setItem(key, value);
  applyAppearance(value);
}

function subscribe(listener: () => void) {
  window.addEventListener(changed, listener);
  return () => window.removeEventListener(changed, listener);
}
export function useResolvedAppearance() {
  return useSyncExternalStore(subscribe, () =>
    document.documentElement.classList.contains("dark") ? "dark" : "light",
  );
}
