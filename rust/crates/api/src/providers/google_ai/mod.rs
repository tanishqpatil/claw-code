
use std::collections::{HashMap, HashSet};

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::stream::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::ApiError;
use crate::http_client::build_http_client_or_default;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, MessageRequest, MessageResponse, MessageDelta, MessageDeltaEvent, MessageStartEvent, MessageStopEvent,
    OutputContentBlock, StreamEvent, ToolResultContentBlock, Usage,
};

use super::preflight_message_request;

pub const DEFAULT_GEMINI_BASE_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCreds {
    pub access_token: String,
    pub scope: String,
    pub token_type: String,
    pub id_token: String,
    #[serde(default)]
    pub expiry_date: u64,
    pub refresh_token: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub expiry: Option<String>,
}

#[derive(Debug, Clone)]
struct ToolCallMeta {
    name: String,
    thought_signature: Option<String>,
}

#[derive(Clone)]
pub struct GoogleAiClient {
    client: Client,
    creds_path: PathBuf,
    creds: Arc<tokio::sync::Mutex<Option<OAuthCreds>>>,
    project_id: Arc<tokio::sync::Mutex<Option<String>>>,
    tool_call_cache: Arc<Mutex<HashMap<String, ToolCallMeta>>>,
}

impl std::fmt::Debug for GoogleAiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleAiClient").finish()
    }
}

pub struct MessageStream {
    stream: Pin<Box<dyn futures::stream::Stream<Item = Result<StreamEvent, ApiError>> + Send>>,
}

impl std::fmt::Debug for MessageStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageStream").finish()
    }
}

impl MessageStream {
    pub fn request_id(&self) -> Option<&str> {
        None
    }
    
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        self.stream.next().await.transpose()
    }
}

fn sanitize_schema(schema: &mut Value) {
    if let Some(obj) = schema.as_object_mut() {
        if let Some(type_val) = obj.get("type") {
            if type_val.is_array() {
                if let Some(first_type) = type_val.as_array().and_then(|a| a.first()) {
                    obj.insert("type".to_string(), first_type.clone());
                } else {
                    obj.remove("type");
                }
            }
        }

        if let Some(properties) = obj.get_mut("properties").and_then(Value::as_object_mut) {
            for (_, prop_schema) in properties {
                sanitize_schema(prop_schema);
            }
        }

        if let Some(items) = obj.get_mut("items") {
            sanitize_schema(items);
        }

        obj.remove("additionalProperties");
        obj.remove("examples");
        obj.remove("default");
    }
}





