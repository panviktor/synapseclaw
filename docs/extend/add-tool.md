# Add A Tool

Every runtime tool must expose an explicit typed `tool_contract()`. Do not use deprecated `x-synapse-*` schema extensions, implicit fallback behavior, or provider-facing metadata as the source of runtime safety.

A new tool should define its role, privacy class, replay policy, and argument policy in the contract. Replay is allowed only when the contract declares safe arguments that can be sanitized.

