//! Planification dans la boucle du moteur (F-16, SPEC §7.11) : une file planifiée ne
//! démarre rien hors de ses plages, ses tâches en cours retournent en file quand elle
//! ferme, et son action de fin (veille, extinction) est annoncée puis exécutée quand elle
//! n'a plus rien à télécharger.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use rusqlite::Connection;
use tokio::time::Instant;

use super::{EngineError, Event, PendingEnd, ScheduleView, Scheduler};
use crate::schedule::{EndAction, LocalNow, Rules, Schedule};
use crate::store::tasks::{self, TaskState, count_pending};
use crate::store::{now_ms, schedules};

/// État de la planification, tenu par la boucle.
#[derive(Debug, Default)]
pub(super) struct State {
    /// Files fermées en ce moment : rien n'y démarre.
    pub closed: HashSet<String>,
    /// Ouverture connue de chaque planification, pour annoncer ses changements.
    open: HashMap<String, bool>,
    /// Prochain contrôle des plages ; `None` : au prochain tour.
    next_check: Option<Instant>,
    /// Files dont une tâche s'est terminée, et dont la planification a une action de fin.
    armed: HashSet<String>,
    /// Action de fin annoncée.
    pub pending: Option<PendingEnd>,
    deadline: Option<Instant>,
}

impl Scheduler {
    fn now_local(&self) -> LocalNow {
        (self.config.planning.clock)(&self.conn)
    }

    fn view(conn: &Connection, schedule: Schedule, now: LocalNow) -> ScheduleView {
        let local = now.local_ms();
        let open = !schedule.enabled || schedule.rules.is_open(local);
        let next_change_at = schedule
            .enabled
            .then(|| schedule.rules.next_change(local))
            .flatten()
            .map(|at| now.utc_of(conn, at));
        ScheduleView {
            schedule,
            open,
            next_change_at,
        }
    }

    /// Planifications, avec leur ouverture à cet instant.
    pub(super) fn schedule_views(&self) -> Result<Vec<ScheduleView>, EngineError> {
        let now = self.now_local();
        Ok(schedules::list_schedules(&self.conn)?
            .into_iter()
            .map(|s| Self::view(&self.conn, s, now))
            .collect())
    }

    /// Recalcule les files fermées (au plus une fois par `check_every`, sauf `force`),
    /// annonce les planifications dont l'ouverture a changé, et arrête les tâches en
    /// cours d'une file fermée : elles retournent en file et attendent la plage suivante.
    pub(super) fn refresh_schedules(&mut self, force: bool) {
        let now = Instant::now();
        if !force && self.planning.next_check.is_some_and(|at| now < at) {
            return;
        }
        // Aucune planification : rien ne change avant qu'on en écrive une, ce qui force le
        // calcul (audit P27).
        if !force && self.planning.next_check.is_some() && self.planning.open.is_empty() {
            return;
        }
        self.planning.next_check = Some(now + self.config.planning.check_every);
        let (Ok(views), Ok(queues)) = (self.schedule_views(), tasks::list_queues(&self.conn))
        else {
            return;
        };
        let mut open = HashMap::new();
        for view in views {
            let id = view.schedule.id.clone();
            let was = self.planning.open.get(&id).copied();
            open.insert(id, view.open);
            if was.is_some_and(|w| w != view.open) {
                self.emit(|rev| Event::ScheduleChanged { rev, view });
            }
        }
        self.planning.closed = queues
            .into_iter()
            .filter(|q| {
                q.schedule_id
                    .as_ref()
                    .is_some_and(|id| open.get(id) == Some(&false))
            })
            .map(|q| q.id)
            .collect();
        self.planning.open = open;
        for run in self.running.values_mut() {
            // Une file fermée arrête ses téléchargements. Un partage de torrent ne suit pas
            // les plages (il repartait aussitôt, revérifiant tout son disque toutes les 15 s,
            // audit P2) ; une vérification ou une commande de fin va jusqu'au bout (P3).
            if self.planning.closed.contains(&run.queue_id) {
                run.requeue_unless(
                    &[TaskState::Seeding, TaskState::Verifying],
                    tasks::PauseReason::Schedule,
                );
            }
        }
    }

