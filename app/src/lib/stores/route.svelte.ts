const search = () => {
  const at = location.hash.indexOf('?');
  return at < 0 ? '' : location.hash.slice(at + 1);
};
const parse = () => /^#\/([a-z0-9-]+)/.exec(location.hash)?.[1] ?? '';

/** Hash route: `#/<view-directory-name>`. */
class Route {
  name = $state(parse());
  search = $state(search());
  constructor() {
    window.addEventListener('hashchange', () => {
      this.name = parse();
      this.search = search();
    });
  }
  go(name: string): void {
    location.hash = `#/${name}`;
  }
}

export const route = new Route();
