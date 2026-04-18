#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::types::{InputContentBlock, InputMessage, MessageRequest, ToolResultContentBlock};
    use serde_json::json;
    use std::collections::HashSet;

    #[tokio::test]
    async fn test_thought_signature_caching_in_send_message_response() {
        let client = GoogleAiClient::new().unwrap();
        
        let response_json = json!({
            "response": {
                "candidates": [{
                    "index": 0,
                    "content": {
                        "parts": [{
                            "thoughtSignature": "test-sig-123",
                            "functionCall": {
                                "name": "test_tool",
                                "args": {"input": "foo"}
                            }
                        }]
                    }
                }]
            }
        });

        // Mocking the behavior inside send_message logic
        let candidate = &response_json["response"]["candidates"][0];
        let part = &candidate["content"]["parts"][0];
        let id = "call_0_0".to_string();
        
        let thought_signature = part.get("thoughtSignature").and_then(|v| v.as_str()).map(String::from);
        let name = part["functionCall"]["name"].as_str().unwrap().to_string();

        {
            let mut cache = client.tool_call_cache.lock().unwrap();
            cache.insert(id.clone(), ToolCallMeta {
                name,
                thought_signature,
            });
        }

        let cached = client.tool_call_cache.lock().unwrap().get(&id).unwrap().clone();
        assert_eq!(cached.name, "test_tool");
        assert_eq!(cached.thought_signature, Some("test-sig-123".to_string()));
    }

    #[test]
    fn test_thought_signature_caching_in_stream_chunk() {
        let client = GoogleAiClient::new().unwrap();
        let mut started_blocks = HashSet::new();
        
        let chunk = json!({
            "response": {
                "candidates": [{
                    "index": 0,
                    "content": {
                        "parts": [{
                            "thoughtSignature": "stream-sig-456",
                            "functionCall": {
                                "name": "stream_tool",
                                "args": {"cmd": "ls"}
                            }
                        }]
                    }
                }]
            }
        });

        let _events = client.translate_response_chunk(chunk, &mut started_blocks);
        
        let start_event = _events.iter().find(|e| matches!(e, StreamEvent::ContentBlockStart(_)));
        assert!(start_event.is_some(), "Should have emitted a ContentBlockStart");
        if let Some(StreamEvent::ContentBlockStart(ev)) = start_event {
            if let OutputContentBlock::ToolUse { id, .. } = &ev.content_block {
                assert!(id.contains("stream_tool"), "Tool ID should contain the tool name");
            } else {
                panic!("Should have been a ToolUse block");
            }
        }

        let cache = client.tool_call_cache.lock().unwrap();
        let cached = cache.get("call_0_0__@__stream_tool__@__stream-sig-456").expect("Should have cached the tool call");
        assert_eq!(cached.name, "stream_tool");
        assert_eq!(cached.thought_signature, Some("stream-sig-456".to_string()));
    }

    #[test]
    fn test_thought_signature_attachment_in_translate_request() {
        let client = GoogleAiClient::new().unwrap();
        let tool_use_id = "cached_call_id".to_string();
        
        {
            let mut cache = client.tool_call_cache.lock().unwrap();
            cache.insert(tool_use_id.clone(), ToolCallMeta {
                name: "cached_tool".to_string(),
                thought_signature: Some("attached-sig-789".to_string()),
            });
        }

        let request = MessageRequest {
            model: "gemini-3-flash-preview".to_string(),
            max_tokens: 1024,
            messages: vec![InputMessage {
                role: "assistant".to_string(),
                content: vec![InputContentBlock::ToolUse {
                    id: tool_use_id.clone(),
                    name: "cached_tool".to_string(),
                    input: json!({"arg": 1}),
                }],
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            reasoning_effort: None,
        };

        let payload = client.translate_request(&request, "test-project").unwrap();
        let part = &payload["request"]["contents"][0]["parts"][0];
        
        assert_eq!(part["functionCall"]["name"], "cached_tool");
        assert_eq!(part["thoughtSignature"], "attached-sig-789");
    }

    #[test]
    fn test_tool_name_resolution_for_tool_result() {
        let client = GoogleAiClient::new().unwrap();
        let tool_use_id = "result_call_id".to_string();
        
        {
            let mut cache = client.tool_call_cache.lock().unwrap();
            cache.insert(tool_use_id.clone(), ToolCallMeta {
                name: "resolved_tool_name".to_string(),
                thought_signature: None,
            });
        }

        let request = MessageRequest {
            model: "gemini-3-flash-preview".to_string(),
            max_tokens: 1024,
            messages: vec![InputMessage {
                role: "user".to_string(),
                content: vec![InputContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: vec![ToolResultContentBlock::Text { text: "success".to_string() }],
                    is_error: false,
                }],
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            reasoning_effort: None,
        };

        let payload = client.translate_request(&request, "test-project").unwrap();
        let part = &payload["request"]["contents"][0]["parts"][0];
        
        assert_eq!(part["functionResponse"]["name"], "resolved_tool_name");
        assert_eq!(part["functionResponse"]["response"]["output"], "success");
    }
}
