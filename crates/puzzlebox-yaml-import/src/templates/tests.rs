use puzzlebox_ir::{NoExternals, compile};
use serde_json::{Value, json};

use super::parse::{E, Part, parse_template};
use crate::Builder;

/// `type[FIELD=v](INPUT=…)`, ids left out.
fn shape(b: &Value) -> String {
    let mut out = b["type"].as_str().unwrap_or("?").to_owned();
    if let Some(f) = b.get("fields").and_then(Value::as_object) {
        let mut fs: Vec<String> = f
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}={s}"),
                Value::Object(o) => format!("{k}={}", o["id"].as_str().unwrap_or("")),
                other => format!("{k}={other}"),
            })
            .collect();
        fs.sort();
        out += &format!("[{}]", fs.join(","));
    }
    if let Some(ins) = b.get("inputs").and_then(Value::as_object) {
        let mut is: Vec<String> = ins
            .iter()
            .map(|(k, v)| format!("{k}={}", shape(v.get("block").or_else(|| v.get("shadow")).unwrap())))
            .collect();
        is.sort();
        out += &format!("({})", is.join(", "));
    }
    out
}

fn builder() -> Builder {
    let mut b = Builder::default();
    for v in ["level", "mode", "items"] {
        b.var_id(v);
    }
    b
}

/// The block must compile inside a program.
fn compiles(b: &mut Builder, value: Value) {
    let target = b.var_id("__out");
    let set = json!({"type": "variables_set", "id": "set", "fields": {"VAR": {"id": target}}, "inputs": {"VALUE": {"block": value}}});
    let top = json!({"type": "ha_on_run", "id": "top", "inputs": {"DO": {"block": set}}});
    let vars: Vec<Value> = b.vars.iter().map(|(n, id)| json!({"name": n, "id": id})).collect();
    let ws = json!({"blocks": {"languageVersion": 0, "blocks": [top]}, "variables": vars});
    if let Err(e) = compile(&ws, &NoExternals) {
        panic!("does not compile: {e:?}\n{value:#}");
    }
}

fn value(t: &str) -> String {
    let mut b = builder();
    let v = b.template_value(t);
    compiles(&mut b, v.clone());
    shape(&v)
}

fn condition(t: &str) -> Option<String> {
    let mut b = builder();
    let before = b.next_id;
    let out = b.template_condition(t);
    match out {
        Some(v) => {
            compiles(&mut b, v.clone());
            Some(shape(&v))
        }
        None => {
            assert_eq!(b.next_id, before, "failed translations leave no gaps in ids");
            None
        }
    }
}

const S_T: &str = "ha_state(ENTITY=ha_entity[ENTITY=sensor.t])";

fn exact_state(e: &str) -> String {
    let s = format!("ha_state(ENTITY=ha_entity[ENTITY={e}])");
    format!("logic_ternary(ELSE={s}, IF=logic_compare[OP=EQ](A={s}, B=logic_null), THEN=text[TEXT=unknown])")
}

#[test]
fn parses_templates() {
    let p = parse_template("a {{- 1 -}}  b {{ '}}' }}").unwrap();
    assert_eq!(
        p,
        vec![
            Part::Text("a".into()),
            Part::Expr(E::Int(1)),
            Part::Text("b ".into()),
            Part::Expr(E::Str("}}".into()))
        ]
    );
    let p = parse_template("{{ x | float(0) * 2 > -1 }}").unwrap();
    assert_eq!(p.len(), 1);
    for bad in [
        "{% if x %}y{% endif %}",
        "{# c #}",
        "{{ x ",
        "{{ x is defined }}",
        "{{ 'a\\d' }}",
        "{{ 1 < x < 3 }}",
        "{{ }}",
        "{{ x[0] }}",
        "{{ 1x }}",
    ] {
        assert_eq!(parse_template(bad), None, "{bad}");
    }
}

