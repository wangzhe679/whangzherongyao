package executor

import (
	"bytes"
	"context"
	"encoding/base64"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	cliproxyauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/executor"
	sdktranslator "github.com/router-for-me/CLIProxyAPI/v7/sdk/translator"
	"github.com/tidwall/gjson"
	"google.golang.org/protobuf/encoding/protowire"
)

func TestAntigravityToolsClaudeRecoveryExecutorPaths(t *testing.T) {
	// A structurally valid synthetic signature must reach the upstream before
	// it can be rejected; malformed signatures are already filtered on ingress.
	channel := []byte{8, 12, 16, 2}
	channel = protowire.AppendTag(channel, 6, protowire.BytesType)
	channel = protowire.AppendString(channel, "claude-opus-4-6")
	container := protowire.AppendTag(nil, 1, protowire.BytesType)
	container = protowire.AppendBytes(container, channel)
	container = protowire.AppendTag(container, 2, protowire.BytesType)
	container = protowire.AppendBytes(container, bytes.Repeat([]byte{0x11}, 48))
	envelope := protowire.AppendTag(nil, 2, protowire.BytesType)
	envelope = protowire.AppendBytes(envelope, container)
	signature := base64.StdEncoding.EncodeToString(envelope)
	for _, stream := range []bool{false, true} {
		t.Run(fmt.Sprintf("stream=%v", stream), func(t *testing.T) {
			var bodies [][]byte
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				body, _ := io.ReadAll(r.Body)
				bodies = append(bodies, body)
				if gjson.GetBytes(body, "request.toolConfig").Exists() {
					t.Error("Tools Claude request must omit toolConfig")
				}
				if len(bodies) == 1 {
					w.WriteHeader(400)
					_, _ = io.WriteString(w, `{"error":{"message":"Invalid thought signature"}}`)
					return
				}
				w.Header().Set("Content-Type", "text/event-stream")
				_, _ = io.WriteString(w, "data: "+`{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}}`+"\n\n")
			}))
			defer server.Close()
			executor := NewAntigravityExecutor(&config.Config{RequestRetry: 1})
			auth := &cliproxyauth.Auth{ID: t.Name(), Attributes: map[string]string{"base_url": server.URL}, Metadata: map[string]any{"access_token": "test-token", "project_id": "project-test", "expired": time.Now().Add(time.Hour).Format(time.RFC3339)}}
			payload := []byte(`{"model":"claude-opus-4-6-thinking","max_tokens":8192,"thinking":{"type":"enabled","budget_tokens":2048},"messages":[{"role":"user","content":"question"},{"role":"assistant","content":[{"type":"thinking","thinking":"reasoning","signature":"` + signature + `"},{"type":"text","text":"answer"}]},{"role":"user","content":"continue"}]}`)
			req := cliproxyexecutor.Request{Model: "claude-opus-4-6-thinking", Payload: payload}
			opts := cliproxyexecutor.Options{SourceFormat: sdktranslator.FormatClaude, Stream: stream}
			if stream {
				result, err := executor.ExecuteStream(context.Background(), auth, req, opts)
				if err != nil {
					t.Fatal(err)
				}
				for chunk := range result.Chunks {
					if chunk.Err != nil {
						t.Fatal(chunk.Err)
					}
				}
			} else {
				result, err := executor.Execute(context.Background(), auth, req, opts)
				if err != nil {
					t.Fatal(err)
				}
				if !strings.Contains(string(result.Payload), "ok") {
					t.Error("missing final text")
				}
			}
			if len(bodies) != 2 {
				t.Fatalf("requests=%d want=2", len(bodies))
			}
			if !gjson.GetBytes(bodies[0], "request.contents.1.parts.0.thought").Bool() {
				t.Fatal("test never sent a thinking block")
			}
			if gjson.GetBytes(bodies[1], "request.contents.1.parts.0.thought").Exists() || gjson.GetBytes(bodies[1], "request.contents.1.parts.0.text").String() != "reasoning" {
				t.Error("thinking history not repaired")
			}
			thinking := "request.generationConfig.thinkingConfig"
			if gjson.GetBytes(bodies[0], thinking).Exists() || gjson.GetBytes(bodies[1], thinking).Exists() {
				t.Error("recovery re-enabled disabled thinking")
			}
		})
	}
}

