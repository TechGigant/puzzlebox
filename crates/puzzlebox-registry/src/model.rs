//! The normalized block definition every source (autogen, packs, Python, WASM) produces.
//! It is also the wire format the editor renders blocks from, and the public
//! block-pack schema (see `sdk/block-pack-schema`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// Statement block.
    Action,
    /// Goes into a "when" block's trigger slot.
    Trigger,
    /// Boolean value block.
    Condition,
    /// Value block.
    Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    /// Blockly block type; namespaced by source (`ha_svc__light__turn_on`, `pack__mypack__x`, ...).
    #[serde(rename = "type")]
    pub block_type: String,
    pub kind: BlockKind,
    /// Toolbox path, e.g. `["Light"]` or `["Spotify", "Playback"]`.
    pub category: Vec<String>,
    /// Theme style name without `_blocks` suffix; unknown names get a stable hashed colour.
    pub style: String,
    pub title: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_url: Option<String>,
    /// Entity/area/device target slot (input `TARGET`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetSpec>,
    #[serde(default)]
    pub params: Vec<Param>,
    pub lowering: Lowering,
    /// Which source contributed it, e.g. `ha`, `pack:my_pack`, `py:spotify`, `wasm:weather`.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TargetSpec {
    /// Entity domains offered by the picker; empty = any.
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// Key in the lowered config (service data key, trigger option, plugin argument).
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub input: ParamInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl Param {
    /// Blockly field/input name.
    pub fn slot(&self) -> String {
        format!("P_{}", self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum ParamInput {
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    Text {
        #[serde(default)]
        multiline: bool,
    },
    Boolean,
    /// Dropdown field (constant).
    Select {
        options: Vec<SelectOption>,
    },
    Entity {
        #[serde(default)]
        domains: Vec<String>,
    },
    /// Seconds.
    Duration,
    /// `HH:MM:SS`.
    Time,
    /// Hex colour field, lowered to `[r, g, b]`.
    Color,
    /// HA `numeric_threshold`: a type (above/below/between/outside/any), one or two
    /// numbers and, when `units` is non-empty, a unit; lowered to HA's threshold object.
    Threshold {
        #[serde(default)]
        mode: String,
        #[serde(default)]
        units: Vec<Option<String>>,
    },
    /// A state (or attribute value) of the block's target entity, e.g. an event entity's
    /// event types; the editor suggests values from that entity. `multiple` lowers to a list.
    StateValue {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attribute: Option<String>,
        #[serde(default)]
        multiple: bool,
    },
    /// Anything (dict/list/expression).
    Any,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Lowering {
    /// Action: call `service` with params as data and `TARGET` as target.
    Service {
        service: String,
        #[serde(default)]
        response: bool,
    },
    /// New-style HA trigger: `{"trigger": name, "target": ..., "options": {params}}`.
    HaTriggerPlatform { trigger: String },
    /// New-style HA condition: `{"condition": name, "target": ..., "options": {params}}`.
    HaConditionPlatform { condition: String },
    /// Raw HA trigger config with `{"$arg": "param"}` / `{"$arg": "target"}` placeholders.
    HaTrigger { config: Value },
    /// Raw HA condition config with placeholders.
    HaCondition { config: Value },
    /// Raw HA action sequence with placeholders.
    HaAction { sequence: Value },
    /// Jinja template rendered by HA; params are available as variables.
    Template { template: String },
    /// Runtime call into a plugin host (Python bridge or WASM).
    Plugin { plugin: String, function: String },
}