#[test]
fn conditions_translate() {
    let st = |op: &str, b: &str| format!("logic_compare[OP={op}](A={S_T}, B={b})");
    let num = |op: &str, d: &str, n: &str| {
        format!(
            "logic_compare[OP={op}](A=ha_number_state(DEFAULT=math_number[NUM={d}], ENTITY=ha_entity[ENTITY=sensor.t]), B=math_number[NUM={n}])"
        )
    };
    let cases: Vec<(&str, String)> = vec![
        ("{{ is_state('sensor.t', 'on') }}", st("EQ", "text[TEXT=on]")),
        ("{{ is_state('sensor.t', 'unknown') }}", st("EQ", "text[TEXT=unknown]")),
        ("  {{ states('sensor.t') == 'on' }}\n", st("EQ", "text[TEXT=on]")),
        ("{{ 'on' != states('sensor.t') }}", st("NEQ", "text[TEXT=on]")),
        // Missing entities are "unknown" in HA.
        (
            "{{ states('sensor.t') == 'unknown' }}",
            format!("logic_compare[OP=EQ](A={}, B=text[TEXT=unknown])", exact_state("sensor.t")),
        ),
        (
            "{{ states('sensor.t') < 'm' }}",
            format!("logic_compare[OP=LT](A={}, B=text[TEXT=m])", exact_state("sensor.t")),
        ),
        ("{{ states('sensor.t') | float(0) > 20 }}", num("GT", "0", "20")),
        ("{{ float(states('sensor.t'), 0) > 20.5 }}", num("GT", "0", "20.5")),
        ("{{ states('sensor.t') | float(default=-1) <= 20 }}", num("LTE", "-1", "20")),
        ("{{ 20 < states('sensor.t') | float(0) }}", num("GT", "0", "20")),
        // int() truncates; against whole numbers the truncation moves into the bound.
        ("{{ states('sensor.t') | int(0) > 5 }}", num("GTE", "0", "6")),
        ("{{ states('sensor.t') | int(0) >= 5 }}", num("GTE", "0", "5")),
        ("{{ states('sensor.t') | int(0) >= 0 }}", num("GT", "0", "-1")),
        ("{{ states('sensor.t') | int(0) > -3 }}", num("GT", "0", "-3")),
        ("{{ states('sensor.t') | int(0) < 5 }}", num("LT", "0", "5")),
        ("{{ states('sensor.t') | int(0) <= 5 }}", num("LT", "0", "6")),
        ("{{ states('sensor.t') | int(0) < -2 }}", num("LTE", "0", "-3")),
        ("{{ int(states('sensor.t'), 0) <= -2 }}", num("LTE", "0", "-2")),
        (
            "{{ is_state('light.a', 'on') and not is_state('light.b', 'off') }}",
            "logic_operation[OP=AND](A=logic_compare[OP=EQ](A=ha_state(ENTITY=ha_entity[ENTITY=light.a]), B=text[TEXT=on]), B=logic_negate(BOOL=logic_compare[OP=EQ](A=ha_state(ENTITY=ha_entity[ENTITY=light.b]), B=text[TEXT=off])))".into(),
        ),
        (
            "{{ is_state('sensor.t', ['on', 'home']) }}",
            format!("logic_operation[OP=OR](A={}, B={})", st("EQ", "text[TEXT=on]"), st("EQ", "text[TEXT=home]")),
        ),
        (
            "{{ states('sensor.t') not in ['on', 'home'] }}",
            format!("logic_negate(BOOL=logic_operation[OP=OR](A={}, B={}))", st("EQ", "text[TEXT=on]"), st("EQ", "text[TEXT=home]")),
        ),
        ("{{ now().hour >= 18 }}", "logic_compare[OP=GTE](A=time_part[PART=hour], B=math_number[NUM=18])".into()),
        (
            "{{ now().weekday() in [5, 6] }}",
            "logic_operation[OP=OR](A=logic_compare[OP=EQ](A=math_arithmetic[OP=MINUS](A=time_part[PART=weekday], B=math_number[NUM=1]), B=math_number[NUM=5]), B=logic_compare[OP=EQ](A=math_arithmetic[OP=MINUS](A=time_part[PART=weekday], B=math_number[NUM=1]), B=math_number[NUM=6]))".into(),
        ),
        (
            "{{ now() > today_at('18:30') }}",
            "logic_compare[OP=GT](A=time_seconds_between(A=time_today_at[TIME=18:30], B=time_now), B=math_number[NUM=0])".into(),
        ),
        (
            "{{ today_at('07:00') >= now() }}",
            "logic_compare[OP=LTE](A=time_seconds_between(A=time_today_at[TIME=07:00], B=time_now), B=math_number[NUM=0])".into(),
        ),
        ("{{ has_value('sensor.t') }}", "ha_has_value(ENTITY=ha_entity[ENTITY=sensor.t])".into()),
        (
            "{{ is_state_attr('climate.x', 'hvac_action', 'heating') }}",
            "logic_compare[OP=EQ](A=ha_attribute[ATTRIBUTE=hvac_action](ENTITY=ha_entity[ENTITY=climate.x]), B=text[TEXT=heating])".into(),
        ),
        (
            "{{ state_attr('climate.x', 'preset') == 'eco' }}",
            "logic_compare[OP=EQ](A=ha_attribute[ATTRIBUTE=preset](ENTITY=ha_entity[ENTITY=climate.x]), B=text[TEXT=eco])".into(),
        ),
        (
            "{{ state_attr('climate.x', 'temperature') | float(0) > 20 }}",
            "logic_compare[OP=GT](A=to_number_ha[KIND=float](DEFAULT=math_number[NUM=0], VALUE=ha_attribute[ATTRIBUTE=temperature](ENTITY=ha_entity[ENTITY=climate.x])), B=math_number[NUM=20])".into(),
        ),
        (
            "{{ trigger.to_state.state == 'on' }}",
            "logic_compare[OP=EQ](A=trigger_info[ATTR=,WHAT=to], B=text[TEXT=on])".into(),
        ),
        (
            "{{ trigger.id == 'motion' }}",
            "logic_compare[OP=EQ](A=ha_trigger_data[PATH=id], B=text[TEXT=motion])".into(),
        ),
        (
            "{{ trigger['to_state']['attributes']['brightness'] | float(0) > 100 }}",
            "logic_compare[OP=GT](A=to_number_ha[KIND=float](DEFAULT=math_number[NUM=0], VALUE=trigger_info[ATTR=brightness,WHAT=to_attr]), B=math_number[NUM=100])".into(),
        ),
        ("{{ states.sensor.t.state == 'on' }}", st("EQ", "text[TEXT=on]")),
        ("{{ level == 'high' }}", "logic_compare[OP=EQ](A=variables_get[VAR=var_1], B=text[TEXT=high])".into()),
        ("{{ mode == true }}", "logic_compare[OP=EQ](A=variables_get[VAR=var_2], B=logic_boolean[BOOL=TRUE])".into()),
        ("{{ not level }}", "logic_negate(BOOL=variables_get[VAR=var_1])".into()),
        ("{{ true }}", "logic_boolean[BOOL=TRUE]".into()),
        ("{{ False }}", "logic_boolean[BOOL=FALSE]".into()),
        ("{{ 'abc' in states('sensor.t') }}", format!("text_contains(FIND=text[TEXT=abc], TEXT={})", exact_state("sensor.t"))),
        ("{{ states('sensor.t') | lower == 'on' }}", format!("logic_compare[OP=EQ](A=text_changeCase[CASE=LOWERCASE](TEXT={}), B=text[TEXT=on])", exact_state("sensor.t"))),
        (
            "{{ (states('sensor.t') | float(0) - 3) * 2 >= 10 }}",
            "logic_compare[OP=GTE](A=math_arithmetic[OP=MULTIPLY](A=math_arithmetic[OP=MINUS](A=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]), B=math_number[NUM=3]), B=math_number[NUM=2]), B=math_number[NUM=10])".into(),
        ),
        (
            "{{ iif(is_state('sensor.t', 'on'), true, false) }}",
            format!("logic_ternary(ELSE=logic_boolean[BOOL=FALSE], IF={}, THEN=logic_boolean[BOOL=TRUE])", st("EQ", "text[TEXT=on]")),
        ),
        (
            "{{ max(1, states('sensor.t') | float(0)) > 3 }}",
            "logic_compare[OP=GT](A=math_on_list[OP=MAX](LIST=lists_create_with(ADD0=math_number[NUM=1], ADD1=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]))), B=math_number[NUM=3])".into(),
        ),
    ];
    for (t, want) in cases {
        assert_eq!(condition(t).as_deref(), Some(want.as_str()), "{t}");
    }
}

