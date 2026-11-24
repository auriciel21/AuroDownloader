# Dépendances corrigées

Copies de crates publiées, modifiées pour AuroDownloader et branchées par
`[patch.crates-io]` (`Cargo.toml` de la racine et `apps/desktop/Cargo.toml`). Elles sont
exclues de l'espace de travail : ni `clippy` ni les tests du projet ne les parcourent.

Licence : Apache-2.0 (texte complet dans `LICENSE-APACHE-2.0`), compatible avec la
GPL-3.0 ou ultérieure du projet (SPEC 8.3). Chaque fichier modifié le dit en tête.

| Crate | Version d'origine | Modification |
|---|---|---|
| `librqbit-dualstack-sockets` | 0.7.0 (commit d'origine dans `.cargo_vcs_info.json`) | Liaison à une interface sous Windows (`src/bind_device.rs`, `src/socket.rs`, `src/connect.rs`, `Cargo.toml`) |
| `librqbit` | 9.1.0 (commit 4309d38769ef93f6ee4f01a1ab147009a197fa8f) | Client HTTP des trackers lié à l'interface sous Windows, et jamais le proxy du système (`src/session.rs`) |

## Liaison à une interface sous Windows (SPEC 7.16, étape 3d, 2026-11-09)

Décision de l'utilisateur du 2026-11-09 : librqbit ne lie pas ses connexions à une interface
réseau sous Windows (`BindDeviceNotSupported`) ; le projet l'ajoute ici.

- L'interface est désignée par son nom affiché (« Wi-Fi », nom de l'adaptateur du VPN),
  résolu par `network-interface`, en index et en adresses. Une interface sans adresse
  utilisable (hors adresses locales au lien : 169.254/16, fe80::/10) est refusée.
- Chaque socket reçoit `IP_UNICAST_IF` (et `IPV6_UNICAST_IF` en IPv6) puis se lie à
  l'adresse de l'interface : une socket demandée sur une adresse non précisée prend
  l'adresse IPv4 de l'interface, sinon son adresse IPv6. Liée ainsi, elle ne reçoit que
  ce qui arrive sur l'interface et, par le modèle d'hôte strict de Windows, n'envoie que
  par elle.
- Une connexion vers un pair d'une famille d'adresses que l'interface n'a pas est refusée,
  jamais tentée par une autre interface.
- Les requêtes HTTP des trackers partent de l'adresse de l'interface (`local_address`).

## Jamais le proxy du système (audit V6, 2026-11-24)

Le client reqwest de librqbit (annonces aux trackers HTTP) suivait le proxy du système
(variables `HTTP_PROXY`, `HTTPS_PROXY`, réglages de Windows) quand la session n'en a pas :
les torrents ne passent jamais par un proxy (SPEC 7.16, F-07). `no_proxy()` sur ce client.

Mise à jour d'une de ces crates : reprendre la nouvelle version publiée, y reporter les
modifications (marquées « AuroDownloader »), et mettre à jour ce tableau. Une contribution
au projet librqbit les rendrait inutiles.
