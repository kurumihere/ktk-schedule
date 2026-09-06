use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// After repeated upstream failures allow one recovery probe, rather than a burst.
pub struct Circuit {
    state: Mutex<State>,
    threshold: usize,
    cooldown: Duration,
}
struct State {
    failures: usize,
    until: Option<Instant>,
    probing: bool,
    generation: u64,
}
pub struct Permit<'a> {
    circuit: &'a Circuit,
    generation: u64,
    probe: bool,
    outcome: Option<bool>,
}

impl Circuit {
    pub fn new(threshold: usize, cooldown: Duration) -> Self {
        Self {
            state: Mutex::new(State {
                failures: 0,
                until: None,
                probing: false,
                generation: 0,
            }),
            threshold,
            cooldown,
        }
    }
    pub fn acquire(&self) -> Option<Permit<'_>> {
        self.acquire_at(Instant::now())
    }
    fn acquire_at(&self, now: Instant) -> Option<Permit<'_>> {
        let mut state = self.state.lock().unwrap();
        if state.probing || state.until.is_some_and(|until| now < until) {
            return None;
        }
        let probe = state.until.is_some();
        state.probing = probe;
        Some(Permit {
            circuit: self,
            generation: state.generation,
            probe,
            outcome: None,
        })
    }
}

impl Permit<'_> {
    pub fn finish(mut self, healthy: bool) {
        self.outcome = Some(healthy);
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.circuit.state.lock().unwrap();
        if state.generation != self.generation {
            return;
        }
        match self.outcome {
            Some(true) => {
                state.failures = 0;
                state.until = None;
                state.probing = false;
            }
            Some(false) | None if self.probe || self.outcome == Some(false) => {
                state.failures += 1;
                if self.probe || state.failures >= self.circuit.threshold {
                    state.until = Some(Instant::now() + self.circuit.cooldown);
                    state.probing = false;
                    state.generation += 1;
                }
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_has_one_probe_and_cancellation_does_not_leave_it_stuck() {
        let c = Circuit::new(2, Duration::from_secs(30));
        c.acquire().unwrap().finish(false);
        c.acquire().unwrap().finish(false);
        assert!(c.acquire().is_none());
        let later = Instant::now() + Duration::from_secs(31);
        let probe = c.acquire_at(later).unwrap();
        assert!(c.acquire_at(later).is_none());
        drop(probe);
        let probe = c.acquire_at(later + Duration::from_secs(31)).unwrap();
        probe.finish(true);
        assert!(c.acquire().is_some());
    }
    #[test]
    fn a_late_success_cannot_close_a_newly_opened_circuit() {
        let c = Circuit::new(1, Duration::from_secs(30));
        let slow = c.acquire().unwrap();
        c.acquire().unwrap().finish(false);
        slow.finish(true);
        assert!(c.acquire().is_none());
    }
}
