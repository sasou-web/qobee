# Procédure de validation d'accessibilité de Qobee

Ce document décrit une procédure **manuelle et reproductible** pour
valider l'accessibilité de l'interface Qobee à l'aide de **NVDA**
(lecteur d'écran) et de **Accessibility Insights for Windows**
(inspecteur d'arbre UI Automation et vérification de contraste). Elle
couvre les trois parcours exigés par la spec : **navigation**,
**lecture / transport** et **gestion de collection** (Requirement
2.7).

> **Pourquoi une procédure manuelle ?** Qobee s'exécute dans une
> WebView Tauri. L'accessibilité réelle perçue par un utilisateur de
> lecteur d'écran dépend de la manière dont la WebView expose ses
> contrôles à l'arbre **UI Automation** de Windows, au-delà de la
> fenêtre Tauri et du `BrowserRootView` (Requirement 2.1). Ces aspects
> ne sont pas couvrables par les tests automatisés Vitest/proptest :
> ils nécessitent un lecteur d'écran réel et un inspecteur UIA. Cette
> procédure est le livrable de validation associé.

---

## 1. Outils et préparation

### 1.1 Installer les outils

1. **NVDA** (NonVisual Desktop Access) — lecteur d'écran gratuit.
   - Téléchargement : [nvaccess.org/download](https://www.nvaccess.org/download/).
   - Lancer NVDA **avant** Qobee. La touche modificatrice NVDA par
     défaut est `Insert` (ou `Verr. maj` en disposition portable).
2. **Accessibility Insights for Windows** — inspecteur d'arbre UIA et
   contrôle de contraste.
   - Téléchargement : [accessibilityinsights.io/downloads](https://accessibilityinsights.io/downloads/).
   - Outils utilisés ici : **Inspect** (survol et arbre UIA) et
     **Color Contrast Analyzer** (ratio de contraste).

### 1.2 Configurer l'environnement de test

- Tester sur la **build de l'app empaquetée** (`cargo tauri build` ou
  un binaire de release) plutôt que sur le seul serveur de dev, afin
  que l'exposition UIA reflète la WebView réelle.
- Activer le **« mode parcours »** de NVDA pour explorer, et le **mode
  formulaire** pour interagir (NVDA bascule automatiquement sur la
  plupart des contrôles).
- Préparer une bibliothèque de test contenant au moins **un album
  multi-pistes**, **un artiste**, **un genre** et **une playlist
  existante**, pour exercer tous les parcours.
- Régler l'échelle Windows sur `150 %` au moins une fois pendant la
  campagne (la lisibilité du focus doit tenir, cf. Requirement 1).

### 1.3 Raccourcis activés par défaut dans Qobee

La liste de référence des raccourcis est consultable **dans
l'application** via l'écran d'aide **ShortcutsHelp**, atteignable
depuis **Réglages → Raccourcis clavier** (Requirement 2.6). Cet écran
dérive ses libellés de la source unique `ui/src/lib/labels.ts`, donc
ce qui y est affiché doit correspondre aux `aria-label` annoncés par
NVDA (Requirement 2.4 / 5.5).

Raccourcis attendus (à recouper avec l'écran ShortcutsHelp) :

| Raccourci | Action |
| --- | --- |
| `Espace` | Lecture / Pause |
| `→` / `←` | Avancer / reculer de 5 s |
| `Alt+→` / `Ctrl+→` | Piste suivante |
| `Alt+←` / `Ctrl+←` | Piste précédente |
| `n` | Piste suivante |
| `p` | Piste précédente |
| `f` | Basculer le mode immersif |
| `Échap` | Quitter le mode immersif / vider la recherche |
| `Ctrl+F` | Aller à la recherche et focaliser le champ |
| `Q` | File d'attente (destination Sidebar) |

> Si un raccourci de l'écran ShortcutsHelp ne correspond pas au
> comportement réel, c'est un défaut à consigner (divergence
> libellé ↔ comportement).

---

## 2. Critères transverses à vérifier sur chaque parcours

Pour **chaque** parcours des sections 3 à 5, vérifier systématiquement
les points suivants. Ils correspondent aux critères du Requirement 2.

| Code | Critère | Comment le vérifier |
| --- | --- | --- |
| **V1 — Rôle + nom (R2.1)** | Chaque contrôle interactif (bouton, lien, rangée de piste, champ, curseur) apparaît dans l'arbre UIA comme un élément **distinct et nommé**, avec un rôle correspondant à sa fonction (`button`, `link`, `edit`/`textbox`, `slider`). | Accessibility Insights → **Inspect** : survoler le contrôle, lire `ControlType` et `Name`. Le nom ne doit pas être vide ni générique (« group », « pane »). |
| **V2 — Focus visible (R2.2)** | Au focus clavier, un indicateur visible entoure le contrôle, avec un contraste ≥ `3:1` vs l'arrière-plan adjacent. | Naviguer au `Tab`, observer l'anneau de focus, puis mesurer le contraste avec le **Color Contrast Analyzer**. |
| **V3 — Ordre de tabulation (R2.3)** | `Tab` parcourt les contrôles dans un ordre **logique = ordre visuel**, `Maj+Tab` revient en arrière, et le focus n'est **jamais piégé** sur un contrôle ou un groupe. | Parcourir tout l'écran au `Tab` jusqu'à revenir au point de départ ; tenter de sortir de chaque zone au clavier seul. |
| **V4 — Activation clavier (R2.8)** | Les boutons/liens s'activent à `Entrée` ou `Espace` ; les curseurs s'ajustent aux **flèches** ; aucune action ne requiert la souris. | Sans toucher la souris, focaliser puis activer chaque contrôle. |
| **V5 — Libellés d'icônes (R2.4)** | Tout contrôle représenté **uniquement par une icône** porte un `aria-label` en français décrivant l'action ; NVDA l'annonce. | Écouter l'annonce NVDA au focus de chaque icône ; comparer avec l'infobulle (`title`) — elles doivent être identiques. |
| **V6 — État des bascules (R2.5)** | Quand l'état d'un toggle change (lecture/pause, favori ajouté/retiré, élément sélectionné), le **nom accessible** ou `aria-pressed` se met à jour et NVDA réannonce le nouvel état. | Activer puis désactiver chaque toggle, écouter les deux annonces successives. |
| **V7 — Annonces live (R2.9)** | Quand un toast ou un message de statut apparaît, il est annoncé par NVDA via une **région live** (`role="status"`, `aria-live="polite"`) **sans déplacer le focus** clavier. | Déclencher une action produisant un toast, vérifier que NVDA lit le message et que le focus reste sur le contrôle initial. |

---

## 3. Parcours A — Navigation (Sidebar et vues principales)

Objectif : piloter toute la navigation principale **au clavier et au
lecteur d'écran**, sans souris.

### 3.1 Étapes

1. Lancer NVDA, puis Qobee.
2. Depuis le shell, presser `Tab` pour entrer dans la **Sidebar**.
3. Parcourir au `Tab` chaque destination dans l'ordre :
   **Accueil → Recherche → Albums → Artistes → Genres → Favoris →
   Nouvelle playlist → File d'attente → playlists existantes**.
4. À chaque destination : écouter l'annonce NVDA (nom + rôle), observer
   l'anneau de focus, puis activer avec `Entrée`.
5. Tester `Maj+Tab` pour remonter dans l'ordre inverse.
6. Tester `Ctrl+F` : le focus doit aller dans le champ de **recherche**.
7. Saisir un terme, vérifier que les résultats sont atteignables au
   `Tab` et que `Échap` vide la recherche.
8. Activer l'option **« Sidebar étendue »** (libellés textuels à côté
   des icônes) et revérifier les annonces.

### 3.2 Checklist Navigation

- [ ] V1 — Chaque entrée de Sidebar est un `link`/`button` **nommé**
      dans l'arbre UIA (Inspect), pas un simple `group` anonyme.
- [ ] V5 — Chaque icône de Sidebar (Accueil, Recherche, Albums,
      Artistes, Genres, Favoris, Nouvelle playlist, File d'attente)
      annonce le **libellé français** attendu et son `aria-label` ==
      son infobulle `title`.
- [ ] V3 — `Tab` suit l'ordre visuel haut→bas ; `Maj+Tab` revient ;
      aucun piège de focus dans la liste des playlists.
- [ ] V2 — L'anneau de focus est visible sur chaque entrée (contraste
      ≥ `3:1`).
- [ ] V4 — `Entrée` change bien de vue ; `Ctrl+F` focalise la
      recherche ; `Échap` vide la recherche.
- [ ] V6 — La destination **active/sélectionnée** est exposée (nom
      accessible ou `aria-current`/`aria-pressed`) et réannoncée au
      changement.

---

## 4. Parcours B — Lecture / transport (Mini_Player et mode immersif)

Objectif : contrôler la lecture entièrement au clavier, vérifier les
bascules d'état et les annonces.

### 4.1 Étapes

1. Depuis une page album, atteindre une **rangée de piste** au `Tab`
   et la lancer avec `Entrée` (vérifier V4 sur une rangée non native
   rendue activable au clavier).
2. Atteindre le **Mini_Player** au `Tab` ; parcourir ses contrôles :
   **Piste précédente, Lecture/Pause, Piste suivante, Favori, File
   d'attente, Mode immersif**.
3. Activer **Lecture/Pause** au clavier (`Espace` global, ou `Entrée`
   sur le bouton focalisé) ; écouter le changement d'annonce
   « Lecture » ↔ « Pause ».
4. Ajuster le **curseur de progression** et le **curseur de volume**
   avec les **flèches** ; vérifier que NVDA annonce un rôle `slider`
   et la valeur.
5. Basculer le **favori** de la piste courante ; écouter l'annonce
   « Ajouter aux favoris » → « Retirer des favoris » et vérifier le
   toast correspondant.
6. Ouvrir le **mode immersif** (`f` ou la carte du Mini_Player), puis
   le quitter avec le contrôle **« Quitter le mode immersif »** ou
   `Échap`.
7. Dans le mode immersif, vérifier l'indice clavier visible **« Esc »**
   et le `aria-label`/`aria-pressed` dynamique du favori.

### 4.2 Checklist Lecture / transport

- [ ] V1 — Chaque contrôle de transport est un `button` **nommé** ;
      les curseurs progression et volume sont des `slider` avec
      `Name`, `Value`, `Min`, `Max` dans Inspect.
- [ ] V5 — Les boutons iconographiques (précédent, lecture/pause,
      suivant, favori, file, immersif) annoncent leur libellé FR ;
      `aria-label` == infobulle `title`.
- [ ] V6 — Lecture↔Pause met à jour le nom accessible / `aria-pressed`
      et NVDA réannonce ; idem pour le favori (ajouté↔retiré).
- [ ] V4 — `Espace` bascule lecture/pause ; les **flèches** ajustent
      progression et volume ; tous les boutons s'activent à `Entrée`.
- [ ] V7 — Le toast « Ajouté aux favoris » / « Retiré des favoris »
      est annoncé via la région live **sans** déplacer le focus.
- [ ] V2 — Focus visible sur chaque contrôle du Mini_Player et du mode
      immersif (contraste ≥ `3:1`).
- [ ] R4.2 — Le mode immersif expose un contrôle de sortie explicite
      « Quitter le mode immersif » + indice **« Esc »** visible ;
      `Échap` ferme bien la vue.

---

## 5. Parcours C — Gestion de collection (playlists et file d'attente)

Objectif : ajouter une piste à une playlist et à la file d'attente,
créer une playlist, gérer la file, en validant le feedback annoncé.

### 5.1 Étapes

1. Sur une rangée de piste, ouvrir le menu contextuel au clavier et
   choisir **« Ajouter à la playlist… »**.
2. Dans le **sous-menu**, vérifier la liste des playlists existantes +
   l'action **« Nouvelle playlist »** ; naviguer au `Tab`/flèches.
3. Ajouter la piste à une playlist `P` ; écouter le toast
   **« Ajouté à `P` »** annoncé via la région live.
4. Choisir **« Nouvelle playlist »**, saisir un nom dans la modale,
   puis valider avec la touche **`Entrée`** (sans cliquer le bouton).
   Vérifier que la modale est centrée et atteignable au clavier.
5. Sur une piste, déclencher **« Ajouter à la file »** ; écouter le
   toast **« Ajouté à la file »**.
6. Ouvrir la **File d'attente** (destination Sidebar `Q`, ou depuis le
   Mini_Player). Vérifier l'**état vide** explicite quand la file est
   vide.
7. Dans la file, parcourir les rangées au `Tab`, tester l'action
   **« Vider la file »** et la **réorganisation** ; écouter les
   annonces de statut.

### 5.2 Checklist Gestion de collection

- [ ] V1 — Le sous-menu « Ajouter à la playlist… », ses entrées, la
      modale de création et les rangées de la file sont des éléments
      UIA **nommés** avec un rôle correct (`menu`/`menuitem`,
      `dialog`, `button`, `listitem`).
- [ ] V4 — La modale de création se valide à **`Entrée`** depuis le
      champ de nom (équivalent au bouton de confirmation) ; toutes les
      actions s'activent au clavier.
- [ ] V7 — Les toasts **« Ajouté à `P` »**, **« Ajouté à la file »**,
      et le toast d'**erreur** (si la persistance échoue) sont annoncés
      via la région live, sans vol de focus.
- [ ] V3 — `Tab`/`Maj+Tab` parcourent le sous-menu, la modale et la
      file sans piège de focus ; `Échap` ferme la modale.
- [ ] R3.6 — L'état vide de la file affiche un message FR explicite
      décrivant **comment ajouter des pistes**, lu par NVDA.
- [ ] V2 — Focus visible sur les entrées de menu, le champ de la
      modale et les rangées de file (contraste ≥ `3:1`).
- [ ] V5 / V6 — Icônes d'action de la file (vider, déplacer) nommées ;
      l'élément **sélectionné** expose son état.

---

## 6. Audit ciblé avec Accessibility Insights

En complément des parcours, réaliser deux audits transverses :

1. **Arbre UI Automation (Inspect).** Parcourir l'arbre depuis la
   fenêtre Qobee : vérifier que les contrôles internes émergent
   **au-delà** du `BrowserRootView` (Requirement 2.1) — pas un seul
   nœud opaque. Noter tout contrôle interactif sans `Name` ou avec un
   `ControlType` incorrect.
2. **Contraste (Color Contrast Analyzer).** Mesurer l'anneau de focus
   sur fond clair et fond sombre des composants principaux ; consigner
   les ratios. Tout ratio `< 3:1` sur un indicateur de focus est un
   défaut (Requirement 2.2).

---

## 7. Comment consigner les résultats

Pour chaque campagne, créer une entrée datée (par exemple un fichier
`docs/a11y-runs/AAAA-MM-JJ.md` ou un ticket) reprenant le tableau
ci-dessous. Reporter **un statut par critère et par parcours**.

### 7.1 Métadonnées de campagne

- **Date** :
- **Version Qobee** (tag / commit) :
- **Version NVDA** :
- **Version Accessibility Insights** :
- **OS / build Windows** :
- **Échelle d'affichage testée** (`100 %` / `150 %` / `200 %`) :

### 7.2 Tableau de résultats

| Parcours | V1 Rôle+nom | V2 Focus | V3 Tab | V4 Clavier | V5 Icônes | V6 États | V7 Live |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A — Navigation | | | | | | | |
| B — Lecture/transport | | | | | | | |
| C — Gestion collection | | | | | | | |

Légende des statuts : **OK** (conforme) · **KO** (défaut, à corriger) ·
**N/A** (non applicable) · **À revoir** (comportement ambigu).

### 7.3 Journal des défauts

Pour chaque case **KO** ou **À revoir**, consigner :

- **Parcours / critère** (ex. « B / V6 ») :
- **Contrôle concerné** (nom UIA ou sélecteur) :
- **Comportement observé** (annonce NVDA, ratio mesuré, etc.) :
- **Comportement attendu** (renvoyer au critère R2.x) :
- **Capture / valeur mesurée** :
- **Suite donnée** (issue créée, correctif, exception documentée) :

### 7.4 Critère de réussite de la campagne

La campagne est considérée **réussie** quand, pour les trois parcours,
les sept critères V1–V7 sont **OK** (ou **N/A** justifié), et que
l'audit Accessibility Insights ne relève **aucun** contrôle interactif
anonyme dans l'arbre UIA ni d'indicateur de focus sous `3:1`. Toute
case **KO** doit être tracée dans le journal des défauts avec une suite
donnée avant de clore la campagne.
