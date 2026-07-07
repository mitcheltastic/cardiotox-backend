use reqwest_eventsource::{Event, EventSource};
use serde_json::Value;
use futures_util::stream::StreamExt;
use std::time::Duration;
use tracing::{error, warn};
use crate::error::AppError;

pub async fn call_gradio(
    client: &reqwest::Client,
    hf_base: &str,
    endpoint: &str,
    data: &Value,
) -> Result<Value, AppError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match do_call_gradio(client, hf_base, endpoint, data).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= 2 {
                    error!("Gradio call failed after {} attempts: {:?}", attempt, e);
                    return Err(AppError::BadGateway("Prediction service error".into()));
                }
                warn!("Gradio call failed (attempt {}): {:?}. Retrying...", attempt, e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn do_call_gradio(
    client: &reqwest::Client,
    hf_base: &str,
    endpoint: &str,
    data: &Value,
) -> anyhow::Result<Value> {
    let post_url = format!("{}/gradio_api/call/{}", hf_base, endpoint);
    
    // Step 1: POST to get event_id
    let res = client
        .post(&post_url)
        .json(&serde_json::json!({ "data": data }))
        .send()
        .await?;

    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        anyhow::bail!("HF Space returned {}: {}", status, text);
    }

    let init_json: Value = res.json().await?;
    let event_id = init_json
        .get("event_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("No event_id in Gradio response"))?;

    // Step 2: GET SSE
    let get_url = format!("{}/gradio_api/call/{}/{}", hf_base, endpoint, event_id);
    let mut es = EventSource::get(get_url);
    
    while let Some(event_res) = es.next().await {
        match event_res {
            Ok(Event::Open) => {}
            Ok(Event::Message(message)) => {
                if message.event == "complete" {
                    es.close();
                    let parsed: Value = serde_json::from_str(&message.data)?;
                    return Ok(parsed);
                } else if message.event == "error" {
                    es.close();
                    anyhow::bail!("Gradio SSE returned error: {}", message.data);
                }
            }
            Err(e) => {
                es.close();
                anyhow::bail!("SSE error: {}", e);
            }
        }
    }

    anyhow::bail!("SSE stream ended without 'complete' event")
}
