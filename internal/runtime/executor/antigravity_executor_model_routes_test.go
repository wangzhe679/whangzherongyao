package executor

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	cliproxyauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/executor"
	sdktranslator "github.com/router-for-me/CLIProxyAPI/v7/sdk/translator"
	"github.com/tidwall/gjson"
)

func TestAntigravityClaudePublicModelRoutes(t *testing.T) {
	for _, model := range []string{"claude-opus-4-6", registry.AntigravityOpusUpstreamModel, registry.AntigravityOpusReasoningModel, registry.AntigravityOpusReasoningFullModel} {
		for _, stream := range []bool{false, true} {
			for _, tc := range []struct {
				name       string
				thinking   string
				maxTokens  int64
				wantBudget int64
				wantOutput int64
			}{
				{"explicit-budget", `,"thinking":{"type":"enabled","budget_tokens":2048}`, 32768, 2048, 32768},
				{"missing", "", 32768, 16384, 32768},
				{"disabled", `,"thinking":{"type":"disabled"}`, 32768, 16384, 32768},
				{"adaptive-small-output", `,"thinking":{"type":"adaptive"}`, 8192, 16384, 24576},
				{"explicit-small-output", `,"thinking":{"type":"enabled","budget_tokens":8192}`, 1024, 8192, 16384},
			} {
				t.Run(fmt.Sprintf("%s/stream=%v/%s", model, stream, tc.name), func(t *testing.T) {
					captured := make(chan []byte, 1)
					server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
						body, _ := io.ReadAll(r.Body)
						captured <- body
						w.Header().Set("Content-Type", "text/event-stream")
						_, _ = io.WriteString(w, "data: "+`{"response":{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"reasoning-sentinel"},{"text":"answer-sentinel"},{"functionCall":{"name":"read_file","args":{"path":"test.txt"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"thoughtsTokenCount":3,"totalTokenCount":6}}}`+"\n\n")
					}))
					defer server.Close()
					auth := &cliproxyauth.Auth{ID: t.Name(), Attributes: map[string]string{"base_url": server.URL}, Metadata: map[string]any{"access_token": "synthetic", "project_id": "synthetic", "expired": time.Now().Add(time.Hour).Format(time.RFC3339)}}
					// Existing New API request fields must not erase the selected route.
					payload := []byte(fmt.Sprintf(`{"model":%q,"max_tokens":%d%s,"messages":[{"role":"user","content":"hello"}]}`, model, tc.maxTokens, tc.thinking))
					req := cliproxyexecutor.Request{Model: model, Payload: payload}
					opts := cliproxyexecutor.Options{SourceFormat: sdktranslator.FormatClaude, OriginalRequest: payload, Stream: stream}
					e := NewAntigravityExecutor(&config.Config{})
					var output strings.Builder
					if stream {
						result, err := e.ExecuteStream(context.Background(), auth, req, opts)
						if err != nil {
							t.Fatal(err)
						}
						for chunk := range result.Chunks {
							if chunk.Err != nil {
								t.Fatal(chunk.Err)
							}
							output.Write(chunk.Payload)
						}
					} else {
						result, err := e.Execute(context.Background(), auth, req, opts)
						if err != nil {
							t.Fatal(err)
						}
						output.Write(result.Payload)
					}
					body := <-captured
					if got := gjson.GetBytes(body, "model").String(); got != registry.AntigravityOpusUpstreamModel {
						t.Fatalf("sent private alias upstream: %q", got)
					}
					visible := registry.IsAntigravityReasoningAlias(model)
					wantOutput := tc.maxTokens
					if visible {
						wantOutput = tc.wantOutput
					}
					if got := gjson.GetBytes(body, "request.generationConfig.maxOutputTokens").Int(); got != wantOutput {
						t.Fatalf("changed output budget: %d", got)
					}
					thinkingConfig := gjson.GetBytes(body, "request.generationConfig.thinkingConfig")
					if visible && (thinkingConfig.Get("thinkingBudget").Int() != tc.wantBudget || !thinkingConfig.Get("includeThoughts").Bool()) {
						t.Fatalf("thinking route not enabled: %s", thinkingConfig.Raw)
					}
					if !visible && thinkingConfig.Get("includeThoughts").Bool() {
						t.Fatalf("changed legacy route: %s", thinkingConfig.Raw)
					}
					if strings.Contains(output.String(), "reasoning-sentinel") != visible {
						t.Fatalf("incorrect thinking visibility: %s", output.String())
					}
					if !strings.Contains(output.String(), "answer-sentinel") || !strings.Contains(output.String(), "read_file") {
						t.Fatalf("lost text or tool call: %s", output.String())
					}
					t.Logf("public=%s upstream=%s visible_thinking=%v", model, gjson.GetBytes(body, "model").String(), visible)
				})
			}
		}
	}
}
