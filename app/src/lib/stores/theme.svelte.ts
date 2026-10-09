export const THEMES = ['woven-atlas', 'observatory'] as const;
export type Theme = (typeof THEMES)[number];

const KEY = 'loomward.theme';

function initial(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'woven-atlas' || v === 'observatory') return v;
  } catch { /* storage may be blocked; the default applies */ }
  return 'woven-atlas';
}

/** Two token sets in styles/tokens.css, switched by `data-theme` on <html>. */
class ThemeStore {
  current = $state<Theme>(initial());
  constructor() {
    document.documentElement.dataset.theme = this.current;
  }
  set(t: Theme): void {
    this.current = t;
    document.documentElement.dataset.theme = t;
    try { localStorage.setItem(KEY, t); } catch { /* per-viewer convenience only */ }
  }
}

export const theme = new ThemeStore();
