//! OpenAI-compatible provider transport, shared by NVIDIA/OpenRouter/custom services.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

pub fn endpoint(base: &str, resource: &str) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse(base.trim()).map_err(|_| "Enter a complete provider base URL")?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(
            "Providers require HTTPS; HTTP is allowed only for a local model server".into(),
        );
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("The base URL cannot contain credentials, a query or a fragment".into());
    }
    let mut path = url.path().trim_end_matches('/').to_owned();
    if path.is_empty() {
        path = match url.host_str() {
            Some("openrouter.ai") => "/api/v1",
            Some("api.groq.com") => "/openai/v1",
            _ => "/v1",
        }
        .into();
    }
    if let Some(prefix) = path.strip_suffix("/chat/completions") {
        path = prefix.to_owned();
    }
    if let Some(prefix) = path.strip_suffix("/models") {
        path = prefix.to_owned();
    }
    let path = path.trim_end_matches('/');
    url.set_path(&format!("{path}/{resource}"));
    Ok(url)
}
fn client(url: &reqwest::Url) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();
    if matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    ) {
        builder = builder.no_proxy();
    }
    builder
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "Could not initialize the provider connection".into())
}
pub async fn models(
    base: &str,
    key: Zeroizing<String>,
) -> Result<Vec<crate::ai_models::Model>, String> {
    let url = endpoint(base, "models")?;
    let mut request = client(&url)?.get(url);
    if !key.is_empty() {
        request = request.bearer_auth(key.as_str());
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| "Could not contact the provider")?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The provider interrupted its model list")?
    {
        if data.len() + chunk.len() > 8 * 1024 * 1024 {
            return Err("Provider model list exceeds 8 MiB".into());
        }
        data.extend_from_slice(&chunk);
    }
    let data: Value =
        serde_json::from_slice(&data).map_err(|_| "The provider returned an invalid model list")?;
    let mut models = data["data"]
        .as_array()
        .ok_or("Provider response has no model list")?
        .iter()
        .filter_map(crate::ai_models::Model::from_provider)
        .collect::<Vec<_>>();
    models.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}
