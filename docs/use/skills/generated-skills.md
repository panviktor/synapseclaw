# Generated Skills

Synapseclaw can propose skills from repeated useful patterns. Generated material should be treated as a candidate until it passes review, replay/eval gates, and policy checks.

## Generated New Skills

A generated new skill is a proposed procedure that does not yet replace an existing skill. It should remain a candidate until an operator reviews whether it is specific, safe, and useful enough to become active.

## Generated Patch Candidates

A generated patch candidate improves an existing skill. The patch must match the current target version, carry enough provenance, and pass replay/eval checks before it can be applied.

## Auto-Promotion

Auto-promotion is policy-gated. It should apply only eligible generated patches when `[skills.auto_promotion].enabled=true` and the candidate passes evaluation, provenance, version, and live trace checks.