#[test]
fn conditions_kept_as_templates() {
    for t in [
        // Only "true" passes a template condition: these render other text.
        "{{ states('sensor.t') }}",
        "{{ 1 }}",
        "{{ level }}",
        "{{ is_state('a.b', 'on') }} and {{ is_state('c.d', 'on') }}",
        // Text against a number by size always raises in Jinja.
        "{{ states('sensor.t') > 20 }}",
        "{{ 'a' < 1 }}",
        "{{ none < 1 }}",
        // Undefined in Jinja (not none), null in Puzzlebox.
        "{{ level == none }}",
        "{{ level is none }}",
        "{{ none != level }}",
        "{{ trigger.to_state.state is none }}",
        "{{ trigger.to_state.attributes.x is not none }}",
        "{{ is_state_attr('climate.x', 'temperature', none) }}",
        // Tests other than none, or what might be a test argument.
        "{{ state_attr('a.b', 'c') is defined }}",
        "{{ state_attr('a.b', 'c') is divisibleby 3 }}",
        "{{ state_attr('a.b', 'c') is none 5 }}",
        // Locale-dependent or differently written strftime codes.
        "{{ now().strftime('%c') == 'x' }}",
        "{{ now().strftime('%f') == '0' }}",
        "{{ now().strftime('%Z') == 'UTC' }}",
        "{{ now().strftime(level) == 'x' }}",
        // round with a negative precision or a default.
        "{{ states('sensor.t') | round(-1) > 2 }}",
        "{{ states('sensor.t') | round(1, default=0) > 2 }}",
        // and/or return operands in Jinja.
        "{{ level and is_state('a.b', 'on') }}",
        "{{ states('sensor.t') or 'x' }}",
        // Python's % takes the divisor's sign.
        "{{ states('sensor.t') | float(0) % -3 > 0 }}",
        "{{ (states('sensor.t') | float(0)) % level > 0 }}",
        // Unknown names, functions, filters, tests and statements.
        "{{ undefined_var == 'x' }}",
        "{{ is_state('Light.A', 'on') }}",
        "{{ states('sensor.t', rounded=True) == '1' }}",
        "{{ x is defined }}",
        "{{ states('sensor.t') | regex_match('a') }}",
        "{% if is_state('a.b', 'on') %}true{% endif %}",
        "{{ now() > today_at('25:00') }}",
        "{{ now() == today_at('10:00') }}",
        "{{ iif(is_state('a.b', 'on'), 1 / 0, 2) == 2 }}",
        "{{ 1 < now().hour < 5 }}",
        "{{ trigger.to_state.items == 'x' }}",
        "{{ state_attr('a.b', 'x/y') == 'z' }}",
        "{{ 'a' in level }}",
    ] {
        assert_eq!(condition(t), None, "{t}");
    }
}

