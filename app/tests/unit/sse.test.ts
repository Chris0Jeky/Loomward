import { describe, expect, it } from 'vitest';
import { SseOverflowError, SseParser } from '../../src/lib/transport/sse';

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

describe('SseParser bounds', () => {
  it('throws on an endless line with no newline', () => {
    const p = new SseParser({ maxLine: 64, maxEvent: 1024 });
    expect(() => { for (let i = 0; i < 100; i++) p.push('x'.repeat(16)); }).toThrow(SseOverflowError);
  });
  it('throws on one over-long complete line', () => {
    expect(() => new SseParser({ maxLine: 64, maxEvent: 1024 }).push('data: ' + 'x'.repeat(100) + '\n')).toThrow(SseOverflowError);
  });
  it('throws when data lines pile up without a blank line', () => {
    const p = new SseParser({ maxLine: 64, maxEvent: 200 });
    expect(() => { for (let i = 0; i < 100; i++) p.push('data: ' + 'y'.repeat(30) + '\n'); }).toThrow(SseOverflowError);
  });
  it('does not count comments or dispatched events against the cap', () => {
    const p = new SseParser({ maxLine: 64, maxEvent: 200 });
    for (let i = 0; i < 100; i++) expect(p.push(': hb\ndata: ' + 'z'.repeat(30) + '\n\n')).toHaveLength(1);
  });
});
