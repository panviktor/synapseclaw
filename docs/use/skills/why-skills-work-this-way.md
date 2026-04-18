# Why Skills Work This Way

Skills are designed to make repeated work reusable without giving the model an ever-growing pile of instructions. The system should improve capability while keeping context small, auditable, and reversible.

## Avoid Prompt Bloat

Inlining every skill body would be easy, but it would make each request larger and noisier. Synapseclaw uses compact cards and `skill_read` so only relevant instructions are loaded.

## Make Self-Improvement Reviewable

Generated skills and generated patches can be useful, but they should not silently rewrite the agent's operating procedures. Candidate review, replay/eval gates, and policy-gated apply keep the loop controlled.

## Keep A Version Trail

Every meaningful update should be reversible. Patch apply and manual updates save version records and rollback snapshots so the operator can undo a bad change without losing the audit trail.

## Measure Utility

A skill that exists but never helps is maintenance cost. Health counters and traces let operators see what was selected, read, helpful, failed, repaired, blocked, or rolled back.

## Separate Human, Generated, And Imported Sources

Human-authored skills, generated candidates, generated patches, and imported packages have different trust levels. Treating them as separate lanes makes policy and review clearer.

