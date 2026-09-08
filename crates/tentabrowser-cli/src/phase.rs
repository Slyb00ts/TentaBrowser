//! Where a cold run's milliseconds went (`docs/spec/13-performance.md` §9).
//!
//! `--log debug` prints one `phases` line per command with the wall time of
//! each stage since the process started. It is the first thing to look at
//! when a `S-cold-fetch` number moves, and it costs one `Instant::now()` per
//! stage when the level is off.

use std::time::Instant;

/// Records the elapsed time at each named stage of a command.
pub struct Phases {
    started: Instant,
    last: Instant,
    marks: Vec<(&'static str, f64)>,
}

impl Phases {
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last: now,
            marks: Vec::with_capacity(8),
        }
    }

    /// Closes the stage that ended here.
    pub fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.marks
            .push((name, now.duration_since(self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }

    /// Prints the line. Called once, at the end of the command.
    pub fn report(&self) {
        if !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        let stages = self
            .marks
            .iter()
            .map(|(name, millis)| format!("{name}={millis:.2}"))
            .collect::<Vec<_>>()
            .join(" ");
        tracing::debug!(
            total_ms = self.started.elapsed().as_secs_f64() * 1000.0,
            "phases {stages}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_recorded_in_order_and_never_negative() {
        let mut phases = Phases::start();
        phases.mark("engine");
        phases.mark("navigate");
        assert_eq!(
            phases
                .marks
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            ["engine", "navigate"]
        );
        assert!(phases.marks.iter().all(|(_, millis)| *millis >= 0.0));
        // Reporting at a level that is off must not panic and must not print.
        phases.report();
    }
}
