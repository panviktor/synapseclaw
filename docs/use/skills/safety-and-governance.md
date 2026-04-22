# Safety And Governance

Skills affect future agent behavior, so they need safety boundaries. A skill should make repeatable work easier, not bypass permissions or hide risky instructions.

## Audit

User-authored and package skills should pass audit before they become usable. Audit should reject obvious prompt-injection patterns, credential exfiltration instructions, unsafe tool claims, and secret material.

## Candidate Review

Generated material starts as a candidate. It should become active only after review, successful checks, and an explicit lifecycle transition.

Generated patches need extra care because they modify existing skills. Apply requires the current target version to match the patch target, which prevents stale patches from overwriting newer work.

## Auto-Promotion

Auto-promotion is not a hidden background rewrite. It is policy-gated and should write only when `[skills.auto_promotion].enabled=true` and the candidate passes eval, provenance, target version, and live trace checks.

## Secrets

Do not store tokens, passwords, OAuth callbacks, temporary device codes, private customer records, or bearer credentials in skill bodies. A skill should describe how to do work, not contain secret data required to do it.

## Imported Packages

Imported and open-skills packages are not the same as local memory-owned skills. They can be indexed and activated, but package ownership and mutation rules should remain explicit.

