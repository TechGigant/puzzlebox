//! Jinja templates → blocks where they map exactly (otherwise a template block HA renders).

use serde_json::{Value, json};

use crate::*;

impl Builder {
    /// A value block for a template string.
    pub(crate) fn template_value(&mut self, s: &str) -> Value {
        self.block("ha_template", json!({"TEMPLATE": s}), vec![], None)
    }
}
