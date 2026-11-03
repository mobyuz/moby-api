use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::Instant;

const FAILURES_TO_OPEN: u8 = 3;
const COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
pub(crate) struct Circuit(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    generation: u64,
    failures: u8,
    open_until: Option<Instant>,
    probing: bool,
}

impl Circuit {
    pub(crate) fn enter(&self, now: Instant) -> Option<Permit> {
        let mut state = self.0.lock().expect("provider circuit lock");
        let probing = state.open_until.is_some();
        if state.open_until.is_some_and(|until| now < until) || state.probing {
            return None;
        }
        state.probing = probing;
        Some(Permit {
            circuit: self.clone(),
            generation: state.generation,
            probing,
            finished: false,
        })
    }
}

pub(crate) struct Permit {
    circuit: Circuit,
    generation: u64,
    probing: bool,
    finished: bool,
}

impl Permit {
    pub(crate) fn finish(mut self, success: bool, now: Instant) {
        self.finished = true;
        let mut state = self.circuit.0.lock().expect("provider circuit lock");
        if state.generation != self.generation {
            return;
        }
        state.probing = false;
        if success {
            state.failures = 0;
            state.open_until = None;
        } else {
            state.failures += 1;
            if self.probing || state.failures == FAILURES_TO_OPEN {
                state.generation += 1;
                state.failures = 0;
                state.open_until = Some(now + COOLDOWN);
            }
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.probing && !self.finished {
            self.circuit
                .0
                .lock()
                .expect("provider circuit lock")
                .probing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COOLDOWN, Circuit, FAILURES_TO_OPEN};
    use tokio::time::Instant;

    #[test]
    fn opens_until_one_successful_probe() {
        let circuit = Circuit::default();
        let now = Instant::now();
        let older_turn = circuit.enter(now).unwrap();
        for _ in 0..FAILURES_TO_OPEN {
            circuit.enter(now).unwrap().finish(false, now);
        }
        older_turn.finish(true, now);
        assert!(circuit.enter(now).is_none());

        let later = now + COOLDOWN;
        let canceled = circuit.enter(later).unwrap();
        assert!(circuit.enter(later).is_none());
        drop(canceled);
        circuit.enter(later).unwrap().finish(false, later);
        assert!(circuit.enter(later).is_none());
        let retry = later + COOLDOWN;
        circuit.enter(retry).unwrap().finish(true, retry);
        assert!(circuit.enter(retry).is_some());
    }
}
