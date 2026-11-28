# Vendored pstack skills

These skills are copied verbatim from the pstack Cursor plugin so this repo
loads them as project skills without the plugin being installed.

- Source: cursor-public marketplace plugin `pstack`, plugin id `9717366`
- Commit: `7a583913eb1b8d1a0ee76753d1a2cf072e7b54f9`
- Plugin version: 0.15.46 (`plugin.json` in that checkout)
- Copied: 2026-11-28, all 58 skill directories, nothing else

To refresh, copy `skills/` from a newer plugin checkout over this directory,
leaving `bugfix-regression-tests/` (this repo's own skill) in place, and update
the commit and version above.

pstack is MIT licensed, copyright Lauren Tan. See the plugin's `LICENSE`.

The per-role model choices live outside this repo in
`~/.cursor/rules/pstack-models.mdc`, which `/setup-pstack` writes.
