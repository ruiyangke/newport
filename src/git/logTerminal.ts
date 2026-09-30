/** Allow display formatting, never terminal clipboard, hyperlinks or device requests. */
export function terminalLogText(text: string): string {
  let result = "";
  for (let index = 0; index < text.length; index++) {
    const code = text.charCodeAt(index);
    if (code === 27) {
      const next = text[index + 1];
      if (next === "[") {
        const start = index;
        index += 2;
        while (index < text.length && !/[\x40-\x7e]/.test(text[index])) index++;
        const sequence = text.slice(start, index + 1);
        // SGR and horizontal cursor/erase operations support progress output
        // without allowing one command to erase other command records.
        if (/^[0-9;:]*[mKGCD]$/.test(sequence.slice(2))) result += sequence;
      } else if (next && "]PX^_".includes(next)) {
        index += 2;
        while (
          index < text.length &&
          text.charCodeAt(index) !== 7 &&
          !(text.charCodeAt(index) === 27 && text[index + 1] === "\\")
        )
          index++;
        if (text.charCodeAt(index) === 27) index++;
      } else {
        index++;
      }
    } else if (code === 0) result += "\n";
    else if (
      code === 8 ||
      code === 9 ||
      code === 10 ||
      code === 13 ||
      (code >= 32 && !(code >= 127 && code <= 159))
    )
      result += text[index];
  }
  return result;
}
