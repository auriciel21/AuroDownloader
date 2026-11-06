//! Affichage : tailles, débits, temps restant, ligne d'avancement.

use std::io::{IsTerminal, Write};
use std::time::Instant;

/// Taille lisible, en unités binaires.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} o")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Durée lisible : « 42 s », « 3 min 05 s », « 2 h 07 min ».
pub fn duration(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// Avancement d'une tâche, tel qu'affiché.
pub struct Line {
    pub done: u64,
    pub total: Option<u64>,
    pub bps: u64,
    pub connections: u64,
    pub eta: Option<u64>,
}

impl Line {
    pub fn render(&self) -> String {
        let done = match self.total {
            Some(total) if total > 0 => format!(
                "{:5.1} %  {} / {}",
                self.done as f64 * 100.0 / total as f64,
                size(self.done),
                size(total)
            ),
            _ => size(self.done),
        };
        let eta = self
            .eta
            .map(|s| format!("  reste {}", duration(s)))
            .unwrap_or_default();
        format!(
            "{done}  {}/s  {} connexion(s){eta}",
            size(self.bps),
            self.connections
        )
    }
}

/// Ligne d'avancement réécrite sur place (sur un terminal), ou une ligne par mise à jour.
pub struct Status {
    terminal: bool,
    shown: usize,
}

impl Status {
    pub fn new() -> Self {
        Status {
            terminal: std::io::stderr().is_terminal(),
            shown: 0,
        }
    }

    /// Remplace le bloc affiché par `lines`.
    pub fn show(&mut self, lines: &[String]) {
        let mut err = std::io::stderr().lock();
        if self.terminal {
            // Remonte au début du bloc précédent et l'efface ligne par ligne.
            if self.shown > 0 {
                let _ = write!(err, "\x1b[{}A", self.shown);
            }
            for line in lines {
                let _ = writeln!(err, "\x1b[2K{line}");
            }
            for _ in lines.len()..self.shown {
                let _ = writeln!(err, "\x1b[2K");
            }
            self.shown = lines.len().max(self.shown);
        } else {
            for line in lines {
                let _ = writeln!(err, "{line}");
            }
        }
        let _ = err.flush();
    }

    /// Le bloc reste à l'écran ; ce qui suit s'écrit dessous.
    pub fn finish(&mut self) {
        self.shown = 0;
    }
}

/// Débit lissé, pour le mode direct, où le moteur ne le calcule pas.
pub struct Speed {
    last: Option<(Instant, u64)>,
    bps: f64,
}

impl Speed {
    pub fn new() -> Self {
        Speed {
            last: None,
            bps: 0.0,
        }
    }

    pub fn update(&mut self, received: u64) -> u64 {
        let now = Instant::now();
        if let Some((at, bytes)) = self.last {
            let elapsed = now.duration_since(at).as_secs_f64();
            if elapsed > 0.0 {
                let instant = received.saturating_sub(bytes) as f64 / elapsed;
                self.bps = if self.bps == 0.0 {
                    instant
                } else {
                    0.7 * self.bps + 0.3 * instant
                };
            }
        }
        self.last = Some((now, received));
        self.bps as u64
    }
}
