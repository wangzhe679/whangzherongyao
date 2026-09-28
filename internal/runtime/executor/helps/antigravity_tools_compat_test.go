package helps

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net/http"
	"strings"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/signature"
	"github.com/tidwall/gjson"
)

type toolsRoundTrip func(*http.Request) (*http.Response, error)

func (f toolsRoundTrip) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

const toolsHistory = `{"model":"unchanged","request":{"sessionId":"same-session","generationConfig":{"thinkingConfig":{"thinkingBudget":16384,"includeThoughts":true}},"contents":[{"role":"user","parts":[{"text":"question"}]},{"role":"model","parts":[{"thought":true,"text":"reasoning","thoughtSignature":"rejected-signature"},{"thoughtSignature":"signature-only"},{"functionCall":{"name":"read_file","args":{"path":"a.txt","thought":"literal"}}}]},{"role":"user","parts":[{"functionResponse":{"name":"read_file","response":{"text":"result"}}}]}]}}`

func TestAntigravityToolsSignatureRecovery(t *testing.T) {
	for _, source := range []string{"claude", "gemini"} {
		t.Run(source, func(t *testing.T) {
			calls := 0
			client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
				calls++
				payload, _ := io.ReadAll(r.Body)
				_ = r.Body.Close()
				if r.Header.Get("Authorization") != "Bearer test" || r.URL.String() != "https://upstream.invalid/generate" {
					t.Error("retry changed credential or URL")
				}
				if calls == 1 {
					if string(payload) != toolsHistory {
						t.Error("healthy request was changed")
					}
					return &http.Response{StatusCode: 400, Body: io.NopCloser(strings.NewReader(`{"error":{"message":"Invalid thought signature"}}`))}, nil
				}
				if calls != 2 {
					t.Fatal("more than one repair retry")
				}
				for _, path := range []string{"model", "request.sessionId", "request.generationConfig", "request.contents.0", "request.contents.2"} {
					if gjson.GetBytes(payload, path).Raw != gjson.Get(toolsHistory, path).Raw {
						t.Errorf("changed %s", path)
					}
				}
				parts := gjson.GetBytes(payload, "request.contents.1.parts").Array()
				if len(parts) != 2 || parts[0].Get("thought").Exists() || parts[0].Get("thoughtSignature").Exists() || parts[0].Get("text").String() != "reasoning" {
					t.Fatalf("invalid recovered history: %s", payload)
				}
				if parts[1].Get("functionCall").Raw != gjson.Get(toolsHistory, "request.contents.1.parts.2.functionCall").Raw {
					t.Error("changed tool arguments")
				}
				if source == "gemini" && parts[1].Get("thoughtSignature").String() != signature.GeminiSkipThoughtSignatureValidator {
					t.Error("missing existing Gemini compatibility fallback")
				}
				if source == "claude" && parts[1].Get("thoughtSignature").Exists() {
					t.Error("invented Claude signature")
				}
				return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader("ok"))}, nil
			})}
			req, _ := http.NewRequest("POST", "https://upstream.invalid/generate", strings.NewReader(toolsHistory))
			req.Header.Set("Authorization", "Bearer test")
			resp, repaired, err := DoAntigravityToolsRequest(client, req, source, source+"-model")
			if err != nil || resp.StatusCode != 200 || len(repaired) == 0 || calls != 2 {
				t.Fatalf("calls=%d, resp=%v, err=%v", calls, resp, err)
			}
			_ = resp.Body.Close()
		})
	}
}

