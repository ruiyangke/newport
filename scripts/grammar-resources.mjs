import { readFileSync } from "node:fs";

export const grammarLanguages = JSON.parse(
  readFileSync(new URL("../resources/grammar-languages.json", import.meta.url), "utf8"),
);
export function grammarLibrary(name, platform = process.platform) {
  if (!grammarLanguages.includes(name)) throw new Error(`Unknown grammar: ${name}`);
  return platform === "win32"
    ? `newport_grammar_${name}.dll`
    : `libnewport_grammar_${name}.${platform === "darwin" ? "dylib" : "so"}`;
}
export function grammarFiles(platform = process.platform) {
  return [
    "catalog.json",
    ...grammarLanguages.map(name => grammarLibrary(name, platform)),
  ];
}
