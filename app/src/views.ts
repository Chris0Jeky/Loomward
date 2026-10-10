import type { Component } from 'svelte';

export interface ViewMeta {
  title: string;
  /** Sort key in the navigation; lower first. */
  order: number;
}
interface ViewModule {
  default: Component;
  meta?: Partial<ViewMeta>;
}

// The shell never imports a view by name: every `views/<name>/View.svelte` is discovered here, and
// `<name>` is its route (`#/<name>`). A view may export `meta` from `<script module>`.
const modules = import.meta.glob<ViewModule>('./views/*/View.svelte', { eager: true });

export interface View extends ViewMeta {
  name: string;
  component: Component;
}

export const views: View[] = Object.entries(modules)
  .map(([path, m]) => {
    const name = /\/views\/([^/]+)\/View\.svelte$/.exec(path)![1]!;
    return {
      name,
      component: m.default,
      title: m.meta?.title ?? name.charAt(0).toUpperCase() + name.slice(1),
      order: m.meta?.order ?? 100,
    };
  })
  .sort((a, b) => a.order - b.order || a.name.localeCompare(b.name));
