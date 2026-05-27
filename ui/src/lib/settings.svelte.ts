// Persisted user preferences. Backed by the SQLite `settings` table
// via the `get_setting` / `set_setting` Tauri commands. Each setting
// has a known key, a TypeScript type, and a default value.
//
// On startup, `settings.load()` pulls every persisted value and applies
// the visual ones (theme, accent) to the document root. Subsequent
// changes are persisted via `settings.set(...)`.

import { getSetting, setSetting } from "./api";

export type Theme = "system" | "dark" | "light";
export type BgTheme = "dark-neutral" | "black" | "gray" | "indigo";

export const BG_THEMES: { id: BgTheme; label: string; sample: string }[] = [
  { id: "dark-neutral", label: "Dark neutral", sample: "#0e0e10" },
  { id: "black", label: "Deep black", sample: "#050507" },
  { id: "gray", label: "Soft gray", sample: "#1a1a1d" },
  { id: "indigo", label: "Indigo tint", sample: "#0e1018" },
];

export interface SettingsValues {
  theme: Theme;
  /** Background palette for the whole shell. Picks `--bg-shell` and
   *  the related `--bg-0/1/2/3` family. */
  bgTheme: BgTheme;
  accent: string;
  /** When true, the accent CSS variable follows the dominant color of
   *  the currently playing cover. Falls back to `accent` otherwise. */
  accentFollowsCover: boolean;
  defaultVolume: number;
  outputMode: "auto" | "shared";
  outputDeviceId: string | null;
  eqGains: number[]; // length 10
  eqEnabled: boolean;
  /** Show the currently playing track on Discord (Rich Presence).
   *  Reconnects silently when Discord starts/restarts; does nothing
   *  visible if Discord is closed. */
  discordRichPresence: boolean;
  /** Discord Application ID required for the IPC handshake. Without
   *  a valid ID nothing shows up — register an app at
   *  <https://discord.com/developers/applications>. */
  discordClientId: string;
  /** Whether the cover-host worker is allowed to upload local album
   *  art to a public service so Discord can display it. Off by
   *  default — must be opt-in to publish bytes off the device. */
  discordCoverUpload: boolean;
}

const DEFAULTS: SettingsValues = {
  theme: "system",
  bgTheme: "dark-neutral",
  accent: "#7c5cff",
  accentFollowsCover: true,
  defaultVolume: 1.0,
  outputMode: "auto",
  outputDeviceId: null,
  eqGains: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  eqEnabled: false,
  discordRichPresence: true,
  discordClientId: "",
  discordCoverUpload: false,
};

const KEYS = {
  theme: "ui.theme",
  bgTheme: "ui.bg_theme",
  accent: "ui.accent",
  accentFollowsCover: "ui.accent_follows_cover",
  defaultVolume: "playback.default_volume",
  outputMode: "playback.output_mode",
  outputDeviceId: "playback.output_device_id",
  eqGains: "audio.eq_gains",
  eqEnabled: "audio.eq_enabled",
  discordRichPresence: "integrations.discord_rich_presence",
  discordClientId: "integrations.discord_client_id",
  discordCoverUpload: "integrations.discord_cover_upload",
} as const;

class SettingsStore {
  values = $state<SettingsValues>({ ...DEFAULTS });
  loaded = $state(false);

  async load(): Promise<void> {
    try {
      const [theme, bgTheme, accent, follow, vol, mode, dev, eqg, eqe, drpc, dcid, dupload] =
        await Promise.all([
          getSetting(KEYS.theme),
          getSetting(KEYS.bgTheme),
          getSetting(KEYS.accent),
          getSetting(KEYS.accentFollowsCover),
          getSetting(KEYS.defaultVolume),
          getSetting(KEYS.outputMode),
          getSetting(KEYS.outputDeviceId),
          getSetting(KEYS.eqGains),
          getSetting(KEYS.eqEnabled),
          getSetting(KEYS.discordRichPresence),
          getSetting(KEYS.discordClientId),
          getSetting(KEYS.discordCoverUpload),
        ]);
      this.values = {
        theme: (theme as Theme) ?? DEFAULTS.theme,
        bgTheme: isBgTheme(bgTheme) ? bgTheme : DEFAULTS.bgTheme,
        accent: accent ?? DEFAULTS.accent,
        accentFollowsCover:
          follow === null ? DEFAULTS.accentFollowsCover : follow === "true",
        defaultVolume:
          vol !== null ? clamp01(Number.parseFloat(vol)) : DEFAULTS.defaultVolume,
        outputMode:
          mode === "shared" || mode === "auto" ? mode : DEFAULTS.outputMode,
        outputDeviceId: dev && dev.length > 0 ? dev : null,
        eqGains: parseGains(eqg) ?? DEFAULTS.eqGains,
        eqEnabled: eqe === "true",
        discordRichPresence:
          drpc === null ? DEFAULTS.discordRichPresence : drpc === "true",
        discordClientId: dcid ?? DEFAULTS.discordClientId,
        // Default OFF for privacy: the user must opt in to publish
        // any bytes off the device.
        discordCoverUpload:
          dupload === null ? DEFAULTS.discordCoverUpload : dupload === "true",
      };
    } catch {
      // best-effort
    }
    this.applyVisual();
    this.loaded = true;
  }

  async set<K extends keyof SettingsValues>(
    key: K,
    value: SettingsValues[K]
  ): Promise<void> {
    this.values = { ...this.values, [key]: value };
    const persisted = serializeValue(value);
    try {
      await setSetting(KEYS[key], persisted);
    } catch {
      // best-effort
    }
    this.applyVisual();
  }

  applyVisual(): void {
    const root = document.documentElement;
    const dark =
      this.values.theme === "dark" ||
      (this.values.theme === "system" &&
        window.matchMedia?.("(prefers-color-scheme: dark)").matches !== false);
    root.dataset.theme = dark ? "dark" : "light";
    root.dataset.bgTheme = this.values.bgTheme;
    root.style.setProperty("--accent", this.values.accent);
    root.style.setProperty("--accent-soft", hexWithAlpha(this.values.accent, 0.18));
  }
}

export const settings = new SettingsStore();

function isBgTheme(v: string | null): v is BgTheme {
  return (
    v === "dark-neutral" ||
    v === "black" ||
    v === "gray" ||
    v === "indigo"
  );
}

function clamp01(v: number): number {
  if (!Number.isFinite(v)) return 0;
  return Math.min(1, Math.max(0, v));
}

/** Add an alpha channel to a `#rrggbb` string. Falls back to the input
 *  if it can't be parsed. */
function hexWithAlpha(hex: string, alpha: number): string {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m || !m[1]) return hex;
  const a = Math.round(clamp01(alpha) * 255)
    .toString(16)
    .padStart(2, "0");
  return `#${m[1]}${a}`;
}

function serializeValue(v: unknown): string {
  if (typeof v === "number") return String(v);
  if (typeof v === "boolean") return v ? "true" : "false";
  if (Array.isArray(v)) return v.map((x) => String(x)).join(",");
  if (v === null) return "";
  return String(v);
}

function parseGains(s: string | null): number[] | null {
  if (!s) return null;
  const parts = s.split(",").map((x) => Number.parseFloat(x));
  if (parts.length !== 10 || parts.some((n) => !Number.isFinite(n))) return null;
  return parts;
}
