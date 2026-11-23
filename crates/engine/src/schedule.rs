//! Planification par plages horaires (F-16, SPEC §7.11) : règles d'une planification,
//! ouverture d'une file à une heure locale donnée, prochain changement.
//!
//! Les heures sont celles de la machine du moteur. L'heure locale vient de SQLite
//! (modificateur `localtime`, qui suit l'heure d'été du système) : aucune base de fuseaux
//! horaires à embarquer.

use rusqlite::Connection;
use serde_json::{Value, json};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Au-delà d'une semaine et un jour sans changement, il n'y en aura plus.
const HORIZON_MINUTES: i64 = 8 * 1440;

/// Une plage : de `start` à `end` (minutes depuis minuit), les jours `days` (1 : lundi
/// … 7 : dimanche). `end` inférieur ou égal à `start` passe minuit ; égal, la plage dure
/// 24 h. Une plage appartient au jour où elle commence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub days: Vec<u8>,
    pub start: u16,
    pub end: u16,
}

/// Ce que fait la machine quand une file planifiée n'a plus rien à télécharger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndAction {
    None,
    Sleep,
    Shutdown,
}

impl EndAction {
    pub fn as_str(self) -> &'static str {
        match self {
            EndAction::None => "none",
            EndAction::Sleep => "sleep",
            EndAction::Shutdown => "shutdown",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(EndAction::None),
            "sleep" => Some(EndAction::Sleep),
            "shutdown" => Some(EndAction::Shutdown),
            _ => None,
        }
    }
}

/// Règles d'une planification : ses plages et son action de fin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub windows: Vec<Window>,
    pub on_done: EndAction,
}

/// Une planification enregistrée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub rules: Rules,
}

/// Heure de la machine : instant UTC et décalage de l'heure locale, en millisecondes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalNow {
    pub utc_ms: i64,
    pub offset_ms: i64,
    pub zone: Zone,
}

/// Fuseau de l'heure locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// Celui de la machine : le décalage change avec l'heure d'été.
    Machine,
    /// Décalage fixe (horloges des tests).
    Fixed,
}

impl LocalNow {
    pub fn local_ms(self) -> i64 {
        self.utc_ms + self.offset_ms
    }

    /// Instant UTC d'une heure locale, avec le décalage de ce moment-là : de l'autre côté
    /// d'un changement d'heure, celui d'aujourd'hui donnait une heure de trop ou de moins
    /// (audit P19). Lu par SQLite pour le fuseau de la machine.
    pub fn utc_of(self, conn: &Connection, local_ms: i64) -> i64 {
        let fixed = local_ms - self.offset_ms;
        if self.zone == Zone::Fixed {
            return fixed;
        }
        conn.query_row(
            "SELECT CAST(strftime('%s', ?1, 'unixepoch', 'utc') AS INTEGER)",
            [local_ms.div_euclid(1000)],
            |r| r.get::<_, i64>(0),
        )
        .map(|s| s * 1000 + local_ms.rem_euclid(1000))
        .unwrap_or(fixed)
    }
}

/// Heure de la machine, décalage de l'heure locale lu par SQLite.
pub fn local_now(conn: &Connection) -> LocalNow {
    let utc_ms = crate::store::now_ms();
    let offset_s: i64 = conn
        .query_row(
            "SELECT CAST(strftime('%s', ?1, 'unixepoch', 'localtime') AS INTEGER) - ?1",
            [utc_ms.div_euclid(1000)],
            |r| r.get(0),
        )
        .unwrap_or(0);
    LocalNow {
        utc_ms,
        offset_ms: offset_s * 1000,
        zone: Zone::Machine,
    }
}

/// Jour de la semaine (1 : lundi … 7 : dimanche) et minute du jour d'une heure locale.
fn day_and_minute(local_ms: i64) -> (u8, u16) {
    let days = local_ms.div_euclid(DAY_MS);
    // Le 1er janvier 1970 était un jeudi.
    let weekday = (days + 3).rem_euclid(7) + 1;
    let minute = local_ms.rem_euclid(DAY_MS) / MINUTE_MS;
    (weekday as u8, minute as u16)
}

fn previous_day(day: u8) -> u8 {
    if day == 1 { 7 } else { day - 1 }
}

impl Window {
    fn contains(&self, day: u8, minute: u16) -> bool {
        if self.start < self.end {
            self.days.contains(&day) && (self.start..self.end).contains(&minute)
        } else {
            // Passe minuit : le soir du jour prévu, ou le matin du lendemain.
            (self.days.contains(&day) && minute >= self.start)
                || (self.days.contains(&previous_day(day)) && minute < self.end)
        }
    }
}

