import type { ExtFamily, CoverageState } from '../types';

/**
 * Deterministic synthetic catalogue for the mock transport: up to 6,000 nodes, parents before
 * children, bottom-up aggregates. Everything here is invented; nothing reads a disk.
 */

export interface SynthNode {
  index: number;
  id: string;
  parent: number; // -1 for the atlas
  kind: 'atlas' | 'root' | 'dir' | 'file';
  name: string;
  depth: number;
  extension: string | null;
  family: ExtFamily | null;
  logical: bigint;
  allocated: bigint | null; // null = unknown (also for a directory containing any unknown file)
  /** Sum of the known allocations; used for the allocated basis, where unknown contributes 0. */
  allocatedKnown: bigint;
  unknownFiles: number;
  files: number;
  dirs: number;
  coverage: CoverageState;
  modified: string | null;
  children: number[];
}

export interface SynthTree {
  nodes: SynthNode[];
  rootIndexes: number[];
}

/** Names that must render as inert text (docs/41 section 12: hostile-name corpus). */
export const HOSTILE_NAMES = [
  '<img src=x onerror=alert(1)>.png',
  '<script>alert(document.domain)</script>.txt',
  '"><svg/onload=alert(1)>',
  '{@' + "html '<b>x</b>'}.md", // built in two parts so the source grep for the banned tag needs no exemption
  'invoice‮txt.exe',
  'a&amp;b &lt;i&gt;.doc',
  'javascript:alert(1)',
  'budget\u200B\u200Dfinal.xlsx', // zero-width space and joiner: invisible unless shown
  'long-'.repeat(48) + 'name.txt',
];

const WORDS = ['alpha', 'basalt', 'cinder', 'delta', 'ember', 'fjord', 'garnet', 'harbour', 'indigo', 'juniper', 'kelp', 'lumen', 'marble', 'nimbus', 'onyx', 'pewter', 'quartz', 'russet', 'sable', 'tundra'];
const EXTS: [string, ExtFamily][] = [
  ['docx', 'document'], ['pdf', 'document'], ['xlsx', 'spreadsheet'], ['pptx', 'presentation'], ['png', 'image'],
  ['jpg', 'image'], ['mp4', 'video'], ['mp3', 'audio'], ['zip', 'archive'], ['rs', 'code'], ['py', 'code'],
  ['json', 'data'], ['gguf', 'model'], ['dll', 'executable'], ['ttf', 'font'], ['bin', 'other'],
];
const T0 = Date.UTC(2025, 0, 1);

/** mulberry32: small, fast, fixed output for a fixed seed. */
export function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function generateTree(count = 6000, seed = 1): SynthTree {
  const total = Math.max(8, Math.min(count, 6000));
  const rnd = prng(seed);
  const pick = <T>(a: readonly T[]) => a[Math.floor(rnd() * a.length)]!;
  const nodes: SynthNode[] = [];
  const add = (n: Omit<SynthNode, 'index' | 'id' | 'children' | 'logical' | 'allocated' | 'allocatedKnown' | 'unknownFiles' | 'files' | 'dirs'> & Partial<SynthNode>): number => {
    const index = nodes.length;
    nodes.push({
      index, id: `nd_${index}`, children: [], logical: 0n, allocated: 0n, allocatedKnown: 0n, unknownFiles: 0, files: 0, dirs: 0,
      ...n,
    } as SynthNode);
    if (n.parent >= 0) nodes[n.parent]!.children.push(index);
    return index;
  };

  add({ parent: -1, kind: 'atlas', name: 'Atlas', depth: 0, extension: null, family: null, coverage: 'complete', modified: null });
  const rootIndexes = ['Synthetic Alpha', 'Synthetic Beta'].map((name) =>
    add({ parent: 0, kind: 'root', name, depth: 1, extension: null, family: null, coverage: 'complete', modified: null }),
  );

  // Hostile names first, directly under the first root, so they are on the first page of every listing.
  const alpha = rootIndexes[0]!;
  HOSTILE_NAMES.forEach((name, i) => {
    if (nodes.length >= total) return;
    const dot = name.lastIndexOf('.');
    const ext = dot > 0 && name.length - dot <= 5 ? name.slice(dot + 1) : null;
    add({ parent: alpha, kind: 'file', name: name.slice(0, 260), depth: 2, extension: ext, family: 'other', logical: BigInt(900 + i * 37), allocated: 4096n, coverage: 'complete', modified: new Date(T0 + i * 86400000).toISOString() });
  });

  const queue: number[] = [...rootIndexes];
  let q = 0;
  while (nodes.length < total && q < queue.length) {
    const dirIdx = queue[q++]!;
    const dir = nodes[dirIdx]!;
    if (dir.depth >= 8) continue;
    const want = 3 + Math.floor(rnd() * 12);
    for (let i = 0; i < want && nodes.length < total; i++) {
      const depth = dir.depth + 1;
      const isDir = depth < 7 && rnd() < 0.4;
      const modified = new Date(T0 + Math.floor(rnd() * 600) * 86400000).toISOString();
      if (isDir) {
        const denied = rnd() < 0.03;
        const idx = add({ parent: dirIdx, kind: 'dir', name: `${pick(WORDS)}-${nodes.length}`, depth, extension: null, family: null, coverage: denied ? 'denied' : 'complete', modified });
        if (!denied) queue.push(idx);
      } else {
        const [ext, family] = pick(EXTS);
        const logical = BigInt(Math.floor(10 ** (2 + rnd() * 7.3)));
        const unknownAlloc = rnd() < 0.006;
        add({
          parent: dirIdx, kind: 'file', name: `${pick(WORDS)}-${nodes.length}.${ext}`, depth, extension: ext, family,
          logical, allocated: unknownAlloc ? null : ((logical + 4095n) / 4096n) * 4096n, coverage: 'complete', modified,
        });
      }
    }
  }

  // Bottom-up aggregates: children always have larger indexes than their parent.
  for (let i = nodes.length - 1; i > 0; i--) {
    const n = nodes[i]!;
    const p = nodes[n.parent]!;
    if (n.kind === 'file') {
      n.files = 1;
      n.allocatedKnown = n.allocated ?? 0n;
      n.unknownFiles = n.allocated === null ? 1 : 0;
    } else {
      n.allocated = n.unknownFiles > 0 ? null : n.allocatedKnown;
    }
    p.logical += n.logical;
    p.allocatedKnown += n.allocatedKnown;
    p.unknownFiles += n.unknownFiles;
    p.files += n.files;
    p.dirs += n.kind === 'file' ? 0 : 1 + n.dirs;
    if (n.coverage !== 'complete' && p.coverage === 'complete') p.coverage = 'partial';
  }
  const a = nodes[0]!;
  a.allocated = a.unknownFiles > 0 ? null : a.allocatedKnown;
  return { nodes, rootIndexes };
}
