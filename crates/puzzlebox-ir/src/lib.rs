//! IR for puzzlebox programs and the Blockly workspace → IR compiler.

mod compile;
mod hints;
mod ir;

pub use compile::{
    BUILTIN_MQTT, Block, BlockLowerer, CompileError, Compiler, NoExternals, compile, compile_partial,
};
pub use hints::does_nothing;
pub use ir::*;
