# Developing TableMaker

TableMaker is written in Rust, as a Cargo workspace under `rust/`:

* `tablemaker-dsp`: the library. WAV I/O (`wav`), oversampling, zero
  crossings and FFT helpers (`signal`), YIN pitch tracking (`pitch`), cycle
  extraction and scoring (`cycles`), table assembly (`table`), the preview
  renderer (`preview`) and the JSON report (`report`).
* `tablemaker-app`: the `tablemaker` command line, and the end-to-end test
  that runs it on synthetic clips.

The DSP crate is optimised even in development builds (see
`rust/Cargo.toml`), so `cargo test` stays quick.

## Run, build & test (Nix only)

```sh
nix run . -- build clips/*.wav -o table.wav   # the command line from this checkout
nix build                                     # sandboxed build running every test
nix flake check                               # the same, as a check

nix develop                                   # pinned nixos-26.05 toolchain
cd rust && cargo test --workspace             # all crates
cargo clippy --all-targets                    # lints
cargo run -p tablemaker-app -- analyze a.wav  # the command line from source
```

The version lives in one place, `workspace.package.version` in
`rust/Cargo.toml`, which `flake.nix` reads too.
