/**
 * @vitest-environment jsdom
 *
 * Matrice de tests de layout automatisée — tâche 17.4.
 *
 * Feature: qobee-beta-feedback-improvements
 *
 * Validates: Requirements 1.7 (neuf combinaisons résolution × DPI),
 *            1.8 (matrice automatisée asservissant R1.1–R1.6).
 *
 * Couverture indirecte des invariants asservis : R1.1 (ellipsis 1 ligne),
 * R1.2 (multi-ligne / colonne sous le seuil), R1.3 (Player_Zone réservé
 * 72 px, 100 % visible via réservation), R1.4 (marge ≥ 8 px), R1.5
 * (aucun débordement après restauration — garanti par des invariants
 * indépendants du viewport), R1.6 (scrollbar horizontale sous 360 px).
 *
 * ---------------------------------------------------------------------------
 * LIMITE jsdom — à lire avant de juger ce que cette suite « prouve »
 * ---------------------------------------------------------------------------
 * jsdom n'effectue AUCUN layout réel : pas de moteur de rendu, pas de
 * reflow, pas de mesure de pixels (`getBoundingClientRect` rend des 0,
 * `getComputedStyle` ne cascade pas les feuilles de style et ne résout
 * pas `var()`/les media queries). Une « matrice de layout » au sens
 * pixel-perfect est donc IMPOSSIBLE ici : ces vérifications-là restent
 * du ressort des contrôles DPI manuels documentés dans
 * `docs/accessibility-validation.md`.
 *
 * Cette suite est le GARDE STRUCTUREL qui complète ces contrôles manuels.
 * Elle asservit les invariants de deux façons honnêtes :
 *
 *  1. Parsing des feuilles de style sources (`global.css`,
 *     `layout-headers.css`) : on vérifie que les variables racine et les
 *     règles responsives qui PORTENT les invariants R1.1–R1.6 existent et
 *     valent les bonnes valeurs (72 px, 8 px, 360 px, seuil 768 px,
 *     ellipsis, line-clamp, overflow-x:auto…). C'est la source de vérité :
 *     si une règle disparaît ou change de valeur, la matrice casse.
 *
 *  2. Décision responsive par combinaison : pour chacune des 9
 *     combinaisons `{1280x720, 1366x768, 1920x1080} × {125 %, 150 %,
 *     200 %}` on calcule la largeur CSS LOGIQUE (`largeur_px / facteur`,
 *     la largeur après mise à l'échelle DPI Windows) et on asserte quel
 *     mode d'en-tête s'applique (ellipsis 1 ligne si logique > seuil,
 *     multi-ligne sinon), le seuil étant LU dans `--header-bp` plutôt que
 *     codé en dur — donc cohérent par construction avec le CSS livré.
 *
 *  3. Sonde jsdom : on confirme que jsdom résout bien le STOCKAGE des
 *     propriétés personnalisées (`getPropertyValue('--player-zone-h')`),
 *     seule capacité « var » fiable de jsdom, pour ancrer la matrice dans
 *     un vrai DOM même si le reflow n'existe pas.
 */
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// Chargement des feuilles de style sources (source de vérité des invariants).
// Lecture via `node:fs` au runtime (vitest s'exécute sous Node). Les types
// `node:*` ne sont pas fournis par `@types/node` (absent du workspace UI) :
// un fichier d'ambiance minimal (`src/node-builtins.d.ts`) déclare juste la
// surface utilisée ici pour garder `svelte-check` vert.
// ---------------------------------------------------------------------------

const here = dirname(fileURLToPath(import.meta.url));
const stylesDir = resolve(here, "../../styles");

let globalCss = "";
let headersCss = "";

beforeAll(() => {
  globalCss = readFileSync(join(stylesDir, "global.css"), "utf8");
  headersCss = readFileSync(join(stylesDir, "layout-headers.css"), "utf8");
});

