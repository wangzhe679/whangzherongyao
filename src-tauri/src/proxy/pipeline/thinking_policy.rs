//! Request-name policy applied after protocol adapters finish normalizing the payload.
//! Keeping the original name separate from the upstream ID makes routing, locking,
//! and thought visibility independent.

use serde_json::{json, Value};

pub const DEFAULT_CLAUDE_THINKING_BUDGET: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestedThinkingPolicy {
    Unspecified,
    Hidden,
    Visible,
}

impl RequestedThinkingPolicy {
    pub fn from_model(model: &str) -> Self {
        let model = model.trim().to_ascii_lowercase();
        match model.strip_prefix("models/").unwrap_or(&model) {
            "claude-opus-4-6" => Self::Hidden,
            "[思考]claude-opus-4-6" => Self::Visible,
            _ => Self::Unspecified,
        }
    }

    pub fn routing_model<'a>(self, requested_model: &'a str) -> &'a str {
        if self != Self::Unspecified {
            let model = requested_model
                .trim()
                .strip_prefix("models/")
                .unwrap_or(requested_model.trim());
            model.strip_prefix("[思考]").unwrap_or(model)
        } else {
            requested_model
        }
    }

    pub fn shows_thoughts(self) -> bool {
        self != Self::Hidden
    }

    pub fn response_model<'a>(self, requested_model: &'a str, routed_model: &'a str) -> &'a str {
        if self == Self::Unspecified {
            routed_model
        } else {
            requested_model
        }
    }

    /// This final pass is authoritative for the two public Opus names. Adapter
    /// defaults, global adaptive mode, and client disabled/-1 values cannot
    /// override the requested visibility or remove the positive budget.
    pub fn apply_upstream(self, payload: &mut Value, client_budget: Option<u64>) {
        if self == Self::Unspecified {
            return;
        }
        // A custom route to another family keeps that family's parameters.
        if payload.get("model").and_then(Value::as_str) != Some("claude-opus-4-6-thinking") {
            return;
        }
        let request = if payload.get("request").is_some() {
            &mut payload["request"]
        } else {
            payload
        };
        self.configure_request(request, client_budget);
    }

    pub fn configure_request(self, request: &mut Value, client_budget: Option<u64>) {
        if self == Self::Unspecified {
            return;
        }
        let config = crate::proxy::config::get_thinking_budget_config();
        self.configure_request_with_config(request, client_budget, &config);
    }

    fn configure_request_with_config(
        self,
        request: &mut Value,
        client_budget: Option<u64>,
        thinking_config: &crate::proxy::config::ThinkingBudgetConfig,
    ) {
        if self == Self::Unspecified {
            return;
        }
        if !request["generationConfig"].is_object() {
            request["generationConfig"] = json!({});
        }
        let config = &mut request["generationConfig"];
        let budget = Self::positive_budget_with_config(client_budget, thinking_config) as u64;
        config["thinkingConfig"] = json!({
            "includeThoughts": self.shows_thoughts(),
            "thinkingBudget": budget
        });
        if config["maxOutputTokens"].as_u64().unwrap_or(0) <= budget {
            config["maxOutputTokens"] = json!(budget + 1);
        }
    }

    pub fn positive_budget(client_budget: Option<u64>) -> u32 {
        Self::positive_budget_with_config(
            client_budget,
            &crate::proxy::config::get_thinking_budget_config(),
        )
    }

    fn positive_budget_with_config(
        client_budget: Option<u64>,
        config: &crate::proxy::config::ThinkingBudgetConfig,
    ) -> u32 {
        // Preserve the existing Gateway/Client budget authority. Only the
        // public Opus names' visibility and positive fallback are enforced here.
        let budget = crate::proxy::model_specs::resolve_custom_budget(
            "claude-opus-4-6-thinking",
            config.effort.as_deref(),
            client_budget,
            config,
            None,
        )
        .and_then(|budget| u64::try_from(budget).ok())
        .filter(|budget| *budget > 0)
        .unwrap_or(DEFAULT_CLAUDE_THINKING_BUDGET as u64)
        .min(crate::proxy::model_specs::get_thinking_budget(
            "claude-opus-4-6-thinking",
            None,
        ))
        .min(
            crate::proxy::model_specs::get_max_output_tokens("claude-opus-4-6-thinking", None)
                .saturating_sub(1),
        );
        budget as u32
    }

    /// Apply only after response capture so hidden thoughts and their signatures
    /// remain available for subsequent tool turns, while no thought is emitted.
    pub fn filter_gemini_output(self, response: &mut Value) {
        if self.shows_thoughts() {
            return;
        }
        let raw = if response.get("response").is_some() {
            &mut response["response"]
        } else {
            response
        };
        if let Some(candidates) = raw.get_mut("candidates").and_then(Value::as_array_mut) {
            for candidate in candidates {
                if let Some(parts) = candidate
                    .pointer_mut("/content/parts")
                    .and_then(Value::as_array_mut)
                {
                    parts.retain(|part| !crate::proxy::thinking_store::is_thought_part(part));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_name_policy_overrides_adaptive_and_disabled_parameters() {
        for (name, include_thoughts) in
            [("claude-opus-4-6", false), ("[思考]claude-opus-4-6", true)]
        {
            let policy = RequestedThinkingPolicy::from_model(name);
            assert_eq!(policy.routing_model(name), "claude-opus-4-6");
            let mut body = json!({"model":"claude-opus-4-6-thinking","request": {
                "generationConfig":{"maxOutputTokens":512,"thinkingConfig": {
                    "includeThoughts":!include_thoughts,"thinkingBudget":-1,"thinkingLevel":"HIGH"
                }}
            }});
            policy.apply_upstream(&mut body, Some(0));
            assert_eq!(
                body["request"]["generationConfig"]["thinkingConfig"],
                json!({"includeThoughts":include_thoughts,"thinkingBudget":1024})
            );
            assert_eq!(body["request"]["generationConfig"]["maxOutputTokens"], 1025);
        }
    }

    #[test]
    fn hidden_thoughts_do_not_remove_text_tool_calls_or_usage() {
        let original = json!({"response":{"usageMetadata":{"totalTokenCount":10},
        "candidates":[{"content":{"parts":[
            {"text":"one","thought":true,"thoughtSignature":"private"},
            {"text":"two","thought":true},
            {"text":"answer","thoughtSignature":"anchor"},
            {"functionCall":{"name":"tool","args":{}},"thoughtSignature":"tool-signature"}
        ]}}]}});
        let mut hidden = original.clone();
        RequestedThinkingPolicy::Hidden.filter_gemini_output(&mut hidden);
        assert_eq!(
            hidden["response"]["candidates"][0]["content"]["parts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            hidden["response"]["usageMetadata"],
            original["response"]["usageMetadata"]
        );
        let mut visible = original.clone();
        RequestedThinkingPolicy::Visible.filter_gemini_output(&mut visible);
        assert_eq!(visible, original);
    }

    #[test]
    fn opus_policy_preserves_positive_budget_within_existing_limits() {
        let config = crate::proxy::config::ThinkingBudgetConfig {
            control_source: crate::proxy::config::ThinkingControlSource::Client,
            ..Default::default()
        };
        let mut body = json!({});
        RequestedThinkingPolicy::Visible.configure_request_with_config(
            &mut body,
            Some(8192),
            &config,
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            8192
        );
        RequestedThinkingPolicy::Visible.configure_request_with_config(
            &mut body,
            Some(50000),
            &config,
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            32768
        );
        assert_eq!(
            RequestedThinkingPolicy::from_model("models/[思考]claude-opus-4-6")
                .routing_model("models/[思考]claude-opus-4-6"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn opus_policy_keeps_gateway_custom_budget_authority_and_visibility() {
        let mut config = crate::proxy::config::ThinkingBudgetConfig {
            claude_budget: 8192,
            ..Default::default()
        };
        for (policy, visible) in [
            (RequestedThinkingPolicy::Hidden, false),
            (RequestedThinkingPolicy::Visible, true),
        ] {
            for client_budget in [None, Some(0), Some(4096)] {
                let mut request = json!({"generationConfig": {
                    "maxOutputTokens":512,"thinkingConfig":{"thinkingLevel":"HIGH"}
                }});
                policy.configure_request_with_config(&mut request, client_budget, &config);
                assert_eq!(
                    request["generationConfig"]["thinkingConfig"],
                    json!({"includeThoughts":visible,"thinkingBudget":8192})
                );
                assert_eq!(request["generationConfig"]["maxOutputTokens"], 8193);
            }
        }
        config.effort = Some("high".into());
        config.claude_high = 12288;
        assert_eq!(
            RequestedThinkingPolicy::positive_budget_with_config(Some(4096), &config),
            12288
        );
    }

    #[test]
    fn opus_policy_keeps_client_control_and_positive_1024_fallback() {
        use crate::proxy::config::{
            ThinkingBudgetConfig, ThinkingBudgetMode, ThinkingControlSource,
        };
        let mut config = ThinkingBudgetConfig::default();
        assert_eq!(
            RequestedThinkingPolicy::positive_budget_with_config(None, &config),
            1024
        );
        config.claude_budget = -1;
        assert_eq!(
            RequestedThinkingPolicy::positive_budget_with_config(Some(4096), &config),
            1024
        );
        config.claude_budget = 8192;
        config.claude_mode = ThinkingBudgetMode::Default;
        assert_eq!(
            RequestedThinkingPolicy::positive_budget_with_config(Some(4096), &config),
            1024
        );
        config.control_source = ThinkingControlSource::Client;
        assert_eq!(
            RequestedThinkingPolicy::positive_budget_with_config(Some(4096), &config),
            4096
        );
        for client_budget in [None, Some(0)] {
            assert_eq!(
                RequestedThinkingPolicy::positive_budget_with_config(client_budget, &config),
                1024
            );
        }
    }

    #[test]
    fn opus_policy_final_upstream_payload_across_all_protocol_adapters() {
        use crate::proxy::mappers::{claude, gemini, openai};
        for (name, visible) in [("claude-opus-4-6", false), ("[思考]claude-opus-4-6", true)] {
            let policy = RequestedThinkingPolicy::from_model(name);
            let real = "claude-opus-4-6-thinking";
            let chat: openai::OpenAIRequest = serde_json::from_value(json!({
                "model":policy.routing_model(name),"messages":[{"role":"user","content":"hello"}],
                "thinking":{"type":"disabled","budget_tokens":0}
            }))
            .unwrap();
            let anthropic: claude::ClaudeRequest = serde_json::from_value(json!({
                "model":real,"max_tokens":2048,"messages":[{"role":"user","content":"hello"}],
                "thinking":{"type":"disabled","budget_tokens":0}
            }))
            .unwrap();
            let native = json!({"model":policy.routing_model(name),
                "contents":[{"role":"user","parts":[{"text":"hello"}]}],
                "generationConfig":{"thinkingConfig":{"thinkingBudget":-1,"includeThoughts":!visible}}
            });
            let (chat_payload, _, _, _) =
                openai::transform_openai_request(&chat, "project", real, None);
            let (responses_payload, _, _, _) = openai::transform_openai_request_with_session(
                &chat,
                "project",
                real,
                None,
                "opus-policy-responses",
                None,
                true,
            );
            let anthropic_payload = claude::transform_claude_request_in(
                &anthropic,
                "project",
                false,
                None,
                "opus-policy-claude",
                None,
            )
            .unwrap();
            let gemini_payload = gemini::wrap_request(
                &native,
                "project",
                real,
                None,
                Some("opus-policy-gemini"),
                None,
            );
            for mut payload in [
                chat_payload,
                responses_payload,
                anthropic_payload,
                gemini_payload,
            ] {
                policy.apply_upstream(&mut payload, None);
                assert_eq!(payload["model"], real);
                assert_eq!(
                    payload["request"]["generationConfig"]["thinkingConfig"],
                    json!({"includeThoughts":visible,"thinkingBudget":1024})
                );
                assert!(
                    payload["request"]["generationConfig"]["maxOutputTokens"]
                        .as_u64()
                        .unwrap()
                        > 1024
                );
            }
        }
    }

    fn mock_stream(
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>> {
        let response = json!({"response":{"modelVersion":"claude-opus-4-6-thinking",
            "candidates":[{"content":{"role":"model","parts":[
                {"thought":true,"text":"thought-one ","thoughtSignature":mock_signature()},
                {"thought":true,"text":"thought-two"},
                {"text":"visible-answer"},
                {"thought":true,"text":"late-thought-three"}
            ]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":4,"totalTokenCount":9}}});
        let raw = format!("data: {response}\n\ndata: [DONE]\n\n");
        // Fragment in the middle of JSON to exercise actual streaming buffering.
        let midpoint = raw.len() / 2;
        Box::pin(futures::stream::iter(vec![
            Ok(bytes::Bytes::copy_from_slice(&raw.as_bytes()[..midpoint])),
            Ok(bytes::Bytes::copy_from_slice(&raw.as_bytes()[midpoint..])),
        ]))
    }

    fn mock_signature() -> String {
        format!("policy-test-signature-{}", "a".repeat(128))
    }

    async fn collect_stream(
        mut stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<bytes::Bytes, String>> + Send>,
        >,
    ) -> String {
        use futures::StreamExt;
        let mut output = String::new();
        while let Some(chunk) = stream.next().await {
            output.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
        }
        output
    }

    #[tokio::test]
    async fn opus_policy_openai_chat_and_responses_streams_preserve_or_hide_all_thoughts() {
        use crate::proxy::mappers::openai::streaming::{
            create_codex_sse_stream, create_openai_sse_stream_with_anchor,
        };
        for (name, visible) in [("claude-opus-4-6", false), ("[思考]claude-opus-4-6", true)] {
            let chat = create_openai_sse_stream_with_anchor(
                mock_stream(),
                name.into(),
                format!("opus-chat-{visible}"),
                1,
                None,
                true,
                None,
            );
            let responses = create_codex_sse_stream(
                mock_stream(),
                name.into(),
                format!("opus-responses-{visible}"),
                1,
                0,
                format!("resp-opus-{visible}"),
                None,
                false,
            );
            for output in [collect_stream(chat).await, collect_stream(responses).await] {
                assert_eq!(output.contains("thought-one"), visible);
                assert_eq!(output.contains("thought-two"), visible);
                assert_eq!(output.contains("late-thought-three"), visible);
                assert!(output.contains("visible-answer"));
            }
            for session in [
                format!("opus-chat-{visible}"),
                format!("opus-responses-{visible}"),
            ] {
                assert_eq!(
                    crate::proxy::SignatureCache::global().get_session_signature(&session),
                    Some(mock_signature())
                );
            }
        }
    }

    #[tokio::test]
    async fn opus_policy_anthropic_stream_preserves_or_hides_all_thoughts() {
        for visible in [false, true] {
            let stream = crate::proxy::mappers::claude::create_claude_sse_stream(
                mock_stream(),
                "trace-opus".into(),
                "mock@example.com".into(),
                Some(format!("opus-anthropic-{visible}")),
                false,
                200000,
                None,
                1,
                None,
                vec![],
                visible,
            );
            let output = collect_stream(stream).await;
            assert_eq!(output.contains("thought-one"), visible);
            assert_eq!(output.contains("thought-two"), visible);
            assert_eq!(output.contains("late-thought-three"), visible);
            assert_eq!(output.contains("thinking_delta"), visible);
            assert!(output.contains("visible-answer"));
            assert!(output.contains("message_stop"));
            assert_eq!(
                crate::proxy::SignatureCache::global()
                    .get_session_signature(&format!("opus-anthropic-{visible}")),
                Some(mock_signature())
            );
        }
    }
}
