# Plugins d'hébergeurs

Un plugin d'AuroDownloader lit la page d'un hébergeur de fichiers et en donne les liens
directs ; le moteur les télécharge ensuite lui-même (SPEC §7.20, F-41). Ce dossier contient
un plugin d'exemple, `archive-org` (fichiers d'un élément d'Internet Archive), écrit en
Rust. C'est un espace de travail Cargo à part, hors de celui du projet.

## Construire et installer l'exemple

```
cd plugins
cargo test                                             # sur la machine
cargo build --release --target wasm32-unknown-unknown  # rustup target add wasm32-unknown-unknown
auro plugin install target/wasm32-unknown-unknown/release/archive_org.wasm   # montre, puis --yes
auro plugin resolve https://archive.org/details/art_of_war_librivox
```

Dans l'application de bureau : Options, rubrique « Plugins », « Installer un plugin… ».
Ensuite, une page `https://archive.org/details/…` collée dans « Ajouter » est lue par le
plugin.

## Ce qu'un plugin peut faire

- Il tourne dans un bac à sable WebAssembly (wasmi). Il n'a ni fichiers, ni système, ni
  horloge, ni réseau propre. Une instance neuve sert chaque résolution.
- Ses limites : 5·10⁹ unités de calcul, 64 Mio de mémoire, 10 000 appels imbriqués et
  60 s en tout.
- Ses requêtes passent par le moteur (`auro.fetch`), seulement vers les domaines déclarés
  dans son manifeste (et leurs sous-domaines), sur le réseau public, redirections
  comprises. Ce sont des `GET` ou des `POST` de formulaire (64 Kio au plus), avec les seuls
  en-têtes `Referer`, `Accept`, `Accept-Language`, `Content-Type` et `X-Requested-With`.
  Il a droit à 32 requêtes, avec des réponses de 8 Mio au plus.
- Il ne voit jamais un secret. Les cookies posés par le site sont gardés par le moteur,
  renvoyés au site, puis donnés au téléchargement des liens.
- Ses liens sont des adresses `http(s)`, 1 000 au plus. Ils sont téléchargés vers le réseau
  public seulement. Un lien expiré (`403`, `410`) est redemandé une fois au plugin.

## Interface (version 1)

Le module [`archive-org/src/auro.rs`](archive-org/src/auro.rs) la met en œuvre ; il se
recopie dans un autre plugin.

- Compilation : `wasm32-unknown-unknown`, `crate-type = ["cdylib"]`. Le plugin ne peut
  importer que `auro.fetch` et `auro.log`.
- Exports :
  - `memory` ;
  - `auro_alloc(taille: i32) -> i32` : une zone où l'hôte écrit l'entrée et les réponses ;
  - `auro_resolve(adresse: i32, taille: i32) -> i64` : rend `(adresse << 32) | taille` de
    sa sortie.
- Entrée : `{"url": "<page>"}`.
- Sortie : `{"links": [{"url", "name"?, "size"?, "referer"?}]}`, ou `{"error": "<texte>"}`.
- `auro.fetch(adresse, taille) -> i64` :
  - requête : `{"url", "method"?: "GET"|"POST", "headers"?: {…}, "form"?: {…}}` ;
  - réponse : `{"status", "url", "headers": {"content-type"}, "body"}`, ou `{"error"}`,
    écrite dans une zone obtenue par `auro_alloc`.
- `auro.log(adresse, taille)` : une ligne montrée à l'utilisateur, sous les liens de la
  page (50 lignes au plus).

## Manifeste

Il est rangé dans la section personnalisée `auro-plugin` du fichier `.wasm`, en JSON. Une
seule empreinte (SHA-256 du fichier) couvre ainsi le code et les permissions. L'exemple
l'y range à la compilation, sans outil :

```rust
#[cfg(target_arch = "wasm32")]
#[unsafe(link_section = "auro-plugin")]
#[used]
static MANIFEST: [u8; include_bytes!("../manifest.json").len()] = *include_bytes!("../manifest.json");
```

Ses champs :
- `abi` : `1` ;
- `id` : de 3 à 64 caractères `a-z`, `0-9`, `.` et `-` ;
- `name`, `version` : obligatoires ;
- `author`, `description` : facultatifs ;
- `sites` : de 1 à 16 domaines des pages prises en charge ;
- `fetch` : de 1 à 16 domaines que le plugin peut faire demander.

Les domaines s'écrivent en minuscules, sans joker ni adresse IP. Les textes font
200 caractères au plus.

## Installation

Un plugin ne s'installe que depuis un fichier choisi par l'utilisateur, après avoir montré
ses domaines et son empreinte, et sur cette empreinte. Le fichier est revérifié à chaque
chargement. Une nouvelle version (même `id`) se montre et s'accepte de nouveau. Il n'y a
ni catalogue ni mise à jour automatique.
