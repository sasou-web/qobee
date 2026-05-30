<script lang="ts">
  // Immersive ambient background.
  //
  // Composition:
  //   * Two stacked copies of the cover, each moderately blurred
  //     and saturation-boosted. They sit at different scales and
  //     rotate in opposite directions on independent slow timers,
  //     so where they overlap the colors mix continuously and the
  //     field never reveals a seam.
  //   * A radial color-boost layer lifts saturation in the center.
  //   * A dark veil keeps foreground text readable.
  //
  // Motion rule: the drift never stops, regardless of playback
  // state. Pausing a track must not snap the field back to its
  // start — that's why no rule below ever rewrites a cover's
  // `animation` property after mount.

  import { coverUrl } from "../lib/api";

  interface Props {
    /** Cover key for the currently displayed track, or null. */
    coverKey: string | null;
    /**
     * Currently unused. Kept on the API so the surrounding view
     * doesn't have to change if we later want to add a *non-
     * destructive* reactive cue (e.g. a separate overlay).
     */
    isPlaying?: boolean;
  }

  let { coverKey }: Props = $props();

  let url = $derived(coverUrl(coverKey ?? null));
  let bgImage = $derived(url ? `url("${url}")` : "none");
  let hasArt = $derived(!!url);
</script>

<div class="ambient" class:has-art={hasArt} aria-hidden="true">
  <!-- Two copies of the cover, scaled and rotated independently.
       Each one carries its own slow timer and direction, so the
       overlap creates continuous color motion. -->
  <div class="cover cover-a" style:background-image={bgImage}></div>
  <div class="cover cover-b" style:background-image={bgImage}></div>

  <!-- Color boost — a soft radial that lifts saturation in the
       center without affecting the edges. -->
  <div class="boost"></div>

  <!-- Fallback wash for when no artwork is available. -->
  <div class="fallback"></div>

  <!-- Dark veil for foreground legibility. -->
  <div class="veil"></div>
</div>

<style>
  .ambient {
    position: absolute;
    inset: 0;
    overflow: hidden;
    pointer-events: none;
    background: #07090c;
    isolation: isolate;
    contain: layout paint;
  }

  /* The blurred cover copies. A generous negative inset gives the
     pan room to roam without ever revealing the dark backdrop at the
     edges. Blur 120px fully dissolves the artwork into pure colour
     fields (Cider-style — no recognizable shapes, just drifting hues);
     saturate + brightness keep the colours vivid as mood lighting. */
  .cover {
    position: absolute;
    inset: -28%;
    background-size: cover;
    background-position: center;
    filter: blur(120px) saturate(190%) brightness(1.02) contrast(105%);
    will-change: transform, opacity, filter;
    transform-origin: 50% 50%;
    transition:
      background-image 800ms ease-out,
      opacity 600ms ease-out;
    opacity: 0;
  }

  .ambient.has-art .cover {
    opacity: 1;
  }

  /* First copy — wanders the corners, zoomed in, on a 38s timer. */
  .cover-a {
    animation: ambient-drift-a 38s ease-in-out infinite;
  }

  /* Second copy — mirrored, drifts on a different 52s path so the two
     colour fields constantly cross and the hues mix continuously. */
  .cover-b {
    transform: scaleX(-1);
    opacity: 0;
    animation: ambient-drift-b 52s ease-in-out infinite;
  }
  .ambient.has-art .cover-b {
    opacity: 0.65;
  }

  /* Color boost — a radial that punches saturation in the center
     using `multiply` blend on top of the saturated covers. */
  .boost {
    position: absolute;
    inset: 0;
    background: radial-gradient(
      ellipse 90% 80% at 50% 45%,
      rgba(255, 255, 255, 0.12) 0%,
      transparent 70%
    );
    mix-blend-mode: overlay;
    pointer-events: none;
    animation: ambient-boost 14s ease-in-out infinite;
  }

  /* Neutral dark wash when no artwork is available. */
  .fallback {
    position: absolute;
    inset: 0;
    background: radial-gradient(
        ellipse 80% 70% at 50% 40%,
        rgba(40, 100, 110, 0.4) 0%,
        transparent 100%
      ),
      linear-gradient(180deg, #0a1014 0%, #050709 100%);
    transition: opacity 600ms ease-out;
  }

  .ambient.has-art .fallback {
    opacity: 0;
  }

  /* Dark veil — strong enough to handle very light or near-white
     artwork. We rely on this veil (not text-shadow) to keep the
     foreground legible, so it has to be opaque enough that white
     text reads against any cover. */
  .veil {
    position: absolute;
    inset: 0;
    background:
      radial-gradient(
        ellipse 115% 100% at 50% 45%,
        rgba(0, 0, 0, 0.4) 0%,
        rgba(0, 0, 0, 0.6) 60%,
        rgba(0, 0, 0, 0.78) 88%,
        rgba(0, 0, 0, 0.88) 100%
      ),
      linear-gradient(
        180deg,
        rgba(0, 0, 0, 0.25) 0%,
        rgba(0, 0, 0, 0.45) 100%
      );
  }

  /* First copy — slow wander across the four corners at a higher
     zoom (scale 1.35–1.55), pure left/right + up/down panning with
     only a hint of rotation. Multi-stop path so it never settles and
     never retraces the same line, the way Cider's immersive field
     keeps roaming. */
  @keyframes ambient-drift-a {
    0% {
      transform: translate3d(-9%, -7%, 0) scale(1.4) rotate(-3deg);
    }
    25% {
      transform: translate3d(8%, -5%, 0) scale(1.5) rotate(2deg);
    }
    50% {
      transform: translate3d(7%, 8%, 0) scale(1.38) rotate(4deg);
    }
    75% {
      transform: translate3d(-7%, 6%, 0) scale(1.52) rotate(-1deg);
    }
    100% {
      transform: translate3d(-9%, -7%, 0) scale(1.4) rotate(-3deg);
    }
  }

  /* Second copy — mirrored, wanders on an offset path and a slower
     timer so the two colour fields drift in and out of overlap. */
  @keyframes ambient-drift-b {
    0% {
      transform: scaleX(-1) translate3d(7%, 6%, 0) scale(1.5) rotate(3deg);
    }
    25% {
      transform: scaleX(-1) translate3d(-8%, 7%, 0) scale(1.38) rotate(-2deg);
    }
    50% {
      transform: scaleX(-1) translate3d(-6%, -8%, 0) scale(1.55) rotate(-4deg);
    }
    75% {
      transform: scaleX(-1) translate3d(8%, -6%, 0) scale(1.4) rotate(1deg);
    }
    100% {
      transform: scaleX(-1) translate3d(7%, 6%, 0) scale(1.5) rotate(3deg);
    }
  }

  /* Boost — gentle position shift so the saturation hot-spot
     migrates with the cover, never sitting still. */
  @keyframes ambient-boost {
    0%,
    100% {
      transform: translate3d(-3%, -2%, 0) scale(1);
      opacity: 0.85;
    }
    50% {
      transform: translate3d(3%, 2%, 0) scale(1.06);
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .cover,
    .boost {
      animation: none !important;
    }
  }
</style>