#[test]
fn values_translate() {
    let cases: Vec<(&str, String)> = vec![
        ("{{ 5 }}", "math_number[NUM=5]".into()),
        ("{{ -2.5 }}", "math_number[NUM=-2.5]".into()),
        ("{{ none }}", "logic_null".into()),
        ("{{ 'heat' }}", "text[TEXT=heat]".into()),
        ("{{ level }}", "variables_get[VAR=var_1]".into()),
        ("{{ states('sensor.t') }}", exact_state("sensor.t")),
        ("{{ state_attr('climate.x', 'temperature') }}", "ha_attribute[ATTRIBUTE=temperature](ENTITY=ha_entity[ENTITY=climate.x])".into()),
        ("{{ trigger.to_state.state }}", "trigger_info[ATTR=,WHAT=to]".into()),
        ("{{ trigger.from_state.state }}", "trigger_info[ATTR=,WHAT=from]".into()),
        ("{{ trigger.entity_id }}", "trigger_entity".into()),
        ("{{ trigger.event.data.button }}", "ha_trigger_data[PATH=event.data.button]".into()),
        ("{{ states('sensor.t') | float(0) * 1.8 + 32 }}", "math_arithmetic[OP=ADD](A=math_arithmetic[OP=MULTIPLY](A=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]), B=math_number[NUM=1.8]), B=math_number[NUM=32])".into()),
        ("{{ (level | float(0)) / 2 }}", "math_arithmetic[OP=DIVIDE](A=to_number_ha[KIND=float](DEFAULT=math_number[NUM=0], VALUE=variables_get[VAR=var_1]), B=math_number[NUM=2])".into()),
        ("{{ 7 // 2 }}", "math_round[OP=ROUNDDOWN](NUM=math_arithmetic[OP=DIVIDE](A=math_number[NUM=7], B=math_number[NUM=2]))".into()),
        ("{{ now().minute % 15 }}", "math_modulo(DIVIDEND=time_part[PART=minute], DIVISOR=math_number[NUM=15])".into()),
        ("{{ 2 ** 3 }}", "math_arithmetic[OP=POWER](A=math_number[NUM=2], B=math_number[NUM=3])".into()),
        ("{{ -(states('sensor.t') | float(0)) | abs }}", "math_single[OP=ABS](NUM=math_single[OP=NEG](NUM=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t])))".into()),
        ("{{ states('sensor.t') | float(0) | round(0, 'floor') }}", "math_round[OP=ROUNDDOWN](NUM=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]))".into()),
        ("{{ 'on' if is_state('sun.sun', 'above_horizon') else 'off' }}", "logic_ternary(ELSE=text[TEXT=off], IF=logic_compare[OP=EQ](A=ha_state(ENTITY=ha_entity[ENTITY=sun.sun]), B=text[TEXT=above_horizon]), THEN=text[TEXT=on])".into()),
        ("{{ iif(level, 'yes', 'no') }}", "logic_ternary(ELSE=text[TEXT=no], IF=variables_get[VAR=var_1], THEN=text[TEXT=yes])".into()),
        ("{{ [1, 2, now().hour] | max }}", "math_on_list[OP=MAX](LIST=lists_create_with(ADD0=math_number[NUM=1], ADD1=math_number[NUM=2], ADD2=time_part[PART=hour]))".into()),
        ("{{ area_entities('Kitchen') }}", "ha_entities_in[DOMAIN=*,KIND=area,NAME=Kitchen]".into()),
        ("{{ as_timestamp(now()) }}", "time_part[PART=timestamp]".into()),
        ("{{ states('sensor.t') | upper }}", format!("text_changeCase[CASE=UPPERCASE](TEXT={})", exact_state("sensor.t"))),
        ("{{ states('sensor.t') | default('x') }}", exact_state("sensor.t")),
        ("{{ is_state('light.a', 'on') }}", "logic_compare[OP=EQ](A=ha_state(ENTITY=ha_entity[ENTITY=light.a]), B=text[TEXT=on])".into()),
        // Text with values.
        (
            "Temperature is {{ states('sensor.t') }} °C",
            format!("text_join(ADD0=text[TEXT=Temperature is ], ADD1={}, ADD2=text[TEXT= °C])", exact_state("sensor.t")),
        ),
        ("{{ trigger.to_state.state }} °C", "text_join(ADD0=trigger_info[ATTR=,WHAT=to], ADD1=text[TEXT= °C])".into()),
        ("{{ now().hour }} h", "text_join(ADD0=time_part[PART=hour], ADD1=text[TEXT= h])".into()),
        ("  Hi {{ trigger.entity_id }}!  ", "text_join(ADD0=text[TEXT=Hi ], ADD1=trigger_entity, ADD2=text[TEXT=!])".into()),
        (
            "Light {{ trigger.entity_id ~ ' is ' ~ trigger.to_state.state }}",
            "text_join(ADD0=text[TEXT=Light ], ADD1=trigger_entity, ADD2=text[TEXT= is ], ADD3=trigger_info[ATTR=,WHAT=to])".into(),
        ),
        ("Count: {{ now().hour | string }}", "text_join(ADD0=text[TEXT=Count: ], ADD1=to_text(VALUE=time_part[PART=hour]))".into()),
        ("Mode {{ 'a' + 'b' }}", "text_join(ADD0=text[TEXT=Mode ], ADD1=text_join(ADD0=text[TEXT=a], ADD1=text[TEXT=b]))".into()),
    ];
    for (t, want) in cases {
        assert_eq!(value(t), want, "{t}");
    }
}

