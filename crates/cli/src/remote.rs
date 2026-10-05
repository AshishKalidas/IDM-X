//! Talking to an IDMX that is already running (the desktop app or `idmx daemon`).

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use idmx_core::types::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Remote {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Remote {
    pub fn connect(data_dir: &std::path::Path) -> Option<Remote> {
        let info = idmx_server::runtime_info(data_dir)?;
        Some(Remote { base: format!("http://127.0.0.1:{}", info.port), token: info.token, http: reqwest::Client::new() })
    }

    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn base(&self) -> &str {
        &self.base
    }

    async fn send<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let resp = rb.bearer_auth(&self.token).send().await.context("couldn't reach IDMX")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            bail!("{}", body["error"].as_str().unwrap_or(status.canonical_reason().unwrap_or("request failed")));
        }
        serde_json::from_value(body).map_err(|e| anyhow!("unexpected reply: {e}"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.http.get(format!("{}{}", self.base, path))).await
    }
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl serde::Serialize) -> Result<T> {
        self.send(self.http.post(format!("{}{}", self.base, path)).json(body)).await
    }
    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.http.delete(format!("{}{}", self.base, path))).await
    }

    pub async fn jobs(&self) -> Result<Vec<Job>> {
        let s: Value = self.get("/api/state").await?;
        Ok(serde_json::from_value(s["jobs"].clone())?)
    }

    /// Live events, parsed from the server-sent-event stream.
    pub async fn events(&self) -> Result<mpsc::Receiver<Event>> {
        let resp = self.http.get(format!("{}/api/events", self.base)).bearer_auth(&self.token).send().await?;
        if !resp.status().is_success() {
            bail!("event stream refused ({})", resp.status());
        }
        let (tx, rx) = mpsc::channel(512);
        tokio::spawn(async move {
            let mut buf = String::new();
            let mut s = resp.bytes_stream();
            while let Some(Ok(chunk)) = s.next().await {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(i) = buf.find("\n\n") {
                    let block: String = buf.drain(..i + 2).collect();
                    let is_special = block.lines().any(|l| l.starts_with("event:"));
                    if is_special {
                        continue; // snapshot / resync: callers fetch state themselves
                    }
                    let data: String = block.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect::<Vec<_>>().join("\n");
                    if let Ok(ev) = serde_json::from_str::<Event>(&data) {
                        if tx.send(ev).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        Ok(rx)
    }
}
