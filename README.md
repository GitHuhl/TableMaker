# TableMaker

Turn a handful of recordings of one instrument into a wavetable.

Give TableMaker a few audio clips of the same source (notes of a synth,
a sung vowel, a bowed string, a field recording with a pitch in it). It
finds every clean single cycle in them, cuts each exactly at its zero
crossings, and mashes the cycles from all the clips together into one
morphable wavetable for Vital, Surge XT, Bitwig's Wavetable oscillator
and the many other synths that read the same format.

```sh
nix run github:GitHuhl/TableMaker -- build cello-*.wav -o cello.wav --order path --preview cello-preview.wav
```

## How it works

1. **Clean up.** Each clip is mixed to mono and DC-blocked with a
   zero-phase high-pass, so the crossings it is cut at are real ones.
2. **Track the pitch.** YIN finds the fundamental over time, which says how
   far apart the two ends of one cycle should be.
3. **Oversample.** The clip is oversampled 16× (Kaiser-windowed sinc, with
   the filter delay taken out) and its rising zero crossings are located
   between samples. The cut points land at a small fraction of an original
   sample from where they really are, whatever the period in samples.
4. **Pair crossings into cycles.** Every crossing is paired with the
   crossing nearest one tracked period later (within `--tolerance`). Each
   pair is scored on how well it matches the period after it, and how well
   the slopes at its two ends agree, since a mismatch there clicks when the
   cycle loops. The best non-overlapping cycles above `--min-score` are kept,
   and quiet tails are gated out.
5. **Render.** Each cycle is read at its native resolution from the
   oversampled signal and moved to the frame size in the frequency domain:
   band-limited, without DC, and looping seamlessly.
6. **Mash.** The cycles from every clip are pooled; keyframes are selected
   (`--select`), ordered (`--order`), rotated to line up in phase with their
   neighbours, morphed between (`--morph`) to fill the table, smoothed and
   normalised.

## Using it

```sh
tablemaker analyze clips/*.wav                       # pitch and usable cycles per clip
tablemaker build clips/*.wav -o table.wav            # 64 frames of 2048, following the clips in order
tablemaker build clips/*.wav -o table.wav \
  --select diverse --order path --morph spectral \
  -f 256 -k 32                                       # 32 distinct cycles, morphed into 256 frames
tablemaker build clips/*.wav -o table.wav \
  --preview p.wav --cycles-dir cycles --report r.json  # audition, single cycles, provenance
```

| Option | What it does |
| --- | --- |
| `-f, --frames` | Frames in the table (default 64). |
| `-o, --output` | `table.wav`: frames back to back with a `clm ` chunk giving the frame size (Vital, Surge XT, Bitwig, most others). `table.wt`: Surge XT's native format (powers of two up to 4096 samples, at most 512 frames, float32 or pcm16). |
| `--frame-size` | Samples per frame (default 2048, the common choice). |
| `-k, --keyframes` | Distinct cycles used; the frames between them are morphed. |
| `--select spread\|best\|diverse` | Evenly through the clips' timeline; the cleanest cycles; or the most different-sounding ones. |
| `--order time\|brightness\|path` | Clip order then time; dark to bright; or the shortest path through spectral space, each frame beside its closest relatives. |
| `--morph crossfade\|spectral` | Crossfade aligned waveforms, or interpolate harmonic amplitudes and phases (no dip between keyframes that disagree in phase). |
| `--no-align` | Keep every cycle starting at its own zero crossing instead of lining it up with its neighbour. |
| `--smooth N` | Average each frame with N neighbours on either side. |
| `--normalize table\|frame\|none` | One gain for the whole table (keeps level changes), every frame to full scale, or neither. |
| `--format float32\|pcm16\|pcm24` | Sample format of everything written. |
| `--oversample` | Oversampling factor for locating crossings (default 16). |
| `--f0`, `--fmin`, `--fmax` | Give the pitch outright, or the range to track it in (30–2000 Hz). |
| `--tolerance`, `--min-score`, `--gate` | How strict cycle detection is. |
| `--preview FILE` | Render the table at `--preview-note` (MIDI, default 48) sweeping every frame. |
| `--cycles-dir DIR` | Also write every frame as a single-cycle WAV. |
| `--report FILE` | JSON: what each clip held, and the clip and time every keyframe came from. |

Clips can be any WAV: 8/16/24/32-bit PCM or 32/64-bit float, any channel
count, any sample rate, and different clips can have different rates.

## Building

TableMaker is written in Rust and built with Nix; see
[docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

GNU Affero General Public License v3.0 or later.
