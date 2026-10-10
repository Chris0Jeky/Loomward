// Shared by the Atlas and Observatory views (lane L10): palette from tokens, and slice navigation.
import { formatBytes } from '../../lib/format/bytes';
import { escapedName } from '../../lib/format/names';
import { session } from '../../lib/stores/session.svelte';
import type { Anchor, Basis, TreeSlice } from '../../lib/contracts.gen';
import type { NodeInfo, Palette, SliceLike } from '../../../viz/types.js';

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

/** What a keyboard cursor or an Inspect press says about a region (denied and unknown parts included). */
export function describeSize(n: NodeInfo): string {
  const src = n.src;
  if (!src) return `${formatApprox(n.size)} · ${n.synthetic === 'fold' ? (n.folded === null ? 'some items folded, count unknown' : `${n.folded} items folded`) : 'not in this slice'}`;
  const z = n.zero.filter((x) => x.coverage === 'denied').length;
  return `${formatApprox(n.size)}${z ? ` · ${z} access denied` : ''}${src.size_unknown_files ? ` · ${src.size_unknown_files} alloc unknown` : ''}`;
}
export const describeNode = (n: NodeInfo): string => `${canvasLabel(n.name)}, ${describeSize(n)}.${n.drillable ? ' Enter opens it.' : ''}`;

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
