<script lang="ts">
  import { playAlbumFromTrack, playTrack, type Track } from "../lib/api";
  import { app } from "../lib/stores.svelte";
  import { formatDuration, formatQuality } from "../lib/format";
  import { openTrackMenu } from "../lib/trackMenu";
  import { splitArtists } from "../lib/artistNames";
  import PlayButton from "./PlayButton.svelte";

  interface Props {
    tracks: Track[];
    /** When provided, clicking a track queues the whole album and
     *  positions the cursor on the clicked track. Without it we fall
     *  back to single-track playback (ad-hoc track lists). */
    albumId?: number | null;
  }

  let { tracks, albumId = null }: Props = $props();

  async function handlePlay(track: Track): Promise<void> {
    try {
      if (albumId !== null) {
        await playAlbumFromTrack(albumId, track.id);
      } else {
        await playTrack(track.id);
      }
    } catch (e) {
      app.lastError = String(e);
    }
  }

  function onContext(event: MouseEvent, track: Track): void {
    openTrackMenu(event, track, {
      // When the parent passed an albumId, "Go to album" knows where
      // to navigate. Without it (ad-hoc track lists), the entry
      // remains disabled.
      onGoToAlbum: albumId !== null ? () => app.selectAlbum(albumId) : undefined,
    });
  }

  function gotoArtist(name: string, e: Event): void {
    // Stop the row's onclick from firing (which would start playback
    // instead of navigating). Same trick as the playlist delete
    // button further down the codebase.
    e.stopPropagation();
    app.selectArtist(name);
  }
</script>

