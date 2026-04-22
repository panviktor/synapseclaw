# Tool Contracts

Tool contracts describe runtime safety, not just documentation. They tell the system what role a tool plays, whether its arguments are private, and whether a call can be replayed for skill evaluation.

Use `ToolProtocolContract`, `ToolPrivacyClass`, replay policy, and argument policy to make those rules explicit. Private, session, memory, or project-like tools should remain excluded from replay until typed privacy classification exists.

