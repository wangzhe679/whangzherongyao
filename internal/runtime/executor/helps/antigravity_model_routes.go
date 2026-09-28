package helps

import (
	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/thinking"
)

// AntigravityUpstreamModel resolves the built-in public alias without sending
// its private name to Google. The caller retains the original model for policy.
func AntigravityUpstreamModel(model string) string {
	base := thinking.ParseSuffix(model).ModelName
	if registry.IsAntigravityReasoningAlias(base) || base == "claude-opus-4-6" {
		return registry.AntigravityOpusUpstreamModel
	}
	return base
}

func AntigravityVisibleThinkingModel(model string) bool {
	return registry.IsAntigravityReasoningAlias(thinking.ParseSuffix(model).ModelName)
}

// AntigravityModelRequest applies public-route semantics after payload rules.
// The reasoning alias opts into automatic thinking and visible summaries;
// existing model names retain the answer-only behavior of custom11.
func AntigravityModelRequest(source, model string, payload []byte) ([]byte, error) {
	if !AntigravityVisibleThinkingModel(model) {
		return AntigravityAnswerOnlyRequest(source, AntigravityUpstreamModel(model), payload)
	}
	upstream := AntigravityUpstreamModel(model)
	info := registry.LookupModelInfo(upstream, "antigravity")
	return thinking.ApplyThinkingWithModelInfoAndSummary(
		payload, nil, upstream+"(auto)", "antigravity", "antigravity", "antigravity", info,
		thinking.SummaryConfig{Mode: thinking.SummaryEnabled, Detail: "auto"},
	)
}
