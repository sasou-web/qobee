// Helpers d'accessibilité partagés (R2.8, R2.9).
//
// Deux briques sans dépendance lourde :
//  - `activateOnKey` : action Svelte rendant un élément non-natif
//    activable au clavier (Entrée / Espace), pour les rangées de track
//    et les cartes cliquables qui ne sont pas des `<button>` (R2.8).
//  - `announce` : pousse un message transitoire dans une région live
//    ARIA partagée, annoncée par les lecteurs d'écran (NVDA) sans
//    déplacer le focus clavier (R2.9). Câblée par `ToastsRoot`.

/**
 * Action Svelte : rend un élément non-natif activable au clavier
 * (Entrée / Espace) sans souris (R2.8). À poser sur les rangées de
 * track et les cartes cliquables qui ne sont pas des `<button>`.
 *
 * Pose `tabindex=0` si l'élément n'est pas déjà focusable, et nettoie
 * son écouteur au `destroy` du nœud.
 *
 * @example
 * ```svelte
 * <div use:activateOnKey={() => openTrack(id)}>…</div>
 * ```
 */
export function activateOnKey(node: HTMLElement, onActivate: () => void) {
  function handle(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onActivate();
    }
  }
  node.addEventListener("keydown", handle);
  if (!node.hasAttribute("tabindex")) node.tabIndex = 0;
  return {
    destroy() {
      node.removeEventListener("keydown", handle);
    },
  };
}

/**
 * Région live ARIA partagée par toute l'application. Créée une seule
 * fois, à la première annonce, puis réutilisée. Elle est visuellement
 * masquée (hors écran) mais reste exposée à l'arbre d'accessibilité.
 */
let liveRegion: HTMLElement | null = null;

function ensureLiveRegion(): HTMLElement | null {
  // Garde SSR / environnements sans DOM : pas de document => pas de région.
  if (typeof document === "undefined") return null;
  if (liveRegion && liveRegion.isConnected) return liveRegion;

  const region = document.createElement("div");
  region.setAttribute("role", "status");
  region.setAttribute("aria-live", "polite");
  region.setAttribute("aria-atomic", "true");
  region.setAttribute("data-a11y-live-region", "");
  // Visuellement masqué sans `display:none` (qui retirerait le nœud de
  // l'arbre d'accessibilité). Technique « visually hidden » classique.
  const s = region.style;
  s.position = "absolute";
  s.width = "1px";
  s.height = "1px";
  s.margin = "-1px";
  s.padding = "0";
  s.border = "0";
  s.overflow = "hidden";
  s.clip = "rect(0 0 0 0)";
  s.clipPath = "inset(50%)";
  s.whiteSpace = "nowrap";

  document.body.appendChild(region);
  liveRegion = region;
  return region;
}

/**
 * Annonce un message transitoire via la région live ARIA partagée,
 * sans déplacer le focus (R2.9). Câblé par `ToastsRoot`.
 *
 * Le contenu est vidé puis réécrit à la frame suivante : cela force la
 * réannonce d'un même message répété, que certains lecteurs d'écran
 * ignoreraient sinon comme un texte inchangé.
 */
export function announce(message: string): void {
  const region = ensureLiveRegion();
  if (!region) return;

  region.textContent = "";
  const write = () => {
    region.textContent = message;
  };
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(write);
  } else {
    write();
  }
}
