// Run the released OpenCode clipboard adapters, including their real Linux dependencies.
import { createInterface } from "node:readline"
import { createHash } from "node:crypto"

const version = process.argv[2]
const clipboard = await (async () => {
  if (version === "v1") return import("./v1")
  if (version === "v2") {
    const { createTuiClipboard } = await import("./v2")
    return createTuiClipboard({
      capabilities: { remote: true },
      copyToClipboardOSC52() { throw new Error("Unexpected clipboard write during read") },
      clearClipboardOSC52() { throw new Error("Unexpected clipboard clear during read") },
    })
  }
  throw new Error("usage: opencode-clipboard-probe v1|v2")
})()

function emit(value: unknown) {
  console.log(JSON.stringify(value))
}

emit({ ready: true, version, pid: process.pid })
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "quit") break
  try {
    if (line !== "read") throw new Error(`Unknown command: ${line}`)
    const value = await clipboard.read()
    if (!value) emit({ empty: true })
    else if (value.mime === "text/plain") emit({ mime: value.mime, text: value.data })
    else emit({ mime: value.mime, sha256: createHash("sha256").update(Buffer.from(value.data, "base64")).digest("hex") })
  } catch (error) {
    emit({ error: error instanceof Error ? error.message : String(error) })
  }
}
if ("dispose" in clipboard) await clipboard.dispose()
// Bun can keep its stdin watcher alive after readline's async iterator closes.
process.exit(0)