impl Rules {
    /// La file peut-elle télécharger à cette heure locale ?
    pub fn is_open(&self, local_ms: i64) -> bool {
        let (day, minute) = day_and_minute(local_ms);
        self.windows.iter().any(|w| w.contains(day, minute))
    }

    /// Prochaine minute locale où l'ouverture change ; `None` si elle ne change jamais.
    pub fn next_change(&self, local_ms: i64) -> Option<i64> {
        let now = self.is_open(local_ms);
        let first = local_ms.div_euclid(MINUTE_MS) * MINUTE_MS;
        (1..=HORIZON_MINUTES)
            .map(|k| first + k * MINUTE_MS)
            .find(|&at| self.is_open(at) != now)
    }

    /// Règles lues dans le JSON du protocole et de la base (`rules_json`).
    pub fn from_json(value: &Value) -> Result<Rules, String> {
        let object = value.as_object().ok_or("rules : objet attendu")?;
        let windows = object
            .get("windows")
            .and_then(Value::as_array)
            .ok_or("rules.windows : tableau attendu")?;
        if windows.is_empty() {
            return Err("rules.windows : au moins une plage".into());
        }
        let windows = windows
            .iter()
            .map(window_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let on_done = match object.get("onDone") {
            None | Some(Value::Null) => EndAction::None,
            Some(v) => v
                .as_str()
                .and_then(EndAction::parse)
                .ok_or("rules.onDone : none, sleep ou shutdown")?,
        };
        Ok(Rules { windows, on_done })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "windows": self.windows.iter().map(|w| json!({
                "days": w.days,
                "start": hhmm(w.start),
                "end": hhmm(w.end),
            })).collect::<Vec<_>>(),
            "onDone": self.on_done.as_str(),
        })
    }
}

fn window_from_json(value: &Value) -> Result<Window, String> {
    let days = value
        .get("days")
        .and_then(Value::as_array)
        .ok_or("plage : days, tableau de jours attendu")?;
    let mut list = Vec::new();
    for day in days {
        let day = day
            .as_u64()
            .filter(|d| (1..=7).contains(d))
            .ok_or("plage : jours de 1 (lundi) à 7 (dimanche)")? as u8;
        if !list.contains(&day) {
            list.push(day);
        }
    }
    if list.is_empty() {
        return Err("plage : au moins un jour".into());
    }
    list.sort_unstable();
    let time = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .and_then(parse_hhmm)
            .ok_or_else(|| format!("plage : {name} au format HH:MM"))
    };
    Ok(Window {
        days: list,
        start: time("start")?,
        end: time("end")?,
    })
}