impl GoogleAiClient {
    pub fn new() -> Result<Self, ApiError> {
        let creds_path = std::env::var("GOOGLE_OAUTH_CREDS_FILE").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            format!("{}/.gemini/oauth_creds.json", home)
        });

        Ok(Self {
            client: build_http_client_or_default(),
            creds_path: PathBuf::from(creds_path),
            creds: Arc::new(tokio::sync::Mutex::new(None)),
            project_id: Arc::new(tokio::sync::Mutex::new(None)),
            tool_call_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn get_valid_token(&self) -> Result<String, ApiError> {
        let mut creds_guard = self.creds.lock().await;
        
        if creds_guard.is_none() {
            let data = fs::read_to_string(&self.creds_path).map_err(|e| {
                ApiError::Auth(format!("Failed to read gemini credentials: {}", e))
            })?;
            let creds: OAuthCreds = serde_json::from_str(&data).map_err(|e| {
                ApiError::Auth(format!("Failed to parse gemini credentials: {}", e))
            })?;
            *creds_guard = Some(creds);
        }

        let creds = creds_guard.as_mut().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut is_expired = now >= creds.expiry_date.saturating_sub(60000);
        
        if !is_expired {
            if let Some(expiry_str) = &creds.expiry {
                if expiry_str.starts_with("2020") || expiry_str.starts_with("201") {
                    is_expired = true;
                }
            }
        }

        if is_expired {
            let client_id = creds.client_id.as_deref().unwrap_or("***REMOVED***");
            let client_secret = creds.client_secret.as_deref().unwrap_or("***REMOVED***");
            
            let params = [
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("grant_type", "refresh_token"),
                ("refresh_token", &creds.refresh_token),
            ];

            let resp = self.client
                .post("https://oauth2.googleapis.com/token")
                .form(&params)
                .send()
                .await
                .map_err(|e| ApiError::Auth(format!("Failed to refresh token: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(ApiError::Auth(format!("Token refresh rejected by provider ({}): {}", status, body)));
            }

            let new_data: Value = resp.json().await.map_err(|_| {
                ApiError::Auth("Failed to parse token refresh response".to_string())
            })?;

            if let Some(token) = new_data.get("access_token").and_then(Value::as_str) {
                creds.access_token = token.to_string();
                if let Some(exp_in) = new_data.get("expires_in").and_then(Value::as_u64) {
                    creds.expiry_date = now + (exp_in * 1000);
                    creds.expiry = None;
                }
                
                if let Some(new_refresh) = new_data.get("refresh_token").and_then(Value::as_str) {
                    creds.refresh_token = new_refresh.to_string();
                }

                let updated_json = serde_json::to_string_pretty(&creds).unwrap();
                fs::write(&self.creds_path, updated_json).unwrap_or_else(|e| {
                    eprintln!("Warning: failed to save refreshed gemini token: {}", e);
                });
            } else {
                return Err(ApiError::Auth("Refresh response missing access_token".to_string()));
            }
        }

        Ok(creds.access_token.clone())
    }

    async fn get_project_id(&self, token: &str) -> Result<String, ApiError> {
      let mut guard = self.project_id.lock().await;

      // 1. Cache
      if let Some(id) = guard.as_ref() {
          return Ok(id.clone());
      }

      // 2. GOOGLE_CLOUD_PROJECT env var
      if let Ok(id) = std::env::var("GOOGLE_CLOUD_PROJECT") {
          if !id.is_empty() {
              *guard = Some(id.clone());
              return Ok(id);
          }
      }

      // 3. Call loadCodeAssist to get the project ID.
      // The API resolves the correct project ID based on the user's OAuth token.
      let mut headers = reqwest::header::HeaderMap::new();
      headers.insert(
          reqwest::header::AUTHORIZATION,
          reqwest::header::HeaderValue::from_str(
              &format!("Bearer {}", token)
          ).map_err(|_| ApiError::Auth("Invalid token".to_string()))?
      );
      headers.insert(
          reqwest::header::CONTENT_TYPE,
          reqwest::header::HeaderValue::from_static("application/json")
      );
      
      let payload = serde_json::json!({
          "metadata": {
              "ideType": "IDE_UNSPECIFIED",
              "pluginType": "GEMINI"
          }
      });

      let resp = self.client
          .post("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist")
          .headers(headers)
          .json(&payload)
          .send()
          .await
          .map_err(ApiError::from)?;

      let status = resp.status();
      let body = resp.text().await
          .map_err(ApiError::from)?;


      if !status.is_success() {
          return Err(ApiError::Auth(format!(
              "loadCodeAssist HTTP {} — {}",
              status,
              &body[..body.len().min(300)]
          )));
      }

      let data: serde_json::Value = serde_json::from_str(&body)
          .map_err(|e| ApiError::json_deserialize(
              "Google", "loadCodeAssist", &body, e
          ))?;

      if let Some(id) = data
          .get("cloudaicompanionProject")
          .and_then(|v| v.as_str())
          .filter(|s| !s.is_empty())
      {
          *guard = Some(id.to_string());
          return Ok(id.to_string());
      }

      Err(ApiError::Auth(
          "Could not resolve project_id. \
           Ensure your account has Gemini Code Assist access \
           and run claw from a directory listed in \
           ~/.gemini/projects.json.".to_string()
      ))
    }
    fn translate_request(&self, request: &MessageRequest, project_id: &str) -> Result<Value, ApiError> {
        let mut contents = Vec::new();
        
        for msg in &request.messages {
            let role = if msg.role == "user" { "user" } else { "model" };
            let mut parts = Vec::new();

            for block in &msg.content {
                match block {
                    InputContentBlock::Text { text } => {
                        parts.push(json!({ "text": text }));
                    }
                    InputContentBlock::ToolUse { id, name, input } => {
                        let mut part = json!({
                            "functionCall": {
                                "name": name,
                                "args": input
                            }
                        });
                        
                        // Parse signature and name from ID if present
                        if id.contains("__@__") {
                            let pieces: Vec<&str> = id.split("__@__").collect();
                            if pieces.len() >= 3 && !pieces[2].is_empty() {
                                part["thoughtSignature"] = json!(pieces[2]);
                            }
                        } else if let Some(meta) = self.tool_call_cache.lock().unwrap().get(id) {
                            // Fallback to cache for legacy turns
                            if let Some(sig) = &meta.thought_signature {
                                part["thoughtSignature"] = json!(sig);
                            }
                        }

                        parts.push(part);
                    }
                    InputContentBlock::ToolResult { tool_use_id, content, is_error: _ } => {
                        let response_val = match content.first() {
                            Some(ToolResultContentBlock::Json { value }) => value.clone(),
                            Some(ToolResultContentBlock::Text { text }) => json!({ "output": text }),
                            None => json!({ "output": "success" }),
                        };
                        
                        let tool_name = {
                            if tool_use_id.contains("__@__") {
                                let pieces: Vec<&str> = tool_use_id.split("__@__").collect();
                                pieces[1].to_string()
                            } else {
                                let cache = self.tool_call_cache.lock().unwrap();
                                cache.get(tool_use_id).map(|m| m.name.clone()).unwrap_or_else(|| "unknown_tool".to_string())
                            }
                        };

                        parts.push(json!({
                            "functionResponse": {
                                "name": tool_name,
                                "response": response_val
                            }
                        }));
                    }
                }
            }
            contents.push(json!({ "role": role, "parts": parts }));
        }

        let mut inner_request = json!({ "contents": contents });
        
        if let Some(sys) = &request.system {
            inner_request["systemInstruction"] = json!({
                "parts": [{ "text": sys }]
            });
        }

        if let Some(tools) = &request.tools {
            let declarations: Vec<Value> = tools.iter().map(|t| {
                let mut parameters = t.input_schema.clone();
                sanitize_schema(&mut parameters);
                json!({
                    "name": t.name,
                    "description": t.description.clone().unwrap_or_default(),
                    "parameters": parameters
                })
            }).collect();
            
            inner_request["tools"] = json!([{ "function_declarations": declarations }]);
        }

        let payload = json!({
            "model": request.model,
            "project": project_id,
            "user_prompt_id": Uuid::new_v4().to_string(),
            "request": inner_request
        });

        Ok(payload)
    }

    fn translate_response_chunk(&self, chunk: Value, started_blocks: &mut HashSet<u32>) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        
        let response = if let Some(r) = chunk.get("response") {
            r
        } else {
            &chunk
        };

        if let Some(candidates) = response.get("candidates").and_then(Value::as_array) {
            for candidate in candidates {
                if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array) {
                    for (i, part) in parts.iter().enumerate() {
                        let idx = i as u32;
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                if !started_blocks.contains(&idx) {
                                    events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                                        index: idx,
                                        content_block: OutputContentBlock::Text { text: String::new() },
                                    }));
                                    started_blocks.insert(idx);
                                }
                                events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                                    index: idx,
                                    delta: ContentBlockDelta::TextDelta { text: text.to_string() }
                                }));
                            }
                        } else if let Some(func_call) = part.get("functionCall") {
                            let name = func_call.get("name").and_then(Value::as_str).unwrap_or("unknown");
                            let args = func_call.get("args").unwrap_or(&json!({})).clone();
                            // thoughtSignature (CamelCase) is at the part level in the API response
                            let thought_signature = part.get("thoughtSignature").and_then(Value::as_str).map(String::from);
                            let base_id = format!("call_{}_{}", candidate.get("index").and_then(Value::as_i64).unwrap_or(0), i);
                            let sig_str = thought_signature.clone().unwrap_or_default();
                            let id = format!("{}__@__{}__@__{}", base_id, name, sig_str);
                            
                            {
                                let mut cache = self.tool_call_cache.lock().unwrap();
                                cache.insert(id.clone(), ToolCallMeta {
                                    name: name.to_string(),
                                    thought_signature,
                                });
                            }

                            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                                index: idx,
                                content_block: OutputContentBlock::ToolUse {
                                    id,
                                    name: name.to_string(),
                                    input: json!({}),
                                }
                            }));
                            
                            events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                                index: idx,
                                delta: ContentBlockDelta::InputJsonDelta { partial_json: args.to_string() }
                            }));
                            
                            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: idx }));
                        }
                    }
                }
                
                if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                    if reason == "STOP" {
                        for idx in started_blocks.clone() {
                            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent { index: idx }));
                        }
                        started_blocks.clear();
                    }
                }
            }
        }
        
        if let Some(usage) = response.get("usageMetadata") {
            let input_tokens = usage.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            let output_tokens = usage.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            let cache_read = usage.get("cachedContentTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32;
            
            if input_tokens > 0 || output_tokens > 0 {
                events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: None,
                        stop_sequence: None,
                    },
                    usage: Usage {
                        input_tokens,
                        output_tokens,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: cache_read,
                    },
                }));
            }
        }

        events
    }

    pub async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        preflight_message_request(request)?;
        
        let token = self.get_valid_token().await?;
        let project_id = self.get_project_id(&token).await?;
        let payload = self.translate_request(request, &project_id)?;
        
        let url = format!("{}:generateContent", DEFAULT_GEMINI_BASE_URL);
        
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("User-Agent", HeaderValue::from_static("GeminiCLI/0.38.1/gemini-3.1-pro-preview (linux; x64; terminal) google-api-nodejs-client/9.15.1"));
        headers.insert("x-goog-authuser", HeaderValue::from_static("0"));
        headers.insert("x-goog-api-client", HeaderValue::from_static("gl-node/22.11.0 auth/9.15.1 gdcl/7.2.0"));
        let mut attempt = 1;
        let response = loop {
            let response_result = self
                .client
                .post(&url)
                .headers(headers.clone())
                .json(&payload)
                .send()
                .await;

            let response = match response_result {
                Ok(resp) => resp,
                Err(e) => return Err(ApiError::from(e)),
            };

            if response.status().is_success() {
                break response;
            }

            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();

            if status == 429 && attempt < 3 {
                let delay_secs =
                    if let Ok(err_json) = serde_json::from_str::<Value>(&err_text) {
                        err_json
                            .pointer("/error/details/0/retryDelay")
                            .and_then(Value::as_str)
                            .and_then(|s| s.trim_end_matches('s').parse::<f64>().ok())
                            .unwrap_or(5.0) // Default delay
                    } else {
                        5.0
                    };

                if delay_secs > 300.0 {
                    return Err(ApiError::Api {
                        status,
                        error_type: Some("gemini_quota_exhausted".to_string()),
                        message: Some("Quota fully exhausted, retry later.".to_string()),
                        request_id: None,
                        body: err_text,
                        retryable: false,
                    });
                }

                eprintln!(
                    "[CLAW RETRY] 429 on model {}, waiting {}s (attempt {}/3)",
                    request.model, delay_secs, attempt
                );
                tokio::time::sleep(tokio::time::Duration::from_secs_f64(delay_secs)).await;
                attempt += 1;
                continue;
            }

            return Err(ApiError::Api {
                status,
                error_type: Some("gemini_error".to_string()),
                message: None,
                request_id: None,
                body: err_text,
                retryable: false,
            });
        };

        let body = response.text().await.map_err(ApiError::from)?;
        let data: Value = serde_json::from_str(&body).map_err(|e| {
            ApiError::json_deserialize("Google", &request.model, &body, e)
        })?;
        
        let resp_json = data.get("response").unwrap_or(&data);
        
        let mut output_blocks = Vec::new();
        if let Some(candidates) = resp_json.get("candidates").and_then(Value::as_array) {
            if let Some(first_candidate) = candidates.first() {
                if let Some(parts) = first_candidate.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array) {
                    for (i, part) in parts.iter().enumerate() {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            output_blocks.push(OutputContentBlock::Text { text: text.to_string() });
                        } else if let Some(func_call) = part.get("functionCall") {
                            let name = func_call.get("name").and_then(Value::as_str).unwrap_or("unknown");
                            let args = func_call.get("args").unwrap_or(&json!({})).clone();
                            let thought_signature = part.get("thoughtSignature").and_then(Value::as_str).map(String::from);
                            let id = format!("call_{}_{}", first_candidate.get("index").and_then(Value::as_i64).unwrap_or(0), i);
                            
                            {
                                let mut cache = self.tool_call_cache.lock().unwrap();
                                cache.insert(id.clone(), ToolCallMeta {
                                    name: name.to_string(),
                                    thought_signature,
                                });
                            }

                            output_blocks.push(OutputContentBlock::ToolUse {
                                id,
                                name: name.to_string(),
                                input: args,
                            });
                        }
                    }
                }
            }
        }

        let usage = resp_json.get("usageMetadata").map(|u| {
            Usage {
                input_tokens: u.get("promptTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32,
                output_tokens: u.get("candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: u.get("cachedContentTokenCount").and_then(Value::as_u64).unwrap_or(0) as u32,
            }
        }).unwrap_or_default();

        Ok(MessageResponse {
            id: "gemini_msg".to_string(),
            kind: "message".to_string(),
            role: "assistant".to_string(),
            content: output_blocks,
            model: request.model.clone(),
            stop_reason: None,
            stop_sequence: None,
            usage,
            request_id: None,
        })
    }

    pub async fn stream_message(&self, request: &MessageRequest) -> Result<MessageStream, ApiError> {
        preflight_message_request(request)?;
        
        let token = self.get_valid_token().await?;
        let project_id = self.get_project_id(&token).await?;
        let payload = self.translate_request(request, &project_id)?;
        
        let url = format!("{}:streamGenerateContent?alt=sse", DEFAULT_GEMINI_BASE_URL);
        
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("User-Agent", HeaderValue::from_static("GeminiCLI/0.38.1/gemini-3.1-pro-preview (linux; x64; terminal) google-api-nodejs-client/9.15.1"));
        headers.insert("x-goog-authuser", HeaderValue::from_static("0"));
        headers.insert("x-goog-api-client", HeaderValue::from_static("gl-node/22.11.0 auth/9.15.1 gdcl/7.2.0"));

        let mut attempt = 1;
        let response = loop {
            let response_result = self
                .client
                .post(&url)
                .headers(headers.clone())
                .json(&payload)
                .send()
                .await;

            let response = match response_result {
                Ok(resp) => resp,
                Err(e) => return Err(ApiError::from(e)),
            };

            if response.status().is_success() {
                break response;
            }

            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();

            if status == 429 && attempt < 3 {
                let delay_secs =
                    if let Ok(err_json) = serde_json::from_str::<Value>(&err_text) {
                        err_json
                            .pointer("/error/details/0/retryDelay")
                            .and_then(Value::as_str)
                            .and_then(|s| s.trim_end_matches('s').parse::<f64>().ok())
                            .unwrap_or(5.0) // Default delay
                    } else {
                        5.0
                    };

                if delay_secs > 300.0 {
                    return Err(ApiError::Api {
                        status,
                        error_type: Some("gemini_quota_exhausted".to_string()),
                        message: Some("Quota fully exhausted, retry later.".to_string()),
                        request_id: None,
                        body: err_text,
                        retryable: false,
                    });
                }

                eprintln!(
                    "[CLAW RETRY] 429 on model {}, waiting {}s (attempt {}/3)",
                    request.model, delay_secs, attempt
                );
                tokio::time::sleep(tokio::time::Duration::from_secs_f64(delay_secs)).await;
                attempt += 1;
                continue;
            }

            return Err(ApiError::Api {
                status,
                error_type: Some("gemini_error".to_string()),
                message: None,
                request_id: None,
                body: err_text,
                retryable: false,
            });
        };

        let client = self.clone();
        let mut response = response;
        let model = request.model.clone();

        let stream = async_stream::stream! {
            yield Ok(StreamEvent::MessageStart(MessageStartEvent {
                message: MessageResponse {
                    id: "gemini_msg".to_string(),
                    kind: "message".to_string(),
                    role: "assistant".to_string(),
                    content: vec![],
                    model,
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Usage::default(),
                    request_id: None,
                }
            }));

            let mut started_blocks = HashSet::new();
            let mut line_buffer = String::new();

            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        let text = String::from_utf8_lossy(&chunk);
                        line_buffer.push_str(&text);
                        
                        while let Some(pos) = line_buffer.find('\n') {
                            let line = line_buffer[..pos].trim().to_string();
                            line_buffer = line_buffer[pos + 1..].to_string();
                            
                            if line.starts_with("data: ") {
                                let data_str = &line[6..];
                                if data_str == "[DONE]" { continue; }
                                match serde_json::from_str::<Value>(data_str) {
                                    Ok(json_chunk) => {
                                        let events = client.translate_response_chunk(json_chunk, &mut started_blocks);
                                        for event in events {
                                            yield Ok(event);
                                        }
                                    }
                                    Err(_e) => {
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        yield Err(ApiError::from(e));
                        break;
                    }
                }
            }
            
            // Process any remaining data in the buffer
            let line = line_buffer.trim().to_string();
            if line.starts_with("data: ") {
                let data_str = &line[6..];
                if data_str != "[DONE]" {
                    if let Ok(json_chunk) = serde_json::from_str::<Value>(data_str) {
                        let events = client.translate_response_chunk(json_chunk, &mut started_blocks);
                        for event in events {
                            yield Ok(event);
                        }
                    }
                }
            }
            
            yield Ok(StreamEvent::MessageStop(MessageStopEvent {}));
        };

        Ok(MessageStream {
            stream: Box::pin(stream)
        })
    }
}

#[cfg(test)]
mod google_ai_tests;
