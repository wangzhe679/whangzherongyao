package helps

import (
	"bytes"
	"encoding/binary"
	"io"
	"net/http"
	"strconv"
	"strings"

	"github.com/google/uuid"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/signature"
	"github.com/tidwall/gjson"
	"github.com/tidwall/sjson"
)

func antigravityToolsRoute(source, model string) bool {
	return source == "claude" && strings.HasPrefix(model, "claude-") ||
		source == "gemini" && strings.HasPrefix(model, "gemini-")
}

// DoAntigravityToolsRequest repairs a recognized pre-stream 400 once, on the same
// credential. It does not buffer successful responses or keep account/session
// state. The returned error body identifies a recovery attempt to the caller,
// which must invalidate its old reasoning replay snapshot before caching again.
func DoAntigravityToolsRequest(client *http.Client, req *http.Request, source, model string) (*http.Response, []byte, error) {
	resp, err := client.Do(req)
	if err != nil || resp.StatusCode != http.StatusBadRequest || req.GetBody == nil || !antigravityToolsRoute(source, model) {
		return resp, nil, err
	}
	// Inspect a bounded prefix. Preserve the original body (and any read error)
	// when no repair is possible, so normal error handling still sees it.
	const maxErrorBytes = 64 * 1024
	prefix, errRead := io.ReadAll(io.LimitReader(resp.Body, maxErrorBytes+1))
	resp.Body = &antigravityPrefixBody{Reader: io.MultiReader(bytes.NewReader(prefix), resp.Body), Closer: resp.Body, readErr: errRead}
	if errRead != nil || len(prefix) > maxErrorBytes {
		return resp, nil, nil
	}
	message := strings.ToLower(gjson.GetBytes(prefix, "error.message").String())
	if message == "" {
		message = strings.ToLower(string(prefix))
	}
	isSignature := antigravityToolsSignatureError(message)
	isSession := source == "gemini" && strings.Contains(message, "exceeds the maximum number of tokens")
	if !isSignature && !isSession {
		return resp, nil, nil
	}
	body, errBody := req.GetBody()
	if errBody != nil {
		return resp, nil, nil
	}
	payload, errPayload := io.ReadAll(body)
	_ = body.Close()
	if errPayload != nil || !gjson.ValidBytes(payload) {
		return resp, nil, nil
	}
	var repaired []byte
	if isSignature {
		repaired = antigravityRepairThoughtHistory(payload, source)
	} else {
		// Tools resets the upstream session when accumulated session context is
		// rejected. The messages are retained; a genuinely oversized request
		// will still fail on the single retry.
		id := uuid.New()
		sessionID := "-" + strconv.FormatUint(binary.BigEndian.Uint64(id[:8])&0x7FFFFFFFFFFFFFFF, 10)
		repaired, _ = sjson.SetBytes(payload, "request.sessionId", sessionID)
	}
	if bytes.Equal(payload, repaired) {
		return resp, nil, nil
	}
	_ = resp.Body.Close()
	if errContext := req.Context().Err(); errContext != nil {
		return nil, prefix, errContext
	}
	retry := req.Clone(req.Context())
	retry.Body = io.NopCloser(bytes.NewReader(repaired))
	retry.ContentLength = int64(len(repaired))
	retry.GetBody = func() (io.ReadCloser, error) { return io.NopCloser(bytes.NewReader(repaired)), nil }
	resp, err = client.Do(retry)
	return resp, prefix, err
}

type antigravityPrefixBody struct {
	io.Reader
	io.Closer
	readErr error
}

func (b *antigravityPrefixBody) Read(p []byte) (int, error) {
	n, err := b.Reader.Read(p)
	if err == io.EOF && b.readErr != nil {
		return n, b.readErr
	}
	return n, err
}

func antigravityToolsSignatureError(message string) bool {
	for _, cue := range []string{
		"invalid thought signature", "invalid `signature`", "invalid signature",
		"thought_signature", "thoughtsignature", "thinking.signature",
		"thinking.thinking", "corrupted thought signature", "thinking block",
		"must be `thinking`", "must be 'thinking'",
	} {
		if strings.Contains(message, cue) {
			return true
		}
	}
	return false
}

// Historical reasoning is retained as text, as in Tools' Claude 400 fallback.
// The current generation's thinking configuration and all tool arguments/results
// stay intact. Gemini function calls use CPA's existing compatibility policy;
// a Gemini bypass signature is never attached to a Claude request.
func antigravityRepairThoughtHistory(payload []byte, source string) []byte {
	contents := gjson.GetBytes(payload, "request.contents")
	if !contents.IsArray() {
		return payload
	}
	changed := false
	var out bytes.Buffer
	out.WriteByte('[')
	for i, content := range contents.Array() {
		if i > 0 {
			out.WriteByte(',')
		}
		if content.Get("role").String() != "model" || !content.Get("parts").IsArray() {
			out.WriteString(content.Raw)
			continue
		}
		var parts bytes.Buffer
		parts.WriteByte('[')
		partCount := 0
		for _, part := range content.Get("parts").Array() {
			p := []byte(part.Raw)
			for _, key := range []string{"thought", "thoughtSignature", "thought_signature"} {
				if part.Get(key).Exists() {
					p, _ = sjson.DeleteBytes(p, key)
					changed = true
				}
			}
			if string(bytes.TrimSpace(p)) == "{}" {
				continue
			}
			if partCount > 0 {
				parts.WriteByte(',')
			}
			parts.Write(p)
			partCount++
		}
		if partCount == 0 {
			// Preserve turn boundaries when the only part was a signature.
			parts.WriteString(`{"text":""}`)
		}
		parts.WriteByte(']')
		updated, _ := sjson.SetRaw(content.Raw, "parts", parts.String())
		out.WriteString(updated)
	}
	out.WriteByte(']')
	if !changed {
		return payload
	}
	updated, _ := sjson.SetRawBytes(payload, "request.contents", out.Bytes())
	if source == "gemini" {
		updated = signature.SanitizeGeminiRequestThoughtSignatures(updated, "request.contents")
	}
	return updated
}
