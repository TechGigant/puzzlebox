//! Blueprints: a pasted blueprint file imports with its defaults, and a `use_blueprint:`
//! automation/script imports when its blueprint is pasted alongside it (we can't read
//! `config/blueprints/`). Substitution follows HA's `blueprint/models.py`.

use serde_json::{Map, Value, json};

use super::tags;

pub(super) struct Blueprint {
    pub name: String,
    pub domain: Option<String>,
    description: Option<String>,
    source_url: Option<String>,
    /// Flattened input definitions (sections' inputs are lifted up, like `Blueprint.inputs`).
    inputs: Map<String, Value>,
    /// The document without its `blueprint:` key.
    body: Map<String, Value>,
}

pub(super) fn parse(doc: &Value) -> Option<Blueprint> {
    let meta = doc.get("blueprint")?.as_object()?;
    let mut inputs = Map::new();
    if let Some(defs) = meta.get("input").and_then(Value::as_object) {
        for (k, def) in defs {
            match def.get("input").and_then(Value::as_object) {
                Some(section) => inputs.extend(section.clone()),
                None => {
                    inputs.insert(k.clone(), def.clone());
                }
            }
        }
    }
    let mut body = doc.as_object()?.clone();
    body.remove("blueprint");
    let text = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Blueprint {
        name: text("name").unwrap_or_else(|| "Blueprint".into()),
        domain: text("domain"),
        description: text("description"),
        source_url: text("source_url"),
        inputs,
        body,
    })
}

impl Blueprint {
    fn defaults(&self) -> Map<String, Value> {
        self.inputs
            .iter()
            .filter_map(|(k, def)| Some((k.clone(), def.as_object()?.get("default")?.clone())))
            .collect()
    }

    /// The blueprint on its own, every input at its default.
    pub fn with_defaults(&self, warnings: &mut Vec<String>) -> Value {
        let mut cfg = self.substitute(&self.defaults());
        if let Some(m) = cfg.as_object_mut() {
            if !m.contains_key("alias") {
                m.insert("alias".into(), json!(self.name));
            }
            if let (false, Some(d)) = (m.contains_key("description"), &self.description) {
                m.insert("description".into(), json!(d));
            }
        }
        warnings.insert(0, format!("Blueprint \"{}\" was imported with its default inputs.", self.name));
        cfg
    }

    /// A `use_blueprint:` config expanded with its inputs (falling back to defaults).
    pub fn instantiate(&self, cfg: &Map<String, Value>, warnings: &mut Vec<String>) -> Value {
        let mut values = self.defaults();
        if let Some(given) = cfg.get("use_blueprint").and_then(|u| u.get("input")).and_then(Value::as_object)
        {
            values.extend(given.clone());
        }
        let mut out = self.substitute(&values);
        if let Some(m) = out.as_object_mut() {
            for (k, v) in cfg {
                if k != "use_blueprint" {
                    m.insert(k.clone(), v.clone());
                }
            }
        }
        warnings.insert(
            0,
            format!(
                "Expanded blueprint \"{}\": the import is a standalone copy and no longer follows the blueprint.",
                self.name
            ),
        );
        out
    }

    /// Inputs without a value keep their `!input` marker; `tags::finish` reports them.
    fn substitute(&self, values: &Map<String, Value>) -> Value {
        substitute(&Value::Object(self.body.clone()), values)
    }

    fn matches_path(&self, path: &str) -> bool {
        let file = |p: &str| p.rsplit('/').next().unwrap_or(p).to_owned();
        self.source_url.as_deref().is_some_and(|u| u.ends_with(&format!("/{path}")) || file(u) == file(path))
    }
}

fn substitute(v: &Value, values: &Map<String, Value>) -> Value {
    if let Some(val) = tags::input_name(v).and_then(|name| values.get(name)) {
        return val.clone();
    }
    match v {
        Value::Array(items) => Value::Array(items.iter().map(|i| substitute(i, values)).collect()),
        Value::Object(m) => {
            Value::Object(m.iter().map(|(k, i)| (k.clone(), substitute(i, values))).collect())
        }
        other => other.clone(),
    }
}

/// The pasted blueprint a `use_blueprint:` config refers to.
pub(super) fn find<'a>(
    blueprints: &'a [Blueprint],
    cfg: &Map<String, Value>,
    domain: Option<&str>,
) -> Result<usize, String> {
    let path = cfg.get("use_blueprint").and_then(|u| u.get("path")).and_then(Value::as_str).unwrap_or("");
    let candidates: Vec<(usize, &'a Blueprint)> = blueprints
        .iter()
        .enumerate()
        .filter(|(_, b)| domain.is_none() || b.domain.is_none() || b.domain.as_deref() == domain)
        .collect();
    if let Some((i, _)) = candidates.iter().find(|(_, b)| !path.is_empty() && b.matches_path(path)) {
        return Ok(*i);
    }
    match candidates.as_slice() {
        [(i, _)] => Ok(*i),
        [] => {
            let dir = domain.unwrap_or("automation");
            Err(format!(
                "Uses the blueprint `{path}`, which isn't in the pasted YAML. Paste the blueprint file \
                 (config/blueprints/{dir}/{path}) together with it to import it."
            ))
        }
        _ => Err(format!(
            "Uses the blueprint `{path}`, but several blueprints were pasted and none has a source_url \
             matching that path."
        )),
    }
}
