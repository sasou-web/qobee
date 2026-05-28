# Configuration Google Cloud pour Qobee

Cette documentation décrit comment créer ton propre client OAuth Google
afin de connecter Qobee à un dossier Google Drive. Qobee n'utilise
**aucun** client OAuth partagé : chaque utilisateur configure le sien,
ce qui garantit qu'aucune donnée de bibliothèque ne transite par un
tiers.

> **Pourquoi cette étape ?** L'API Google Drive impose un client OAuth
> par application. Pour les apps de bureau non vérifiées, Google
> exige aussi que les comptes utilisés soient déclarés en _test users_
> tant que l'app n'est pas passée en production. C'est ici qu'on
> configure tout ça.

---

## 1. Créer un projet Google Cloud

1. Ouvre [Google Cloud Console](https://console.cloud.google.com/).
2. En haut à gauche, ouvre le sélecteur de projet puis clique sur
   **« Nouveau projet »**.
3. Nomme le projet (par exemple `qobee-personal`) et valide.
4. Sélectionne le projet une fois créé.

---

## 2. Activer l'API Google Drive

1. Va dans **APIs & Services → Library**
   ([lien direct](https://console.cloud.google.com/apis/library/drive.googleapis.com)).
2. Cherche **Google Drive API** et clique sur **Enable**.
3. Attends quelques secondes que l'activation soit prise en compte.

---

## 3. Configurer l'écran de consentement OAuth

1. Va dans **APIs & Services → OAuth consent screen**
   ([lien direct](https://console.cloud.google.com/apis/credentials/consent)).
2. Choisis le type **External** et clique sur **Create**.
3. Remplis les informations obligatoires :
   - **App name** : `Qobee` (ou ce que tu veux)
   - **User support email** : ton email
   - **Developer contact information** : ton email
4. Sur l'écran **Scopes**, ajoute exactement les deux scopes suivants
   (rien d'autre — Qobee ne demande que la lecture, en lecture seule) :
   - `https://www.googleapis.com/auth/drive.readonly`
   - `https://www.googleapis.com/auth/drive.metadata.readonly`
5. Sur l'écran **Test users**, ajoute **ton adresse Google** (celle
   du compte qui contient le dossier Drive à connecter). Tant que
   l'application reste en mode `Testing`, **seuls les test users
   listés peuvent passer le consentement** ; tous les autres comptes
   verront l'erreur `access_denied`.
6. Termine et reviens au tableau de bord.

---

## 4. Créer un client OAuth « Desktop »

1. Va dans **APIs & Services → Credentials**
   ([lien direct](https://console.cloud.google.com/apis/credentials)).
2. Clique sur **Create Credentials → OAuth client ID**.
3. Choisis **Application type : Desktop app**, donne-lui un nom
   (par exemple `Qobee Desktop`) puis **Create**.
4. Une popup affiche le **Client ID** et le **Client Secret**.
   Copie-les : ce sont ces deux chaînes que Qobee va te demander
   dans la fenêtre de connexion à Google Drive.

> **Note sur la redirect URI.** Le client de type _Desktop app_ ne
> nécessite **pas** de redirect URI explicite : Google accepte
> automatiquement n'importe quel port local de la forme
> `http://127.0.0.1:<port>/qobee/oauth/callback`. Qobee choisit un
> port libre à chaque flow ; il n'y a rien à enregistrer côté Google
> Cloud Console.

---

## 5. Connecter Qobee

1. Dans Qobee, ouvre **Settings → Remote sources → + Connect Google
   Drive**.
2. Colle le **Client ID** et le **Client Secret** obtenus à l'étape 4.
3. Clique sur **Authorize in browser**. Ton navigateur s'ouvre sur
   la page de consentement Google.
4. Choisis le compte Google **déclaré en test user** à l'étape 3.
5. Accepte les autorisations `drive.readonly` et
   `drive.metadata.readonly`. Qobee indexe ensuite ton dossier Drive.

---

## Passer en production (optionnel)

Tant que tu es le seul utilisateur de ton client OAuth, **tu peux
laisser le projet en mode `Testing`** : il n'y a aucune limite tant
que tu restes dans la liste des _test users_.

Si tu veux ouvrir l'accès à d'autres personnes (famille,
collaborateurs), tu peux passer en production :

1. Sur l'écran **OAuth consent screen**, clique sur **Publish App**.
2. Google déclenche un processus de vérification (logo, domaine,
   homepage). Pour les scopes `drive.readonly` /
   `drive.metadata.readonly`, le processus peut prendre plusieurs
   semaines.
3. Une fois publié, n'importe quel compte Google peut autoriser
   Qobee sans figurer dans la liste des _test users_.

Pour un usage personnel, **rester en `Testing` est largement
suffisant**.

---

## Dépannage

### « Connexion refusée » (`access_denied`)

C'est l'erreur la plus fréquente. Causes habituelles :

- Le compte Google utilisé **n'est pas dans la liste des test
  users** (étape 3 ci-dessus). Solution : ajouter l'email à la liste,
  attendre quelques minutes, retenter.
- L'application est gérée par un compte **Google Workspace** dont
  l'admin a interdit les apps tierces. Le message Google précise
  alors `admin_policy_enforced`. Solution : demander à l'admin
  Workspace d'autoriser l'app, ou utiliser un compte personnel.
- Le client OAuth a été créé avec le **mauvais type** (Web app,
  Android, etc.). Le code Google est `unauthorized_client`.
  Solution : recréer un client en **Desktop app**.
- Tu as **refusé un ou plusieurs scopes** sur l'écran de
  consentement. Solution : retenter et accepter les deux scopes
  `drive.readonly` et `drive.metadata.readonly`.

### « Google did not return a refresh_token »

Google ne renvoie un _refresh token_ que lors du **premier**
consentement d'un client. Si tu as déjà autorisé Qobee sur ce compte
puis l'as révoqué, va sur
[myaccount.google.com → Sécurité → Applications tierces](https://myaccount.google.com/permissions),
supprime l'entrée Qobee, puis relance le flow.

### Aucun fichier audio détecté après l'indexation

L'indexation parcourt **uniquement le dossier choisi** (et ses
sous-dossiers). Vérifie que tu as bien choisi le bon dossier dans le
sélecteur Qobee, et que celui-ci contient des fichiers audio
(`.flac`, `.wav`, `.mp3`, `.m4a`, `.aac`, `.ogg`, `.opus`).

---

## Liens utiles

- [Google Cloud Console — Credentials](https://console.cloud.google.com/apis/credentials)
- [Google Cloud Console — OAuth consent screen](https://console.cloud.google.com/apis/credentials/consent)
- [Google Cloud Console — Drive API library](https://console.cloud.google.com/apis/library/drive.googleapis.com)
- [Documentation OAuth 2.0 pour applications de bureau](https://developers.google.com/identity/protocols/oauth2/native-app)
- [Tableau de bord des autorisations Google](https://myaccount.google.com/permissions)