    /// Délai jusqu'au prochain contrôle des plages ou à l'action de fin ; sans
    /// planification, aucun contrôle à attendre (audit P27).
    pub(super) fn planning_wake(&self, now: Instant) -> Duration {
        let check = match self.planning.next_check {
            None => Duration::ZERO,
            Some(_) if self.planning.open.is_empty() => Duration::MAX,
            Some(at) => at.saturating_duration_since(now),
        };
        match self.planning.deadline {
            Some(at) => check.min(at.saturating_duration_since(now)),
            None => check,
        }
    }

    /// Crée ou remplace une planification, l'annonce, puis applique ses plages.
    pub(super) fn set_schedule(
        &mut self,
        schedule_id: Option<String>,
        name: &str,
        enabled: bool,
        rules: Rules,
    ) -> Result<ScheduleView, EngineError> {
        if name.trim().is_empty() {
            return Err(EngineError::BadRequest("nom de planification vide".into()));
        }
        let id =
            schedules::save_schedule(&self.conn, schedule_id.as_deref(), name, enabled, &rules)?
                .ok_or_else(|| EngineError::ScheduleNotFound(schedule_id.unwrap_or_default()))?;
        let schedule = schedules::load_schedule(&self.conn, &id)?
            .ok_or_else(|| EngineError::ScheduleNotFound(id.clone()))?;
        let view = Self::view(&self.conn, schedule, self.now_local());
        // Annoncée ici : le contrôle qui suit ne la redit pas.
        self.planning.open.insert(id, view.open);
        let announced = view.clone();
        self.emit(|rev| Event::ScheduleChanged {
            rev,
            view: announced,
        });
        self.refresh_schedules(true);
        Ok(view)
    }

    /// Supprime une planification ; ses files la perdent et l'annoncent.
    pub(super) fn delete_schedule(&mut self, schedule_id: &str) -> Result<(), EngineError> {
        let attached: Vec<String> = tasks::list_queues(&self.conn)?
            .into_iter()
            .filter(|q| q.schedule_id.as_deref() == Some(schedule_id))
            .map(|q| q.id)
            .collect();
        if !schedules::delete_schedule(&self.conn, schedule_id)? {
            return Err(EngineError::ScheduleNotFound(schedule_id.to_owned()));
        }
        self.planning.open.remove(schedule_id);
        let id = schedule_id.to_owned();
        self.emit(|rev| Event::ScheduleRemoved {
            rev,
            schedule_id: id,
        });
        for queue_id in attached {
            self.emit(|rev| Event::QueueUpdated { rev, queue_id });
        }
        self.refresh_schedules(true);
        Ok(())
    }

    /// Attache une planification à une file, ou l'en détache. L'appelant
    /// (`update_queue`) a vérifié qu'elle existe.
    pub(super) fn attach_schedule(
        &mut self,
        queue_id: &str,
        schedule_id: Option<&str>,
    ) -> Result<(), EngineError> {
        if !tasks::set_queue_schedule(&self.conn, queue_id, schedule_id)? {
            return Err(EngineError::QueueNotFound(queue_id.to_owned()));
        }
        self.refresh_schedules(true);
        Ok(())
    }

    /// Action de fin de la planification d'une file ; `None` sans planification active.
    fn end_action_of(&self, queue_id: &str) -> EndAction {
        let Ok(Some(queue)) = tasks::load_queue(&self.conn, queue_id) else {
            return EndAction::None;
        };
        let Some(id) = queue.schedule_id else {
            return EndAction::None;
        };
        match schedules::load_schedule(&self.conn, &id) {
            Ok(Some(s)) if s.enabled => s.rules.on_done,
            _ => EndAction::None,
        }
    }

