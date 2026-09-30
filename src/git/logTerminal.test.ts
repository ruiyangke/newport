import { expect, it, vi } from "vitest";
import { terminalLogText } from "./logTerminal";
it("keeps progress, backspace, color and horizontal erase controls for xterm", () => {
  const output =
    "progress 1%\r\x1b[2Kprogress 100%\nfixx\b \b\x1b[32mDone\x1b[0m";
  expect(terminalLogText(output)).toBe(output);
});
it("shows NUL records as lines without changing literal backslash-zero filenames", () => {
  expect(terminalLogText("file-a\0file-b\0literal\\0name")).toBe(
    "file-a\nfile-b\nliteral\\0name",
  );
});
it("drops terminal side effects and incomplete escape sequences", () => {
  expect(
    terminalLogText(
      "safe\x1b]52;c;c2VjcmV0\x07\x1b]8;;https://example.org\x1b\\link\x1b]8;;\x1b\\\x1b[2J\x1b[6n\x1b[31",
    ),
  ).toBe("safelink");
  expect(terminalLogText("a\x1bPignored\x1b\\b\x07")).toBe("ab");
});

it("renders progress and backspace through the actual xterm parser", async () => {
  vi.stubGlobal("self", globalThis);
  const { Terminal } = await import("@xterm/xterm");
  const term = new Terminal({ cols: 80, rows: 24, convertEol: true });
  try {
    await new Promise<void>((done) =>
      term.write(
        terminalLogText(
          "progress 1%\r\x1b[2Kprogress 100%\nfixx\b \b\n\x1b[32mDone\x1b[0m",
        ),
        done,
      ),
    );
    expect(term.buffer.active.getLine(0)?.translateToString(true)).toBe(
      "progress 100%",
    );
    expect(term.buffer.active.getLine(1)?.translateToString(true)).toBe("fix ");
    expect(term.buffer.active.getLine(2)?.translateToString(true)).toBe("Done");
    expect(term.buffer.active.getLine(2)?.getCell(0)?.getFgColor()).toBe(2);
  } finally {
    term.dispose();
    vi.unstubAllGlobals();
  }
});