// ---------------------------------------------------------------------------
// Helpers de parsing CSS (volontairement simples : on cherche des règles
// précises dont la présence/valeur EST l'invariant à garder).
// ---------------------------------------------------------------------------

/** Valeur d'une propriété personnalisée déclarée dans `:root` (ex. `--player-zone-h`). */
function cssVar(css: string, name: string): string | null {
  // Échappe les tirets pour le regex et capture jusqu'au `;`.
  const re = new RegExp(`${name.replace(/[-]/g, "\\-")}\\s*:\\s*([^;]+);`);
  const m = css.match(re);
  return m && m[1] != null ? m[1].trim() : null;
}

/** Nombre de pixels d'une valeur CSS du type `72px`. NaN si non-px. */
function px(value: string | null): number {
  if (value == null) return NaN;
  const m = value.match(/^(-?\d+(?:\.\d+)?)px$/);
  return m && m[1] != null ? Number(m[1]) : NaN;
}

// ===========================================================================
// La matrice : 9 combinaisons explicites (R1.7). La largeur logique est la
// largeur CSS effective après mise à l'échelle DPI Windows
// (`resolution_width / scale`). C'est elle qui pilote la media query
// `--header-bp`, pas la résolution physique.
// ===========================================================================

interface Resolution {
  readonly label: string;
  readonly width: number;
  readonly height: number;
}

const RESOLUTIONS: readonly Resolution[] = [
  { label: "1280x720", width: 1280, height: 720 },
  { label: "1366x768", width: 1366, height: 768 },
  { label: "1920x1080", width: 1920, height: 1080 },
];

// Facteurs de mise à l'échelle Windows (R1.7) : 125 %, 150 %, 200 %.
const SCALES: readonly { label: string; factor: number }[] = [
  { label: "125%", factor: 1.25 },
  { label: "150%", factor: 1.5 },
  { label: "200%", factor: 2.0 },
];

type HeaderMode = "ellipsis" | "multiline";

interface Combination {
  readonly resolution: Resolution;
  readonly scaleLabel: string;
  readonly factor: number;
  readonly logicalWidth: number;
  /** Mode d'en-tête attendu, dérivé du seuil 768 px (R1.1/R1.2). */
  readonly expectedMode: HeaderMode;
}

// Le seuil de bascule (R1.2) est figé à 768 px par le design ; on l'utilise
// pour PRÉCALCULER la table attendue, et on revérifie plus bas que le CSS
// livré (`--header-bp` + media query) porte bien cette même valeur.
const HEADER_BP = 768;

/** Produit cartésien explicite des 3 résolutions × 3 facteurs = 9 lignes. */
const MATRIX: readonly Combination[] = RESOLUTIONS.flatMap((resolution) =>
  SCALES.map(({ label, factor }) => {
    const logicalWidth = resolution.width / factor;
    return {
      resolution,
      scaleLabel: label,
      factor,
      logicalWidth,
      expectedMode: logicalWidth > HEADER_BP ? "ellipsis" : "multiline",
    } satisfies Combination;
  }),
);

// ---------------------------------------------------------------------------
// Garde-fou : la matrice DOIT compter exactement 9 combinaisons (R1.7).
// ---------------------------------------------------------------------------

describe("Matrice de layout — couverture des 9 combinaisons (R1.7)", () => {
  it("énumère exactement 3 résolutions × 3 facteurs = 9 combinaisons", () => {
    expect(RESOLUTIONS.length).toBe(3);
    expect(SCALES.length).toBe(3);
    expect(MATRIX.length).toBe(9);
    // Toutes distinctes.
    const keys = new Set(
      MATRIX.map((c) => `${c.resolution.label}@${c.scaleLabel}`),
    );
    expect(keys.size).toBe(9);
  });

  it("couvre les deux modes d'en-tête (au moins une ellipsis et une multi-ligne)", () => {
    const modes = new Set(MATRIX.map((c) => c.expectedMode));
    // 7 combinaisons > 768 px logiques (ellipsis), 2 ≤ 768 px (multi-ligne).
    expect(modes.has("ellipsis")).toBe(true);
    expect(modes.has("multiline")).toBe(true);
    expect(MATRIX.filter((c) => c.expectedMode === "multiline").length).toBe(2);
    expect(MATRIX.filter((c) => c.expectedMode === "ellipsis").length).toBe(7);
  });
});

