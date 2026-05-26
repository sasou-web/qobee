<script lang="ts">
  // Settings → Audio tab. Thin shell around AudioSettingsPanel which
  // groups every `audio.*` knob plus the Bit-Perfect Health and
  // Null-Test sub-panels.
  import AudioSettingsPanel from "./AudioSettingsPanel.svelte";
</script>

<section class="audio-settings">
  <AudioSettingsPanel />
</section>

<style>
  .audio-settings {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
</style>
