//! The YAML documents users paste or that Home Assistant stores: an automation, a script,
//! lists and maps of them, `configuration.yaml` and package files, blueprints, and several
//! of these separated by `---`.

mod blueprints;
mod load;
mod tags;
#[cfg(test)]
mod tests;

use std::cell::RefCell;

use serde::Serialize;
use serde::de::DeserializeSeed;
use serde_json::{Map, Value, json};

use crate::is_config;
use blueprints::Blueprint;

/// One automation or script found in the pasted YAML.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Document {
    /// The script id when it came from a map (`scripts.yaml`: `{script_id: {...}}`).
    pub key: Option<String>,
    /// The config with document-level keys normalised (`trigger` → `triggers`, …).
    pub config: Value,
    /// Things the user should know about this item (placeholders, expanded blueprints, …).
    pub warnings: Vec<String>,
    /// Set when the item was found but can't be imported (e.g. its blueprint is missing).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Documents {
    pub items: Vec<Document>,
    /// Things that apply to no single item (e.g. `automation: !include automations.yaml`).
    pub warnings: Vec<String>,
}

/// The automations/scripts in a document, with their key when they came from a map.
pub(crate) fn configs(text: &str) -> Result<Vec<(Option<String>, Value)>, String> {
    let docs = documents(text)?;
    let ok: Vec<(Option<String>, Value)> =
        docs.items.iter().filter(|d| d.error.is_none()).map(|d| (d.key.clone(), d.config.clone())).collect();
    if ok.is_empty() {
        let reasons: Vec<&str> = docs
            .items
            .iter()
            .filter_map(|d| d.error.as_deref())
            .chain(docs.warnings.iter().map(String::as_str))
            .collect();
        if !reasons.is_empty() {
            return Err(format!("no automations or scripts could be imported: {}", reasons.join(" ")));
        }
    }
    Ok(ok)
}

/// Finds every automation and script in `text`, however it is laid out.
pub fn documents(text: &str) -> Result<Documents, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut finder = Finder::default();
    let duplicates = RefCell::new(Vec::new());
    for (i, de) in serde_yaml::Deserializer::from_str(text).enumerate() {
        let mut doc =
            load::Loose { duplicates: &duplicates }.deserialize(de).map_err(|e| yaml_error(i, &e))?;
        doc.apply_merge().map_err(|e| yaml_error(i, &e))?;
        finder.document(tags::to_json(doc), i);
    }
    let mut docs = finder.finish();
    for key in duplicates.into_inner() {
        tags::push_unique(
            &mut docs.warnings,
            format!(
                "The key `{key}` appears twice in the same place; the last one is used, as in Home Assistant."
            ),
        );
    }
    Ok(docs)
}

fn yaml_error(doc: usize, e: &serde_yaml::Error) -> String {
    if doc == 0 { format!("invalid YAML: {e}") } else { format!("invalid YAML in document {}: {e}", doc + 1) }
}

enum Found {
    Config { key: Option<String>, cfg: Value },
    UseBlueprint { key: Option<String>, cfg: Map<String, Value>, domain: Option<&'static str> },
    Blueprint(usize),
}

#[derive(Default)]
struct Finder {
    found: Vec<Found>,
    blueprints: Vec<Blueprint>,
    warnings: Vec<String>,
}

/// The integration a top-level `configuration.yaml` key configures (`automation manual` → `automation`).
fn domain(key: &str) -> &str {
    key.split(' ').next().unwrap_or(key)
}

const DOMAINS: [&str; 3] = ["automation", "script", "homeassistant"];

fn is_entry(v: &Value) -> bool {
    v.get("use_blueprint").is_some() || (is_config(v) && !is_action_step(v))
}

/// A single action such as `{action: light.turn_on, target: …}`, which `is_config` would
/// otherwise mistake for an automation whose actions are a string.
fn is_action_step(v: &Value) -> bool {
    let Some(m) = v.as_object() else { return false };
    let has = |k: &str| m.contains_key(k);
    let service = m.get("action").or_else(|| m.get("service")).is_some_and(Value::is_string);
    service && !["triggers", "trigger", "actions", "sequence", "use_blueprint"].iter().any(|k| has(k))
}

const ACTION_KEYS: &[&str] = &[
    "action",
    "service",
    "delay",
    "wait_template",
    "wait_for_trigger",
    "choose",
    "if",
    "repeat",
    "parallel",
    "sequence",
    "event",
    "scene",
    "stop",
    "variables",
    "condition",
    "device_id",
    "set_conversation_response",
];

/// Actions copied out of an automation (`- action: …` lines) without the automation around them.
fn is_action_list(items: &[Value]) -> bool {
    !items.is_empty()
        && items.iter().any(is_action_step)
        && items.iter().all(|i| {
            i.as_object().is_some_and(|m| m.keys().any(|k| ACTION_KEYS.contains(&k.as_str()))) && !is_entry(i)
        })
}

impl Finder {
    fn document(&mut self, doc: Value, index: usize) {
        let where_ = if index == 0 { "The YAML".to_owned() } else { format!("Document {}", index + 1) };
        if let Some(bp) = blueprints::parse(&doc) {
            self.found.push(Found::Blueprint(self.blueprints.len()));
            self.blueprints.push(bp);
            return;
        }
        match doc {
            Value::Null => {}
            _ if tags::tag(&doc).is_some() => self.tagged_container(&doc, &where_),
            Value::Array(items) if is_action_list(&items) => self.bare_actions(items),
            Value::Array(items) => {
                for (i, item) in items.into_iter().enumerate() {
                    self.entry(None, item, None, || format!("Item {} of the list", i + 1));
                }
            }
            Value::Object(_) if is_entry(&doc) => self.entry(None, doc, None, String::new),
            Value::Object(_) if is_action_step(&doc) => self.bare_actions(vec![doc]),
            Value::Object(m) if m.keys().any(|k| DOMAINS.contains(&domain(k))) => self.config_file(m, true),
            Value::Object(m) => self.map(m),
            _ => tags::push_unique(
                &mut self.warnings,
                format!("{where_} is a single value, not an automation, script, list or map; skipped."),
            ),
        }
    }

