<script lang="ts">
  // Tiny inline-SVG icon set. Keeping all glyphs in one component avoids
  // pulling in an icon font and lets each consumer size them via CSS
  // `font-size` (we use `1em` for stroke width consistency).

  type IconName =
    | "play"
    | "pause"
    | "next"
    | "prev"
    | "library"
    | "artists"
    | "albums"
    | "scan"
    | "volume"
    | "home"
    | "search"
    | "genres"
    | "playlist"
    | "settings"
    | "shuffle"
    | "repeat"
    | "repeat-one"
    | "heart"
    | "heart-filled"
    | "queue"
    | "trash"
    | "drag"
    | "infinity"
    | "folder-plus"
    | "music"
    | "zap"
    | "shield"
    | "expand"
    | "compress"
    | "quote"
    | "minimize-2"
    | "plus";

  interface Props {
    name: IconName;
    size?: number;
  }

  let { name, size = 16 }: Props = $props();
</script>

{#if name === "play"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M8 5.5c0-1 1.1-1.6 1.95-1.1l9.9 6c.83.5.83 1.7 0 2.2l-9.9 6C9.1 19.1 8 18.5 8 17.5v-12z" />
  </svg>
{:else if name === "pause"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <rect x="6.5" y="4.5" width="3.8" height="15" rx="1.6" />
    <rect x="13.7" y="4.5" width="3.8" height="15" rx="1.6" />
  </svg>
{:else if name === "next"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M3.4 6.6c0-.88.97-1.42 1.71-.96l7.4 4.6a1.13 1.13 0 0 1 0 1.92l-7.4 4.6c-.74.46-1.71-.08-1.71-.96V6.6z" />
    <path d="M12.4 6.6c0-.88.97-1.42 1.71-.96l7.4 4.6a1.13 1.13 0 0 1 0 1.92l-7.4 4.6c-.74.46-1.71-.08-1.71-.96V6.6z" />
  </svg>
{:else if name === "prev"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M20.6 6.6c0-.88-.97-1.42-1.71-.96l-7.4 4.6a1.13 1.13 0 0 0 0 1.92l7.4 4.6c.74.46 1.71-.08 1.71-.96V6.6z" />
    <path d="M11.6 6.6c0-.88-.97-1.42-1.71-.96l-7.4 4.6a1.13 1.13 0 0 0 0 1.92l7.4 4.6c.74.46 1.71-.08 1.71-.96V6.6z" />
  </svg>
{:else if name === "heart"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M12 20.5c-.6 0-1.1-.2-1.5-.6l-7-7C2.2 11.6 1.7 10.3 1.7 9c0-1.4.5-2.6 1.5-3.6 2-2 5.2-2 7.2 0L12 6.6l1.5-1.2c2-2 5.2-2 7.2 0 1 1 1.5 2.2 1.5 3.6 0 1.3-.5 2.6-1.5 3.6L13.5 20c-.4.3-.9.5-1.5.5z" />
  </svg>
{:else if name === "heart-filled"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <path d="M12 20.5c-.6 0-1.1-.2-1.5-.6l-7-7C2.2 11.6 1.7 10.3 1.7 9c0-1.4.5-2.6 1.5-3.6 2-2 5.2-2 7.2 0L12 6.6l1.5-1.2c2-2 5.2-2 7.2 0 1 1 1.5 2.2 1.5 3.6 0 1.3-.5 2.6-1.5 3.6L13.5 20c-.4.3-.9.5-1.5.5z" />
  </svg>
{:else if name === "quote"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <line x1="5" y1="7" x2="19" y2="7" />
    <line x1="5" y1="12" x2="19" y2="12" />
    <line x1="5" y1="17" x2="13" y2="17" />
  </svg>
{:else if name === "compress"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M9 3v4a2 2 0 0 1-2 2H3" />
    <path d="M15 3v4a2 2 0 0 0 2 2h4" />
    <path d="M9 21v-4a2 2 0 0 0-2-2H3" />
    <path d="M15 21v-4a2 2 0 0 1 2-2h4" />
  </svg>
{:else if name === "library"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d="M4 19V5a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v14" />
    <path d="M4 19h16" />
    <path d="M9 7v10" />
    <path d="M14 7v10" />
  </svg>
{:else if name === "artists"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <circle cx="12" cy="8" r="4" />
    <path d="M4 21c0-4 4-6 8-6s8 2 8 6" />
  </svg>
{:else if name === "albums"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="3" />
  </svg>
{:else if name === "scan"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d="M3 7V5a2 2 0 0 1 2-2h2" />
    <path d="M21 7V5a2 2 0 0 0-2-2h-2" />
    <path d="M3 17v2a2 2 0 0 0 2 2h2" />
    <path d="M21 17v2a2 2 0 0 1-2 2h-2" />
    <path d="M7 12h10" />
  </svg>
{:else if name === "volume"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M11 5L6.5 9H3v6h3.5L11 19V5z" fill="currentColor" stroke="none" />
    <path d="M15.5 8.5a5 5 0 0 1 0 7" />
    <path d="M18.5 6a8 8 0 0 1 0 12" />
  </svg>
{:else if name === "home"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d="M3 11l9-8 9 8" />
    <path d="M5 10v10h14V10" />
  </svg>
{:else if name === "search"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <circle cx="11" cy="11" r="7" />
    <path d="m20 20-3.5-3.5" />
  </svg>
{:else if name === "genres"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d="M9 18V5l12-2v13" />
    <circle cx="6" cy="18" r="3" />
    <circle cx="18" cy="16" r="3" />
  </svg>
{:else if name === "playlist"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d="M3 6h13M3 12h13M3 18h9" />
    <path d="M16 14v6l5-3-5-3z" fill="currentColor" stroke="none" />
  </svg>
{:else if name === "settings"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </svg>
{:else if name === "shuffle"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M3 7h3.5c1 0 1.9.5 2.5 1.3l5.5 7.4c.6.8 1.5 1.3 2.5 1.3H21" />
    <path d="M3 17h3.5c1 0 1.9-.5 2.5-1.3l1.5-2" />
    <path d="M14.5 8.3 16 7c.6-.5 1.3-.7 2-.7H21" />
    <polyline points="18 4 21 7 18 10" />
    <polyline points="18 14 21 17 18 20" />
  </svg>
{:else if name === "repeat"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M4 12V8a3 3 0 0 1 3-3h11" />
    <polyline points="15 2 18 5 15 8" />
    <path d="M20 12v4a3 3 0 0 1-3 3H6" />
    <polyline points="9 22 6 19 9 16" />
  </svg>
{:else if name === "repeat-one"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M4 12V8a3 3 0 0 1 3-3h11" />
    <polyline points="15 2 18 5 15 8" />
    <path d="M20 12v4a3 3 0 0 1-3 3H6" />
    <polyline points="9 22 6 19 9 16" />
    <path d="M11.6 11l1.4-.6V14" stroke-width="2.4" />
  </svg>
{:else if name === "queue"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <line x1="3" y1="7" x2="14" y2="7" />
    <line x1="3" y1="12" x2="14" y2="12" />
    <line x1="3" y1="17" x2="11" y2="17" />
    <path d="M16 17V8.4l5-1.4v9" />
    <circle cx="14.5" cy="17" r="1.5" fill="currentColor" stroke="none" />
    <circle cx="19.5" cy="15.6" r="1.5" fill="currentColor" stroke="none" />
  </svg>
{:else if name === "trash"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <polyline points="3 6 5 6 21 6" />
    <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
    <path d="M10 11v6" />
    <path d="M14 11v6" />
    <path d="M9 6V4a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v2" />
  </svg>
{:else if name === "drag"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
    <circle cx="9" cy="6" r="1.5" />
    <circle cx="15" cy="6" r="1.5" />
    <circle cx="9" cy="12" r="1.5" />
    <circle cx="15" cy="12" r="1.5" />
    <circle cx="9" cy="18" r="1.5" />
    <circle cx="15" cy="18" r="1.5" />
  </svg>
{:else if name === "infinity"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M18.178 8c5.096 0 5.096 8 0 8-5.095 0-7.005-8-12.1-8-5.097 0-5.097 8 0 8 5.095 0 7.005-8 12.1-8z" />
  </svg>
{:else if name === "folder-plus"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
    <line x1="12" y1="11" x2="12" y2="17" />
    <line x1="9" y1="14" x2="15" y2="14" />
  </svg>
{:else if name === "music"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M9 18V5l12-2v13" />
    <circle cx="6" cy="18" r="3" />
    <circle cx="18" cy="16" r="3" />
  </svg>
{:else if name === "zap"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
  </svg>
{:else if name === "shield"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
  </svg>
{:else if name === "expand"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <polyline points="15 3 21 3 21 9" />
    <polyline points="9 21 3 21 3 15" />
    <line x1="21" y1="3" x2="14" y2="10" />
    <line x1="3" y1="21" x2="10" y2="14" />
  </svg>
{:else if name === "minimize-2"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <polyline points="4 14 10 14 10 20" />
    <polyline points="20 10 14 10 14 4" />
  </svg>
{:else if name === "plus"}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <line x1="12" y1="5" x2="12" y2="19" />
    <line x1="5" y1="12" x2="19" y2="12" />
  </svg>
{/if}