func TestAntigravityToolsGeminiRecoveryExecutorPaths(t *testing.T) {
	for _, model := range []string{"gemini-3.7-flash", "gemini-3-pro"} {
		for _, stream := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/stream=%v", model, stream), func(t *testing.T) {
				var bodies [][]byte
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					body, _ := io.ReadAll(r.Body)
					bodies = append(bodies, body)
					if r.Header.Get("Authorization") != "Bearer test-token" {
						t.Error("credential changed")
					}
					if len(bodies) == 1 {
						w.WriteHeader(400)
						_, _ = io.WriteString(w, `{"error":{"message":"exceeds the maximum number of tokens"}}`)
						return
					}
					response := `{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}}}`
					if strings.Contains(r.URL.Path, "streamGenerateContent") {
						w.Header().Set("Content-Type", "text/event-stream")
						_, _ = io.WriteString(w, "data: "+response+"\n\n")
					} else {
						_, _ = io.WriteString(w, response)
					}
				}))
				defer server.Close()
				executor := NewAntigravityExecutor(&config.Config{RequestRetry: 1})
				auth := &cliproxyauth.Auth{ID: t.Name(), Attributes: map[string]string{"base_url": server.URL}, Metadata: map[string]any{
					"access_token": "test-token", "project_id": "project-test", "expired": time.Now().Add(time.Hour).Format(time.RFC3339),
				}}
				req := cliproxyexecutor.Request{Model: model, Payload: []byte(`{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}`)}
				opts := cliproxyexecutor.Options{SourceFormat: sdktranslator.FormatGemini, Stream: stream}
				if stream {
					result, err := executor.ExecuteStream(context.Background(), auth, req, opts)
					if err != nil {
						t.Fatal(err)
					}
					for chunk := range result.Chunks {
						if chunk.Err != nil {
							t.Fatal(chunk.Err)
						}
					}
				} else {
					result, err := executor.Execute(context.Background(), auth, req, opts)
					if err != nil {
						t.Fatal(err)
					}
					if !strings.Contains(string(result.Payload), "ok") {
						t.Fatalf("missing response: %s", result.Payload)
					}
				}
				if len(bodies) != 2 {
					t.Fatalf("requests=%d want=2", len(bodies))
				}
				if gjson.GetBytes(bodies[0], "request.sessionId").String() == gjson.GetBytes(bodies[1], "request.sessionId").String() {
					t.Error("session unchanged")
				}
				for _, path := range []string{"model", "request.contents", "request.generationConfig"} {
					if gjson.GetBytes(bodies[0], path).Raw != gjson.GetBytes(bodies[1], path).Raw {
						t.Errorf("changed %s", path)
					}
				}
			})
		}
	}
}

func TestAntigravityToolsClaudeToolConfigScoped(t *testing.T) {
	executor := NewAntigravityExecutor(&config.Config{})
	auth := &cliproxyauth.Auth{Metadata: map[string]any{"project_id": "project-test"}}
	for _, source := range []string{"claude", "openai", "gemini", ""} {
		for _, withTools := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/tools=%v", source, withTools), func(t *testing.T) {
				payload := `{"request":{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}}`
				if withTools {
					payload = `{"request":{"contents":[{"role":"user","parts":[{"text":"hello"}]}],"tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]}]}}`
				}
				req, err := executor.buildRequestForProtocol(context.Background(), auth, "test", "claude-opus-4-6-thinking", []byte(payload), false, "", "https://upstream.invalid", source)
				if err != nil {
					t.Fatal(err)
				}
				body, _ := io.ReadAll(req.Body)
				_ = req.Body.Close()
				if got := gjson.GetBytes(body, "request.toolConfig").Exists(); got != (source != "claude") {
					t.Fatalf("source=%s toolConfig exists=%v", source, got)
				}
				if withTools && !gjson.GetBytes(body, "request.tools.0.functionDeclarations.0").Exists() {
					t.Error("lost tools")
				}
			})
		}
	}
}

// Explicit client choices must not disappear when omitting CPA's forced mode.
func TestAntigravityToolsPreservesExplicitToolChoice(t *testing.T) {
	executor := NewAntigravityExecutor(&config.Config{})
	auth := &cliproxyauth.Auth{Metadata: map[string]any{"project_id": "project-test"}}
	for _, mode := range []string{"AUTO", "NONE", "ANY"} {
		payload := []byte(`{"request":{"contents":[{"role":"user","parts":[{"text":"hello"}]}],"toolConfig":{"functionCallingConfig":{"mode":"` + mode + `","allowedFunctionNames":["read"]}}}}`)
		req, err := executor.buildRequestForProtocol(context.Background(), auth, "test", "claude-opus-4-6-thinking", payload, false, "", "https://upstream.invalid", "claude")
		if err != nil {
			t.Fatal(err)
		}
		body, _ := io.ReadAll(req.Body)
		_ = req.Body.Close()
		if gjson.GetBytes(body, "request.toolConfig").Raw != gjson.GetBytes(payload, "request.toolConfig").Raw {
			t.Errorf("lost explicit tool choice %s", mode)
		}
	}
}
