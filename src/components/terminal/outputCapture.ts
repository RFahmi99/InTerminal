// Reads what a command printed straight from the terminal's rendered buffer.
// The buffer holds clean text (escape codes already interpreted), which is
// more reliable than re-parsing the raw ConPTY stream.

export interface BufferLineLike {
  isWrapped: boolean;
  translateToString(trimRight?: boolean): string;
}

export interface BufferLike {
  length: number;
  getLine(index: number): BufferLineLike | undefined;
}

/** Text error rules can match on: enough for any error message, bounded for IPC. */
export const MAX_CAPTURE_CHARS = 8000;

/**
 * Text of lines `[from, to)`, re-joining lines the terminal soft-wrapped so
 * a long error message reads as one line. Keeps the end when it's too long,
 * since errors are usually printed last.
 */
export function readLines(buffer: BufferLike, from: number, to: number, maxChars = MAX_CAPTURE_CHARS): string {
  const start = Math.max(0, from);
  const end = Math.min(buffer.length, to);
  let text = "";
  for (let i = start; i < end; i++) {
    const line = buffer.getLine(i);
    if (!line) continue;
    if (i > start && !line.isWrapped) text += "\n";
    text += line.translateToString(true);
  }
  text = text.replace(/\s+$/, "");
  return text.length > maxChars ? text.slice(text.length - maxChars) : text;
}
