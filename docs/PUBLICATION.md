# Publication d'AuroDownloader

Procédure du mainteneur (phase 7, étape 8 ; SPEC 4.6). Tout ce qui crée un dépôt, une clé
ou un secret, et tout envoi vers GitHub, se fait par le mainteneur lui-même : jamais par
Claude. Les clés privées ne quittent pas sa machine, sauf vers les secrets GitHub.

Vue d'ensemble :

| Quoi | Où | Obligatoire |
|---|---|---|
| Dépôt `auriciel21/AuroDownloader`, public | GitHub | Oui |
| Clé des mises à jour (Tauri) | Machine du mainteneur, secrets GitHub, clé publique dans `tauri.conf.json` | Oui |
| Magasin de clés Android | Machine du mainteneur, secrets GitHub | Oui |
| Signature Windows par SignPath Foundation | Compte SignPath, secrets et variables GitHub | Non : sans elle, installateurs non signés |

## 1. Créer le dépôt et y pousser le code

1. Le dépôt **auriciel21/AuroDownloader** existe depuis le 2026-11-25 : **public**, créé
   vide (sans README, licence ni `.gitignore` : ils existent déjà). Son adresse est écrite
   dans les README, le guide, `scripts/latest-json.mjs` et l'application de bureau
   (`update.rs`, adresse des mises à jour de `tauri.conf.json`) : la changer partout si le
   dépôt change de nom ou de propriétaire.
2. Dans le dossier du projet :

   ```
   git remote add origin https://github.com/auriciel21/AuroDownloader.git
   git push -u origin main
   ```