fn status_message(status: u16) -> String {
    match status {
        401|403=>"The provider rejected this API key or does not grant access to this model".into(),
        404=>"The provider did not find this exact model or API URL. Refresh models and check the base URL".into(),
        402=>"The service rejected this request with HTTP 402 (payment or account budget). Check its API credits and key spending limit, even for models labelled free. You can also select another saved service.".into(),
        429=>"The provider's rate or quota limit was reached".into(),
        500..=599=>"The provider is temporarily unavailable".into(),
        300..=399=>"The provider redirected the request. Enter its final API URL; API keys are never forwarded to redirects".into(),
        _=>format!("Provider request failed (HTTP {status}). Check model capabilities and request limits"),
    }
}
// Classify a bounded error document, rather than discarding the provider's reason
// or exposing raw response bodies (which may echo keys, prompts or tool data).
fn provider_error(status: u16, value: &Value) -> String {
    let error = &value["error"];
    let message = error["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let source = error["metadata"]["limit_source"]
        .as_str()
        .unwrap_or_default();
    if status == 402 {
        if source == "openrouter_in_flight_budget" {
            return "OpenRouter's simultaneous-request spending budget is reserved (HTTP 402). Wait for other requests to finish, then try again.".into();
        }
        if message.contains("max_tokens")
            || message.contains("max_completion_tokens")
            || message.contains("fewer tokens")
        {
            return "The service cannot cover the requested reply limit (HTTP 402). Lower Output limit in AI settings, shorten the conversation, or check the service's API credits and key spending limit. A model labelled free may still have service limits.".into();
        }
    }
    if status == 403
        && matches!(
            source,
            "openrouter_key_limit" | "openrouter_workspace_budget" | "openrouter_guardrail_budget"
        )
    {
        return "The service's API-key or workspace spending budget was reached (HTTP 403). Check its budget limits or wait for the budget window to reset.".into();
    }
    status_message(status)
}
async fn response_error(mut response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    let host = response.url().host_str().unwrap_or("service").to_owned();
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if bytes.len() + chunk.len() > 16 * 1024 {
            return format!("{host}: {}", status_message(status));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    format!("{host}: {}", provider_error(status, &value))
}
pub enum StreamEvent {
    Text(String),
}
#[derive(Default)]
struct Accumulator {
    text: String,
    calls: BTreeMap<u64, Value>,
    bytes: Vec<u8>,
    size: usize,
    finished: bool,
}
impl Accumulator {
    fn chunk(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        self.size += chunk.len();
        if self.size > 4 * 1024 * 1024 {
            return Err("Provider response exceeded 4 MiB".into());
        }
        self.bytes.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(end) = self.bytes.iter().position(|b| *b == b'\n') {
            let line = self.bytes.drain(..=end).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|_| "Invalid UTF-8 in provider stream")?
                .trim();
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data == "[DONE]" {
                self.finished = true;
                continue;
            }
            if data.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(data).map_err(|_| "Invalid JSON in provider stream")?;
            if value.get("error").is_some() {
                if let Some(status) = value["error"]["code"]
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                {
                    return Err(provider_error(status, &value));
                }
                return Err("The provider reported an error while streaming this response".into());
            }
            let Some(choice) = value["choices"].as_array().and_then(|v| v.first()) else {
                continue;
            };
            if !choice["finish_reason"].is_null() {
                self.finished = true;
            }
            let delta = &choice["delta"];
            if let Some(text) = delta["content"].as_str() {
                self.text.push_str(text);
                output.push(text.to_owned());
            }
            if let Some(calls) = delta["tool_calls"].as_array() {
                for call in calls {
                    let index = call["index"].as_u64().ok_or("Tool call index missing")?;
                    if index > 63 {
                        return Err("Too many simultaneous tool calls".into());
                    }
                    let entry = self.calls.entry(index).or_insert_with(
                        || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                    );
                    for (parent, key) in
                        [("", "id"), ("function", "name"), ("function", "arguments")]
                    {
                        let source = if parent.is_empty() {
                            &call[key]
                        } else {
                            &call[parent][key]
                        };
                        if let Some(part) = source.as_str() {
                            let target = if parent.is_empty() {
                                &mut entry[key]
                            } else {
                                &mut entry[parent][key]
                            };
                            *target = Value::String(format!(
                                "{}{part}",
                                target.as_str().unwrap_or_default()
                            ));
                        }
                    }
                }
            }
        }
        Ok(output)
    }
}
pub async fn complete(
    base: &str,
    key: Zeroizing<String>,
    body: Value,
    events: mpsc::Sender<StreamEvent>,
) -> Result<Value, String> {
    if serde_json::to_vec(&body).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
        return Err("Conversation exceeds the request size limit. Start a new chat".into());
    }
    let url = endpoint(base, "chat/completions")?;
    let client = client(&url)?;
    let start = Instant::now();
    let mut attempt = 0;
    let mut response = loop {
        let mut request = client.post(url.clone()).json(&body);
        if !key.is_empty() {
            request = request.bearer_auth(key.as_str());
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => break response,
            Ok(response) => {
                let status = response.status().as_u16();
                if !matches!(status, 429 | 500 | 502 | 503 | 504) || attempt >= 3 {
                    return Err(response_error(response).await);
                }
                let delay = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .min(30);
                if start.elapsed() + Duration::from_secs(delay) > Duration::from_secs(60) {
                    return Err(response_error(response).await);
                }
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
            Err(_) if attempt < 3 && start.elapsed() < Duration::from_secs(50) => {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await
            }
            Err(_) => {
                return Err(
                    "Could not contact the provider. Check the API URL and connection".into(),
                )
            }
        }
        attempt += 1;
    };
    // Retry only before any response is delivered: never replay partial output or tools.
    let mut state = Accumulator::default();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "The provider interrupted its response; it has not been replayed")?
    {
        for text in state.chunk(&chunk)? {
            events
                .send(StreamEvent::Text(text))
                .await
                .map_err(|_| "Chat closed")?;
        }
    }
    if !state.bytes.is_empty() {
        for text in state.chunk(b"\n")? {
            events
                .send(StreamEvent::Text(text))
                .await
                .map_err(|_| "Chat closed")?;
        }
    }
    if !state.finished {
        return Err("The provider stream ended without a completion marker".into());
    }
    let calls: Vec<Value> = state.calls.into_values().collect();
    if calls.iter().any(|c| {
        c["id"].as_str().unwrap_or("").is_empty()
            || c["function"]["name"].as_str().unwrap_or("").is_empty()
    }) {
        return Err("The provider returned an incomplete tool call".into());
    }
    let mut message = json!({"role":"assistant","content":state.text});
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    Ok(message)
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn fixture(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut request = Vec::new();
                let mut header_end = None;
                loop {
                    let mut buffer = [0; 4096];
                    let size =
                        tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
                            .await
                            .unwrap()
                            .unwrap();
                    if size == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..size]);
                    assert!(request.len() < 8 * 1024 * 1024);
                    if header_end.is_none() {
                        header_end = request
                            .windows(4)
                            .position(|w| w == b"\r\n\r\n")
                            .map(|i| i + 4);
                    }
                    if let Some(end) = header_end {
                        let header = String::from_utf8_lossy(&request[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                // Split transport writes exercise the real HTTP/SSE consumer.
                for bytes in response.as_bytes().chunks(11) {
                    if socket.write_all(bytes).await.is_err() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            }
            requests
        });
        (format!("http://{address}/custom/v1"), task)
    }
    fn response(status: &str, headers: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
    }
    const ANSWER:&str="data: {\"choices\":[{\"delta\":{\"content\":\"héllo\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    #[tokio::test]
    async fn payment_errors_explain_limits_without_echoing_secrets_or_replaying_requests() {
        for (document, expected) in [
            (
                json!({"error":{"code":402,"message":"Insufficient credits fixture-secret"}}),
                "API credits",
            ),
            (
                json!({"error":{"code":402,"message":"This request requires more credits, or fewer max_tokens fixture-secret"}}),
                "Lower Output limit",
            ),
            (
                json!({"error":{"code":402,"metadata":{"limit_source":"openrouter_in_flight_budget","raw":"fixture-secret"}}}),
                "simultaneous-request",
            ),
        ] {
            let (url, server) = fixture(vec![response(
                "402 Payment Required",
                "Content-Type: application/json\r\n",
                &document.to_string(),
            )])
            .await;
            let (sender, mut events) = mpsc::channel(16);
            let error = complete(&url, Zeroizing::new("fixture-secret".into()),
                json!({"model":"custom/free","max_tokens":512,"messages":[{"role":"user","content":"fixture-prompt"}]}), sender).await.unwrap_err();
            assert!(error.contains(expected) && error.contains("402"), "{error}");
            assert!(!error.contains("fixture-secret") && !error.contains("fixture-prompt"));
            assert!(events.recv().await.is_none());
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].contains("\"max_tokens\":512"));
            assert!(requests[0].contains("\"model\":\"custom/free\""));
        }
        let mut stream = Accumulator::default();
        let error = stream.chunk(b"data: {\"error\":{\"code\":402,\"message\":\"fewer max_tokens fixture-secret\"}}\n").unwrap_err();
        assert!(error.contains("Lower Output limit") && !error.contains("fixture-secret"));
    }
    #[tokio::test]
    async fn real_http_preserves_custom_paths_keys_paid_models_and_streamed_unicode() {
        let (url,server)=fixture(vec![response("200 OK","Content-Type: application/json\r\n","{\"data\":[{\"id\":\"vendor/paid\",\"pricing\":{\"prompt\":\"1\"}},{\"id\":\"vendor/free\"}]}"),response("200 OK","Content-Type: text/event-stream\r\n",ANSWER)]).await;
        let catalog = models(&url, Zeroizing::new("fixture-key".into()))
            .await
            .unwrap();
        assert_eq!(catalog.len(), 2);
        let (sender, mut events) = mpsc::channel(16);
        let message = complete(
            &url,
            Zeroizing::new("fixture-key".into()),
            json!({"model":"vendor/paid","messages":[{"role":"user","content":"hello"}]}),
            sender,
        )
        .await
        .unwrap();
        assert_eq!(message["content"], "héllo");
        let mut text = String::new();
        while let Some(StreamEvent::Text(part)) = events.recv().await {
            text.push_str(&part);
        }
        assert_eq!(text, "héllo");
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /custom/v1/models "));
        assert!(requests[1].starts_with("POST /custom/v1/chat/completions "));
        assert!(requests[1]
            .to_lowercase()
            .contains("authorization: bearer fixture-key"));
        assert!(requests[1].contains("vendor/paid"));
    }
    #[tokio::test]
    async fn transient_http_retries_before_output_but_never_replays_an_interrupted_stream() {
        let (url, server) = fixture(vec![
            response("503 Unavailable", "Retry-After: 0\r\n", ""),
            response("200 OK", "Content-Type: text/event-stream\r\n", ANSWER),
        ])
        .await;
        let (sender, _receiver) = mpsc::channel(16);
        assert!(complete(
            &url,
            Zeroizing::new(String::new()),
            json!({"model":"one"}),
            sender
        )
        .await
        .is_ok());
        assert_eq!(server.await.unwrap().len(), 2);
        let partial = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let broken=format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: text/event-stream\r\n\r\n{partial}",partial.len()+100);
        let (url, server) = fixture(vec![broken]).await;
        let (sender, mut events) = mpsc::channel(16);
        assert!(complete(
            &url,
            Zeroizing::new(String::new()),
            json!({"model":"one"}),
            sender
        )
        .await
        .is_err());
        assert_eq!(server.await.unwrap().len(), 1);
        let mut delivered = String::new();
        while let Some(StreamEvent::Text(part)) = events.recv().await {
            delivered.push_str(&part);
        }
        assert!(delivered.is_empty() || delivered == "partial");
    }
    #[tokio::test]
    async fn redirects_and_missing_models_do_not_forward_keys_or_retry_actions() {
        for (status, headers) in [
            ("302 Redirect", "Location: http://127.0.0.1:1/stolen\r\n"),
            ("404 Not Found", ""),
        ] {
            let (url, server) = fixture(vec![response(status, headers, "")]).await;
            let (sender, _) = mpsc::channel(16);
            let result = complete(
                &url,
                Zeroizing::new("fixture-key".into()),
                json!({"model":"unknown"}),
                sender,
            )
            .await;
            assert!(result.is_err());
            assert_eq!(server.await.unwrap().len(), 1);
        }
    }
    #[test]
    fn endpoints_preserve_provider_model_namespaces_and_reject_secret_redirect_urls() {
        assert_eq!(
            endpoint("https://openrouter.ai/api/v1/", "chat/completions")
                .unwrap()
                .as_str(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            endpoint(
                "https://integrate.api.nvidia.com/v1/chat/completions",
                "models"
            )
            .unwrap()
            .path(),
            "/v1/models"
        );
        assert!(endpoint("http://example.com/v1", "models").is_err());
        assert!(endpoint("https://key@example.com/v1", "models").is_err());
    }
    #[test]
    fn streaming_handles_split_utf8_null_content_and_tool_arguments() {
        let mut a = Accumulator::default();
        let data="data: {\"choices\":[{\"delta\":{\"content\":\"é\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":null,\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n";
        for b in data.as_bytes() {
            a.chunk(&[*b]).unwrap();
        }
        assert_eq!(a.text, "é");
        assert!(a.finished);
        assert_eq!(a.calls[&0]["function"]["arguments"], "{}");
    }
}
