// Lightweight global store for the "Properties" modal.
//
// Three tabs: Details (metadata + audio traits), Artists (one row per
// participant, click-to-navigate), and Artwork (cover preview with an
// extracted color palette). Each context-menu entry pushes a payload
// here; the dialog renders all three tabs from the same shape.

export interface PropertiesField {
  label: string;
  value: string;
  /** Monospace rendering for paths, hashes, raw IDs. */
  monospace?: boolean;
}

export interface ArtistEntry {
  name: string;
  /** "Artist" / "Album artist" / "Composer"… */
  role: string;
  /** Click handler. When omitted the row is rendered as plain text. */
  onSelect?: () => void;
}

export interface PropertiesPayload {
  title: string;
  subtitle?: string;
  /** Path-or-URL key for the cover. Drives the Artwork tab and the
   *  small thumbnail next to the header. `null` when the item has no
   *  embedded artwork (the Artwork tab degrades gracefully). */
  coverKey: string | null;
  /** Generic metadata: title, year, genre, file size… */
  details: PropertiesField[];
  /** Codec / sample rate / bit depth / channels — anything that
   *  describes the *audio*, separated so the UI can present it as a
   *  distinct group. */
  audioTraits: PropertiesField[];
  artists: ArtistEntry[];
}

export type PropertiesTab = "details" | "artists" | "artwork";

interface PropertiesState {
  open: boolean;
  payload: PropertiesPayload | null;
  tab: PropertiesTab;
}

class PropertiesDialogStore {
  state = $state<PropertiesState>({ open: false, payload: null, tab: "details" });

  show(payload: PropertiesPayload, initialTab: PropertiesTab = "details"): void {
    this.state = { open: true, payload, tab: initialTab };
  }

  setTab(tab: PropertiesTab): void {
    this.state = { ...this.state, tab };
  }

  close(): void {
    this.state = { ...this.state, open: false };
  }
}

export const propertiesDialog = new PropertiesDialogStore();
