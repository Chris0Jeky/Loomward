// Shared by the Atlas and Observatory views (lane L10): palette from tokens, and slice navigation.
import { formatBytes, parseBytes } from '../../lib/format/bytes';
import { formatCount } from '../../lib/format/time';
import { escapedName } from '../../lib/format/names';
import { session } from '../../lib/stores/session.svelte';
import type { Anchor, Basis, TreeSlice } from '../../lib/contracts.gen';
import type { NodeInfo, Palette, SliceLike, SliceNodeLike } from '../../../viz/types.js';

/** Read the viz palette from the CSS tokens of the current theme (styles/tokens.css is the only source). */
export function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const t = (k: string) => cs.getPropertyValue(k).trim();
  const obs = document.documentElement.dataset.theme === 'observatory';
  const ui = t('--font-ui'), display = t('--font-display'), mono = t('--font-mono'), num = obs ? mono : ui;
  return {
    ink: t('--cloth-ink'), frame: t('--cloth-frame'), head: t('--cloth-head'), scrim: t('--cloth-scrim'),
    label: t('--cloth-label'), labelDim: t('--cloth-label-dim'), fringe: t('--cloth-fringe'), loose: t('--cloth-loose'),
    cursor: t('--cloth-cursor'), hover: t('--cloth-hover'), shuttle: t('--cloth-shuttle'), track: t('--cloth-track'), scale: t('--cloth-scale'),
    dyes: [1, 2, 3, 4, 5, 6, 7, 8].map((i) => t(`--dye-${i}`)), undyed: t('--undyed'), neutral: t('--neutral-thread'),
    metals: [t('--weft-hot'), t('--weft-warm'), t('--weft-cold')], hollow: t('--weft-hollow'),
    permission: t('--selvedge'), unknown: t('--unknown'),
    fontDisplay: obs ? `600 18px ${t('--font-ui-display')}` : `400 21px ${display}`,
    fontHead: obs ? `600 12.5px ${mono}` : `400 16px ${display}`,
    fontLabel: `600 11.5px ${ui}`,
    fontNum: obs ? `400 10.5px ${num}` : `500 11px ${num}`,
    fontSmall: `400 11px ${num}`,
  };
}

/** Canvas and aria text for an untrusted name: hidden characters spelled out (one escape, at the slice boundary). */
export const canvasLabel = (name: string) => escapedName(name);

/** Byte label for a lossy layout number (exact strings are shown wherever the slice has them). */
export const formatApprox = (n: number) => formatBytes(String(Math.max(0, Math.round(n))));

const KIND: Record<string, string> = { dir: 'folder', file: 'file', root: 'root folder', volume: 'volume', atlas: 'all roots', other: 'folded items' };
export const kindLabel = (kind: string): string => KIND[kind] ?? kind;

/** Coverage states in which a size of 0 means "nothing was measured here", not "empty". */
const UNMEASURED = new Set(['unscanned', 'unknown', 'stale', 'cancelled']);

/**
 * A node's size is unknown (never to be read as 0 B) when the slice carries none, or carries 0 for a node that
 * was denied, never measured, or holds files of unknown allocation. `allocated` is the basis: under the logical
 * basis files of unknown allocation do not make a size unknown.
 */
export function sizeUnknown(n: Pick<SliceNodeLike, 'size_bytes' | 'coverage' | 'size_unknown_files' | 'threads'>, allocated: boolean): boolean {
  const bytes = parseBytes(n.size_bytes);
  if (bytes === null) return true;
  return bytes === 0n && (isDenied(n) || UNMEASURED.has(n.coverage) || (allocated && n.size_unknown_files > 0));
}
export const isDenied = (n: Pick<SliceNodeLike, 'coverage' | 'threads'>): boolean => n.coverage === 'denied' || n.threads.permission.state === 'denied';

/** What the three threads say about a node, in words: residency, meaning, and any permission other than plain granted or denied. */
export function threadNotes(n: Pick<SliceNodeLike, 'threads'>): string[] {
  const t = n.threads;
  const out = [t.residency.tier === null ? 'residency unknown' : `tier ${t.residency.tier}`];
  if (t.meaning.state !== 'none') out.push(`meaning ${t.meaning.state}${t.meaning.label ? ` ${canvasLabel(t.meaning.label)}` : ''}`);
  if (t.permission.state !== 'granted' && t.permission.state !== 'denied') out.push(`permission ${t.permission.state}`);
  return out;
}

