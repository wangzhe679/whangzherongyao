package helps

import (
	"strconv"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/thinking"
	"github.com/tidwall/gjson"
	"github.com/tidwall/sjson"
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
// The reasoning alias opts into budgeted thinking and visible summaries;
// existing model names retain the answer-only behavior of custom11.
func AntigravityModelRequest(source, model string, payload, sourcePayload []byte) ([]byte, error) {
	if !AntigravityVisibleThinkingModel(model) {
		return AntigravityAnswerOnlyRequest(source, AntigravityUpstreamModel(model), payload)
	}
	upstream := AntigravityUpstreamModel(model)
	info := registry.LookupModelInfo(upstream, "antigravity")
	// Native Claude intent must be read before translation: an adaptive request
	// may already have become a positive budget, or a small output ceiling may
	// have reduced an explicit budget. Other formats use their effective budget.
	budget := gjson.GetBytes(payload, "request.generationConfig.thinkingConfig.thinkingBudget").Int()
	if source == "claude" && len(sourcePayload) > 0 {
		budget = gjson.GetBytes(sourcePayload, "thinking.budget_tokens").Int()
	}
	// The model name opts into thinking even when the client omits or disables it.
	// Missing, disabled and automatic budgets use the Tools Claude default.
	if budget <= 0 {
		budget = 16384
	}
	maxOutput := int64(64000)
	if info != nil {
		if info.MaxCompletionTokens > 0 {
			maxOutput = int64(info.MaxCompletionTokens)
		}
		if info.Thinking != nil {
			if info.Thinking.Min > 0 && budget < int64(info.Thinking.Min) {
				budget = int64(info.Thinking.Min)
			}
			if info.Thinking.Max > 0 && budget > int64(info.Thinking.Max) {
				budget = int64(info.Thinking.Max)
			}
		}
	}
	if budget >= maxOutput {
		budget = maxOutput - 1
	}
	// Give the provider a valid output ceiling before its budget normalization;
	// otherwise a small max_tokens can reduce or remove thinking entirely.
	output := gjson.GetBytes(payload, "request.generationConfig.maxOutputTokens").Int()
	if output <= budget {
		output = budget + 8192
	}
	if output > maxOutput {
		output = maxOutput
	}
	var err error
	payload, err = sjson.SetBytes(payload, "request.generationConfig.maxOutputTokens", output)
	if err != nil {
		return nil, err
	}
	return thinking.ApplyThinkingWithModelInfoAndSummary(
		payload, nil, upstream+"("+strconv.FormatInt(budget, 10)+")", "antigravity", "antigravity", "antigravity", info,
		thinking.SummaryConfig{Mode: thinking.SummaryEnabled, Detail: "auto"},
	)
}