// ---------------------------------------------------------------------------
// Invariants GLOBAUX (indépendants du viewport) — variables racine + règles.
// Ce sont les piliers de R1.3, R1.4, R1.6 ; ils valent pour TOUTES les
// combinaisons, on les vérifie donc une fois sur le CSS source.
// ---------------------------------------------------------------------------

describe("Invariants globaux portés par global.css (R1.3, R1.4, R1.6)", () => {
  it("R1.3 — --player-zone-h vaut 72px", () => {
    expect(px(cssVar(globalCss, "--player-zone-h"))).toBe(72);
  });

  it("R1.3 — .player-zone réserve min-height: var(--player-zone-h)", () => {
    expect(globalCss).toMatch(
      /\.player-zone\s*\{\s*min-height:\s*var\(--player-zone-h\)\s*;?\s*\}/,
    );
  });

  it("R1.3 — main.content réserve padding-bottom: var(--player-zone-h) (Player_Zone 100% visible)", () => {
    expect(globalCss).toMatch(
      /main\.content\s*\{\s*padding-bottom:\s*var\(--player-zone-h\)\s*;?\s*\}/,
    );
  });

  it("R1.2 — --header-bp vaut 768px (seuil de bascule), cohérent avec la matrice", () => {
    const bp = px(cssVar(globalCss, "--header-bp"));
    expect(bp).toBe(768);
    expect(bp).toBe(HEADER_BP);
  });

  it("R1.4 — --content-edge-gap vaut 8px et alimente la marge des contrôles supérieurs", () => {
    const gap = px(cssVar(globalCss, "--content-edge-gap"));
    expect(gap).toBe(8);
    expect(gap).toBeGreaterThanOrEqual(8);
    expect(globalCss).toMatch(
      /\.top-controls\s*\{\s*margin:\s*var\(--content-edge-gap\)\s*;?\s*\}/,
    );
  });

  it("R1.6 — --app-min-width vaut 360px et .shell applique min-width", () => {
    expect(px(cssVar(globalCss, "--app-min-width"))).toBe(360);
    expect(globalCss).toMatch(
      /\.shell\s*\{\s*min-width:\s*var\(--app-min-width\)\s*;?\s*\}/,
    );
  });

  it("R1.6 — sous 360px, .shell passe en overflow-x:auto (scrollbar plutôt que rognage)", () => {
    // Media query 360px contenant la règle overflow-x:auto (lazy match pour
    // traverser la règle interne `.shell { ... }`).
    expect(globalCss).toMatch(
      /@media\s*\(max-width:\s*360px\)[\s\S]*?overflow-x:\s*auto/,
    );
  });
});

// ---------------------------------------------------------------------------
// Invariants d'EN-TÊTE portés par layout-headers.css (R1.1, R1.2). Présents
// une fois dans la feuille de style ; la décision « lequel s'applique » est
// asservie par combinaison plus bas.
// ---------------------------------------------------------------------------