    /// `scripts.yaml` (`{id: script}`), or packages pasted as `{package_name: {automation: …}}`.
    fn map(&mut self, m: Map<String, Value>) {
        for (k, v) in m {
            if tags::tag(&v).is_some() {
                self.tagged_container(&v, &format!("`{k}`"));
            } else if is_entry(&v) {
                self.entry(Some(k), v, None, String::new);
            } else if let Value::Object(pkg) = v
                && pkg.keys().any(|k| DOMAINS.contains(&domain(k)))
            {
                self.config_file(pkg, false);
            }
        }
    }

    /// `configuration.yaml` or a package: only the `automation`/`script` keys matter.
    fn config_file(&mut self, m: Map<String, Value>, top_level: bool) {
        for (k, v) in m {
            match domain(&k) {
                "automation" => self.automations(&k, v),
                "script" => self.scripts(&k, v),
                // Like HA, packages can't define more packages.
                "homeassistant" if top_level => self.packages(v),
                _ => {}
            }
        }
    }

    fn packages(&mut self, core: Value) {
        let Some(packages) = core.get("packages") else { return };
        if tags::tag(packages).is_some() {
            return self.tagged_container(packages, "`homeassistant: packages:`");
        }
        let Some(packages) = packages.as_object() else { return };
        for (name, pkg) in packages {
            match pkg {
                v if tags::tag(v).is_some() => self.tagged_container(v, &format!("Package `{name}`")),
                Value::Object(m) => self.config_file(m.clone(), false),
                _ => {}
            }
        }
    }

    fn automations(&mut self, key: &str, v: Value) {
        match v {
            Value::Null => {}
            v if tags::tag(&v).is_some() => self.tagged_container(&v, &format!("`{key}:`")),
            Value::Array(items) => {
                for (i, item) in items.into_iter().enumerate() {
                    self.entry(None, item, Some("automation"), || format!("Item {} under `{key}:`", i + 1));
                }
            }
            v => self.entry(None, v, Some("automation"), || format!("`{key}:`")),
        }
    }

    fn scripts(&mut self, key: &str, v: Value) {
        match v {
            Value::Null => {}
            v if tags::tag(&v).is_some() => self.tagged_container(&v, &format!("`{key}:`")),
            Value::Object(m) => {
                for (id, cfg) in m {
                    let what = format!("Script `{id}` under `{key}:`");
                    self.entry(Some(id), cfg, Some("script"), || what);
                }
            }
            _ => tags::push_unique(
                &mut self.warnings,
                format!("`{key}:` should be a map of script ids to scripts; skipped."),
            ),
        }
    }

    /// Something like `automation: !include automations.yaml`: the content lives in a file we can't read.
    fn tagged_container(&mut self, v: &Value, what: &str) {
        let Some((t, inner)) = tags::tag(v) else { return };
        let text = tags::describe(t, inner);
        let msg = if tags::is_include(t) {
            format!(
                "{what} is `{text}`, which points to other files. Paste the content of those files instead."
            )
        } else {
            format!("{what} is `{text}`, which can't be read here; skipped.")
        };
        tags::push_unique(&mut self.warnings, msg);
    }

