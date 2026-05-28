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

  /* The blurred cover copies. Inset covers the diagonal of the
     rotated rectangle (~16% slack at our max angle).
     Blur 90px is the sweet spot: enough to fully erase artwork
     structure (no recognizable shapes), but not so much that all
     hues collapse into a single tint. Saturate + brightness lift
     the colors so the field reads as a vivid mood lighting. */
  .cover {
    position: absolute;
    inset: -16%;
    background-size: cover;
    background-position: center;
    filter: blur(90px) saturate(200%) brightness(1) contrast(108%);
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

  /* First copy — pans and rotates clockwise on a 22s timer. */
  .cover-a {
    animation: ambient-spin-a 22s ease-in-out infinite alternate;
  }

  /* Second copy — mirrored, rotates counter-clockwise on a 28s
     timer. The mirror plus the opposite rotation means the two
     color fields constantly cross each other. */
  .cover-b {
    transform: scaleX(-1);
    opacity: 0;
    animation: ambient-spin-b 28s ease-in-out infinite alternate;
  }
  .ambient.has-art .cover-b {
    opacity: 0.7;
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

  /* First copy — pan + zoom + rotation clockwise. The scale is
     kept modest so the cover stays "atmospheric" rather than
     pushed against the screen; the rotation (±20°) is what
     makes the color field actually spin. */
  @keyframes ambient-spin-a {
    0% {
      transform: translate3d(-3%, -2%, 0) scale(0.95) rotate(-12deg);
    }
    100% {
      transform: translate3d(4%, 3%, 0) scale(1.1) rotate(20deg);
    }
  }

  /* Second copy — mirrored, counter-rotates. */
  @keyframes ambient-spin-b {
    0% {
      transform: scaleX(-1) translate3d(-2%, 3%, 0) scale(1.05)
        rotate(15deg);
    }
    100% {
      transform: scaleX(-1) translate3d(3%, -2%, 0) scale(0.92)
        rotate(-22deg);
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