<table class="tracks">
  <colgroup>
    <col class="col-num" />
    <col />
    <col class="col-quality" />
    <col class="col-duration" />
  </colgroup>
  <thead>
    <tr>
      <th>#</th>
      <th>Title</th>
      <th>Quality</th>
      <th>Duration</th>
    </tr>
  </thead>
  <tbody>
    {#each tracks as track (track.id)}
      {@const isCurrent = app.player.current_track_id === String(track.id)}
      {@const isPlaying = isCurrent && app.player.status === "playing"}
      <tr
        class:current={isCurrent}
        onclick={() => handlePlay(track)}
        ondblclick={() => handlePlay(track)}
        oncontextmenu={(e) => onContext(e, track)}
        role="button"
        tabindex="0"
        aria-label={`Lire « ${track.title} » — ${track.artist}`}
        onkeydown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            handlePlay(track);
          }
        }}
      >
        <td class="num">
          <!-- Always render the number; on the playing row it's
               hidden (visibility) so the cell keeps its intrinsic
               width and the equalizer can overlay without nudging
               the rest of the row. The hover-revealed Play button
               sits in the same cell and inherits the hidden state
               so the row stays steady when the cursor enters. -->
          <span class="num-text" class:hidden={isPlaying}>
            {track.track_number ?? ""}
          </span>
          {#if isPlaying}
            <span class="row-eq" aria-hidden="true">
              <span></span><span></span><span></span>
            </span>
          {/if}
          <span class="row-play">
            <PlayButton
              target={{ kind: "track", id: track.id }}
              size="sm"
            />
          </span>
        </td>
        <td class="title-cell">
          <div class="title">{track.title}</div>
          <div class="artist">
            {#each splitArtists(track.artist) as name, i (name + i)}
              {#if i > 0}<span class="sep">,</span>{/if}
              <button
                class="artist-link"
                onclick={(e) => gotoArtist(name, e)}
                title={`Open ${name}`}
              >
                {name}
              </button>
            {/each}
          </div>
        </td>
        <td class="quality">{formatQuality(track.sample_rate, track.bit_depth)}</td>
        <td class="duration">{formatDuration(track.duration_seconds)}</td>
      </tr>
    {/each}
  </tbody>
</table>

<style>
  .tracks {
    width: 100%;
    /* Lock column widths via <col> widths declared below so the
       layout stays stable when the playing row swaps content in. */
    table-layout: fixed;
    border-collapse: collapse;
    margin-top: var(--space-2);
  }
  .col-num { width: 48px; }
  .col-quality { width: 140px; }
  .col-duration { width: 80px; }

  thead th {
    text-align: left;
    color: var(--fg-2);
    font-weight: 500;
    font-size: 11px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: var(--space-3) var(--space-2);
    border-bottom: 1px solid var(--border);
  }
  tbody td {
    height: 56px;
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
    overflow: hidden;
    transition: color var(--dur-fast) var(--ease-out);
  }
  tbody tr {
    cursor: pointer;
    transition: background var(--dur-base) var(--ease-out);
  }
  tbody tr:hover {
    background: var(--bg-2);
  }
  tbody tr:hover .title {
    color: var(--fg-0);
  }
  tbody tr.current {
    /* Inset shadow draws the left accent bar without needing a
       positioned ::before on a <tr>, which is fragile across
       Chromium versions. Slide-in via box-shadow animation. */
    background: var(--accent-soft);
    box-shadow: inset 3px 0 0 0 var(--accent);
    animation: row-in 240ms var(--ease-spring);
  }
  tbody tr.current .title {
    color: var(--accent);
    font-weight: 600;
  }
  @keyframes row-in {
    from { box-shadow: inset 0 0 0 0 var(--accent); }
    to { box-shadow: inset 3px 0 0 0 var(--accent); }
  }

  .num {
    position: relative;
    color: var(--fg-2);
    text-align: right;
    padding-right: 14px;
  }
  .num-text {
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  .num-text.hidden {
    /* Keep layout space — visibility: hidden retains box metrics
       while opacity makes the swap less abrupt. */
    visibility: hidden;
    opacity: 0;
  }
  /* Equalizer pinned to the cell, never affects layout because it's
     out of the flow. */
  .row-eq {
    position: absolute;
    right: 14px;
    top: 50%;
    transform: translateY(-50%);
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 14px;
    width: 14px;
    pointer-events: none;
  }
  .row-eq > span {
    width: 2px;
    background: var(--accent);
    border-radius: 1px;
    transform-origin: bottom;
    animation: row-bar 900ms var(--ease-in-out) infinite;
  }
  .row-eq > span:nth-child(1) { height: 60%; animation-delay: 0ms; }
  .row-eq > span:nth-child(2) { height: 100%; animation-delay: 180ms; }
  .row-eq > span:nth-child(3) { height: 75%; animation-delay: 360ms; }
  @keyframes row-bar {
    0%, 100% { transform: scaleY(0.4); }
    50% { transform: scaleY(1); }
  }

  /* Hover-revealed Play button. It overlays the same cell as the
     track number / equalizer indicator and only becomes visible on
     row hover (or on focus-within for keyboard). The track number
     fades out underneath. We use opacity + pointer-events so the
     button keeps its layout box stable across the swap. */
  .row-play {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--dur-fast) var(--ease-out);
  }
  tbody tr:hover .row-play,
  tbody tr:focus-within .row-play {
    opacity: 1;
    pointer-events: auto;
  }
  /* When the row's Play button shows up, fade the numeric label
     and the equalizer so they don't compete visually. */
  tbody tr:hover .num-text,
  tbody tr:focus-within .num-text {
    opacity: 0;
  }
  tbody tr:hover .row-eq,
  tbody tr:focus-within .row-eq {
    opacity: 0;
  }

  .title {
    color: var(--fg-1);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .artist {
    color: var(--fg-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    display: flex;
    align-items: baseline;
    gap: 2px;
    min-width: 0;
  }
  .artist-link {
    background: transparent;
    border: none;
    padding: 0;
    margin: 0;
    color: inherit;
    cursor: pointer;
    font: inherit;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: color var(--dur-fast) var(--ease-out);
  }
  .artist-link:hover {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .artist .sep {
    color: var(--fg-3, var(--fg-2));
    flex-shrink: 0;
    margin-right: 2px;
  }
  .quality, .duration {
    color: var(--fg-1);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
</style>
