use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::agent::WorkerEvent;
use crate::api::{ChatCompletionsStreamState, ModelTurnRequest, ModelTurnResponse, ModelTurnTransport};
use crate::auth::CodexAuth;

#[derive(Debug, Clone)]
pub struct LocalChatCompletionsClient {
    client: reqwest::blocking::Client,
    base_url: String,
    api_key: String,
}

impl LocalChatCompletionsClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self> {
        let client = reqwest::blocking::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
        })
    }
}

impl ModelTurnTransport for LocalChatCompletionsClient {
    fn execute_turn(
        &self,
        _auth: &CodexAuth,
        request: &ModelTurnRequest,
        session_id: String,
        turn_id: String,
        tx: &Sender<WorkerEvent>,
    ) -> Result<ModelTurnResponse> {
        let response = self
            .client
            .post(&self.base_url)
            .bearer_auth(&self.api_key)
            .header("Accept", "text/event-stream")
            .json(&request.body)
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            let truncated: String = body.chars().take(500).collect();
            return Err(anyhow!("http {status}: {truncated}"));
        }

        let reader = BufReader::new(response);
        let mut data = String::new();
        let mut stream_state = ChatCompletionsStreamState::default();
        for line in reader.lines() {
            let line = line?;
            if line.is_empty() {
                if !data.is_empty() {
                    let done = stream_state.ingest_event(
                        &data,
                        &session_id,
                        &turn_id,
                        request.emit_text_events,
                        tx,
                    )?;
                    data.clear();
                    if done {
                        break;
                    }
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                if !data.is_empty() {
                    if rest.trim().starts_with('{') || rest.trim() == "[DONE]" {
                        let done = stream_state.ingest_event(
                            &data,
                            &session_id,
                            &turn_id,
                            request.emit_text_events,
                            tx,
                        )?;
                        data.clear();
                        if done {
                            break;
                        }
                    } else {
                        data.push('\n');
                    }
                }
                data.push_str(rest);
            }
        }

        if !data.is_empty() {
            stream_state
                .ingest_event(
                    &data,
                    &session_id,
                    &turn_id,
                    request.emit_text_events,
                    tx,
                )
                .ok();
        }

        Ok(stream_state.finalize())
    }
}
