//! Breakpoints, pausing and stepping. Pausing is just awaiting a `Notify` at the
//! statement hook, so the interpreter needs no special support.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, broadcast};

use crate::engine::EngineEvent;
use crate::interp::RunCtx;

#[derive(Default)]
struct RunDebug {
    /// Pause before the next statement (single-step).
    step: bool,
    paused_at: Option<String>,
    resume: Arc<Notify>,
}

#[derive(Default)]
struct State {
    watchers: HashMap<String, usize>,
    breakpoints: HashMap<String, HashSet<String>>,
    /// Pause every new run of the program at its first statement.
    pause_on_start: HashSet<String>,
    runs: HashMap<String, RunDebug>,
}

#[derive(Default)]
pub struct Debugger {
    state: Mutex<State>,
}

/// Drops the watch when the debug session ends.
pub struct WatchGuard<'a> {
    dbg: &'a Debugger,
    program_id: String,
}

impl Drop for WatchGuard<'_> {
    fn drop(&mut self) {
        self.dbg.unwatch(&self.program_id);
    }
}

impl Debugger {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn watch(&self, program_id: &str) -> WatchGuard<'_> {
        *self.st().watchers.entry(program_id.to_owned()).or_default() += 1;
        WatchGuard { dbg: self, program_id: program_id.to_owned() }
    }

    fn unwatch(&self, program_id: &str) {
        let mut st = self.st();
        let remaining = match st.watchers.get_mut(program_id) {
            Some(n) => {
                *n = n.saturating_sub(1);
                *n
            }
            None => 0,
        };
        if remaining == 0 {
            st.watchers.remove(program_id);
            // Nobody is left to resume paused runs: clear debugging state and release them.
            st.breakpoints.remove(program_id);
            st.pause_on_start.remove(program_id);
            for r in st.runs.values_mut() {
                r.step = false;
                if r.paused_at.take().is_some() {
                    r.resume.notify_one();
                }
            }
        }
    }

    pub fn is_watched(&self, program_id: &str) -> bool {
        self.st().watchers.contains_key(program_id)
    }

    pub fn set_breakpoints(&self, program_id: &str, blocks: impl IntoIterator<Item = String>) {
        self.st().breakpoints.insert(program_id.to_owned(), blocks.into_iter().collect());
    }

    pub fn set_pause_on_start(&self, program_id: &str, on: bool) {
        let mut st = self.st();
        if on {
            st.pause_on_start.insert(program_id.to_owned());
        } else {
            st.pause_on_start.remove(program_id);
        }
    }

    pub fn resume(&self, run_id: &str) -> bool {
        self.release(run_id, false)
    }

    pub fn step(&self, run_id: &str) -> bool {
        self.release(run_id, true)
    }

    fn release(&self, run_id: &str, step: bool) -> bool {
        let mut st = self.st();
        let Some(r) = st.runs.get_mut(run_id) else { return false };
        r.step = step;
        if r.paused_at.take().is_some() {
            r.resume.notify_one();
            true
        } else {
            false
        }
    }

    pub fn paused_runs(&self) -> Vec<(String, String)> {
        self.st().runs.iter().filter_map(|(id, r)| r.paused_at.clone().map(|b| (id.clone(), b))).collect()
    }

    pub(crate) fn run_finished(&self, run_id: &str) {
        self.st().runs.remove(run_id);
    }

    pub(crate) async fn maybe_pause(
        &self,
        ctx: &RunCtx,
        block: &str,
        events: &broadcast::Sender<EngineEvent>,
    ) {
        let notify = {
            let mut st = self.st();
            if !st.watchers.contains_key(&ctx.program_id) {
                return;
            }
            let first = !st.runs.contains_key(&ctx.run_id);
            let on_start = first && st.pause_on_start.contains(&ctx.program_id);
            let bp = st.breakpoints.get(&ctx.program_id).is_some_and(|b| b.contains(block));
            let run = st.runs.entry(ctx.run_id.clone()).or_default();
            if !(run.step || bp || on_start) {
                return;
            }
            run.step = false;
            run.paused_at = Some(block.to_owned());
            run.resume.clone()
        };
        let _ = events.send(EngineEvent::Paused {
            program_id: ctx.program_id.clone(),
            run_id: ctx.run_id.clone(),
            block_id: block.to_owned(),
            variables: ctx.variables_snapshot(),
        });
        notify.notified().await;
        let _ = events
            .send(EngineEvent::Resumed { program_id: ctx.program_id.clone(), run_id: ctx.run_id.clone() });
    }
}
