# Security patches

These are source copies of published upstream crates. Their original licenses
and copyright notices remain in each directory. EdenVoice changes are dated
2026-10-24; they do not change the upstream licenses.

* `tract-nnef-0.19.16`: backports the checked tensor shape and byte arithmetic
  from upstream 0.21.16 for RUSTSEC-2026-0217, rejects missing packed-boolean
  payloads without unsigned underflow, and initializes string read buffers.
  Regression cases live in `client/tests/nnef_security.rs`. The 0.21.16+
  dependency constraints conflict with the GUI's `half` and Tauri's `time`
  requirements, so the parser fix is applied to the compatible 0.19.16 line,
  keeping the original pinned DeepFilterNet engine and model behavior.
  Source: https://crates.io/crates/tract-nnef/0.19.16
  Fix: https://rustsec.org/advisories/RUSTSEC-2026-0217.html
* `tract-onnx-0.19.16`: updates the memory-map dependency to `memmap2` 0.9.12,
  fixing RUSTSEC-2026-0186 without changing the inference API.
* `tauri-plugin-updater-2.11.0`: changes the Windows ZIP dependency from 4.x to
  9.0.5, containing the extraction fixes for RUSTSEC-2026-0367/0368/0369.
  The updater's signed-download verification and installer code are unchanged.
  Source: https://crates.io/crates/tauri-plugin-updater/2.11.0
* `glib-0.18.5`: backports the mutable out-parameter fix for
  `VariantStrIter::impl_get` from gtk-rs/gtk-rs-core#1343 (RUSTSEC-2024-0429).
  Linux GTK3 still requires the 0.18 API; a major GLib upgrade cannot be used
  without migrating that toolkit. Original iterator tests remain in this copy.
  An optimized regression is in `tools/glib-security-check`; run its
  `cargo test --release --locked` on Linux with GLib development headers.
  Source: https://crates.io/crates/glib/0.18.5

Both Cargo roots explicitly select these patches. Do not remove them merely to
regenerate a lockfile: first verify that the selected upstream versions contain
the equivalent fixes. Path patches are not covered by registry vulnerability
matching, so run the parser regressions in addition to `cargo audit`.
