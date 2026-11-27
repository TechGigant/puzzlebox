//! Engine-native trigger sources (cron, webhook, MQTT). HA triggers are delegated instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use croner::Cron;
use puzzlebox_ir::Trigger;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, MissedTickBehavior};

/// Delivers a trigger payload; returns false once the program is gone.
pub type TriggerSink = Box<dyn Fn(Value) -> bool + Send + Sync>;

pub trait TriggerSource: Send + Sync {
    fn register(&self, program_id: &str, trigger: &Trigger, sink: TriggerSink) -> Result<(), String>;
    fn unregister_program(&self, program_id: &str);
}

#[derive(Default)]
pub struct TriggerSources {
    by_kind: RwLock<HashMap<&'static str, Arc<dyn TriggerSource>>>,
}

fn kind(t: &Trigger) -> &'static str {
    match t {
        Trigger::Ha { .. } => "ha",
        Trigger::Cron { .. } => "cron",
        Trigger::Interval { .. } => "interval",
        Trigger::Webhook { .. } => "webhook",
        Trigger::Mqtt { .. } => "mqtt",
    }
}

impl TriggerSources {
    pub fn add(&self, kind: &'static str, source: Arc<dyn TriggerSource>) {
        self.by_kind.write().unwrap_or_else(|p| p.into_inner()).insert(kind, source);
    }

    pub fn register(&self, program_id: &str, t: &Trigger, sink: TriggerSink) -> Result<(), String> {
        let k = kind(t);
        let src = self.by_kind.read().unwrap_or_else(|p| p.into_inner()).get(k).cloned();
        match src {
            Some(s) => s.register(program_id, t, sink),
            None => Err(format!("{k} triggers are not available")),
        }
    }

    pub fn unregister_program(&self, program_id: &str) {
        for s in self.by_kind.read().unwrap_or_else(|p| p.into_inner()).values() {
            s.unregister_program(program_id);
        }
    }
}

/// Cron schedules in local time (HA's time zone); 5 or 6 fields (seconds optional).
#[derive(Default)]
pub struct CronSource {
    tasks: Mutex<HashMap<String, Vec<JoinHandle<()>>>>,
}

pub fn parse_cron(expr: &str) -> Result<Cron, String> {
    expr.trim().parse::<Cron>().map_err(|e| format!("invalid cron expression: {e}"))
}

impl TriggerSource for CronSource {
    fn register(&self, program_id: &str, trigger: &Trigger, sink: TriggerSink) -> Result<(), String> {
        let Trigger::Cron { expr, .. } = trigger else { return Err("not a cron trigger".into()) };
        let cron = parse_cron(expr)?;
        let expr = expr.clone();
        let task = tokio::spawn(async move {
            loop {
                let now = crate::clock::now();
                let Some(next) = crate::clock::next_cron(&cron) else { return };
                let wait = (next - now).to_std().unwrap_or_default();
                // Look again now and then: the time zone, daylight saving or the clock may change.
                if wait > Duration::from_secs(60) {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    continue;
                }
                tokio::time::sleep(wait).await;
                if !sink(json!({"platform": "cron", "cron": expr, "now": next.to_rfc3339()})) {
                    return;
                }
            }
        });
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(program_id.to_owned())
            .or_default()
            .push(task);
        Ok(())
    }

    fn unregister_program(&self, program_id: &str) {
        if let Some(tasks) = self.tasks.lock().unwrap_or_else(|p| p.into_inner()).remove(program_id) {
            for t in tasks {
                t.abort();
            }
        }
    }
}

/// "every 7 minutes": an exact interval, first one interval after the program starts.
#[derive(Default)]
pub struct IntervalSource {
    tasks: Mutex<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl TriggerSource for IntervalSource {
    fn register(&self, program_id: &str, trigger: &Trigger, sink: TriggerSink) -> Result<(), String> {
        let Trigger::Interval { seconds, .. } = trigger else { return Err("not an interval trigger".into()) };
        let seconds = *seconds;
        if !(seconds.is_finite() && seconds >= 1.0) {
            return Err("repeat at most once a second".into());
        }
        let period = Duration::from_secs_f64(seconds);
        let task = tokio::spawn(async move {
            let mut ticks = tokio::time::interval_at(Instant::now() + period, period);
            // After a stall (a suspended host), carry on from now instead of firing in a burst.
            ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                let now = crate::clock::now().to_rfc3339();
                if !sink(json!({"platform": "interval", "seconds": seconds, "now": now})) {
                    return;
                }
            }
        });
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(program_id.to_owned())
            .or_default()
            .push(task);
        Ok(())
    }

    fn unregister_program(&self, program_id: &str) {
        if let Some(tasks) = self.tasks.lock().unwrap_or_else(|p| p.into_inner()).remove(program_id) {
            for t in tasks {
                t.abort();
            }
        }
    }
}
