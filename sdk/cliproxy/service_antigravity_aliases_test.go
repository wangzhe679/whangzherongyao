package cliproxy

import (
	"context"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	"github.com/router-for-me/CLIProxyAPI/v7/sdk/config"
)

func TestAntigravityReasoningAliasRegistrationAndExclusions(t *testing.T) {
	for _, tc := range []struct {
		excluded    string
		short, full bool
	}{
		{"", true, true},
		{registry.AntigravityOpusUpstreamModel, false, false},
		{registry.AntigravityOpusReasoningModel, false, true},
	} {
		t.Run(tc.excluded, func(t *testing.T) {
			s := &Service{cfg: &config.Config{}}
			a := &coreauth.Auth{ID: t.Name(), Provider: "antigravity", Attributes: map[string]string{"auth_kind": "oauth", "excluded_models": tc.excluded}}
			defer GlobalModelRegistry().UnregisterClient(a.ID)
			s.registerModelsForAuth(context.Background(), a)
			s.antigravityProbeWg.Wait()
			found := map[string]bool{}
			for _, m := range GlobalModelRegistry().GetModelsForClient(a.ID) {
				found[m.ID] = true
			}
			if found[registry.AntigravityOpusReasoningModel] != tc.short || found[registry.AntigravityOpusReasoningFullModel] != tc.full {
				t.Fatalf("incorrect alias registration: %v", found)
			}
		})
	}
}