func TestAntigravityToolsRecoveryBoundaries(t *testing.T) {
	for _, tc := range []struct {
		name, source, model, message string
		status, wantCalls            int
	}{
		{"repeat-400", "claude", "claude-opus-4-6-thinking", "Invalid thought signature", 400, 2},
		{"ordinary-400", "gemini", "gemini-3.7-flash", "Unknown field unrelated", 400, 1},
		{"429", "claude", "claude-opus-4-6-thinking", "Invalid thought signature", 429, 1},
		{"503", "claude", "claude-opus-4-6-thinking", "Invalid thought signature", 503, 1},
		{"openai", "openai", "claude-opus-4-6-thinking", "Invalid thought signature", 400, 1},
		{"cross-protocol", "claude", "gemini-3.7-flash", "Invalid thought signature", 400, 1},
		{"large-error", "gemini", "gemini-3.7-flash", "Invalid thought signature" + strings.Repeat("x", 65536), 400, 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			calls := 0
			client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
				calls++
				_ = r.Body.Close()
				return &http.Response{StatusCode: tc.status, Body: io.NopCloser(strings.NewReader(tc.message))}, nil
			})}
			req, _ := http.NewRequest("POST", "https://upstream.invalid", strings.NewReader(toolsHistory))
			resp, _, err := DoAntigravityToolsRequest(client, req, tc.source, tc.model)
			if err != nil {
				t.Fatal(err)
			}
			body, err := io.ReadAll(resp.Body)
			_ = resp.Body.Close()
			if err != nil || string(body) != tc.message || calls != tc.wantCalls {
				t.Fatalf("calls=%d want=%d, response preserved=%v, err=%v", calls, tc.wantCalls, string(body) == tc.message, err)
			}
		})
	}
}

func TestAntigravityToolsCanceledRecovery(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	calls := 0
	client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
		calls++
		_ = r.Body.Close()
		cancel()
		return &http.Response{StatusCode: 400, Body: io.NopCloser(strings.NewReader("Invalid thought signature"))}, nil
	})}
	req, _ := http.NewRequestWithContext(ctx, "POST", "https://upstream.invalid", strings.NewReader(toolsHistory))
	_, _, err := DoAntigravityToolsRequest(client, req, "claude", "claude-opus-4-6-thinking")
	if !errors.Is(err, context.Canceled) || calls != 1 {
		t.Fatalf("calls=%d err=%v", calls, err)
	}
}

type toolsUnreadBody struct{ reads int }

func (b *toolsUnreadBody) Read([]byte) (int, error) { b.reads++; return 0, io.EOF }
func (b *toolsUnreadBody) Close() error             { return nil }

func TestAntigravityToolsDoesNotReadSuccessfulStream(t *testing.T) {
	body := &toolsUnreadBody{}
	client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
		_ = r.Body.Close()
		return &http.Response{StatusCode: 200, Body: body}, nil
	})}
	req, _ := http.NewRequest("POST", "https://upstream.invalid", strings.NewReader(toolsHistory))
	resp, recovered, err := DoAntigravityToolsRequest(client, req, "gemini", "gemini-3.7-flash")
	if err != nil || resp.Body != body || body.reads != 0 || len(recovered) != 0 {
		t.Fatal("buffered or changed successful stream")
	}
}

func TestAntigravityToolsSessionResetPreservesHistory(t *testing.T) {
	calls := 0
	client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
		calls++
		payload, _ := io.ReadAll(r.Body)
		_ = r.Body.Close()
		if calls == 2 {
			if gjson.GetBytes(payload, "request.sessionId").String() == "same-session" {
				t.Error("session not reset")
			}
			if gjson.GetBytes(payload, "request.contents").Raw != gjson.Get(toolsHistory, "request.contents").Raw {
				t.Error("history changed")
			}
		}
		return &http.Response{StatusCode: 400, Body: io.NopCloser(strings.NewReader("exceeds the maximum number of tokens"))}, nil
	})}
	req, _ := http.NewRequest("POST", "https://upstream.invalid", strings.NewReader(toolsHistory))
	resp, _, err := DoAntigravityToolsRequest(client, req, "gemini", "gemini-3.7-flash")
	if err != nil || calls != 2 {
		t.Fatalf("calls=%d err=%v", calls, err)
	}
	_ = resp.Body.Close()
}

func TestAntigravityToolsUnrepairableHistory(t *testing.T) {
	for _, payload := range []string{`{"request":{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}}`, `{broken`} {
		calls := 0
		client := &http.Client{Transport: toolsRoundTrip(func(r *http.Request) (*http.Response, error) {
			calls++
			_ = r.Body.Close()
			return &http.Response{StatusCode: 400, Body: io.NopCloser(strings.NewReader("Invalid thought signature"))}, nil
		})}
		req, _ := http.NewRequest("POST", "https://upstream.invalid", bytes.NewBufferString(payload))
		resp, recovered, err := DoAntigravityToolsRequest(client, req, "claude", "claude-opus-4-6-thinking")
		if err != nil || calls != 1 || len(recovered) != 0 {
			t.Fatalf("calls=%d err=%v", calls, err)
		}
		_ = resp.Body.Close()
	}
}
