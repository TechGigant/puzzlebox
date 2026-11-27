//! Lists changed in place, running other programs and fading lights step by step
//! (frontend/src/lib/blocks/more.ts).

use super::{Block, Compiler};
use crate::ir::*;

impl Compiler<'_> {
    /// `name = builtin(name, ITEM)` for the list in the block's variable field.
    fn list_update(&mut self, b: &Block<'_>, builtin: &str) -> StmtKind {
        let (name, scope) = self.var(b, "VAR");
        let current = Expr { id: b.id().into(), kind: ExprKind::Var { name: name.clone(), scope } };
        let item = self.value(b, "ITEM");
        let value = Expr {
            id: b.id().into(),
            kind: ExprKind::Builtin { name: builtin.into(), args: vec![current, item] },
        };
        StmtKind::Set { name, scope, value }
    }

    pub(super) fn more_stmt(&mut self, b: &Block<'_>) -> Option<StmtKind> {
        Some(match b.ty() {
            "lists_add" => self.list_update(b, "list_append"),
            "lists_remove_value" => self.list_update(b, "list_remove_value"),
            "program_run" | "program_run_wait" => {
                let program = b.field_str("PROGRAM").unwrap_or("").trim().to_owned();
                if program.is_empty() {
                    self.err(b, "choose the program to run");
                }
                StmtKind::RunProgram { program, wait: b.ty() == "program_run_wait" }
            }
            "light_fade" => {
                let target = self.value_opt(b, "TARGET");
                if target.is_none() {
                    self.err(b, "choose the lights to fade");
                }
                StmtKind::Fade {
                    target: target.unwrap_or_else(|| Expr::lit(b.id(), serde_json::Value::Null)),
                    from: self.value(b, "FROM"),
                    to: self.value(b, "TO"),
                    seconds: self.value(b, "SECONDS"),
                }
            }
            _ => return None,
        })
    }

    pub(super) fn more_expr(&mut self, b: &Block<'_>) -> Option<ExprKind> {
        Some(match b.ty() {
            "lists_contains" => ExprKind::Builtin {
                name: "list_contains".into(),
                args: vec![self.value(b, "LIST"), self.value(b, "ITEM")],
            },
            _ => return None,
        })
    }
}
