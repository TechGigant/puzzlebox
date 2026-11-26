use puzzlebox_ir::{BinOp, Expr, ExprKind, NoExternals, StmtKind, compile};
use puzzlebox_yaml_import::import_yaml;

fn data(kind: &StmtKind) -> &[(String, Expr)] {
    let StmtKind::CallService { data: Some(d), .. } = kind else { panic!("{kind:?}") };
    let ExprKind::Dict { entries } = &d.kind else { panic!() };
    entries
}

#[test]
fn templates_become_blocks_and_compile() {
    let imp = import_yaml(include_str!("fixtures/templates/thermostat.yaml")).unwrap().remove(0);
    let program = compile(&imp.workspace, &NoExternals).expect("imported workspace compiles");
    let body = &program.handlers[0].body;

    let StmtKind::Set { name, value, .. } = &body[0].kind else { panic!() };
    assert_eq!(name, "target");
    assert!(matches!(&value.kind, ExprKind::State { attribute: Some(_), .. }));
    let StmtKind::Set { value, .. } = &body[1].kind else { panic!() };
    let ExprKind::Binary { bin: BinOp::Gt, left, .. } = &value.kind else { panic!("{value:?}") };
    assert!(matches!(&left.kind, ExprKind::Builtin { name, .. } if name == "ha:number_state"));
    let StmtKind::Set { value, .. } = &body[2].kind else { panic!() };
    assert!(matches!(&value.kind, ExprKind::Ternary { .. }));

    for (_, e) in data(&body[3].kind) {
        assert!(matches!(&e.kind, ExprKind::Builtin { name, .. } if name == "text_join"), "{e:?}");
    }
    assert!(matches!(&data(&body[4].kind)[0].1.kind, ExprKind::Var { name, .. } if name == "target"));
    // round() rounds half to even in HA, like the "round … like Home Assistant" block.
    let ExprKind::Builtin { name, args } = &data(&body[5].kind)[0].1.kind else { panic!() };
    assert_eq!(name, "py_round");
    assert!(matches!(&args[0].kind, ExprKind::Builtin { name, .. } if name == "ha:number_state"));
    // float(0) prints "0" or "21.0", which no block can tell apart, so it stays a template.
    assert_eq!(
        data(&body[6].kind)[0].1.kind,
        ExprKind::Template { template: "{{ states('sensor.living_temperature') | float(0) }} °C".into() }
    );
}
