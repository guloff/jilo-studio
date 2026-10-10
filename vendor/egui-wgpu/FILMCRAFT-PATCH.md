# egui-wgpu 0.36.2 — FilmCraft patch

Unmodified crates.io source of egui-wgpu 0.36.2 (MIT OR Apache-2.0, © the egui authors), used
through `[patch.crates-io]` in the workspace `Cargo.toml`, with these changes (each marked
"FilmCraft patch" in the source):

- `WgpuConfiguration::hdr_surface`: an optional surface format and colour space (FilmCraft asks
  for `Rgba16Float` + `ExtendedSrgb` for EDR on macOS). Used only when the surface supports both;
  otherwise egui picks its usual 8-bit format. `RenderState::target_color_space` records the choice
  and `configure_surface` passes it to wgpu, whose Metal backend then sets
  `wantsExtendedDynamicRangeContent` and the layer colour space.
- Screen capture reads an `Rgba16Float` surface back as half floats (clamped to 0–1).

To update: copy the new upstream release here and reapply the changes above (or drop the patch
once egui-wgpu offers the same option).
