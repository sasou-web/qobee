// Tiny global store for the right-click context menu.
//
// One menu lives at the root of the app (rendered by ContextMenuRoot)
// and any component can call `contextMenu.open(event, items)` from a
// `oncontextmenu` handler. Items are simple {label, onSelect, danger?}
// records; separators are represented by `null`.

export interface ContextMenuItem {
  label: string;
  onSelect: () => void | Promise<void>;
  danger?: boolean;
  disabled?: boolean;
}

export type ContextMenuEntry = ContextMenuItem | "separator";

interface ContextMenuState {
  open: boolean;
  x: number;
  y: number;
  items: ContextMenuEntry[];
}

class ContextMenuStore {
  state = $state<ContextMenuState>({ open: false, x: 0, y: 0, items: [] });

  /**
   * Open the menu anchored to the mouse position from `event`. Calls
   * `preventDefault` so the browser's native context menu doesn't show.
   */
  open(event: MouseEvent, items: ContextMenuEntry[]): void {
    event.preventDefault();
    event.stopPropagation();
    this.state = {
      open: true,
      x: event.clientX,
      y: event.clientY,
      items,
    };
  }

  close(): void {
    this.state = { ...this.state, open: false };
  }
}

export const contextMenu = new ContextMenuStore();
