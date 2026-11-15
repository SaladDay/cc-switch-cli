# Claude default-model 1M control

The TUI follows [cc-switch PR #5124](https://github.com/farion1231/cc-switch/pull/5124)
(commit `3538b392462ed22890a44dc8c55df0f1066ee119`): the default model stored in
`ANTHROPIC_MODEL` has its own opt-in 1M declaration. Role declarations do not
implicitly enable it. Editing the model preserves the declaration; an empty
model cannot enable it. The existing case-insensitive marker parser and `[1M]`
write convention are retained.

The CLI presents this as a standard checkbox row immediately after the default
model, with Enter/Space to toggle and `?` for contextual help. No backend routing,
global context-window override or automatic role-to-default propagation is added.
Existing records remain untouched until the user edits the default model or its
1M setting. The help explains that direct Claude Code sessions use
`ANTHROPIC_MODEL` ahead of role aliases.
