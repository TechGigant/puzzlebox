//! Declarative block packs: a YAML/JSON file of block definitions.

use serde::{Deserialize, Serialize};

use crate::model::*;

pub const PACK_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackFile {
    /// Pack id: `[a-z0-9_]+`, unique per installation.
    pub pack: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default)]
    pub description: String,
    pub blocks: Vec<PackBlock>,
}

fn default_schema() -> u32 {
    PACK_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackBlock {
    /// `[a-z0-9_]+`, unique within the pack.
    pub id: String,
    pub kind: BlockKind,
    pub title: String,
    #[serde(default)]
    pub tooltip: String,
    #[serde(default)]
    pub help_url: Option<String>,
    /// Defaults to the pack name.
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub target: Option<TargetSpec>,
    #[serde(default)]
    pub params: Vec<Param>,
    pub lowering: Lowering,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub blocks: Vec<BlockDef>,
}

fn valid_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn lowering_fits(kind: BlockKind, l: &Lowering) -> bool {
    use Lowering::*;
    match kind {
        BlockKind::Action => matches!(l, Service { response: false, .. } | HaAction { .. } | Plugin { .. }),
        BlockKind::Trigger => matches!(l, HaTrigger { .. } | HaTriggerPlatform { .. }),
        BlockKind::Condition => {
            matches!(l, HaCondition { .. } | HaConditionPlatform { .. } | Template { .. } | Plugin { .. })
        }
        BlockKind::Value => matches!(l, Template { .. } | Plugin { .. } | Service { response: true, .. }),
    }
}

/// Parses and validates a pack. `source_kind` is `pack`, `py` or `wasm`; only `py`/`wasm`
/// sources may use `plugin` lowerings, and only for their own plugin id.
pub fn parse_pack(text: &str, source_kind: &str) -> Result<Pack, String> {
    let file: PackFile = serde_yaml::from_str(text).map_err(|e| format!("invalid pack: {e}"))?;
    from_file(file, source_kind)
}

pub fn from_file(file: PackFile, source_kind: &str) -> Result<Pack, String> {
    if !valid_ident(&file.pack) {
        return Err(format!("pack id '{}' must match [a-z0-9_]+", file.pack));
    }
    if file.schema > PACK_SCHEMA_VERSION {
        return Err(format!("pack needs schema {} but this app supports {PACK_SCHEMA_VERSION}", file.schema));
    }
    let source = format!("{source_kind}:{}", file.pack);
    let mut seen = std::collections::HashSet::new();
    let mut blocks = Vec::new();
    for b in file.blocks {
        if !valid_ident(&b.id) {
            return Err(format!("block id '{}' must match [a-z0-9_]+", b.id));
        }
        if !seen.insert(b.id.clone()) {
            return Err(format!("duplicate block id '{}'", b.id));
        }
        if !lowering_fits(b.kind, &b.lowering) {
            return Err(format!("block '{}': lowering does not fit kind {:?}", b.id, b.kind));
        }
        if let Lowering::Plugin { plugin, .. } = &b.lowering
            && (source_kind == "pack" || plugin != &file.pack)
        {
            return Err(format!(
                "block '{}': plugin lowerings are only allowed for the plugin's own id",
                b.id
            ));
        }
        let mut names = std::collections::HashSet::new();
        for p in &b.params {
            if !names.insert(&p.name) || p.name == "target" {
                return Err(format!("block '{}': duplicate or reserved param '{}'", b.id, p.name));
            }
        }
        blocks.push(BlockDef {
            block_type: format!("{source_kind}__{}__{}", file.pack, b.id),
            kind: b.kind,
            category: if b.category.is_empty() { vec![file.name.clone()] } else { b.category },
            style: b.style.unwrap_or_else(|| format!("{source_kind}_{}", file.pack)),
            title: b.title,
            tooltip: b.tooltip,
            help_url: b.help_url,
            target: b.target,
            params: b.params,
            lowering: b.lowering,
            source: source.clone(),
        });
    }
    Ok(Pack {
        id: file.pack,
        name: file.name,
        version: file.version,
        description: file.description,
        source,
        blocks,
    })
}
