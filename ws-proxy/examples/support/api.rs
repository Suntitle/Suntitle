//! Real API requests and bounded observation polling shared by E2E harnesses.
use super::ChildProcess;
use serde_json::Value;
use std::{error::Error, io, time::Duration};
use tokio::time::{Instant, sleep};

/// Reusable connection pool for the local test control API.
pub struct ApiClient {
    /// Every request has a timeout, including response-body reads.
    client: reqwest::Client,
}

impl ApiClient {
    /// Create one client for all requests in a harness.
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()?,
        })
    }

    /// Send JSON and verify the HTTP status before interpreting the response body.
    /// Paths MUST include the /api/v1 prefix. Empty response bodies return JSON null.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        expected: u16,
    ) -> Result<Value, Box<dyn Error>> {
        let mut request = self
            .client
            .request(method.parse()?, format!("http://127.0.0.1:3031{path}"));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let text = response.text().await?;
        if status != expected {
            return Err(
                format!("{method} {path}: expected {expected}, got {status}: {text}").into(),
            );
        }
        Ok(if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        })
    }

    /// Poll API observations, retaining the last response on timeout.
    /// Predicate errors abort the wait, allowing required child exits to fail fast.
    pub async fn wait_for(
        &self,
        path: &str,
        duration: Duration,
        mut predicate: impl FnMut(&Value) -> io::Result<bool>,
    ) -> Result<Value, Box<dyn Error>> {
        let deadline = Instant::now() + duration;
        loop {
            let result = self.request("GET", path, None, 200).await;
            if let Ok(value) = &result
                && predicate(value)?
            {
                return Ok(value.clone());
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting for {path}: {result:?}").into());
            }
            sleep(Duration::from_millis(100)).await;
        }
    }

    /// Return one host with the requested presence while required processes remain alive.
    pub async fn wait_for_host(
        &self,
        name: &str,
        presence: &str,
        duration: Duration,
        processes: &mut [&mut ChildProcess],
    ) -> Result<Value, Box<dyn Error>> {
        let matches =
            |host: &Value| host["spec"]["name"] == name && host["status"]["presence"] == presence;
        let hosts = self
            .wait_for("/api/v1/hosts", duration, |hosts| {
                for process in processes.iter_mut() {
                    process.ensure_running()?;
                }
                Ok(hosts["items"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(&matches)))
            })
            .await?;
        hosts["items"]
            .as_array()
            .and_then(|items| items.iter().find(|host| matches(host)))
            .cloned()
            .ok_or_else(|| "matching host disappeared from response".into())
    }
}
