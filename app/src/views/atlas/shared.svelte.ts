// Shared by the Atlas and Observatory views (lane L10): palette from tokens, and slice navigation.
import { formatBytes } from '../../lib/format/bytes';
import { session } from '../../lib/stores/session.svelte';
import type { Anchor, Basis, TreeSlice } from '../../lib/types';
import type { Palette } from '../../../viz/types.js';

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

/** Byte label for a lossy layout number (exact strings are shown wherever the slice has them). */
export const formatApprox = (n: number) => formatBytes(String(Math.max(0, Math.round(n))));

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

  async load(): Promise<void> {
    const c = session.client;
    const here = this.here;
    if (!c || !here) return;
    const mine = ++this.ticket;
    this.busy = true;
    try {
      const { result } = await c.call('tree.slice', {
        anchor: here.anchor, depth: this.opts.depth, max_nodes: this.opts.maxNodes, min_share: this.opts.minShare, basis: this.basis, include_files: true,
      });
      if (mine !== this.ticket) return;
      this.slice = result;
      this.error = '';
    } catch (e) {
      if (mine !== this.ticket) return;
      this.error = session.handle(e);
      this.slice = null;
    } finally {
      if (mine === this.ticket) this.busy = false;
    }
  }

  start(): Promise<void> {
    this.trail = [{ id: 'atlas', name: 'All roots', anchor: { kind: 'atlas' } }];
    return this.load();
  }
  drill(id: string, name: string): Promise<void> {
    if (this.here?.id === id) return Promise.resolve();
    this.trail = [...this.trail, { id, name, anchor: { kind: 'node', node_id: id } }];
    return this.load();
  }
  back(): Promise<void> {
    if (this.trail.length < 2) return Promise.resolve();
    this.trail = this.trail.slice(0, -1);
    return this.load();
  }
  jump(i: number): Promise<void> {
    if (i >= this.trail.length - 1) return Promise.resolve();
    this.trail = this.trail.slice(0, i + 1);
    return this.load();
  }
  setBasis(b: Basis): Promise<void> {
    if (b === this.basis) return Promise.resolve();
    this.basis = b;
    return this.load();
  }
}