3. Réglages du dépôt, « Actions », « General » : autoriser les actions ; « Workflow
   permissions » : laisser « Read repository contents » (la publication demande elle-même
   le droit d'écrire, pour son seul brouillon).
4. Le premier envoi lance la CI (`.github/workflows/ci.yml`). Elle n'a jamais tourné avant :
   voir la section 6 pour ce qui reste à vérifier à ce premier passage.

## 2. Clé des mises à jour (obligatoire)

Elle signe chaque mise à jour du bureau ; l'application refuse une mise à jour qui n'est pas
signée par elle.

1. Créer la clé, avec un mot de passe solide :

   ```
   cargo tauri signer generate -w %USERPROFILE%\.tauri\aurodownloader.key
   ```

   Deux fichiers : `aurodownloader.key` (privée) et `aurodownloader.key.pub` (publique).
2. Copier le contenu de `aurodownloader.key.pub` dans `apps/desktop/tauri.conf.json`, à la
   place de la valeur vide de `plugins.updater.pubkey` ; faire un commit et le pousser. Tant
   que cette valeur est vide, la mise à jour reste « non configurée » et la publication
   refuse de démarrer.
3. Dans le dépôt, « Settings », « Secrets and variables », « Actions », « New repository
   secret » :
   - `TAURI_SIGNING_PRIVATE_KEY` : tout le contenu du fichier `aurodownloader.key` ;
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` : son mot de passe.
4. **Sauvegarde** : garder la clé privée et son mot de passe en deux endroits sûrs (par
   exemple un gestionnaire de mots de passe et une copie hors ligne). Perdue, plus aucune
   mise à jour ne peut atteindre les installations existantes ; volée, quelqu'un qui
   contrôlerait aussi les publications pourrait faire passer une fausse mise à jour.

## 3. Magasin de clés Android (obligatoire)

Il signe l'APK. Android n'installe une nouvelle version par-dessus l'ancienne que si elle est
signée par la même clé.

1. Créer le magasin (le `keytool` du JDK) :

   ```
   keytool -genkeypair -v -keystore aurodownloader.jks -alias aurodownloader -keyalg RSA -keysize 4096 -validity 10000
   ```

2. Son contenu en base64, copié dans le presse-papiers (PowerShell) :

   ```
   [Convert]::ToBase64String([IO.File]::ReadAllBytes("aurodownloader.jks")) | Set-Clipboard
   ```

3. Secrets du dépôt :
   - `ANDROID_KEYSTORE_BASE64` : ce texte ;
   - `ANDROID_KEYSTORE_PASSWORD` : le mot de passe du magasin ;
   - `ANDROID_KEY_ALIAS` : `aurodownloader` ;
   - `ANDROID_KEY_PASSWORD` : le mot de passe de la clé (souvent le même).
4. **Sauvegarde** : comme pour la clé des mises à jour. Perdu, les utilisateurs devront
   désinstaller l'application pour installer une version signée d'une autre clé.

## 4. Signature Windows par SignPath (facultative)

Sans elle, Windows SmartScreen affiche « éditeur inconnu » à l'installation ; tout le reste
fonctionne. Elle s'active d'elle-même quand le secret `SIGNPATH_API_TOKEN` existe.

1. Relire les conditions de SignPath Foundation (`signpath.org`, page des conditions) : elles
   peuvent changer. Au 2026-11-18 : licence libre reconnue par l'OSI (GPL-3.0 : oui), dépôt
   public, paquets construits par la CI à partir de ce dépôt, mention de SignPath et page de
   politique de signature publiées par le projet ; l'éditeur affiché par Windows est
   « SignPath Foundation ».
2. Déposer la demande. Une fois le projet accepté, dans SignPath : une configuration
   d'artefact qui signe le fichier `.exe` de l'artefact (installateur NSIS), et une
   politique de signature de publication.
3. Dans le dépôt :
   - secret `SIGNPATH_API_TOKEN` ;
   - variables (« Variables », pas « Secrets ») `SIGNPATH_ORGANIZATION_ID`,
     `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG`.
4. À la publication suivante, chaque installateur Windows part chez SignPath, revient signé,
   et sa signature est vérifiée avant d'être signé pour la mise à jour.

## 5. Publier une version

1. Régler la version partout, puis rafraîchir les `Cargo.lock` :

   ```
   sh scripts/version.sh 0.2.0
   cargo update --workspace --offline
   cargo update --workspace --offline --manifest-path apps/desktop/Cargo.toml
   ```

   `sh scripts/version.sh` seul affiche la version de chaque fichier ; `--check 0.2.0`
   vérifie qu'ils portent tous celle-là (c'est ce que fait la publication).
2. Faire passer la CI sur ce commit (pousser `main`, attendre le vert).
3. Créer l'étiquette **annotée** : son message devient les notes de la version, montrées
   dans l'application avant la mise à jour (texte simple, en français et en anglais si
   possible).

   ```
   git tag -a v0.2.0 -m "Notes de la version…"
   git push origin v0.2.0
   ```

4. Le workflow « Publication » (`.github/workflows/release.yml`) :
   - vérifie la version, la clé publique et les secrets ;
   - construit les installateurs NSIS (x64 et ARM64), les `.deb` et AppImage (x64 et
     aarch64), l'APK (arm64) ;
   - fait signer les installateurs Windows par SignPath si elle est configurée ;
   - signe les installateurs et les AppImage pour la mise à jour, puis l'APK ;
   - prépare un **brouillon** de publication avec `latest.json` et `SHA256SUMS`.
5. Relire le brouillon (onglet « Releases ») : quatre installateurs ou AppImage avec leur
   `.sig`, deux `.deb`, l'APK, `latest.json`, `SHA256SUMS` ; essayer au moins un installateur.
   Puis « Publish release ».
6. Une fois publiée, elle devient la « dernière publication » : les applications installées
   la trouvent dans la journée (au plus une recherche par jour), et la proposent.

Une publication ratée : supprimer le brouillon et l'étiquette (`git push origin
:refs/tags/v0.2.0`, puis `git tag -d v0.2.0`), corriger, recommencer.

## 6. À vérifier au premier passage

Écrit et essayé en local autant que possible, mais jamais lancé sur GitHub :

- les machines ARM (`windows-11-arm`, `ubuntu-24.04-arm`, `ubuntu-22.04-arm`) : la
  compilation d'aws-lc sous Windows ARM64 peut demander un outil de plus (clang, NASM) ;
- l'émulateur Android de la CI (tests instrumentés) ;
- l'AppImage aarch64 (outil de Tauri pour cette architecture) ;
- SignPath, quand elle sera configurée.

La première version publiée sert d'essai de bout en bout de la mise à jour : installer la
version précédente, publier la suivante, vérifier que l'application la propose, l'installe et
redémarre.
