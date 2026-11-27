//! Block registry: merges every block source and lowers their blocks to IR.

pub mod autogen;
mod lower;
mod model;
pub mod pack;
pub mod z2m;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

pub use lower::{BUILTIN_Z2M, RegistryLowerer};
pub use model::*;
pub use pack::{Pack, PackFile, parse_pack};

#[derive(Default)]
pub struct Registry {
    sources: RwLock<BTreeMap<String, Vec<BlockDef>>>,
}

impl Registry {
    /// Replaces every block contributed by `source`. Returns true if anything changed.
    pub fn set_source(&self, source: &str, defs: Vec<BlockDef>) -> bool {
        let mut s = self.sources.write().unwrap_or_else(|p| p.into_inner());
        if s.get(source) == Some(&defs) {
            return false;
        }
        s.insert(source.to_owned(), defs);
        true
    }

    pub fn remove_source(&self, source: &str) -> bool {
        self.sources.write().unwrap_or_else(|p| p.into_inner()).remove(source).is_some()
    }

    /// The blocks `source` contributes.
    pub fn source(&self, source: &str) -> Option<Vec<BlockDef>> {
        self.sources.read().unwrap_or_else(|p| p.into_inner()).get(source).cloned()
    }

    /// Whether any source has a block of this type.
    pub fn contains(&self, block_type: &str) -> bool {
        let s = self.sources.read().unwrap_or_else(|p| p.into_inner());
        s.values().any(|defs| defs.iter().any(|d| d.block_type == block_type))
    }

    pub fn sources(&self) -> Vec<String> {
        self.sources.read().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect()
    }

    /// All blocks; on a type clash the later source (by name) wins, so plugins can't be shadowed by `ha`.
    pub fn all(&self) -> Vec<BlockDef> {
        let s = self.sources.read().unwrap_or_else(|p| p.into_inner());
        let mut by_type: HashMap<String, BlockDef> = HashMap::new();
        let mut order = Vec::new();
        for defs in s.values() {
            for d in defs {
                if by_type.insert(d.block_type.clone(), d.clone()).is_none() {
                    order.push(d.block_type.clone());
                }
            }
        }
        order.into_iter().filter_map(|t| by_type.remove(&t)).collect()
    }

    pub fn lowerer(&self) -> Arc<RegistryLowerer> {
        let defs = self.all().into_iter().map(|d| (d.block_type.clone(), d)).collect();
        Arc::new(RegistryLowerer { defs })
    }
}
