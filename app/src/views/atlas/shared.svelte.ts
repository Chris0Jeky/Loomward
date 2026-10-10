// Shared by the Atlas and Observatory views (lane L10): palette from tokens, and slice navigation.
import { formatBytes } from '../../lib/format/bytes';
import { escapedName } from '../../lib/format/names';
import { session } from '../../lib/stores/session.svelte';
import type { Anchor, Basis, TreeSlice } from '../../lib/types';
import type { NodeInfo, Palette } from '../../../viz/types.js';

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

/**
 * Names from the start of the trail to `node`, every directory between the slice anchor and a deep
 * descendant included. Parent links come from the renderer's `info`; the slice root is the last crumb
 * (it keeps the crumb's label, e.g. "All roots"), so it is not repeated.
 */
export function pathTo(node: NodeInfo, info: (id: string) => NodeInfo | null, trail: string[]): string[] {
  const below: string[] = [];
  for (let n: NodeInfo | null = node; n?.parentId; n = info(n.parentId)) below.push(n.name);
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
