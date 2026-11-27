//! Projets dans la boucle du moteur (SPEC §7.18 quater, F-44) : enregistrement, lancement à
//! la main ou à l'heure dite, occurrence manquée faite une fois au démarrage suivant,
//! occurrence sautée quand la tâche du projet tourne encore.
//!
//! Un projet a une tâche de copie, créée à son premier lancement et reprise aux suivants :
//! mise à jour sur place (`update_site`) d'une copie finie ou en échec, reprise d'une copie
//! en pause ; une copie annulée ou retirée en fait créer une nouvelle.

use std::time::Duration;

use tokio::time::Instant;

use super::{AddTask, EngineError, Event, Scheduler, SiteRequest, check_site};
use crate::store::projects::{self, Project, ProjectSpec, Schedule};
use crate::store::tasks::{self, TaskState, load_task};

/// Nom d'un projet au plus, en caractères.
pub const MAX_PROJECT_NAME: usize = 100;

/// La copie que lance un projet. Une copie sans aucun filtre ne prend pas de fichier ; des
/// fichiers seuls sans filtre les prennent tous (comme `auro links` sans option).
pub(super) fn site_request(spec: &ProjectSpec) -> SiteRequest {
    let s = &spec.settings;
    let files_only = !spec.copy_site;
    SiteRequest {
        page_url: spec.page_url.clone(),
        depth: Some(s.depth),
        max_pages: Some(s.max_pages),
        max_bytes: Some(s.max_bytes),
        exclude: s.exclude.clone(),
        same_folder: s.same_folder,
        ignore_robots: s.ignore_robots,
        files: Vec::new(),
        files_only,
        keep_tree: spec.keep_tree,
        filters: (files_only || !spec.filters.is_empty()).then(|| spec.filters.clone()),
    }
}

impl Scheduler {
    pub(super) fn projects(&self) -> Result<Vec<Project>, EngineError> {
        Ok(projects::list(&self.conn)?)
    }

    /// Prochain lancement d'un programme, en millisecondes Unix, d'après l'heure locale du
    /// moteur (horloge des plages, réglable dans les tests).
    fn next_run_of(&self, schedule: Schedule) -> i64 {
        let now = self.now_local();
        let local = crate::schedule::next_at(schedule.minute, schedule.days, now.local_ms());
        now.utc_of(&self.conn, local)
    }

    /// Crée (sans `project_id`) ou remplace un projet, vérifié comme la copie qu'il lancera ;
    /// `remote` : par une connexion à jeton (dossier sous celui des téléchargements, réseau
    /// public seulement pour un projet qu'elle crée).
    pub(super) fn set_project(
        &mut self,
        project_id: Option<String>,
        mut spec: ProjectSpec,
        remote: bool,
        public_only: bool,
    ) -> Result<String, EngineError> {
        spec.name = spec.name.trim().to_owned();
        if spec.name.is_empty() || spec.name.chars().count() > MAX_PROJECT_NAME {
            return Err(EngineError::BadRequest(format!(
                "projet : un nom de 1 à {MAX_PROJECT_NAME} caractères"
            )));
        }
        if projects::name_taken(&self.conn, &spec.name, project_id.as_deref())? {
            return Err(EngineError::ProjectExists(spec.name));
        }
        let checked = check_site(&site_request(&spec))?;
        spec.page_url = checked.page.to_string();
        spec.settings = checked.settings;
        if let Some(filters) = checked.filters {
            spec.filters = filters;
        }
        // Dossier et file : vérifiés maintenant, gardés tels quels (un dossier absent suit
        // le réglage des téléchargements à chaque lancement).
        self.destination(spec.dest_dir.clone(), spec.queue_id.clone(), remote)?;
        let next = spec.schedule.map(|s| self.next_run_of(s));
        let id = match project_id {
            Some(id) => {
                if !projects::replace(&self.conn, &id, &spec, next)? {
                    return Err(EngineError::ProjectNotFound(id));
                }
                id
            }
            None => projects::create(&self.conn, &spec, next, public_only)?,
        };
        self.projects_due = None;
        self.project_changed(&id);
        Ok(id)
    }

    /// Supprime un projet ; sa tâche et ses fichiers restent.
    pub(super) fn delete_project(&mut self, project_id: &str) -> Result<(), EngineError> {
        if !projects::delete(&self.conn, project_id)? {
            return Err(EngineError::ProjectNotFound(project_id.to_owned()));
        }
        self.projects_due = None;
        let project_id = project_id.to_owned();
        self.emit(|rev| Event::ProjectChanged {
            rev,
            project_id,
            project: None,
        });
        Ok(())
    }