/**
 * For every node of the slice, how many denied nodes sit below it in this slice (a lower bound: a denied
 * folder's contents are not listed, and the slice is bounded). Parents precede children, so each walk up
 * moves to a smaller index and a malformed slice ends it.
 */
export function deniedBelow(slice: SliceLike | null | undefined): Map<string, number> {
  const out = new Map<string, number>();
  const nodes = slice?.nodes ?? [];
  nodes.forEach((n, i) => {
    if (!isDenied(n)) return;
    let prev = i;
    for (let j = n.parent; j !== null && j < prev; j = nodes[j]!.parent) {
      const id = nodes[j]!.node_id;
      out.set(id, (out.get(id) ?? 0) + 1);
      prev = j;
    }
  });
  return out;
}
export const deniedBelowText = (count: number): string => `at least ${formatCount(count)} access denied below`;

/**
 * What a keyboard cursor or an Inspect press says about a region (denied and unknown parts included).
 * The layout number `n.size` turns an unknown size into 0, so a real node's size is read from the slice
 * node itself: unknown, or 0 with access denied or files of unknown allocation, is "size unknown".
 * `below` is `deniedBelow(slice)`; without it only the zero-size cells the renderer lists are counted.
 */
export function describeSize(n: NodeInfo, below?: Map<string, number>): string {
  const src = n.src;
  if (!src) return `${formatApprox(n.size)} · ${n.synthetic === 'fold' ? (n.folded === null ? 'some items folded, count unknown' : `${n.folded} items folded`) : 'not in this slice'}`;
  const z = n.zero.filter((x) => x.coverage === 'denied').length;
  const inside = below?.get(n.id) ?? 0;
  const denied = isDenied(src);
  // the basis is not known here, so files of unknown allocation always count
  const size = sizeUnknown(src, true) ? `size unknown${denied ? ', access denied' : ''}` : formatBytes(src.size_bytes);
  const deniedText = below ? (inside ? ` · ${deniedBelowText(inside)}` : '') : z ? ` · ${z} access denied` : '';
  return `${size}${deniedText}${src.size_unknown_files ? ` · ${src.size_unknown_files} alloc unknown` : ''}`;
}
/** Name, kind, size and the thread states; "Enter opens it" when it can be opened. */
export const describeNode = (n: NodeInfo, below?: Map<string, number>): string => {
  const kind = n.src ? `${kindLabel(n.src.kind)}, ` : '';
  const threads = n.src ? ` · ${threadNotes(n.src).join(' · ')}` : '';
  return `${canvasLabel(n.name)}, ${kind}${describeSize(n, below)}${threads}.${n.drillable ? ' Enter opens it.' : ''}`;
};

/**
 * Names from the start of the trail to `node`, read from the current slice rather than from renderer
 * state (which can lag a slice behind, e.g. a node selected before it was drilled into): the node is
 * found in `slice.nodes` by id and parent indices lead up to the anchor at index 0, whose name is the
 * last crumb already ("All roots"), so it is not repeated. A synthetic cell (a fold or the loose
 * remainder) is not in the slice: it is named after the real node it sits in. Parents precede
 * children, so the walk only moves to a smaller index; a malformed slice (a parent at or after its
 * child, a second root) ends the walk instead of looping.
 */
export function pathTo(node: Pick<NodeInfo, 'id' | 'name' | 'parentId' | 'src'>, slice: SliceLike | null | undefined, trail: string[]): string[] {
  const nodes = slice?.nodes ?? [];
  const find = (id: string | null) => (id === null ? -1 : nodes.findIndex((n) => n.node_id === id));
  const own = node.src ? find(node.id) : -1;
  const below: string[] = own >= 0 ? [] : [node.name];
  let i = own >= 0 ? own : find(node.parentId);
  for (let steps = 0; i > 0 && steps < nodes.length; steps++) {
    below.push(nodes[i]!.name);
    const p = nodes[i]!.parent;
    if (p === null || p >= i) break;
    i = p;
  }
  return [...trail, ...below.reverse()];
}

