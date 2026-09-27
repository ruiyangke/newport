// Browser preferences survive the Porthop → Newport rebrand.
export const appearanceKey = "newport-appearance";

export function readAppearance(): "system" | "light" | "dark" {
  try {
    const value =
      localStorage.getItem(appearanceKey) ??
      localStorage.getItem("porthop-appearance");
    if (
      (value === "light" || value === "dark" || value === "system") &&
      localStorage.getItem(appearanceKey) === null
    ) {
      try {
        localStorage.setItem(appearanceKey, value);
      } catch {
        // Keep using the legacy preference if storage is temporarily unwritable.
      }
    }
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}