#[test]
fn values_kept_as_templates() {
    for t in [
        // HA would parse these texts into a number, boolean or list.
        "{{ '5' }}",
        "{{ 'True' }}",
        "{{ ' padded' }}",
        "{{ '1' if level else '2' }}",
        "{{ level }}{{ mode }}",
        "[{{ level }}]",
        "True {{ level }}",
        "b'{{ level }}'",
        // Printed differently depending on a type only known when it runs: float(0) gives
        // 0 or a float ("0" / "20.0"), attributes may be tuples or datetimes.
        "Temp {{ states('sensor.t') | float(0) }}",
        "Lvl {{ 'x' ~ (level | int(0.5)) }}",
        "Attr: {{ state_attr('a.b', 'c') }}",
        "Value: {{ level }}",
        "{{ level }} 5",
        // Python's timestamps have microseconds, Puzzlebox's milliseconds.
        "Ts {{ as_timestamp(now()) }}",
        "Ts {{ now().timestamp() / 2 }}",
        // UTC instead of local time.
        "Now {{ now().timestamp() | timestamp_custom('%H:%M', false) }}",
        "Now {{ states('sensor.t') | timestamp_custom('%H:%M') }}",
        "Now {{ now().strftime('%x') }}",
        // No exact block.
        "{{ now() }}",
        "{{ 2.5 | round(-1) }}",
        "{{ trigger.to_state.state | default('x') }}",
        "{{ level | lower }}",
        "{{ states('sensor.t') + 1 }}",
        "{{ 'a' ~ level }}",
        "{{ 'x' if level }}",
        "{{ [1, 2] }}",
        "{{ max([]) }}",
        "{{ area_entities(\"it's\") }}",
        "{{ trigger }}",
        "{{ repeat.index }}",
        "{{ level.attr }}",
        "{% set x = 1 %}{{ x }}",
    ] {
        let mut b = builder();
        let v = b.template_value(t);
        assert_eq!(v, json!({"type": "ha_template", "id": "imp1", "fields": {"TEMPLATE": t}}), "{t}");
    }
}

