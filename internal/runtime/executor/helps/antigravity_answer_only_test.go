package helps

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	_ "github.com/router-for-me/CLIProxyAPI/v7/internal/thinking/provider/antigravity"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/executor"
	"github.com/tidwall/gjson"
)

func TestAntigravityAnswerOnlyRequest(t *testing.T) {
	payload := []byte(`{"model":"do-not-change","request":{"contents":[{"role":"model","parts":[{"functionCall":{"name":"read","args":{}},"thoughtSignature":"keep"}]}],"generationConfig":{"thinkingConfig":{"thinkingBudget":8192,"includeThoughts":true}}}}`)
	for _, tc := range []struct {
		source, model       string
		disabled, untouched bool
	}{
		{"claude", "claude-opus-4-6-thinking", true, false},
		{"gemini", "gemini-3.1-flash-lite", true, false},
		{"gemini", "gemini-3.7-flash-high", false, false},
		{"openai", "claude-opus-4-6-thinking", false, true},
		{"gemini", "gemini-unknown-test", false, true},
	} {
		t.Run(tc.source+"/"+tc.model, func(t *testing.T) {
			got, err := AntigravityAnswerOnlyRequest(tc.source, tc.model, payload)
			if err != nil {
				t.Fatal(err)
			}
			if tc.untouched {
				if string(got) != string(payload) {
					t.Fatal("changed unrelated route")
				}
				return
			}
			for _, path := range []string{"model", "request.contents"} {
				if gjson.GetBytes(got, path).Raw != gjson.GetBytes(payload, path).Raw {
					t.Errorf("changed %s", path)
				}
			}
			config := gjson.GetBytes(got, "request.generationConfig.thinkingConfig")
			if tc.disabled {
				if config.Get("thinkingBudget").Int() != 0 || config.Get("thinkingLevel").Exists() {
					t.Fatalf("not disabled: %s", got)
				}
			} else {
				if config.Get("thinkingBudget").Int() != 8192 || config.Get("includeThoughts").Bool() {
					t.Fatalf("changed amount or exposed thoughts: %s", got)
				}
			}
		})
	}
}

func TestAntigravityAnswerOnlyNonStream(t *testing.T) {
	claude := []byte(`{"content":[{"type":"thinking","thinking":"secret","signature":"opaque"},{"type":"text","text":"answer"},{"type":"tool_use","id":"tool-1","name":"read","input":{}},{"type":"redacted_thinking","data":"redacted"}],"usage":{"output_tokens":42}}`)
	got := AntigravityAnswerOnlyResponse("claude", claude)
	if strings.Contains(string(got), "secret") || strings.Contains(string(got), "redacted") || gjson.GetBytes(got, "content.#").Int() != 2 || gjson.GetBytes(got, "content.1.id").String() != "tool-1" || gjson.GetBytes(got, "usage.output_tokens").Int() != 42 {
		t.Fatalf("bad Claude filter: %s", got)
	}
	gemini := []byte(`{"candidates":[{"content":{"parts":[{"thought":true,"text":"secret"},{"text":"answer","thoughtSignature":"keep"},{"functionCall":{"name":"read","args":{}},"thoughtSignature":"tool-keep"}]},"finishReason":"STOP"}],"usageMetadata":{"thoughtsTokenCount":42}}`)
	got = AntigravityAnswerOnlyResponse("gemini", gemini)
	if strings.Contains(string(got), "secret") || gjson.GetBytes(got, "candidates.0.content.parts.#").Int() != 2 || gjson.GetBytes(got, "candidates.0.content.parts.1.thoughtSignature").String() != "tool-keep" || gjson.GetBytes(got, "usageMetadata.thoughtsTokenCount").Int() != 42 {
		t.Fatalf("bad Gemini filter: %s", got)
	}
}

func TestAntigravityAnswerOnlyStreamThinkingThenTextAndTool(t *testing.T) {
	s := antigravityAnswerStream{format: "claude"}
	frames := []string{
		`{"type":"message_start","message":{"content":[]}}`,
		`{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"secret"}}`,
		`{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"opaque"}}`,
		`{"type":"content_block_stop","index":0}`,
		`{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}`,
		`{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}`,
		`{"type":"content_block_stop","index":1}`,
		`{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool-1","name":"read","input":{}}}`,
		`{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}`,
		`{"type":"content_block_stop","index":2}`,
		`{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}`,
		`{"type":"message_stop"}`,
	}
	var output string
	for i, frame := range frames {
		got := s.filter([]byte("event: " + gjson.Get(frame, "type").String() + "\ndata: " + frame + "\n\n"))
		if i >= 1 && i <= 4 && len(got) != 0 {
			t.Fatalf("exposed thinking event: %s", got)
		}
		if i >= 5 && i <= 7 && !strings.Contains(string(got), `"index":0`) {
			t.Fatalf("text index not compact: %s", got)
		}
		if i >= 8 && i <= 10 && !strings.Contains(string(got), `"index":1`) {
			t.Fatalf("tool index not compact: %s", got)
		}
		output += string(got)
	}
	if strings.Contains(output, "thinking") || strings.Contains(output, "opaque") || !strings.Contains(output, "answer") || !strings.Contains(output, "message_stop") || !strings.Contains(output, "tool-1") {
		t.Fatal(output)
	}
}

func TestAntigravityAnswerOnlyForwardDoesNotWaitForEOF(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	input := make(chan cliproxyexecutor.StreamChunk)
	out := make(chan cliproxyexecutor.StreamChunk)
	ticks := make(chan time.Time)
	done := make(chan struct{})
	go func() { defer close(done); forwardAntigravityAnswers(ctx, "claude", input, out, ticks) }()
	input <- cliproxyexecutor.StreamChunk{Payload: []byte("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n")}
	ticks <- time.Time{}
	if got := <-out; !strings.Contains(string(got.Payload), `"type":"ping"`) {
		t.Fatalf("no heartbeat: %s", got.Payload)
	}
	input <- cliproxyexecutor.StreamChunk{Payload: []byte("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"answer\"}}\n\n")}
	if got := <-out; !strings.Contains(string(got.Payload), "answer") {
		t.Fatal("text not forwarded while upstream remains open")
	}
	failure := errors.New("upstream interrupted")
	input <- cliproxyexecutor.StreamChunk{Err: failure}
	if got := <-out; got.Err != failure {
		t.Fatal("stream error hidden")
	}
	<-done
}

func TestAntigravityAnswerOnlyCancellationReleasesBlockedSend(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	input := make(chan cliproxyexecutor.StreamChunk, 1)
	input <- cliproxyexecutor.StreamChunk{Payload: []byte(`{"candidates":[]}`)}
	out := make(chan cliproxyexecutor.StreamChunk)
	done := make(chan struct{})
	go func() { defer close(done); forwardAntigravityAnswers(ctx, "gemini", input, out, nil) }()
	cancel()
	<-done
}
