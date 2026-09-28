package registry

const (
	AntigravityOpusReasoningModel     = "[思考]claude-opus-4-6"
	AntigravityOpusReasoningFullModel = "[思考]claude-opus-4-6-thinking"
	AntigravityOpusUpstreamModel      = "claude-opus-4-6-thinking"
)

// WithAntigravityReasoningAlias derives the public route from the current
// upstream definition, including refreshed capabilities and output limits.
// A missing upstream removes the alias rather than advertising an unusable route.
func WithAntigravityReasoningAlias(models []*ModelInfo) []*ModelInfo {
	out := make([]*ModelInfo, 0, len(models)+2)
	var upstream *ModelInfo
	for _, model := range models {
		if model == nil || IsAntigravityReasoningAlias(model.ID) {
			continue
		}
		out = append(out, model)
		if model.ID == AntigravityOpusUpstreamModel {
			upstream = model
		}
	}
	if upstream != nil {
		for _, name := range []string{AntigravityOpusReasoningModel, AntigravityOpusReasoningFullModel} {
			alias := cloneModelInfo(upstream)
			alias.ID = name
			alias.Name = name
			alias.DisplayName = "Claude Opus 4.6 (Visible Thinking)"
			alias.Description = "CPA alias that enables thinking and returns upstream thinking content."
			out = append(out, alias)
		}
	}
	return out
}

func IsAntigravityReasoningAlias(model string) bool {
	return model == AntigravityOpusReasoningModel || model == AntigravityOpusReasoningFullModel
}
