<script lang="ts">
  import { coverUrl } from "../lib/api";

  interface Props {
    coverKey: string | null | undefined;
    size?: number;
    /** Used as a fallback "letter art" when no cover is available. */
    title?: string;
  }

  let { coverKey, size = 160, title = "" }: Props = $props();

  // The protocol returns the file from the on-disk cache; raw image
  // bytes never go through IPC. See `qobee-cover` registration in
  // `src-tauri/src/lib.rs`.
  let url = $derived(coverUrl(coverKey ?? null));
  let initials = $derived(
    title
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => (w[0] ?? "").toUpperCase())
      .join("")
  );

  let loaded = $state(false);

  // Reset the loaded flag whenever the URL changes so the new image
  // gets its own fade-in.
  $effect(() => {
    void url;
    loaded = false;
  });
</script>

<div class="cover" style:width={`${size}px`} style:height={`${size}px`}>
  {#if url}
    <img
      src={url}
      alt={title}
      loading="lazy"
      decoding="async"
      class:loaded
      onload={() => (loaded = true)}
    />
  {:else}
    <span class="placeholder">{initials || "?"}</span>
  {/if}
</div>

<style>
  .cover {
    position: relative;
    background: var(--bg-2);
    border-radius: var(--radius-lg);
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: var(--shadow-1);
    flex-shrink: 0;
    transition: transform var(--dur-base) var(--ease-out),
      box-shadow var(--dur-base) var(--ease-out);
  }
  /* Image starts slightly zoomed and translucent, settles in once
     the browser fires `load`. Cheap, works with the lazy loader. */
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    opacity: 0;
    transform: scale(1.04);
    transition: opacity var(--dur-slow) var(--ease-out),
      transform 320ms var(--ease-out);
  }
  img.loaded {
    opacity: 1;
    transform: scale(1);
  }
  .placeholder {
    font-size: 28px;
    font-weight: 600;
    color: var(--fg-2);
    letter-spacing: 0.05em;
  }
  /* Parent components opt into the hover zoom by adding the
     `.lift` class on their wrapping element. */
  :global(.lift:hover) .cover {
    transform: translateY(-3px);
    box-shadow: var(--shadow-lift);
  }
  :global(.lift:hover) img {
    transform: scale(1.06);
  }
</style>
