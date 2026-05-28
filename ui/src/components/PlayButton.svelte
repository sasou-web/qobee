<script lang="ts">
  // R3 Play_Button — single component used everywhere a "play this
  // thing now" affordance shows up: track rows, album cards / album
  // detail, mini-player, the player bar.
  //
  // Invariants (R3.1, R3.2):
  //  - The button is ALWAYS a real <button type="button">, never an
  //    <a>. Parents that care about navigation (track row, album
  //    card) are themselves clickable; a click on us must NOT
  //    bubble up and trigger that navigation, hence the explicit
  //    e.stopPropagation() + e.preventDefault() in the handler.
  //  - We never call any router function. The only side effect of
  //    a click is `invoke()` against a transport command.
  //
  // State derivation (R3.3, R3.4, R3.5, R3.6):
  //  - `isCurrent` is whether this UI target points at the track
  //    currently loaded by the engine.
  //  - The visible state mixes `isCurrent` with `playerStore.status`
  //    and the scoped `lastError`. The error state is ONLY raised
  //    when the engine reported an error AND that error pointed at
  //    our target.id (R3.6). No other signal — loading, navigation,
  //    network — lights up the error state.
  //
  // Click routing:
  //  - `playing` (we own the engine) → `pause`.
  //  - `isCurrent && paused` → `resume`.
  //  - otherwise → start fresh playback for the right kind.

  import {
    pause as apiPause,
    playAlbum,
    playPlaylist,
    playTrack,
    resume as apiResume,
  } from "../lib/api";
  import { playerStore, type PlayerTarget } from "../lib/playerStore.svelte";
  import Icon from "./Icon.svelte";

  type Size = "sm" | "md" | "lg";

  interface Props {
    target: PlayerTarget;
    size?: Size;
    /** Optional override for the visual variant. Mostly used by
     *  the mini-player / player bar where the button sits on a
     *  surface that already provides its own background and we
     *  don't want a second pill. Currently informational only —
     *  callers can set their own classes via `class` if needed. */
    class?: string;
  }

  let { target, size = "md", class: extraClass = "" }: Props = $props();

  /** Start a fresh playback for the right kind. We route through
   *  the typed wrappers in `lib/api.ts` rather than raw `invoke`
   *  so the argument shapes stay in lockstep with the Rust side. */
  function startPlayback(): Promise<void> {
    switch (target.kind) {
      case "track":
        return playTrack(target.id);
      case "album":
        return playAlbum(target.id);
      case "playlist":
        return playPlaylist(target.id);
    }
  }

  // Local "we just clicked, waiting for the engine" guard. Held
  // briefly to swallow double-clicks (the click handler returns
  // early if `pending`). It does NOT drive any visual state — we
  // used to render a spinner ring during this window, but it
  // looked like buffering on a fast local file (the `Started`
  // event arrives in ~50 ms) and confused users on album/playlist
  // buttons (where `isCurrent` is always false, see below). The
  // visual stays `idle` until the bus confirms playback.
  let pending: boolean = $state(false);

  // Whether this button "owns" the engine right now.
  //
  // The store's `matches()` only compares the *track* id. That
  // works for `kind === "track"` but is misleading for albums and
  // playlists: the engine doesn't expose a `current_album_id` /
  // `current_playlist_id` today, so we can't tell whether "the
  // album currently playing" matches "this album button". Rather
  // than guess (an album id might numerically collide with the
  // currently playing track id), we treat album / playlist
  // buttons as "always start fresh" — they never enter the
  // playing / paused states, never offer pause/resume. A future
  // iteration that adds engine-side album/playlist tracking can
  // relax this guard.
  let isCurrent = $derived(
    target.kind === "track" && playerStore.matches(target)
  );

  // Scoped error detection (R3.6). We only light up the red state
  // when the last engine error was carrying our exact target id.
  // For album / playlist play buttons this means the error
  // bubbles up only when the *currently playing* track inside
  // them failed; that's intentional — anything else would be
  // misleading.
  let scopedError = $derived(
    playerStore.lastError !== null &&
      playerStore.lastError.trackId !== null &&
      playerStore.lastError.trackId === target.id
  );

  /** Final visual state, mapped to a `state-*` class in CSS.
   *  Error wins over playing / paused; idle is the catch-all.
   *  No `loading` state — see the comment on `pending` above. */
  type VisualState = "idle" | "playing" | "paused" | "error";

  let visualState: VisualState = $derived(
    scopedError
      ? "error"
      : isCurrent && playerStore.status === "playing"
        ? "playing"
        : isCurrent && playerStore.status === "paused"
          ? "paused"
          : "idle"
  );

  // Once the engine confirms playback for this exact target, drop
  // the local pending guard. This used to also gate a `loading`
  // visual state; today it just unblocks the next click.
  $effect(() => {
    if (pending && isCurrent && playerStore.status === "playing") {
      pending = false;
    }
  });

  // Aria label is binary per R3.3 / R3.4: "Pause" while we're
  // currently playing this target, "Play" otherwise. The error
  // state still says "Play" — the underlying action is to retry,
  // and a screen reader user gets the cause through `title` /
  // aria-describedby.
  let ariaLabel = $derived(visualState === "playing" ? "Pause" : "Play");

  // Visible icon. Pause when we're actively playing, Play in
  // every other state (including paused, idle, loading, error).
  let iconName: "play" | "pause" = $derived(
    visualState === "playing" ? "pause" : "play"
  );

  // Tooltip + sr-only description text for the error state. We
  // keep it strictly null while there is no scoped error so the
  // attribute disappears from the DOM.
  let errorMessage = $derived(
    scopedError && playerStore.lastError !== null
      ? playerStore.lastError.message
      : null
  );

  // Stable id for the sr-only span so we can wire `aria-describedby`
  // from the button. `crypto.randomUUID()` would work but adds a
  // tiny runtime dep we don't need; a counter is enough.
  let descId = `pb-desc-${nextId()}`;

  function nextId(): number {
    // Module-scoped counter folded into a closure so multiple
    // PlayButton instances on the page don't collide on aria id.
    if (!("__pbCounter" in globalThis)) {
      // @ts-expect-error stash a counter on globalThis
      globalThis.__pbCounter = 0;
    }
    // @ts-expect-error read/write the counter
    return ++globalThis.__pbCounter;
  }

  async function onClick(e: MouseEvent): Promise<void> {
    // R3.1 / R3.2 — the click MUST NOT navigate. We belt-and-
    // braces both stopPropagation (block the parent's onclick)
    // AND preventDefault (in case the host platform turned the
    // button into a default-submit somehow).
    e.stopPropagation();
    e.preventDefault();
    if (pending) return;

    const action: "pause" | "resume" | "start" =
      visualState === "playing"
        ? "pause"
        : isCurrent && playerStore.status === "paused"
          ? "resume"
          : "start";

    pending = true;
    // Belt-and-braces ceiling so a never-resolving command can't
    // latch the guard forever (e.g. backend hung). After 4 s
    // we drop pending unconditionally.
    const ceiling = setTimeout(() => {
      pending = false;
    }, 4000);

    try {
      if (action === "pause") {
        await apiPause();
      } else if (action === "resume") {
        await apiResume();
      } else {
        await startPlayback();
      }
    } catch (err) {
      console.error("PlayButton invoke failed", err);
    } finally {
      // Pause / resume flip the engine state synchronously, so
      // by the time the command resolves the bus has already
      // emitted `Paused` / `Resumed`. Drop the guard right away.
      // Fresh-start playbacks also drop here; the visual state
      // won't show "playing" until the bus confirms it, but the
      // button is once again clickable.
      pending = false;
      clearTimeout(ceiling);
    }
  }
</script>

<button
  type="button"
  class={`play-btn size-${size} state-${visualState}${extraClass ? " " + extraClass : ""}`}
  aria-label={ariaLabel}
  aria-describedby={errorMessage ? descId : undefined}
  title={errorMessage ?? undefined}
  onclick={onClick}
>
  <span class="pb-icon">
    <Icon name={iconName} size={size === "sm" ? 12 : size === "lg" ? 22 : 16} />
  </span>
  {#if errorMessage}
    <span class="pb-sr" id={descId}>{errorMessage}</span>
  {/if}
</button>
