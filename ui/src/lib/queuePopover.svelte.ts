// Tiny store for the "queue" popover anchored above the PlayerBar.
//
// The PlayerBar's queue button toggles this; the popover component
// listens to `open` and renders itself when true. Click-outside and
// Escape close it.

class QueuePopoverStore {
  open = $state<boolean>(false);

  toggle(): void {
    this.open = !this.open;
  }

  show(): void {
    this.open = true;
  }

  close(): void {
    this.open = false;
  }
}

export const queuePopover = new QueuePopoverStore();
