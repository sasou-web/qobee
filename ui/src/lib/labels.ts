// Source unique des libellés FR de l'interface (R2.4, R5.5).
//
// Chaque destination de Sidebar et chaque contrôle iconographique du
// Mini_Player / mode immersif a une entrée `{ key, label, shortcut? }`.
// Les `aria-label` (accessibilité, R2.4) ET les `title` d'infobulle
// (R5.5) lisent la même chaîne depuis ce module, garantissant zéro
// divergence entre ce qu'annonce un lecteur d'écran et ce que montre
// l'infobulle.

export interface UiLabel { key: string; label: string; shortcut?: string }

export const SIDEBAR_LABELS = {
  home:      { key: "home",      label: "Accueil" },
  search:    { key: "search",    label: "Recherche", shortcut: "Ctrl+F" },
  albums:    { key: "albums",    label: "Albums" },
  artists:   { key: "artists",   label: "Artistes" },
  genres:    { key: "genres",    label: "Genres" },
  favorites: { key: "favorites", label: "Favoris" },
  newPlaylist: { key: "newPlaylist", label: "Nouvelle playlist" },
  queue:     { key: "queue",     label: "File d'attente", shortcut: "Q" },
} satisfies Record<string, UiLabel>;

export const PLAYER_LABELS = {
  play:      { key: "play",      label: "Lecture",  shortcut: "Espace" },
  pause:     { key: "pause",     label: "Pause",    shortcut: "Espace" },
  next:      { key: "next",      label: "Piste suivante" },
  previous:  { key: "previous",  label: "Piste precedente" },
  favorite:  { key: "favorite",  label: "Ajouter aux favoris" },
  unfavorite:{ key: "unfavorite",label: "Retirer des favoris" },
  immersive: { key: "immersive", label: "Mode immersif" },
  exitImmersive: { key: "exitImmersive", label: "Quitter le mode immersif", shortcut: "Esc" },
  queue:     { key: "queue",     label: "File d'attente" },
} satisfies Record<string, UiLabel>;
