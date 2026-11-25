# Developing TableMaker

TableMaker is written in Rust, as a Cargo workspace under `rust/`:

* `tablemaker-dsp`: analysis. WAV I/O (`wav`), gathering recordings
  (`inputs`), oversampling, zero crossings and FFT helpers (`signal`),
  resampling (`resample`), pitch (`pitch`), cycle extraction (`cycles`),
  linear prediction and formants (`lpc`, `formant`), spectral envelopes
  (`envelope`), the speaker profile and vowel table (`speaker`), and the
  classic wavetable builder (`table`, `preview`, `report`). `testsignal`
  makes synthetic voices for the tests.
* `tablemaker-synth`: the instrument. The pre-rendered bank (`bank`) and
  the real-time engine (`engine`), which never allocates or locks while
  processing.
* `tablemaker-app`: the `tablemaker` binary. The window (`gui`, egui on
  wgpu), live audio and MIDI (`live`, cpal and midir over ALSA, which
  PipeWire serves), and the commands (`voice`, `perform`).

The DSP and synth crates are optimised even in development builds (see
`rust/Cargo.toml`), so tests stay quick.

## Run, build & test (Nix only)

```sh
nix run                                       # the app
nix run . -- gui recordings/                  # the app, analysing recordings first
nix build                                     # sandboxed build running every test
nix flake check                               # the same, as a check

nix develop                                   # pinned nixos-26.05 toolchain, ALSA, GUI libraries
cd rust && cargo test --workspace             # all crates
cargo clippy --all-targets                    # lints
cargo run -p tablemaker-app                   # the app from source
cargo run --release -p tablemaker-dsp --example gen_voice -- /tmp/voices   # synthetic test voices
cargo run --release -p tablemaker-dsp --example inspect -- take.wav        # pitch and formants of a render
```

Headless screenshots (Xvfb and Mesa's software Vulkan, both in the dev
shell): the app saves one and quits when `TABLEMAKER_SCREENSHOT` is set.

```sh
VK_ICD_FILENAMES=$TABLEMAKER_LAVAPIPE_ICD TABLEMAKER_SCREENSHOT=shot.png \
  xvfb-run -a -s '-screen 0 1280x800x24' cargo run -p tablemaker-app -- gui voice.json
```

The version lives in one place, `workspace.package.version` in
`rust/Cargo.toml`, which `flake.nix` reads too.
