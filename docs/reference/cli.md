# CLI Reference

This page is a compact map of current CLI areas. For detailed Skills lifecycle commands, use [skill-lifecycle.md](skill-lifecycle.md).

Useful Skills commands include:

```bash
synapseclaw skills create
synapseclaw skills authored
synapseclaw skills learned
synapseclaw skills candidates
synapseclaw skills diff <candidate-id>
synapseclaw skills test <candidate-id>
synapseclaw skills apply <candidate-id>
synapseclaw skills versions <skill-id-or-name>
synapseclaw skills rollback <apply-record-or-ref>
synapseclaw skills health
synapseclaw skills autopromote
synapseclaw skills export <id-or-name>
synapseclaw skills scaffold <name>
```

Other command areas are still evolving and should be checked against `synapseclaw --help` in the current build.

Current voice/runtime inspection commands include:

```bash
synapseclaw voice status
synapseclaw voice doctor
synapseclaw voice mode status --json
synapseclaw voice mode on --session local-voice
synapseclaw voice mode turn --file /tmp/inbound.ogg --json
synapseclaw voice profiles --json
synapseclaw voice voices --json
synapseclaw voice call status --json
synapseclaw voice call sessions --json
synapseclaw voice call start --channel clawdtalk --to +15551234567 --confirm
synapseclaw voice call answer --channel clawdtalk --call-control-id call_123 --confirm
synapseclaw voice call speak --channel clawdtalk --call-control-id call_123 --text "Hello." --confirm
synapseclaw voice call hangup --channel clawdtalk --call-control-id call_123 --confirm
```
