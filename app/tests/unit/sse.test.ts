import { describe, expect, it } from 'vitest';
import { SseParser } from '../../src/lib/transport/sse';

const feed = (chunks: string[]) => {
  const p = new SseParser();
  return chunks.flatMap((c) => p.push(c));
};

describe('SseParser', () => {
  it('parses a frame split at every character', () => {
    const text = 'id: 7\nevent: job.state\ndata: {"a":1}\n\n';
    expect(feed([...text])).toEqual([{ id: '7', event: 'job.state', data: '{"a":1}' }]);
  });
  it('handles CRLF and lone CR, including a CR/LF pair split across chunks', () => {
    expect(feed(['id: 1\r\ndata: x\r\n\r\n'])).toHaveLength(1);
    expect(feed(['id: 1\rdata: x\r\r'])).toHaveLength(1);
    expect(feed(['data: x\r', '\n\r', '\n'])).toEqual([{ id: null, event: 'message', data: 'x' }]);
  });
  it('ignores heartbeat comments and joins multi-line data', () => {
    expect(feed([': keep-alive\n\n'])).toEqual([]);
    expect(feed(['data: a\ndata: b\n\n'])[0]!.data).toBe('a\nb');
  });
  it('does not dispatch a frame without data and carries the last id forward', () => {
    expect(feed(['id: 3\n\n', 'data: x\n\n'])).toEqual([{ id: '3', event: 'message', data: 'x' }]);
  });
  it('holds a partial frame until the blank line', () => {
    const p = new SseParser();
    expect(p.push('data: x\n')).toEqual([]);
    expect(p.push('\n')).toHaveLength(1);
  });
});