/** What makes a slice worth announcing again: where it is, on what basis, and whether it is partial or provisional. */
export interface SliceKey { anchor: string; basis: string; provisional: boolean; truncated: boolean }

/**
 * The "Showing ..." line for a newly arrived slice, or null when it is only a refetch of the same view
 * (a tree.invalidated during a scan bumps the epoch up to twice a second: announcing each one would repeat
 * itself and drop the cursor announcements queued behind it). A drill, back, jump or basis change always differs.
 */
export function announceSlice(prev: SliceKey | null, s: SliceLike, name: string): { key: SliceKey; text: string | null } {
  const key: SliceKey = { anchor: s.anchor_node_id, basis: s.basis, provisional: s.aggregate_state === 'provisional_live', truncated: s.truncated };
  const same = prev !== null && prev.anchor === key.anchor && prev.basis === key.basis && prev.provisional === key.provisional && prev.truncated === key.truncated;
  const text = same ? null : `Showing ${canvasLabel(name)}: ${formatCount(s.nodes.length)} nodes${key.truncated ? ', more exist than this slice holds' : ''}${key.provisional ? ', provisional sums from a running scan' : ''}.`;
  return { key, text };
}

export interface Crumb { id: string; name: string; anchor: Anchor }

/**
 * Where the view is: a trail of anchors, the basis, and the current slice. Every change fetches a
 * bounded slice; a response for an older request is dropped, and an error leaves nothing stale on screen.
 */
export class SliceNav {
  trail = $state<Crumb[]>([]);
  slice = $state<TreeSlice | null>(null);
  basis = $state<Basis>('logical');
  error = $state('');
  busy = $state(false);
  private ticket = 0;

  constructor(private readonly opts: { depth: number; maxNodes: number; minShare: number }) {}

  get here(): Crumb | undefined { return this.trail[this.trail.length - 1]; }

  /**
   * Fetch the slice for `trail` and only then make it current, so the breadcrumb always matches
   * what is drawn. On failure the trail stays where it was and the slice is dropped (the view clears
   * the canvas): nothing stale stays on screen or clickable.
   */
  private async go(trail: Crumb[], basis: Basis = this.basis): Promise<boolean> {
    const c = session.client;
    const here = trail[trail.length - 1];
    if (!c || !here) return false;
    const mine = ++this.ticket;
    this.busy = true;
    try {
      const { result } = await c.call('tree.slice', {
        anchor: here.anchor, depth: this.opts.depth, max_nodes: this.opts.maxNodes, min_share: this.opts.minShare, basis, include_files: true,
      });
      if (mine !== this.ticket) return false;
      this.trail = trail;
      this.basis = basis;
      this.slice = result;
      this.error = '';
      return true;
    } catch (e) {
      if (mine !== this.ticket) return false;
      this.error = session.handle(e);
      this.slice = null;
      return false;
    } finally {
      if (mine === this.ticket) this.busy = false;
    }
  }

  /** Retry the current view; with no trail yet (the first load failed) that is the start. */
  load(): Promise<boolean> { return this.trail.length ? this.go(this.trail) : this.start(); }
  start(): Promise<boolean> { return this.go([{ id: 'atlas', name: 'All roots', anchor: { kind: 'atlas' } }]); }
  drill(id: string, name: string): Promise<boolean> {
    if (this.here?.id === id) return Promise.resolve(true);
    return this.go([...this.trail, { id, name, anchor: { kind: 'node', node_id: id } }]);
  }
  back(): Promise<boolean> {
    if (this.trail.length < 2) return Promise.resolve(false);
    return this.go(this.trail.slice(0, -1));
  }
  jump(i: number): Promise<boolean> {
    if (i >= this.trail.length - 1) return Promise.resolve(false);
    return this.go(this.trail.slice(0, i + 1));
  }
  setBasis(b: Basis): Promise<boolean> {
    if (b === this.basis) return Promise.resolve(true);
    return this.go(this.trail, b);
  }
}
