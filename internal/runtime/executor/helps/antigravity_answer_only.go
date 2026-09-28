package helps

import (
	"bytes"
	"context"
	"strconv"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/thinking"
	cliproxyexecutor "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/executor"
	"github.com/tidwall/gjson"
	"github.com/tidwall/sjson"
)

// AntigravityAnswerOnlyRequest disables reasoning where the registered model
// permits it. Otherwise it changes visibility only, never the selected model.
// Historical signatures are deliberately retained for tool replay.
func AntigravityAnswerOnlyRequest(source, model string, payload []byte) ([]byte, error) {
	if !antigravityToolsRoute(source, model) {
		return payload, nil
	}
	info := registry.GetGlobalRegistry().GetModelInfo(model, "antigravity")
	if info == nil {
		info = registry.LookupStaticModelInfoByChannel(model, "antigravity")
	}
	if info == nil || info.Thinking == nil {
		return payload, nil
	}
	if info.Thinking.ZeroAllowed {
		return thinking.ApplyThinkingWithModelInfoAndSummary(payload, nil, model+"(none)", "antigravity", "antigravity", "antigravity", info, thinking.SummaryConfig{Mode: thinking.SummaryDisabled})
	}
	// No capability evidence means no forced zero budget: that could cause 400.
	return sjson.SetBytes(payload, "request.generationConfig.thinkingConfig.includeThoughts", false)
}

// AntigravityAnswerOnlyResponse filters after translation, so existing signature
// caches still see the complete upstream response. Usage is not rewritten.
func AntigravityAnswerOnlyResponse(format string, payload []byte) []byte {
	if !gjson.ValidBytes(payload) {
		return payload
	}
	if format == "claude" {
		return filterAnswerParts(payload, "content", func(part gjson.Result) bool {
			kind := part.Get("type").String()
			return kind != "thinking" && kind != "redacted_thinking"
		})
	}
	if format == "gemini" {
		candidates := gjson.GetBytes(payload, "candidates").Array()
		for i := range candidates {
			path := "candidates." + strconv.Itoa(i) + ".content.parts"
			payload = filterAnswerParts(payload, path, func(part gjson.Result) bool { return !part.Get("thought").Bool() })
		}
	}
	return payload
}

func filterAnswerParts(payload []byte, path string, keep func(gjson.Result) bool) []byte {
	parts := gjson.GetBytes(payload, path)
	if !parts.IsArray() {
		return payload
	}
	var out bytes.Buffer
	out.WriteByte('[')
	count, changed := 0, false
	for _, part := range parts.Array() {
		if !keep(part) {
			changed = true
			continue
		}
		if count > 0 {
			out.WriteByte(',')
		}
		out.WriteString(part.Raw)
		count++
	}
	if !changed {
		return payload
	}
	out.WriteByte(']')
	result, err := sjson.SetRawBytes(payload, path, out.Bytes())
	if err != nil {
		return payload
	}
	return result
}

type antigravityAnswerStream struct {
	format    string
	indexes   map[int]int
	nextIndex int
}

func (s *antigravityAnswerStream) filter(payload []byte) []byte {
	if s.format == "gemini" {
		return AntigravityAnswerOnlyResponse("gemini", payload)
	}
	var out bytes.Buffer
	for _, frame := range bytes.Split(payload, []byte("\n\n")) {
		if len(bytes.TrimSpace(frame)) == 0 {
			continue
		}
		start := bytes.Index(frame, []byte("data: "))
		if start < 0 {
			out.Write(frame)
			out.WriteString("\n\n")
			continue
		}
		start += len("data: ")
		end := bytes.IndexByte(frame[start:], '\n')
		if end < 0 {
			end = len(frame)
		} else {
			end += start
		}
		data := frame[start:end]
		if !gjson.ValidBytes(data) {
			out.Write(frame)
			out.WriteString("\n\n")
			continue
		}
		kind := gjson.GetBytes(data, "type").String()
		index := int(gjson.GetBytes(data, "index").Int())
		if kind == "content_block_start" {
			block := gjson.GetBytes(data, "content_block.type").String()
			if s.indexes == nil {
				s.indexes = make(map[int]int)
			}
			if block == "thinking" || block == "redacted_thinking" {
				s.indexes[index] = -1
				continue
			}
			s.indexes[index] = s.nextIndex
			s.nextIndex++
		}
		if kind == "content_block_start" || kind == "content_block_delta" || kind == "content_block_stop" {
			mapped, exists := s.indexes[index]
			if exists && mapped < 0 {
				continue
			}
			if exists {
				data, _ = sjson.SetBytes(data, "index", mapped)
			}
		}
		out.Write(frame[:start])
		out.Write(data)
		out.Write(frame[end:])
		out.WriteString("\n\n")
	}
	return out.Bytes()
}

// AntigravityAnswerOnlyStream forwards each visible chunk immediately. Heartbeats
// keep an accepted HTTP stream alive while an upstream that cannot disable
// reasoning is silent. They never fabricate completion or impose a read timeout.
func AntigravityAnswerOnlyStream(ctx context.Context, source, model, format string, input <-chan cliproxyexecutor.StreamChunk) <-chan cliproxyexecutor.StreamChunk {
	if !antigravityToolsRoute(source, model) || (format != "claude" && format != "gemini") {
		return input
	}
	out := make(chan cliproxyexecutor.StreamChunk)
	go func() {
		defer close(out)
		ticker := time.NewTicker(15 * time.Second)
		defer ticker.Stop()
		forwardAntigravityAnswers(ctx, format, input, out, ticker.C)
	}()
	return out
}

func forwardAntigravityAnswers(ctx context.Context, format string, input <-chan cliproxyexecutor.StreamChunk, out chan<- cliproxyexecutor.StreamChunk, ticks <-chan time.Time) {
	filter := antigravityAnswerStream{format: format}
	send := func(chunk cliproxyexecutor.StreamChunk) bool {
		select {
		case out <- chunk:
			return true
		case <-ctx.Done():
			return false
		}
	}
	for {
		select {
		case <-ctx.Done():
			return
		case chunk, ok := <-input:
			if !ok {
				return
			}
			if chunk.Err != nil {
				send(chunk)
				return
			}
			chunk.Payload = filter.filter(chunk.Payload)
			if len(chunk.Payload) > 0 && !send(chunk) {
				return
			}
		case <-ticks:
			payload := []byte("event: ping\ndata: {\"type\":\"ping\"}\n\n")
			if format == "gemini" {
				payload = []byte(`{"candidates":[]}`)
			}
			if !send(cliproxyexecutor.StreamChunk{Payload: payload}) {
				return
			}
		}
	}
}