    /// Lance un projet : sa copie mise à jour, reprise, ou créée. Une copie qui tourne encore
    /// est refusée (`INVALID_STATE`).
    pub(super) fn run_project(&mut self, project_id: &str) -> Result<String, EngineError> {
        let project = projects::get(&self.conn, project_id)?
            .ok_or_else(|| EngineError::ProjectNotFound(project_id.to_owned()))?;
        let task = match &project.task_id {
            Some(id) => load_task(&self.conn, id).ok(),
            None => None,
        };
        let task_id = match task {
            Some(task)
                if self.running.contains_key(&task.id)
                    || !matches!(
                        task.state,
                        TaskState::Paused
                            | TaskState::Completed
                            | TaskState::Failed
                            | TaskState::Cancelled
                    ) =>
            {
                return Err(Self::invalid(&task, "lancement du projet"));
            }
            Some(task) if task.state == TaskState::Paused => {
                self.resume(&task.id)?;
                task.id
            }
            Some(task)
                if matches!(task.state, TaskState::Completed | TaskState::Failed)
                    && task.file_name.is_some()
                    && self.same_copy(&task.id, &project.spec)? =>
            {
                // Les réglages du projet, peut-être changés depuis, valent pour ce lancement.
                let request = site_request(&project.spec);
                let checked = check_site(&request)?;
                crate::store::site::set_settings(
                    &self.conn,
                    &task.id,
                    &checked.settings,
                    checked.filters.as_ref(),
                    project.spec.keep_tree,
                )?;
                self.update_site(&task.id)?;
                task.id
            }
            // Jamais lancé, retiré, annulé, ou en échec avant d'avoir son dossier : une
            // nouvelle copie.
            _ => self.add_site(
                AddTask {
                    site: Some(site_request(&project.spec)),
                    dest_dir: project.spec.dest_dir.clone(),
                    queue_id: project.spec.queue_id.clone(),
                    remote: project.public_only,
                    public_only: project.public_only,
                    ..AddTask::default()
                },
                tasks::Start::default(),
            )?,
        };
        let now = self.now_local().utc_ms;
        projects::mark_run(&self.conn, project_id, &task_id, now)?;
        self.project_changed(project_id);
        Ok(task_id)
    }

    /// Annonce un projet tel qu'il est maintenant.
    fn project_changed(&self, project_id: &str) {
        let project = projects::get(&self.conn, project_id)
            .ok()
            .flatten()
            .map(Box::new);
        let project_id = project_id.to_owned();
        self.emit(|rev| Event::ProjectChanged {
            rev,
            project_id,
            project,
        });
    }

    /// La copie `task_id` est-elle celle que le projet lancerait : même page, même mode ?
    /// Sinon (projet modifié), une nouvelle copie.
    fn same_copy(&self, task_id: &str, spec: &ProjectSpec) -> Result<bool, EngineError> {
        let job = crate::store::site::load_job(&self.conn, task_id)?;
        let mode = if spec.copy_site {
            crate::store::site::Mode::Copy
        } else {
            crate::store::site::Mode::Files
        };
        Ok(job.page_url == spec.page_url && job.mode == mode)
    }

    /// Lance les projets dus (une seule fois chacun, même après plusieurs occurrences
    /// manquées), puis prévoit leur occurrence suivante ; une occurrence dont la copie tourne
    /// encore est sautée.
    pub(super) fn check_projects(&mut self) {
        let now = self.now_local().utc_ms;
        if self.projects_due.is_none() {
            self.projects_due = Some(projects::next_due(&self.conn).ok().flatten());
        }
        if !self.projects_due.flatten().is_some_and(|at| at <= now) {
            return;
        }
        let due = projects::due(&self.conn, now).unwrap_or_default();
        for project in due {
            match self.run_project(&project.id) {
                Ok(_) => {}
                Err(EngineError::InvalidState { .. }) => {
                    let _ = projects::mark_skipped(&self.conn, &project.id, now);
                    tracing::info!(projet = %project.id, "projet encore en cours : occurrence sautée");
                }
                Err(e) => {
                    tracing::warn!(projet = %project.id, erreur = %e, "projet non lancé");
                }
            }
            // Une seule fois : le programme s'efface ; sinon l'occurrence suivante.
            let next = project
                .spec
                .schedule
                .filter(|s| s.days != 0)
                .map(|s| self.next_run_of(s));
            let _ = projects::set_next_run(&self.conn, &project.id, next);
            self.project_changed(&project.id);
        }
        self.projects_due = Some(projects::next_due(&self.conn).ok().flatten());
    }

    /// Délai jusqu'au prochain lancement prévu d'un projet.
    pub(super) fn projects_wake(&self, _now: Instant) -> Duration {
        match self.projects_due {
            None => Duration::ZERO,
            Some(None) => Duration::MAX,
            Some(Some(at)) => {
                let now = self.now_local().utc_ms;
                Duration::from_millis(u64::try_from(at.saturating_sub(now)).unwrap_or(0))
            }
        }
    }
}

#[cfg(test)]
mod tests;