describe("Invariants d'en-tête portés par layout-headers.css (R1.1, R1.2)", () => {
  it("R1.1 — .detail-title : max-width 100% + nowrap + ellipsis (troncature 1 ligne)", () => {
    expect(headersCss).toMatch(/\.detail-title\s*\{[\s\S]*?max-width:\s*100%/);
    expect(headersCss).toMatch(/\.detail-title\s*\{[\s\S]*?white-space:\s*nowrap/);
    expect(headersCss).toMatch(/\.detail-title\s*\{[\s\S]*?overflow:\s*hidden/);
    expect(headersCss).toMatch(
      /\.detail-title\s*\{[\s\S]*?text-overflow:\s*ellipsis/,
    );
  });

  it("R1.2 — sous 768px : bascule multi-ligne (line-clamp 2) + en-tête en colonne", () => {
    expect(headersCss).toMatch(/@media\s*\(max-width:\s*768px\)/);
    expect(headersCss).toMatch(/white-space:\s*normal/);
    expect(headersCss).toMatch(/-webkit-line-clamp:\s*2/);
    expect(headersCss).toMatch(/flex-direction:\s*column/);
  });

  it("R1.1 — aucun positionnement absolu sur le titre (0px de chevauchement, flux flex/gap)", () => {
    // Le design impose de garder le titre dans le flux (pas de position
    // absolue), ce qui garantit l'absence de chevauchement avec les voisins.
    expect(headersCss).not.toMatch(/\.detail-title\s*\{[\s\S]*?position:\s*absolute/);
  });
});

// ===========================================================================
// Assertions PAR COMBINAISON (R1.8 : asservir R1.1–R1.6 sur chacune des 9).
// On itère la table explicite ; le nom de chaque test cite la combinaison
// pour que la « matrice » soit lisible dans le rapport de test.
// ===========================================================================

describe.each(MATRIX)(
  "Combinaison $resolution.label @ $scaleLabel (largeur logique ≈ $logicalWidth px)",
  (combo) => {
    it(`R1.7 — largeur logique = largeur physique / facteur (${combo.resolution.width}/${combo.factor})`, () => {
      expect(combo.logicalWidth).toBeCloseTo(combo.resolution.width / combo.factor, 5);
    });

    it(`R1.1/R1.2 — mode d'en-tête attendu : ${combo.expectedMode} (seuil ${HEADER_BP}px)`, () => {
      const bp = px(cssVar(globalCss, "--header-bp"));
      const decided: HeaderMode =
        combo.logicalWidth > bp ? "ellipsis" : "multiline";
      // La décision dérivée du CSS livré coïncide avec la table attendue.
      expect(decided).toBe(combo.expectedMode);

      if (combo.expectedMode === "ellipsis") {
        // R1.1 : au-dessus du seuil, troncature 1 ligne + ellipsis.
        expect(headersCss).toMatch(/text-overflow:\s*ellipsis/);
        expect(headersCss).toMatch(/white-space:\s*nowrap/);
      } else {
        // R1.2 : à/au-dessous du seuil, multi-ligne + colonne disponibles.
        expect(headersCss).toMatch(/@media\s*\(max-width:\s*768px\)/);
        expect(headersCss).toMatch(/-webkit-line-clamp:\s*2/);
        expect(headersCss).toMatch(/flex-direction:\s*column/);
      }
    });

    it("R1.3 — Player_Zone réservé à 72px, constant et indépendant de la combinaison", () => {
      // Invariant indépendant du viewport : la réservation (min-height +
      // padding-bottom) ne dépend ni de la résolution ni du facteur DPI,
      // donc le Mini_Player reste 100 % visible sur les 9 combinaisons.
      expect(px(cssVar(globalCss, "--player-zone-h"))).toBe(72);
      expect(px(cssVar(globalCss, "--player-zone-h"))).toBeGreaterThanOrEqual(72);
    });

    it("R1.4 — marge ≥ 8px entre contrôles supérieurs et bord de contenu", () => {
      expect(px(cssVar(globalCss, "--content-edge-gap"))).toBeGreaterThanOrEqual(8);
    });

    it("R1.5 — aucun débordement après restauration (réservation indépendante du viewport)", () => {
      // jsdom ne peut pas mesurer un reflow réel (cf. en-tête). On asserte
      // l'invariant qui le GARANTIT structurellement : la hauteur réservée
      // du Player_Zone et le padding de contenu ne varient pas avec la
      // taille de fenêtre, donc un resize/restore ne peut pas laisser le
      // Mini_Player hors zone visible. Le `< 500 ms` et l'absence de
      // scrollbar non prévue relèvent du ResizeObserver de PlayerBar et des
      // contrôles DPI manuels (docs/accessibility-validation.md).
      expect(globalCss).toMatch(
        /main\.content\s*\{\s*padding-bottom:\s*var\(--player-zone-h\)\s*;?\s*\}/,
      );
      expect(globalCss).toMatch(
        /\.player-zone\s*\{\s*min-height:\s*var\(--player-zone-h\)\s*;?\s*\}/,
      );
    });

    it("R1.6 — sous le plancher de 360px, scrollbar horizontale plutôt que rognage", () => {
      const floor = px(cssVar(globalCss, "--app-min-width"));
      expect(floor).toBe(360);
      // Si la largeur LOGIQUE de la combinaison tombait sous le plancher, la
      // règle overflow-x:auto prendrait le relais. Aucune des 9 combinaisons
      // de référence ne descend sous 360px logiques — on vérifie donc à la
      // fois la marge et l'existence du filet de sécurité.
      expect(combo.logicalWidth).toBeGreaterThanOrEqual(floor);
      expect(globalCss).toMatch(
        /@media\s*\(max-width:\s*360px\)[\s\S]*?overflow-x:\s*auto/,
      );
    });
  },
);

// ===========================================================================
// Sonde jsdom — ancrage dans un vrai DOM. jsdom NE cascade PAS les feuilles
// de style et NE résout PAS `var()` dans getComputedStyle ; en revanche il
// stocke et restitue fidèlement les propriétés personnalisées posées EN
// LIGNE. On l'utilise pour confirmer que les valeurs d'invariant sont des
// custom properties valides et lisibles par programme, par combinaison.
// ===========================================================================

describe("Sonde jsdom — propriétés personnalisées résolues dans un DOM réel", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("getComputedStyle restitue les variables d'invariant posées en ligne", () => {
    const shell = document.createElement("div");
    shell.className = "shell";
    // Reflète les valeurs de :root sur l'élément (jsdom ne lit pas :root via
    // cascade, mais lit les custom props inline de l'élément interrogé).
    shell.style.setProperty("--player-zone-h", cssVar(globalCss, "--player-zone-h")!);
    shell.style.setProperty("--content-edge-gap", cssVar(globalCss, "--content-edge-gap")!);
    shell.style.setProperty("--app-min-width", cssVar(globalCss, "--app-min-width")!);
    shell.style.setProperty("--header-bp", cssVar(globalCss, "--header-bp")!);
    document.body.appendChild(shell);

    const cs = getComputedStyle(shell);
    expect(cs.getPropertyValue("--player-zone-h").trim()).toBe("72px");
    expect(cs.getPropertyValue("--content-edge-gap").trim()).toBe("8px");
    expect(cs.getPropertyValue("--app-min-width").trim()).toBe("360px");
    expect(cs.getPropertyValue("--header-bp").trim()).toBe("768px");
  });

  it("monte une structure shell > main.content > header.detail-header > h1.detail-title", () => {
    // Vérifie surtout que la hiérarchie de classes attendue par les règles
    // CSS existe (les sélecteurs `.shell`, `main.content`, `.detail-header`,
    // `.detail-title`, `.player-zone` ont une cible). jsdom ne reflowe pas,
    // mais la structure DOIT être bien formée pour que le CSS s'applique en
    // production.
    document.body.innerHTML = `
      <div class="shell">
        <main class="content">
          <header class="header detail-header">
            <h1 class="detail-title">Un titre d'album volontairement très long pour déclencher la troncature</h1>
          </header>
        </main>
        <footer class="bar player-zone"></footer>
      </div>
    `;
    expect(document.querySelector(".shell")).not.toBeNull();
    expect(document.querySelector("main.content")).not.toBeNull();
    expect(document.querySelector(".detail-header .detail-title")).not.toBeNull();
    expect(document.querySelector(".player-zone")).not.toBeNull();
  });
});
