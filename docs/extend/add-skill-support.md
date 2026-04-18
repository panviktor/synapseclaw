# Add Skill Support

Skills are stored, indexed, audited, selected, activated, measured, and versioned through the runtime. New skill behavior should reuse the existing lifecycle instead of creating a separate web-only or channel-only path.

The correct loading model is compact catalog, then `skill_read`, then compact activation receipt. Do not inline full skill bodies directly into provider context as a shortcut.

