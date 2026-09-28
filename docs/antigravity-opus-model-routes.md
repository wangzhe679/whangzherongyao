# Claude Opus 4.6 public model routes

The custom build accepts these names through the Claude Messages API:

| Model received by CPA | Upstream model | Response |
| --- | --- | --- |
| `claude-opus-4-6` | `claude-opus-4-6-thinking` | Existing answer-only behavior |
| `claude-opus-4-6-thinking` | `claude-opus-4-6-thinking` | Existing answer-only behavior |
| `[思考]claude-opus-4-6` | `claude-opus-4-6-thinking` | Automatic thinking with upstream thinking content preserved |
| `[思考]claude-opus-4-6-thinking` | `claude-opus-4-6-thinking` | Automatic thinking with upstream thinking content preserved |

Both prefixed names are advertised by the Antigravity model registry. They derive
capabilities from the upstream model after catalog refresh. Excluding the upstream
also excludes these aliases. Both spellings are accepted because a gateway may
remove the `-thinking` suffix before forwarding the request.

The prefixed routes explicitly request automatic thinking and visible thoughts;
they take precedence over a request's disabled thinking or hidden-summary setting.
They do not fabricate reasoning content: the upstream must return it. Existing
unprefixed routes retain the custom11 answer-only policy. Output budgets and
model output limits still apply independently.

No New API configuration is changed by this patch. New API must already permit
the prefixed model, or its channel/model allowlist must be updated separately.
CPA cannot distinguish requests if a gateway removes the prefix as well.

The change covers streaming, non-streaming, and upstream model resolution for
token counting. Local tests use a mock upstream; live Google behavior is not
validated by those tests.
