//! `http_request` block runtime.

use std::time::Duration;

use async_trait::async_trait;
use puzzlebox_engine::HttpClient;
use serde_json::{Map, Value, json};

pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("puzzlebox/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        Self { client }
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
        headers: Option<Value>,
    ) -> Result<Value, String> {
        let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL '{url}': {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("only http and https URLs are supported".into());
        }
        let mut req = self.client.request(method, parsed);
        if let Some(Value::Object(h)) = headers {
            for (k, v) in h {
                let v = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                req = req.header(k, v);
            }
        }
        req = match body {
            None | Some(Value::Null) => req,
            Some(Value::String(s)) => req.body(s),
            Some(other) => req.json(&other),
        };
        let res = req.send().await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let headers: Map<String, Value> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), json!(v.to_str().unwrap_or(""))))
            .collect();
        let text = res.text().await.map_err(|e| e.to_string())?;
        let body = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        Ok(json!({"status": status, "ok": (200..300).contains(&status), "headers": headers, "body": body}))
    }
}
