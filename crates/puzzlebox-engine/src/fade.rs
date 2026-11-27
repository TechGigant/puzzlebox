//! "fade … from … % to … % over …": brightness changed by the engine in small steps, since
//! lights' own transitions are often limited to seconds or minutes, or jump at the end.
//!
//! Each step goes about 1 % of the way (fewer steps for short fades: at least `MIN_STEP`
//! apart) and is itself a smooth transition up to the next step. Before each step the lights
//! are checked: if one was turned off or its brightness is far from where the fade left it,
//! someone changed it by hand and the fade stops there (the run carries on). Stopping the run
//! stops the fade, since the steps are awaited by the run.

use std::time::Duration;

use serde_json::{Value, json};

use super::{AtBlock, Frame, R, RunCtx};
use crate::value::{to_num, to_target};
use puzzlebox_ir::Expr;

/// Shortest time between two steps, in seconds.
const MIN_STEP: f64 = 2.0;
/// How far (in %) a light may be from the last step before it counts as changed by hand.
const TOLERANCE: f64 = 8.0;
/// Lights may report a finished transition late; a difference is checked again after this.
const RECHECK: Duration = Duration::from_millis(1500);

/// `(steps, seconds between them)` for a fade of `span` % over `seconds`.
pub(crate) fn plan(span: f64, seconds: f64) -> (u32, f64) {
    if seconds <= 0.0 {
        return (1, 0.0);
    }
    let by_percent = span.abs().ceil().max(1.0);
    let by_time = (seconds / MIN_STEP).floor().max(1.0);
    let steps = by_percent.min(by_time);
    (steps as u32, seconds / steps)
}

/// Which light is no longer where the fade left it (`expected` %), if any.
fn changed_by_hand(ctx: &RunCtx, ids: &[String], expected: f64) -> Option<String> {
    ids.iter()
        .find(|id| {
            let Some(s) = ctx.ha.state(id) else { return false };
            match s["state"].as_str() {
                Some("off") => expected > 0.0,
                Some("on") => s["attributes"]["brightness"]
                    .as_f64()
                    .is_some_and(|b| (b / 255.0 * 100.0 - expected).abs() > TOLERANCE),
                // Unavailable or unknown: nobody changed it.
                _ => false,
            }
        })
        .cloned()
}

impl RunCtx {
    async fn set_brightness(&self, id: &str, target: &Value, pct: f64, transition: f64) -> R<()> {
        let mut data =
            if pct <= 0.0 { json!({}) } else { json!({"brightness_pct": (pct * 10.0).round() / 10.0}) };
        if transition > 0.0 {
            data["transition"] = json!((transition * 10.0).round() / 10.0);
        }
        let service = if pct <= 0.0 { "light.turn_off" } else { "light.turn_on" };
        self.ha.call_service_checked(service, Some(data), Some(target.clone()), false).await.at(id)?;
        Ok(())
    }

    pub(super) async fn fade(
        &self,
        frame: &Frame,
        id: &str,
        target: &Expr,
        from: &Expr,
        to: &Expr,
        seconds: &Expr,
    ) -> R<()> {
        let target = to_target(self.eval(frame, target).await?).at(id)?;
        let from = to_num(&self.eval(frame, from).await?).at(id)?.clamp(0.0, 100.0);
        let to = to_num(&self.eval(frame, to).await?).at(id)?.clamp(0.0, 100.0);
        let seconds = to_num(&self.eval(frame, seconds).await?).at(id)?.max(0.0);
        let ids: Vec<String> = match &target["entity_id"] {
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => vec![],
        };
        let (steps, interval) = plan(to - from, seconds);
        if interval == 0.0 {
            return self.set_brightness(id, &target, to, 0.0).await;
        }
        self.set_brightness(id, &target, from, 0.0).await?;
        self.waiting(id, format!("fading to {to} %"), Some(seconds));
        let mut expected = from;
        for i in 1..=steps {
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
            if changed_by_hand(self, &ids, expected).is_some() {
                tokio::time::sleep(RECHECK).await;
                if let Some(light) = changed_by_hand(self, &ids, expected) {
                    let name = self
                        .ha
                        .state(&light)
                        .and_then(|s| s["attributes"]["friendly_name"].as_str().map(str::to_owned))
                        .unwrap_or(light);
                    self.waited(id, format!("stopped at {expected:.0} %: {name} was changed by hand"));
                    return Ok(());
                }
            }
            let pct = from + (to - from) * f64::from(i) / f64::from(steps);
            self.set_brightness(id, &target, pct, interval).await?;
            expected = pct;
        }
        self.waited(id, format!("faded to {to} %"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::plan;

    #[test]
    fn steps_are_about_one_percent_and_not_too_close() {
        assert_eq!(plan(99.0, 900.0), (99, 900.0 / 99.0));
        assert_eq!(plan(-50.0, 3600.0), (50, 72.0));
        // A short fade: one step every 2 seconds.
        assert_eq!(plan(99.0, 10.0), (5, 2.0));
        assert_eq!(plan(0.0, 60.0), (1, 60.0));
        assert_eq!(plan(50.0, 0.0), (1, 0.0));
    }
}
