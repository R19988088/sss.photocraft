# vendor/craft-ui

This directory is craft-libs' `crates/ui` (the `craft-ui` crate), pinned by `Cargo.toml`'s
`[patch."https://github.com/storytold/craft-libs"]` to a path inside this repository so PhotoCraft
can carry one patch of its own.

* Upstream revision: `1b34eba41e62138fc2438d2734523b302840b45d`
  (`https://github.com/storytold/craft-libs`), the same revision `workspace.dependencies` pins for
  `craft-print`, `craft-pdf`, `craft-docpath`, `craft-jpeg` and `craft-update`.
* Every file is byte-identical to that revision except:
  * `Cargo.toml` — workspace-inherited keys inlined (`workspace = true` becomes the literal
    value), the crate declared its own workspace by the root's
    `exclude = ["vendor/craft-ui"]`, `tests/` dropped with its dev-dependencies and `[[test]]`.
  * `src/docking/surface.rs` — `DockContent::tab_menu_tail`, a hook defaulting to `true`, and the
    `Overrides` wrapper in front of an application's content forwards it (that wrapper forwards a
    named list of methods, so a hook it does not forward silently falls back to its default).
  * `src/docking/render/group.rs` — a tab's context menu stops after the host's `panel_menu`
    items when `tab_menu_tail` returns `false`.

PhotoCraft uses it for the Color panel: a tab drag floats a panel and the Window menu closes
it, so the stock Float / Move to group / Close / group rows repeated those gestures in the
menu ("右键菜单出现这个是弱智行为", 2026-11-14).

## Following an upstream craft-libs revision

1. `robocopy <new checkout>\crates\ui vendor\craft-ui /E /XD tests` over this directory.
2. Re-apply the two source edits above (`Cargo.toml` needs the same inlining pass: workspace
   keys, `exclude`, no `tests/`, no dev-dependencies).
3. Bump the revision in `vendor/craft-ui/Cargo.toml` (the two `craft-*` git dependencies) and in
   the root `Cargo.toml` for every other craft-libs crate of the same revision.
4. `cargo build --release -p photocraft` and drop the panel's menu tail check.
