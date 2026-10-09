const parse = () => /^#\/([a-z0-9-]+)/.exec(location.hash)?.[1] ?? '';

/** Hash route: `#/<view-directory-name>`. */
class Route {
  name = $state(parse());
  constructor() {
    window.addEventListener('hashchange', () => (this.name = parse()));
  }
  go(name: string): void {
    location.hash = `#/${name}`;
  }
}

export const route = new Route();
