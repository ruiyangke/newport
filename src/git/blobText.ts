import type { GitBlobPage } from "../domain/gitResponses";

/** Incremental reader for immutable pages. A replaced query or rolled-back
 * render resets the decoder; revisiting the same page is idempotent. */
export function createBlobTextReader() {
  let previous: GitBlobPage | null = null;
  let decoder = new TextDecoder();
  let result: string | null = "";
  let ended = false;
  return (page: GitBlobPage): string | null => {
    const extendsPrevious =
      previous !== null &&
      previous.snapshot === page.snapshot &&
      previous.metadata.size === page.metadata.size &&
      previous.metadata.oid.hex === page.metadata.oid.hex &&
      previous.metadata.oid.algorithm === page.metadata.oid.algorithm &&
      previous.entries.length <= page.entries.length &&
      previous.entries.every((entry, index) => entry === page.entries[index]) &&
      (!ended || previous.entries.length === page.entries.length);
    const start = extendsPrevious ? previous!.entries.length : 0;
    if (!extendsPrevious) {
      decoder = new TextDecoder();
      result = "";
      ended = false;
    }
    if (result !== null) {
      for (let index = start; index < page.entries.length; index++) {
        const raw = atob(page.entries[index].bytesB64);
        const bytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
        if (bytes.includes(0)) {
          result = null;
          break;
        }
        result += decoder.decode(bytes, { stream: true });
      }
      // Keep a split UTF-8 sequence pending until the true end of the blob.
      if (result !== null && page.nextCursor === null && !ended) {
        result += decoder.decode();
        ended = true;
      }
    }
    previous = page;
    return result;
  };
}
export function blobText(page: GitBlobPage): string | null {
  return createBlobTextReader()(page);
}