    fn entry(
        &mut self,
        key: Option<String>,
        v: Value,
        domain: Option<&'static str>,
        what: impl FnOnce() -> String,
    ) {
        if tags::tag(&v).is_some() {
            let what = what();
            self.tagged_container(&v, if what.is_empty() { "An item" } else { &what });
        } else if v.get("use_blueprint").is_some()
            && let Value::Object(cfg) = v
        {
            self.found.push(Found::UseBlueprint { key, cfg, domain });
        } else if is_entry(&v) || (domain.is_some() && v.is_object()) {
            // Under `automation:`/`script:` it is one even when incomplete; the importer copes.
            self.found.push(Found::Config { key, cfg: v });
        } else if domain.is_some() || v.is_object() {
            let what = what();
            if !what.is_empty() {
                tags::push_unique(
                    &mut self.warnings,
                    format!("{what} has no triggers, actions or sequence; skipped."),
                );
            }
        }
    }

    fn bare_actions(&mut self, actions: Vec<Value>) {
        self.found.push(Found::Config { key: None, cfg: json!({"sequence": actions}) });
        tags::push_unique(
            &mut self.warnings,
            "Actions without an automation or script around them were imported as a script.".into(),
        );
    }

    fn finish(self) -> Documents {
        let Finder { found, blueprints, mut warnings } = self;
        let mut used = vec![false; blueprints.len()];
        let mut slots: Vec<Result<Document, usize>> = Vec::new();
        for f in found {
            slots.push(match f {
                Found::Config { key, cfg } => Ok(item(key, cfg, Vec::new())),
                Found::UseBlueprint { key, cfg, domain } => match blueprints::find(&blueprints, &cfg, domain)
                {
                    Ok(i) => {
                        used[i] = true;
                        let mut w = Vec::new();
                        let cfg = blueprints[i].instantiate(&cfg, &mut w);
                        Ok(item(key, cfg, w))
                    }
                    Err(e) => Ok(Document { error: Some(e), ..item(key, Value::Object(cfg), Vec::new()) }),
                },
                Found::Blueprint(i) => Err(i),
            });
        }
        let mut items = Vec::new();
        for slot in slots {
            match slot {
                Ok(d) => items.push(d),
                // A blueprint pasted with automations that use it is their source, not an item itself.
                Err(i) if used[i] => {}
                Err(i) => {
                    let bp = &blueprints[i];
                    if let Some(d) = bp.domain.as_deref().filter(|d| !["automation", "script"].contains(d)) {
                        warnings.push(format!(
                            "Blueprint \"{}\" is for {d}, not an automation or script; skipped.",
                            bp.name
                        ));
                        continue;
                    }
                    let mut w = Vec::new();
                    let cfg = bp.with_defaults(&mut w);
                    items.push(item(None, cfg, w));
                }
            }
        }
        Documents { items, warnings }
    }
}

fn item(key: Option<String>, mut cfg: Value, mut warnings: Vec<String>) -> Document {
    tags::finish(&mut cfg, &mut warnings);
    normalise(&mut cfg, &mut warnings);
    Document { key, config: cfg, warnings, error: None }
}

/// Legacy spellings and loosely typed values HA accepts, in the form the importer reads.
fn normalise(cfg: &mut Value, warnings: &mut Vec<String>) {
    let Some(m) = cfg.as_object_mut() else { return };
    for (old, new) in [("trigger", "triggers"), ("condition", "conditions"), ("action", "actions")] {
        if !m.contains_key(old) {
            continue;
        }
        if m.contains_key(new) {
            m.remove(old);
            warnings.push(format!(
                "Both `{old}` and `{new}` are set (Home Assistant rejects this); kept `{new}`."
            ));
        } else {
            rename_key(m, old, new);
        }
    }
    for k in ["alias", "description", "id", "icon", "mode", "max_exceeded"] {
        if let Some(v) = m.get_mut(k)
            && let Value::Number(_) | Value::Bool(_) = v
        {
            *v = Value::String(v.to_string());
        }
    }
    if let Some(Value::String(mode)) = m.get_mut("mode") {
        *mode = mode.trim().to_lowercase();
    }
    if let Some(v) = m.get_mut("max")
        && let Some(n) = v.as_str().and_then(|s| s.trim().parse::<u64>().ok())
    {
        *v = json!(n);
    }
    if let Some(v) = m.get_mut("initial_state")
        && let Some(b) = v.as_str().and_then(boolean)
    {
        *v = json!(b);
    }
}

/// HA's `cv.boolean` for strings.
fn boolean(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enable" => Some(true),
        "0" | "false" | "no" | "off" | "disable" => Some(false),
        _ => None,
    }
}

/// Renames a key in place, keeping its position (the map preserves order).
fn rename_key(m: &mut Map<String, Value>, from: &str, to: &str) {
    *m = std::mem::take(m).into_iter().map(|(k, v)| (if k == from { to.to_owned() } else { k }, v)).collect();
}
