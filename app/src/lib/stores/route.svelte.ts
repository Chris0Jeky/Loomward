const parse = () => /^#\/([a-z0-9-]+)/.exec(location.hash)?.[1] ?? '';

/** Hash route: `#/<view-directory-name>`. */
class Route {
  name = $state(parse());
  search = $state(location.hash.split('?')[1] ?? '');
  constructor() {
    window.addEventListener('hashchange', () => {
      this.name = parse();
      this.search = location.hash.split('?')[1] ?? '';
    });
  }
  go(name: string): void {
    location.hash = `#/${name}`;
  }
}

export const route = new Route();
