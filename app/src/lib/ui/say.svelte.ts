/**
 * Text for a persistent live region. `say` empties the region and fills it on a later task, so an
 * identical repeat ("no region to the right" twice) is announced again; a region that is created
 * already filled is not announced by many screen readers, which is why it stays in the page.
 */
export class Say {
  text = $state('');
  private timer: ReturnType<typeof setTimeout> | undefined;

  say(msg: string): void {
    clearTimeout(this.timer);
    this.text = '';
    this.timer = setTimeout(() => { this.text = msg; }, 60);
  }

  clear(): void {
    clearTimeout(this.timer);
    this.text = '';
  }
}
