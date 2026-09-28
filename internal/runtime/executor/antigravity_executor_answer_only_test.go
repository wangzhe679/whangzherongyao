package executor

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	cliproxyauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/executor"
	sdktranslator "github.com/router-for-me/CLIProxyAPI/v7/sdk/translator"
)

func TestAntigravityAnswerOnlyExecutorPaths(t *testing.T) {
	for _, model := range []string{"claude-opus-4-6-thinking", "gemini-3.7-flash-high", "gemini-3-pro"} {
		for _, stream := range []bool{false, true} {
			t.Run(fmt.Sprintf("%s/stream=%v", model, stream), func(t *testing.T) {
				ctx, cancel := context.WithCancel(context.Background())
				defer cancel()
				release := make(chan struct{})
				var once sync.Once
				finish := func() { once.Do(func() { close(release) }) }
				server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					response := `{"response":{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"hidden-reasoning-sentinel"},{"text":"visible-answer-sentinel"}]}}]}}`
					if strings.Contains(r.URL.Path, "streamGenerateContent") {
						w.Header().Set("Content-Type", "text/event-stream")
						_, _ = io.WriteString(w, "data: "+response+"\n\n")
						w.(http.Flusher).Flush()
						if stream {
							select {
							case <-release:
							case <-r.Context().Done():
								return
							}
						}
						_, _ = io.WriteString(w, "data: "+`{"response":{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2,"totalTokenCount":3}}}`+"\n\n")
					} else {
						_, _ = io.WriteString(w, response)
					}
				}))
				defer server.Close()
				defer finish()
				defer cancel()
				auth := &cliproxyauth.Auth{ID: t.Name(), Attributes: map[string]string{"base_url": server.URL}, Metadata: map[string]any{"access_token": "test-token", "project_id": "project-test", "expired": time.Now().Add(time.Hour).Format(time.RFC3339)}}
				payload := []byte(`{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}`)
				format := sdktranslator.FormatGemini
				if strings.HasPrefix(model, "claude-") {
					format = sdktranslator.FormatClaude
					payload = []byte(`{"max_tokens":8192,"messages":[{"role":"user","content":"hello"}]}`)
				}
				req := cliproxyexecutor.Request{Model: model, Payload: payload}
				opts := cliproxyexecutor.Options{SourceFormat: format, Stream: stream}
				executor := NewAntigravityExecutor(&config.Config{})
				var output strings.Builder
				if stream {
					result, err := executor.ExecuteStream(ctx, auth, req, opts)
					if err != nil {
						t.Fatal(err)
					}
					// A watchdog detects deadlocks; ordering is controlled by release.
					watchdog := time.NewTimer(10 * time.Second)
					defer watchdog.Stop()
				loop:
					for {
						select {
						case chunk, ok := <-result.Chunks:
							if !ok {
								break loop
							}
							if chunk.Err != nil {
								t.Fatal(chunk.Err)
							}
							output.Write(chunk.Payload)
							if strings.Contains(output.String(), "visible-answer-sentinel") {
								finish()
							}
						case <-watchdog.C:
							t.Fatal("answer was not forwarded before upstream completion")
						}
					}
				} else {
					result, err := executor.Execute(ctx, auth, req, opts)
					if err != nil {
						t.Fatal(err)
					}
					output.Write(result.Payload)
				}
				if strings.Contains(output.String(), "hidden-reasoning-sentinel") || !strings.Contains(output.String(), "visible-answer-sentinel") {
					t.Fatalf("unexpected visible response: %s", output.String())
				}
			})
		}
	}
}
