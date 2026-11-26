//! Value blocks that work exactly like Home Assistant templates: strict number conversion,
//! rounding, printing and comparisons the way Python does them.

use serde_json::json;

use super::{Block, Compiler};
use crate::ir::*;

impl Compiler<'_> {
    /// `None` if `b` isn't one of these blocks.
    pub(super) fn ha_value_expr(&mut self, b: &Block<'_>) -> Option<ExprKind> {
        let lit = |v: serde_json::Value| Expr::lit(b.id(), v);
        let (name, args) = match b.ty() {
            "to_number_ha" => {
                let name = if b.field_str("KIND") == Some("int") { "py_int" } else { "py_float" };
                let mut args = vec![self.value(b, "VALUE")];
                args.extend(self.value_opt(b, "DEFAULT"));
                (name, args)
            }
            "math_round_ha" => {
                let decimals = b.field_f64("DECIMALS").unwrap_or(0.0).max(0.0).trunc();
                let method = b.field_str("METHOD").unwrap_or("common");
                ("py_round", vec![self.value(b, "NUM"), lit(json!(decimals as i64)), lit(json!(method))])
            }
            "to_text_ha" => {
                let mode = if b.field_str("NUMBERS") == Some("float") { "float" } else { "auto" };
                ("py_text", vec![self.value(b, "VALUE"), lit(json!(mode))])
            }
            "logic_compare_strict" => {
                let op = b.field_str("OP").unwrap_or("EQ");
                ("py_compare", vec![self.value(b, "A"), self.value(b, "B"), lit(json!(op))])
            }
            "logic_is_null" => ("is_null", vec![self.value(b, "VALUE")]),
            "time_format" => {
                let pattern = b.field_str("FORMAT").unwrap_or("%H:%M");
                ("time_format", vec![self.value(b, "DATETIME"), lit(json!(pattern))])
            }
            _ => return None,
        };
        Some(ExprKind::Builtin { name: name.into(), args })
    }
}