/// Templates that need the "like Home Assistant" blocks: strict comparisons, `is none`,
/// float/int without a default, rounding half to even.
#[test]
fn ha_like_conditions_translate() {
    let state = "ha_state(ENTITY=ha_entity[ENTITY=sensor.t])";
    let temp = "ha_attribute[ATTRIBUTE=temperature](ENTITY=ha_entity[ENTITY=climate.x])";
    let strict = |op: &str, a: &str, b: &str| format!("logic_compare_strict[OP={op}](A={a}, B={b})");
    let num = |op: &str, a: &str, n: &str| format!("logic_compare[OP={op}](A={a}, B=math_number[NUM={n}])");
    let cases: Vec<(&str, String)> = vec![
        // "5" is text in Jinja, so it isn't equal to 5 and "10" < "9".
        ("{{ is_state('sensor.t', '5') }}", strict("EQ", state, "text[TEXT=5]")),
        (
            "{{ is_state('sensor.t', ['5', 'on']) }}",
            format!(
                "logic_operation[OP=OR](A={}, B=logic_compare[OP=EQ](A={state}, B=text[TEXT=on]))",
                strict("EQ", state, "text[TEXT=5]")
            ),
        ),
        ("{{ states('sensor.t') == '5' }}", strict("EQ", state, "text[TEXT=5]")),
        ("{{ '5' != states('sensor.t') }}", strict("NEQ", state, "text[TEXT=5]")),
        ("{{ states('sensor.t') > '10' }}", strict("GT", &exact_state("sensor.t"), "text[TEXT=10]")),
        (
            "{{ states('sensor.a') == states('sensor.b') }}",
            strict("EQ", &exact_state("sensor.a"), &exact_state("sensor.b")),
        ),
        ("{{ states('sensor.t') == 5 }}", strict("EQ", &exact_state("sensor.t"), "math_number[NUM=5]")),
        (
            "{{ trigger.to_state.state == 5 }}",
            strict("EQ", "trigger_info[ATTR=,WHAT=to]", "math_number[NUM=5]"),
        ),
        ("{{ level == 5 }}", strict("EQ", "variables_get[VAR=var_1]", "math_number[NUM=5]")),
        // A missing attribute is None, which ordering raises on like the strict block.
        ("{{ state_attr('climate.x', 'temperature') > 20 }}", strict("GT", temp, "math_number[NUM=20]")),
        ("{{ is_state_attr('climate.x', 'temperature', 20) }}", strict("EQ", temp, "math_number[NUM=20]")),
        ("{{ is_state_attr('climate.x', 'temperature', '20') }}", strict("EQ", temp, "text[TEXT=20]")),
        (
            "{{ states('sensor.t') in ['5', 'on'] }}",
            format!(
                "logic_operation[OP=OR](A={}, B=logic_compare[OP=EQ](A={state}, B=text[TEXT=on]))",
                strict("EQ", state, "text[TEXT=5]")
            ),
        ),
        (
            "{{ now().strftime('%H') == '10' }}",
            strict("EQ", "time_format[FORMAT=%H](DATETIME=time_now)", "text[TEXT=10]"),
        ),
        // None, which state_attr gives for a missing attribute.
        ("{{ state_attr('climate.x', 'temperature') is none }}", format!("logic_is_null(VALUE={temp})")),
        ("{{ state_attr('climate.x', 'temperature') == none }}", format!("logic_is_null(VALUE={temp})")),
        (
            "{{ state_attr('climate.x', 'temperature') is not none }}",
            format!("logic_negate(BOOL=logic_is_null(VALUE={temp}))"),
        ),
        (
            "{{ not state_attr('climate.x', 'temperature') is none }}",
            format!("logic_negate(BOOL=logic_is_null(VALUE={temp}))"),
        ),
        (
            "{{ none != state_attr('climate.x', 'temperature') }}",
            format!("logic_negate(BOOL=logic_is_null(VALUE={temp}))"),
        ),
        (
            "{{ state_attr('climate.x', 'temperature') is none or is_state('sensor.t', 'on') }}",
            format!(
                "logic_operation[OP=OR](A=logic_is_null(VALUE={temp}), B=logic_compare[OP=EQ](A={state}, B=text[TEXT=on]))"
            ),
        ),
        // float/int without a default stop with an error for non-numbers, like in HA.
        (
            "{{ states('sensor.t') | float > 20 }}",
            num("GT", &format!("to_number_ha[KIND=float](VALUE={state})"), "20"),
        ),
        (
            "{{ float(states('sensor.t')) > 20 }}",
            num("GT", &format!("to_number_ha[KIND=float](VALUE={state})"), "20"),
        ),
        (
            "{{ states('sensor.t') | int > 20 }}",
            num("GT", &format!("to_number_ha[KIND=int](VALUE={state})"), "20"),
        ),
        (
            "{{ int(states('sensor.t')) <= 20 }}",
            num("LTE", &format!("to_number_ha[KIND=int](VALUE={state})"), "20"),
        ),
        (
            "{{ state_attr('climate.x', 'temperature') | float(5) > 20 }}",
            num("GT", &format!("to_number_ha[KIND=float](DEFAULT=math_number[NUM=5], VALUE={temp})"), "20"),
        ),
        (
            "{{ states('sensor.t') | int(0) == 5 }}",
            num("EQ", &format!("to_number_ha[KIND=int](DEFAULT=math_number[NUM=0], VALUE={state})"), "5"),
        ),
        (
            "{{ states('sensor.t') | int(0.5) > 5 }}",
            num("GT", &format!("to_number_ha[KIND=int](DEFAULT=math_number[NUM=0.5], VALUE={state})"), "5"),
        ),
        // round() rounds half to even.
        (
            "{{ states('sensor.t') | float(0) | round > 2 }}",
            num(
                "GT",
                "math_round_ha[DECIMALS=0,METHOD=common](NUM=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]))",
                "2",
            ),
        ),
        (
            "{{ states('sensor.t') | round(1) > 2 }}",
            num("GT", &format!("math_round_ha[DECIMALS=1,METHOD=common](NUM={state})"), "2"),
        ),
        (
            "{{ states('sensor.t') | round(2, 'ceil') > 2 }}",
            num("GT", &format!("math_round_ha[DECIMALS=2,METHOD=ceil](NUM={state})"), "2"),
        ),
        (
            "{{ states('sensor.t') | round(precision=1, method='half') > 2 }}",
            num("GT", &format!("math_round_ha[DECIMALS=1,METHOD=half](NUM={state})"), "2"),
        ),
        // Any other method is "common" in HA.
        (
            "{{ states('sensor.t') | round(0, 'up') > 2 }}",
            num("GT", &format!("math_round_ha[DECIMALS=0,METHOD=common](NUM={state})"), "2"),
        ),
    ];
    for (t, want) in cases {
        assert_eq!(condition(t).as_deref(), Some(want.as_str()), "{t}");
    }
}

