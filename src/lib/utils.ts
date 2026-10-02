export { cn } from "cn";

/** "1 file" / "2 files" — one rule for every count the UI states. */
export function plural(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
