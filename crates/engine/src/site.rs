//! Copie d'un site pour la navigation hors ligne (F-43, SPEC §7.18 ter, étape B du
//! « Grabber ») : une tâche de sorte `site` lit le site avec les règles de l'exploration
//! (`linkscan::crawl`), enregistre chaque page lue avec ses ressources d'affichage (images,
//! styles, scripts, polices) et les fichiers cochés, puis réécrit les liens.
//!
//! - [`html`] : liens d'une page et réécriture ;
//! - [`css`] : adresses d'un style et réécriture ;
//! - [`layout`] : de l'adresse au chemin dans le dossier de la copie.

pub mod css;
pub mod html;
pub mod layout;
pub mod run;