#[test]
fn ha_like_values_translate() {
    let state = "ha_state(ENTITY=ha_entity[ENTITY=sensor.t])";
    let join = |items: &[&str]| {
        let parts: Vec<String> = items.iter().enumerate().map(|(i, s)| format!("ADD{i}={s}")).collect();
        format!("text_join({})", parts.join(", "))
    };
    let py = |mode: &str, v: &str| format!("to_text_ha[NUMBERS={mode}](VALUE={v})");
    let float = format!("to_number_ha[KIND=float](VALUE={state})");
    let cases: Vec<(&str, String)> = vec![
        ("{{ states('sensor.t') | float }}", float.clone()),
        ("{{ states('sensor.t') | int }}", format!("to_number_ha[KIND=int](VALUE={state})")),
        ("{{ states('sensor.t') | int(0) }}", format!("to_number_ha[KIND=int](DEFAULT=math_number[NUM=0], VALUE={state})")),
        (
            "{{ (states('sensor.t') | float(0) * 1.5) | int }}",
            "to_number_ha[KIND=int](VALUE=math_arithmetic[OP=MULTIPLY](A=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]), B=math_number[NUM=1.5]))".into(),
        ),
        (
            "{{ state_attr('a.b', 'c') | float(state_attr('a.b', 'd')) }}",
            "to_number_ha[KIND=float](DEFAULT=ha_attribute[ATTRIBUTE=d](ENTITY=ha_entity[ENTITY=a.b]), VALUE=ha_attribute[ATTRIBUTE=c](ENTITY=ha_entity[ENTITY=a.b]))".into(),
        ),
        (
            "{{ states('sensor.t') | float(0) | round(1) }}",
            "math_round_ha[DECIMALS=1,METHOD=common](NUM=ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t]))".into(),
        ),
        ("{{ 2.5 | round }}", "math_round_ha[DECIMALS=0,METHOD=common](NUM=math_number[NUM=2.5])".into()),
        ("{{ now().strftime('%-d.%-m.%Y') }}", "time_format[FORMAT=%-d.%-m.%Y](DATETIME=time_now)".into()),
        // Text: printed like Python ("True", "None", "21.0").
        ("On: {{ is_state('a.b', 'on') }}", join(&["text[TEXT=On: ]", &py("auto", "logic_compare[OP=EQ](A=ha_state(ENTITY=ha_entity[ENTITY=a.b]), B=text[TEXT=on])")])),
        ("Value {{ none }}", join(&["text[TEXT=Value ]", &py("auto", "logic_null")])),
        ("Temp {{ states('sensor.t') | float }} °C", join(&["text[TEXT=Temp ]", &py("float", &float), "text[TEXT= °C]"])),
        (
            "Temp {{ states('sensor.t') | float(0.0) }}",
            join(&["text[TEXT=Temp ]", &py("float", "ha_number_state(DEFAULT=math_number[NUM=0], ENTITY=ha_entity[ENTITY=sensor.t])")]),
        ),
        (
            "Temp {{ states('sensor.t') | round(1) }} °C",
            join(&["text[TEXT=Temp ]", &py("float", &format!("math_round_ha[DECIMALS=1,METHOD=common](NUM={state})")), "text[TEXT= °C]"]),
        ),
        // Whole numbers print the same either way.
        ("Temp {{ states('sensor.t') | round }}", join(&["text[TEXT=Temp ]", &format!("math_round_ha[DECIMALS=0,METHOD=common](NUM={state})")])),
        ("Lvl {{ level | int }}", join(&["text[TEXT=Lvl ]", "to_number_ha[KIND=int](VALUE=variables_get[VAR=var_1])"])),
        (
            "Half {{ 7 / 2 }}",
            join(&["text[TEXT=Half ]", &py("float", "math_arithmetic[OP=DIVIDE](A=math_number[NUM=7], B=math_number[NUM=2])")]),
        ),
        ("x: {{ 'a' ~ none }}", join(&["text[TEXT=x: ]", "text[TEXT=a]", &py("auto", "logic_null")])),
        ("Now {{ now().strftime('%H:%M') }}", join(&["text[TEXT=Now ]", "time_format[FORMAT=%H:%M](DATETIME=time_now)"])),
        (
            "Now {{ as_timestamp(now()) | timestamp_custom('%H:%M') }}",
            join(&["text[TEXT=Now ]", "time_format[FORMAT=%H:%M](DATETIME=time_now)"]),
        ),
        (
            "Now {{ now().timestamp() | timestamp_custom(date_format='%A', local=true) }}",
            join(&["text[TEXT=Now ]", "time_format[FORMAT=%A](DATETIME=time_now)"]),
        ),
        ("{{ as_timestamp(now()) | timestamp_custom }}", "time_format[FORMAT=%Y-%m-%d](DATETIME=time_now)".into()),
    ];
    for (t, want) in cases {
        assert_eq!(value(t), want, "{t}");
    }
}
