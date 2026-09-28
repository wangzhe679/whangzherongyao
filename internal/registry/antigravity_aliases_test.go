package registry

import "testing"

func TestAntigravityReasoningAliasesFollowUpstream(t *testing.T) {
	upstream := &ModelInfo{ID: AntigravityOpusUpstreamModel, MaxCompletionTokens: 32768, Thinking: &ThinkingSupport{Min: 1024, Max: 32768, DynamicAllowed: true}}
	models := WithAntigravityReasoningAlias([]*ModelInfo{upstream})
	if len(models) != 3 {
		t.Fatalf("expected upstream and two aliases, got %d", len(models))
	}
	for _, alias := range models[1:] {
		if !IsAntigravityReasoningAlias(alias.ID) || alias.MaxCompletionTokens != 32768 || alias.Thinking == nil || !alias.Thinking.DynamicAllowed {
			t.Fatalf("alias lost capabilities: %+v", alias)
		}
		alias.Thinking.Max = 1
	}
	if upstream.Thinking.Max != 32768 {
		t.Fatal("alias mutated upstream capabilities")
	}
	if len(WithAntigravityReasoningAlias(models)) != 3 {
		t.Fatal("duplicated aliases")
	}
	if len(WithAntigravityReasoningAlias(models[1:])) != 0 {
		t.Fatal("advertised aliases without upstream")
	}
	for _, name := range []string{AntigravityOpusReasoningModel, AntigravityOpusReasoningFullModel} {
		if LookupStaticModelInfo(name) == nil || LookupStaticModelInfoByChannel(name, "antigravity") == nil {
			t.Fatalf("alias not discoverable: %s", name)
		}
	}
}
