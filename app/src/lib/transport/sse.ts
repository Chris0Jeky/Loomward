export interface SseFrame {
  id: string | null;
  event: string;
  data: string;
}

/**
 * Incremental parser for `text/event-stream` (WHATWG rules: LF, CRLF or CR line ends, comment lines
 * start with ':', multiple `data:` lines join with LF, a blank line dispatches). Feed it decoded text
 * chunks in any split; it keeps the partial line between calls.
 */
export class SseOverflowError extends Error {
  override name = 'SseOverflowError';
}

export interface SseLimits {
  /** Longest single line, in UTF-16 units; also bounds the partial line held between chunks. */
  maxLine: number;
  /** Total `data:` payload of one event not yet dispatched. */
  maxEvent: number;
}
const DEFAULT_LIMITS: SseLimits = { maxLine: 1 << 20, maxEvent: 2 << 20 };

export class SseParser {
  constructor(private readonly limits: SseLimits = DEFAULT_LIMITS) {}
  private dataSize = 0;
  private buf = '';
  private skipLf = false;
  private id: string | null = null;
  private event = '';
  private data: string[] = [];
  private lastId: string | null = null;

  push(chunk: string): SseFrame[] {
    const frames: SseFrame[] = [];
    if (this.skipLf && chunk.startsWith('\n')) chunk = chunk.slice(1);
    this.skipLf = false;
    this.buf += chunk;
    const lines = this.buf.split(/\r\n|\r|\n/);
    this.buf = lines.pop() ?? '';
    // A stream that never ends a line or never dispatches must not grow state without bound.
    if (this.buf.length > this.limits.maxLine || lines.some((l) => l.length > this.limits.maxLine)) {
      throw new SseOverflowError('event stream line too long');
    }
    // A chunk ending in a lone CR may be the first half of CRLF: the partial line is empty, remember to eat the LF.
    if (chunk.endsWith('\r')) this.skipLf = true;
    for (const line of lines) {
      if (line === '') {
        if (this.data.length) frames.push({ id: this.id ?? this.lastId, event: this.event || 'message', data: this.data.join('\n') });
        if (this.id !== null) this.lastId = this.id;
        this.id = null;
        this.event = '';
        this.data = [];
        this.dataSize = 0;
      } else if (line.startsWith(':')) {
        // comment / heartbeat
      } else {
        const i = line.indexOf(':');
        const field = i < 0 ? line : line.slice(0, i);
        let value = i < 0 ? '' : line.slice(i + 1);
        if (value.startsWith(' ')) value = value.slice(1);
        if (field === 'data') {
          this.data.push(value);
          this.dataSize += value.length + 1;
          if (this.dataSize > this.limits.maxEvent) throw new SseOverflowError('event stream event too large');
        }
        else if (field === 'event') this.event = value;
        else if (field === 'id' && !value.includes('\0')) this.id = value;
      }
    }
    return frames;
  }
}
