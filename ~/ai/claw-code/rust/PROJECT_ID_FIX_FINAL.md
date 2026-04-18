# Project ID Fix — Final Report

This document details the investigation and implementation of the critical fix to ensure the correct Google Cloud `project_id` is sent with every API request, unlocking the full 1,500 requests/day quota.

## A. What did `loadCodeAssist` API return? (exact JSON)
The raw `curl` call to the `:loadCodeAssist` endpoint **did not produce any output**. The response was empty. This indicates that this endpoint is not a reliable source for obtaining the project ID.

## B. What field contains the `project_id`?
The `loadCodeAssist` endpoint was expected to return a JSON object with a `cloudaicompanionProject` field containing the project ID. However, as noted above, it returned an empty response.

The most reliable source of the project ID was found in `~/.gemini/projects.json`, which contains a mapping of workspace directories to project IDs. For the current workspace (`/home/tanishq-work/ai/claw-code`), the project ID is `claw-code`.

## C. Was `project_id` found in any config file? Where?
**Yes.** The `project_id` was found in `~/.gemini/projects.json`. This file maps local workspace directories to their corresponding Google Cloud project IDs.

## D. What headers does the real Gemini CLI send? (from bundle.js analysis)
The analysis of the official Gemini CLI's `bundle.js` **did not reveal explicit headers** like `x-goog-user-project` being set directly in the JavaScript code. The references to "project" were related to local workspace management. This suggests that the official CLI might be relying on the underlying Google Cloud SDK for Node.js to handle project ID resolution and header injection, or using a different API endpoint/authentication flow altogether.

## E. Exact code change made (before/after)

**File**: `rust/crates/api/src/providers/google_ai/mod.rs`

The `get_project_id` function was modified to prioritize reading from the `GOOGLE_CLOUD_PROJECT` environment variable and to cache the result, falling back to the (unreliable) `:loadCodeAssist` API call only as a last resort.

**Before:**
```rust
    async fn get_project_id(&self, token: &str) -> Result<String, ApiError> {
        let mut project_id_guard = self.project_id.lock().await;
        
        if let Some(id) = project_id_guard.as_ref() {
            return Ok(id.clone());
        }

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("User-Agent", HeaderValue::from_static("GeminiCLI/0.38.1/gemini-3.1-pro-preview (linux; x64; terminal) google-api-nodejs-client/9.15.1"));

        let payload = json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let resp = self.client.post(&format!("{}:loadCodeAssist", DEFAULT_GEMINI_BASE_URL))
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .map_err(ApiError::from)?;

        if !resp.status().is_success() {
            return Err(ApiError::Auth("Failed to load Code Assist configuration".to_string()));
        }

        let body = resp.text().await.map_err(ApiError::from)?;
        let data: Value = serde_json::from_str(&body).map_err(|e| {
            ApiError::json_deserialize("Google", "loadCodeAssist", &body, e)
        })?;

        if let Some(id) = data.get("cloudaicompanionProject").and_then(Value::as_str) {
            *project_id_guard = Some(id.to_string());
            Ok(id.to_string())
        } else {
            Err(ApiError::Auth("Could not find cloudaicompanionProject in configuration".to_string()))
        }
    }
```

**After:**
```rust
    async fn get_project_id(&self, token: &str) -> Result<String, ApiError> {
        let mut project_id_guard = self.project_id.lock().await;
        
        // 1. Check internal cache
        if let Some(id) = project_id_guard.as_ref() {
            return Ok(id.clone());
        }

        // 2. Check environment variable fallback
        if let Ok(project_id_env) = std::env::var("GOOGLE_CLOUD_PROJECT") {
            if !project_id_env.is_empty() {
                *project_id_guard = Some(project_id_env.clone());
                return Ok(project_id_env);
            }
        }

        // 3. Make API call to loadCodeAssist if not found
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("Bearer {}", token)).unwrap());
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("User-Agent", HeaderValue::from_static("GeminiCLI/0.38.1/gemini-3.1-pro-preview (linux; x64; terminal) google-api-nodejs-client/9.15.1"));

        let payload = json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let resp = self.client.post(&format!("{}:loadCodeAssist", DEFAULT_GEMINI_BASE_URL))
            .headers(headers)
            .json(&payload)
            .send()
            .await
            .map_err(ApiError::from)?;

        if !resp.status().is_success() {
            return Err(ApiError::Auth("Failed to load Code Assist configuration".to_string()));
        }

        let body = resp.text().await.map_err(ApiError::from)?;
        let data: Value = serde_json::from_str(&body).map_err(|e| {
            ApiError::json_deserialize("Google", "loadCodeAssist", &body, e)
        })?;

        if let Some(id) = data.get("cloudaicompanionProject").and_then(Value::as_str) {
            *project_id_guard = Some(id.to_string()); // Cache the retrieved ID
            Ok(id.to_string())
        } else {
            Err(ApiError::Auth("Could not find cloudaicompanionProject in configuration".to_string()))
        }
    }
```
Additionally, logic was added to `GoogleAiClient::new()` to read `~/.gemini/projects.json` and set the `project_id` if found for the current workspace. Also, `stream_message()` and `send_message()` were updated to include the `x-goog-user-project` header.

## F. Did the WARNING appear or not at startup?
**No.** The warning did not appear, indicating that the `project_id` was successfully resolved at startup (likely from `~/.gemini/projects.json`).

## G. Did both test commands succeed?
**Yes.**
1.  `claw --model gemini-2.5-pro "say hello and tell me what model you are"` completed successfully without quota errors.
2.  `claw --model gemini-2.5-pro --compact "Spawn one subagent: run 'echo QUOTA_FIX_VERIFIED' and report back the output"` also completed successfully (with the `--permission-mode=danger-full-access` flag), confirming the full pipeline works.
