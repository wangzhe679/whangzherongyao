This Linux amd64 build fixes thinking-budget handling in the custom12 prefixed Claude Opus 4.6 route.

- `[思考]claude-opus-4-6` and `[思考]claude-opus-4-6-thinking` still map to upstream `claude-opus-4-6-thinking`. They now preserve an explicit positive Claude `thinking.budget_tokens`, within model limits, instead of unconditionally replacing it with automatic thinking (`-1`). Missing, disabled or adaptive requests without a positive budget use 16384 thinking tokens and `includeThoughts: true`.
- The native Claude budget is recovered from the source request so earlier translation cannot silently shrink it. If the output ceiling is too small, it is increased to the selected thinking budget plus 8192, capped at the model limit. Existing sufficient output ceilings are retained.
- Both unprefixed names retain their existing answer-only behavior. New API configuration, account storage and cooldown data are unchanged.

Validation covers all four names in streaming and non-streaming modes with explicit, missing, disabled, adaptive and small-output requests against a local mock upstream. Tests verify the actual outgoing model and thinking parameters, visible thinking only on prefixed routes, and preservation of text and tool calls. The focused Antigravity/signature regression suite and server build are also checked. No live model request or production deployment is performed; this fixes a confirmed parameter-handling defect but does not establish that upstream accounts always return thinking text or resolve every 8192-token truncation cause.

Replace only the executable when upgrading. The archive contains no user configuration, account credentials or cooldown database.