    /// Une tâche de la file vient de se terminer : l'action de fin suivra quand la file
    /// n'aura plus rien à télécharger.
    pub(super) fn arm_end_action(&mut self, queue_id: &str) {
        if self.end_action_of(queue_id) != EndAction::None {
            self.planning.armed.insert(queue_id.to_owned());
        }
    }

    /// L'utilisateur arrête (pause, annulation) une tâche de la file : l'action de fin
    /// armée se désarme, sinon la file vidée par ses pauses éteignait la machine (audit P5,
    /// décision 4). Une autre tâche qui se termine d'elle-même la réarme.
    pub(super) fn disarm_end_action(&mut self, queue_id: &str) {
        self.planning.armed.remove(queue_id);
    }

    /// La file a-t-elle encore à télécharger ? En cas de doute (base illisible), oui. Un
    /// partage de torrent n'est pas un téléchargement : il ne retient pas l'action de fin
    /// (SPEC §7.11 ; sinon, avec un ratio jamais atteint, la machine ne s'éteignait jamais,
    /// audit P4).
    fn has_work(&self, queue_id: &str) -> bool {
        self.running
            .values()
            .any(|r| r.queue_id == queue_id && r.shown != TaskState::Seeding)
            || count_pending(&self.conn, queue_id).map_or(true, |n| n > 0)
    }

    /// Annonce l'action de fin d'une file armée qui n'a plus rien à télécharger ; annule
    /// celle annoncée si sa file a de nouveau du travail, ou si sa planification ne la
    /// demande plus (désactivée, changée, détachée) ; l'exécute à l'heure dite.
    pub(super) fn check_end_action(&mut self) {
        if let Some(pending) = self.planning.pending.clone() {
            if self.has_work(&pending.queue_id)
                || self.end_action_of(&pending.queue_id) != pending.action
            {
                self.cancel_end_action();
            } else if self
                .planning
                .deadline
                .is_some_and(|at| Instant::now() >= at)
            {
                self.planning.pending = None;
                self.planning.deadline = None;
                // Hors de la boucle (audit P6) : la mise en veille ne rend la main qu'au
                // réveil, et la boucle doit continuer de servir ses clients jusque-là. Son
                // issue revient par une note, que la boucle annonce.
                let (power, action) = (self.config.planning.power, pending.action);
                let notes = self.prepared.clone();
                tokio::task::spawn_blocking(move || {
                    let error = power(action).err().map(|e| e.to_string());
                    let _ = notes.send((0, super::RunNote::EndActionDone(error)));
                });
            }
            return;
        }
        let ready: Vec<String> = self
            .planning
            .armed
            .iter()
            .filter(|q| !self.has_work(q))
            .cloned()
            .collect();
        for queue_id in ready {
            self.planning.armed.remove(&queue_id);
            let action = self.end_action_of(&queue_id);
            if action == EndAction::None {
                continue;
            }
            let delay = self.config.planning.end_delay;
            let pending = PendingEnd {
                queue_id,
                action,
                at_ms: now_ms() + i64::try_from(delay.as_millis()).unwrap_or(i64::MAX),
            };
            self.planning.deadline = Some(Instant::now() + delay);
            self.planning.pending = Some(pending.clone());
            self.emit(|rev| Event::EndAction {
                rev,
                pending: Some(pending),
                error: None,
            });
            // Une seule à la fois : la machine ne s'éteint qu'une fois.
            break;
        }
    }

    /// Annule l'action de fin annoncée ; `false` s'il n'y en avait pas.
    pub(super) fn cancel_end_action(&mut self) -> bool {
        if self.planning.pending.take().is_none() {
            return false;
        }
        self.planning.deadline = None;
        self.emit(|rev| Event::EndAction {
            rev,
            pending: None,
            error: None,
        });
        true
    }
}
