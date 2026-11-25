use puzzlebox_pdk::prelude::*;

/// Dew point (°C) via the Magnus formula.
#[plugin_fn]
pub fn dew_point(input: Json<puzzlebox_pdk::Call>) -> FnResult<Json<Value>> {
    let t = input.0.arg_f64("temperature").unwrap_or(20.0);
    let rh = input.0.arg_f64("humidity").unwrap_or(50.0).clamp(1.0, 100.0);
    let (a, b) = (17.62, 243.12);
    let gamma = (rh / 100.0).ln() + a * t / (b + t);
    let dp = b * gamma / (a - gamma);
    Ok(Json(json!((dp * 10.0).round() / 10.0)))
}

/// Counts how often it ran (persisted) and turns on a light once it is dark.
#[plugin_fn]
pub fn light_when_dark(input: Json<puzzlebox_pdk::Call>) -> FnResult<Json<Value>> {
    let count = kv_get("runs")?.as_u64().unwrap_or(0) + 1;
    kv_set("runs", json!(count))?;
    let sun = get_state("sun.sun")?;
    if sun["state"] == "below_horizon" {
        let light = input.0.arg_str("light").unwrap_or("light.porch");
        call_service("light.turn_on", None, Some(json!({"entity_id": light})))?;
        log(&format!("turned on {light} (run {count})"))?;
    }
    Ok(Json(Value::Null))
}
