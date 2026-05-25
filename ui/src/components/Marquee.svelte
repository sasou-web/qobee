<script lang="ts">
  import { onMount } from "svelte";

  // Single-line text that shows an ellipsis when it fits its
  // container, and gently scrolls back-and-forth on hover when it
  // doesn't. Intended to be wrapped in an element carrying the
  // class `marquee-host` (typically the card the user hovers).

  interface Props {
    text: string;
    /** Extra classes forwarded to the outer element. */
    class?: string;
  }

  let { text, class: className = "" }: Props = $props();

  let outer: HTMLDivElement | undefined = $state();
  let inner: HTMLSpanElement | undefined = $state();
  let distance = $state(0);
  let duration = $state(8);
  let overflow = $state(false);

  // Constant scroll speed feels more natural than a fixed duration:
  // a slightly-too-long title scrolls fast, a way-too-long one takes
  // longer. Pixels per second of marquee travel.
  const SPEED_PX_PER_S = 55;

  function measure(): void {
    if (!outer || !inner) return;
    const oW = outer.clientWidth;
    const iW = inner.scrollWidth;
    const diff = iW - oW;
    if (diff > 1) {
      overflow = true;
      distance = -diff;
      duration = Math.max(4, Math.min(20, (diff / SPEED_PX_PER_S) * 2 + 2));
    } else {
      overflow = false;
      distance = 0;
    }
  }

  onMount(() => {
    measure();
    if (!outer || !inner) return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(outer);
    ro.observe(inner);
    return () => ro.disconnect();
  });

  $effect(() => {
    void text;
    queueMicrotask(measure);
  });
</script>

<div
  class={`marquee ${className}`}
  class:overflow
  bind:this={outer}
  style:--marquee-d={`${distance}px`}
  style:--marquee-dur={`${duration}s`}
>
  <span class="inner" bind:this={inner}>{text}</span>
</div>

<style>
  .marquee {
    display: block;
    overflow: hidden;
    white-space: nowrap;
    width: 100%;
    max-width: 100%;
    /* Default: clip with ellipsis. The ellipsis sits on the .marquee
       container; the inner span is treated as plain inline content. */
    text-overflow: ellipsis;
  }
  .inner {
    display: inline-block;
    white-space: nowrap;
    will-change: transform;
    /* Critical: when content fits, max-width caps the inline-block
       so the parent's text-overflow can render the ellipsis. When
       content overflows (animation case), the inner is allowed to
       extend past the parent so it can slide into view. */
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    vertical-align: bottom;
  }
  .marquee.overflow .inner {
    max-width: none;
    overflow: visible;
  }
  /* Animate only when an ancestor opted in via `.marquee-host`. */
  :global(.marquee-host:hover) .marquee.overflow .inner {
    animation: marquee var(--marquee-dur, 8s) linear infinite;
  }
  @keyframes marquee {
    0%, 15% { transform: translateX(0); }
    55%, 70% { transform: translateX(var(--marquee-d)); }
    100% { transform: translateX(0); }
  }
</style>
