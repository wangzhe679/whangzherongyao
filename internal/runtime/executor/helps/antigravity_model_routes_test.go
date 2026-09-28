package helps

import (
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	_ "github.com/router-for-me/CLIProxyAPI/v7/internal/thinking/provider/antigravity"
	"github.com/tidwall/gjson"
)

func TestAntigravityPrefixedRouteEnablesThinkingAfterOverrides(t *testing.T) {
	for _, tt := range []struct {
		payload string
		budget  int64
		output  int64
	}{
		{`{"request":{"generationConfig":{"maxOutputTokens":32768}}}`, 16384, 32768},
		{`{"request":{"generationConfig":{"maxOutputTokens":32768,"thinkingConfig":{"thinkingBudget":0,"includeThoughts":false}}}}`, 16384, 32768},
		{`{"request":{"generationConfig":{"maxOutputTokens":32768,"thinkingConfig":{"thinkingBudget":2048,"includeThoughts":false}}}}`, 2048, 32768},
		{`{"request":{"generationConfig":{"maxOutputTokens":8192,"thinkingConfig":{"thinkingBudget":-1}}}}`, 16384, 24576},
		{`{"request":{"generationConfig":{"maxOutputTokens":1024,"thinkingConfig":{"thinkingBudget":8192}}}}`, 8192, 16384},
		{`{"request":{"generationConfig":{"maxOutputTokens":128000,"thinkingConfig":{"thinkingBudget":128000}}}}`, 63999, 64000},
		{`{"request":{"generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH"}}}}`, 16384, 24576},
	} {
		got, err := AntigravityModelRequest("claude", registry.AntigravityOpusReasoningModel, []byte(tt.payload), nil)
		if err != nil {
			t.Fatal(err)
		}
		if gjson.GetBytes(got, "request.generationConfig.thinkingConfig.thinkingBudget").Int() != tt.budget || !gjson.GetBytes(got, "request.generationConfig.thinkingConfig.includeThoughts").Bool() {
			t.Fatalf("alias did not enable visible thinking: %s", got)
		}
		if gjson.GetBytes(got, "request.generationConfig.maxOutputTokens").Int() != tt.output {
			t.Fatalf("incorrect output ceiling: %s", got)
		}
		if gjson.GetBytes(got, "request.generationConfig.thinkingConfig.thinkingLevel").Exists() {
			t.Fatalf("conflicting thinking level retained: %s", got)
		}
	}
}