/// « HH:MM », de 00:00 à 23:59.
fn parse_hhmm(text: &str) -> Option<u16> {
    let (h, m) = text.split_once(':')?;
    // Deux chiffres de chaque côté : `parse` accepterait aussi « +1 ».
    let two_digits = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    if !two_digits(h) || !two_digits(m) {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

fn hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Heure locale du lundi 2026-11-02 (jour 20759 depuis 1970) à `h:m`, plus `days`.
    fn at(days: i64, h: i64, m: i64) -> i64 {
        (20_759 + days) * DAY_MS + h * 3_600_000 + m * MINUTE_MS
    }

    fn rules(value: Value) -> Rules {
        Rules::from_json(&value).unwrap()
    }

    #[test]
    fn days_of_the_week_start_on_monday() {
        assert_eq!(day_and_minute(at(0, 0, 0)), (1, 0));
        assert_eq!(day_and_minute(at(6, 23, 59)), (7, 1439));
        assert_eq!(day_and_minute(0), (4, 0), "1er janvier 1970, un jeudi");
    }

    #[test]
    fn a_window_inside_a_day() {
        let r = rules(
            json!({"windows": [{"days": [1, 2, 3, 4, 5], "start": "09:00", "end": "17:30"}]}),
        );
        assert!(!r.is_open(at(0, 8, 59)));
        assert!(r.is_open(at(0, 9, 0)));
        assert!(r.is_open(at(4, 17, 29)), "vendredi");
        assert!(!r.is_open(at(4, 17, 30)), "fin exclue");
        assert!(!r.is_open(at(5, 12, 0)), "samedi");
        assert_eq!(r.on_done, EndAction::None);
    }

    #[test]
    fn a_night_window_belongs_to_the_day_it_starts() {
        // Le vendredi soir jusqu'au samedi 7 h, mais pas le dimanche soir.
        let r = rules(json!({"windows": [{"days": [5], "start": "23:00", "end": "07:00"}]}));
        assert!(!r.is_open(at(4, 22, 59)));
        assert!(r.is_open(at(4, 23, 0)));
        assert!(r.is_open(at(5, 6, 59)), "samedi matin");
        assert!(!r.is_open(at(5, 7, 0)));
        assert!(
            !r.is_open(at(0, 3, 0)),
            "lundi matin : la plage de dimanche n'existe pas"
        );
        // La nuit du dimanche au lundi : le dimanche (7) précède le lundi.
        let sunday = rules(json!({"windows": [{"days": [7], "start": "22:00", "end": "02:00"}]}));
        assert!(sunday.is_open(at(0, 1, 0)));
    }

    #[test]
    fn equal_start_and_end_last_a_whole_day() {
        let r = rules(json!({"windows": [{"days": [3], "start": "08:00", "end": "08:00"}]}));
        assert!(r.is_open(at(2, 8, 0)));
        assert!(r.is_open(at(3, 7, 59)));
        assert!(!r.is_open(at(3, 8, 0)));
    }

    #[test]
    fn the_next_change_is_found_minute_by_minute() {
        let r = rules(json!({"windows": [{"days": [1], "start": "23:00", "end": "07:00"}]}));
        assert_eq!(r.next_change(at(0, 12, 30) + 15_000), Some(at(0, 23, 0)));
        assert_eq!(r.next_change(at(0, 23, 0)), Some(at(1, 7, 0)));
        // Ouverte tout le temps : jamais de changement.
        let always = rules(
            json!({"windows": [{"days": [1, 2, 3, 4, 5, 6, 7], "start": "00:00", "end": "00:00"}]}),
        );
        assert!(always.is_open(at(3, 4, 5)));
        assert_eq!(always.next_change(at(3, 4, 5)), None);
    }

    #[test]
    fn rules_are_checked_and_written_back_the_same() {
        let value = json!({
            "windows": [{"days": [5, 1, 1], "start": "23:00", "end": "07:00"}],
            "onDone": "shutdown",
        });
        let r = rules(value);
        assert_eq!(r.windows[0].days, vec![1, 5], "triés, sans doublon");
        assert_eq!(r.on_done, EndAction::Shutdown);
        assert_eq!(
            r.to_json(),
            json!({"windows": [{"days": [1, 5], "start": "23:00", "end": "07:00"}], "onDone": "shutdown"})
        );
        for bad in [
            json!({"windows": []}),
            json!({"windows": [{"days": [], "start": "01:00", "end": "02:00"}]}),
            json!({"windows": [{"days": [8], "start": "01:00", "end": "02:00"}]}),
            json!({"windows": [{"days": [1], "start": "24:00", "end": "02:00"}]}),
            json!({"windows": [{"days": [1], "start": "1:00", "end": "02:00"}]}),
            json!({"windows": [{"days": [1], "start": "+1:00", "end": "02:00"}]}),
            json!({"windows": [{"days": [1], "start": "01:00", "end": "02:00"}], "onDone": "reboot"}),
            json!([]),
        ] {
            assert!(Rules::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_local_offset_comes_from_the_system() {
        let conn = Connection::open_in_memory().unwrap();
        let now = local_now(&conn);
        // Aucun fuseau ne s'écarte de plus de 14 h de l'heure universelle.
        assert!(now.offset_ms.abs() <= 14 * 3_600_000, "{now:?}");
        assert_eq!(now.offset_ms % 60_000, 0);
    }

    /// Audit P19 : la prochaine ouverture, de l'autre côté d'un changement d'heure, tombe
    /// à l'heure locale prévue. Le samedi 28 mars 2026, veille du passage à l'heure d'été
    /// en Europe ; sur une machine sans heure d'été, le cas est trivial.
    #[test]
    fn the_next_change_across_a_clock_change_keeps_its_local_hour() {
        let conn = Connection::open_in_memory().unwrap();
        let utc_ms: i64 = 1_774_699_200_000;
        let offset_s: i64 = conn
            .query_row(
                "SELECT CAST(strftime('%s', ?1, 'unixepoch', 'localtime') AS INTEGER) - ?1",
                [utc_ms / 1000],
                |r| r.get(0),
            )
            .unwrap();
        let now = LocalNow {
            utc_ms,
            offset_ms: offset_s * 1000,
            zone: Zone::Machine,
        };
        let rules = Rules::from_json(
            &json!({"windows": [{"days": [1], "start": "09:00", "end": "10:00"}]}),
        )
        .unwrap();
        let local = rules.next_change(now.local_ms()).unwrap();
        let utc = now.utc_of(&conn, local);
        let shown: String = conn
            .query_row(
                "SELECT strftime('%w %H:%M', ?1, 'unixepoch', 'localtime')",
                [utc / 1000],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(shown, "1 09:00", "lundi 9 h, heure locale");
    }
}
