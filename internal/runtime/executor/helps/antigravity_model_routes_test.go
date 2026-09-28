package helps

import (
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	_ "github.com/router-for-me/CLIProxyAPI/v7/internal/thinking/provider/antigravity"
	"github.com/tidwall/gjson"
)

func TestAntigravityPrefixedRouteEnablesThinkingAfterOverrides(t *testing.T) {
	for _, payload := range []string{
		`{"request":{"generationConfig":{"maxOutputTokens":32768}}}`,
		`{"request":{"generationConfig":{"maxOutputTokens":32768,"thinkingConfig":{"thinkingBudget":0,"includeThoughts":false}}}}`,
		`{"request":{"generationConfig":{"maxOutputTokens":32768,"thinkingConfig":{"thinkingBudget":2048,"includeThoughts":false}}}}`,
	} {
		got, err := AntigravityModelRequest("claude", registry.AntigravityOpusReasoningModel, []byte(payload))
		if err != nil {
			t.Fatal(err)
		}
		if gjson.GetBytes(got, "request.generationConfig.thinkingConfig.thinkingBudget").Int() != -1 || !gjson.GetBytes(got, "request.generationConfig.thinkingConfig.includeThoughts").Bool() {
			t.Fatalf("alias did not enable visible thinking: %s", got)
		}
		if gjson.GetBytes(got, "request.generationConfig.maxOutputTokens").Int() != 32768 {
			t.Fatal("output budget changed")
		}
	}
}
