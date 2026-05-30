<script lang="ts">
  // Écran d'aide listant les raccourcis clavier disponibles (R2.6).
  //
  // Les libellés d'action sont dérivés de la source unique
  // `ui/src/lib/labels.ts` (champs `label` / `shortcut` de
  // SIDEBAR_LABELS et PLAYER_LABELS), exactement comme les `aria-label`
  // et les infobulles des contrôles. Afficher la même chaîne ici
  // garantit zéro divergence entre ce que montre cet écran et ce
  // qu'annonce un lecteur d'écran (R2.4 / R5.5).
  //
  // Les combinaisons de touches reflètent les raccourcis réellement
  // câblés dans `ui/src/lib/keyboardShortcuts.ts` (transport, recherche,
  // mode immersif). Si un raccourci y change, mettre à jour ce tableau.

  import { SIDEBAR_LABELS, PLAYER_LABELS, type UiLabel } from "../lib/labels";

  interface ShortcutRow {
    /** Une ou plusieurs combinaisons équivalentes, rendues en `<kbd>`. */
    keys: string[];
    /** Libellé FR de l'action, issu de `labels.ts`. */
    action: string;
  }
  interface ShortcutGroup {
    title: string;
    rows: ShortcutRow[];
  }

  // Accès tolérants à la source `labels.ts` : les tables sont typées
  // `Record<string, UiLabel>`, l'indexation peut donc être `undefined`.
  // On retombe sur un libellé/raccourci explicite plutôt que de planter.
  function label(entry: UiLabel | undefined, fallback: string): string {
    return entry?.label ?? fallback;
  }
  function shortcut(entry: UiLabel | undefined, fallback: string): string {
    return entry?.shortcut ?? fallback;
  }

  // Groupes dérivés de `labels.ts` (+ `keyboardShortcuts.ts`).
  const groups: ShortcutGroup[] = [
    {
      title: "Lecture",
      rows: [
        {
          keys: [shortcut(PLAYER_LABELS.play, "Espace")],
          action: `${label(PLAYER_LABELS.play, "Lecture")} / ${label(PLAYER_LABELS.pause, "Pause")}`,
        },
        { keys: ["\u2192", "\u2190"], action: "Avancer / reculer de 5 s" },
        {
          keys: ["Alt+\u2192", "Ctrl+\u2192", "N"],
          action: label(PLAYER_LABELS.next, "Piste suivante"),
        },
        {
          keys: ["Alt+\u2190", "Ctrl+\u2190", "P"],
          action: label(PLAYER_LABELS.previous, "Piste precedente"),
        },
      ],
    },
    {
      title: "Navigation",
      rows: [
        {
          keys: [shortcut(SIDEBAR_LABELS.search, "Ctrl+F")],
          action: label(SIDEBAR_LABELS.search, "Recherche"),
        },
        {
          keys: [shortcut(SIDEBAR_LABELS.queue, "Q")],
          action: label(SIDEBAR_LABELS.queue, "File d'attente"),
        },
      ],
    },
    {
      title: "Mode immersif",
      rows: [
        { keys: ["F"], action: label(PLAYER_LABELS.immersive, "Mode immersif") },
        {
          keys: [shortcut(PLAYER_LABELS.exitImmersive, "Esc")],
          action: label(PLAYER_LABELS.exitImmersive, "Quitter le mode immersif"),
        },
      ],
    },
  ];
</script>

<div class="block shortcuts-help">
  <h3 id="shortcuts-help-title">Raccourcis clavier</h3>
  <p class="intro">
    Ces raccourcis sont actifs lorsque vous ne saisissez pas de texte.
  </p>

  {#each groups as group (group.title)}
    <table class="shortcuts" aria-labelledby="shortcuts-help-title">
      <caption>{group.title}</caption>
      <thead>
        <tr>
          <th scope="col">Touche</th>
          <th scope="col">Action</th>
        </tr>
      </thead>
      <tbody>
        {#each group.rows as row (row.action)}
          <tr>
            <td class="keys">
              {#each row.keys as key, i (key)}
                {#if i > 0}<span class="sep" aria-hidden="true">/</span>{/if}
                <kbd>{key}</kbd>
              {/each}
            </td>
            <td class="action">{row.action}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/each}
</div>

<style>
  .shortcuts-help {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .intro {
    margin: 0 0 var(--space-2);
    color: var(--fg-2);
    font-size: 13px;
  }

  table.shortcuts {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  table.shortcuts caption {
    text-align: left;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-2);
    padding: var(--space-3) 0 var(--space-2);
  }
  table.shortcuts th {
    text-align: left;
    font-weight: 600;
    color: var(--fg-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
  }
  table.shortcuts td {
    padding: 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  table.shortcuts td.action {
    color: var(--fg-0);
  }
  table.shortcuts td.keys {
    white-space: nowrap;
    width: 1%;
  }

  kbd {
    display: inline-block;
    min-width: 1.4em;
    text-align: center;
    padding: 2px 6px;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 6px;
    color: var(--fg-1);
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 12px;
    line-height: 1.4;
  }
  .sep {
    color: var(--fg-2);
    margin: 0 4px;
  }
</style>
