# TableMaker

### Record a voice. Play it.

TableMaker turns recordings of one person into an instrument that sings
with their voice. Drop in a few files or a whole folder of them (speech
or singing), and it works out everything about that voice by itself:
their pitch and range, every vowel they make and its formants, how
breathy they are, how their pitch and loudness waver, and their vibrato.
Then play it from a MIDI keyboard, the on-screen keys or your computer
keyboard, moving between their vowels as you go.

The formants stay where the speaker made them at any note you play, so an
"ah" sung two octaves above how it was spoken is still that person's "ah",
not a chipmunk.

```sh
nix run github:GitHuhl/TableMaker
```

## Playing it

* **Drop recordings in** (WAV files or folders, searched all the way
  down), or copy them in your file manager and paste them into the
  window. Pick *Man*, *Woman* or *Child* if you know who it is; it tunes the
  pitch search, how high formants are looked for and how vowels are named.
  *Auto* works it out.
* **The vowel pad** lays out every vowel found the way they sit in the
  mouth: front to back across, close to open down. Press anywhere to sing
  at the speaker's own pitch and slide to move between vowels. Press a
  vowel to see its formants and where in the recordings it was heard.
* **Keys**: a MIDI keyboard (every input is connected), the on-screen
  keyboard, or A W S E D F T G Y H U J K O L on the computer keyboard
  (Z and X change octave).
* **MIDI**: mod wheel moves through the vowels, CC2 (breath) the breath
  noise, CC11 expression, channel pressure adds vibrato, sustain pedal and
  pitch bend as usual.
* **Controls** start at the speaker's own measurements: vibrato depth and
  rate, breath, pitch and level wander, attack, release, glide and legato.
  *Reset to the speaker* brings them back.
* **Compare with a glottal pulse.** The *Source* slider (and *A/B*)
  crossfades from the recorded timbre to a synthetic glottal pulse put
  through the same vowel's formant filters (F1-F4, then the vocal tract's
  higher resonances). The pulse's open quotient and skew can be set by
  hand or fitted to the speaker's spectral tilt.
* **Missing formants are completed.** Where a vowel's F2-F4 could not be
  measured, TableMaker predicts them from the speaker's other vowels, looks
  for them in that vowel's spectrum (*found*) and otherwise keeps the
  prediction (*inferred*). With *Restore missing formants & highs* on, the
  instrument adds a resonance for every inferred formant and, for
  recordings at a low sample rate, continues the speaker's spectrum above
  the recording's band instead of leaving it silent.
* **Save** the voice (to reopen without analysing again), a demo phrase or
  a recorded-then-pulse comparison as WAV, or the vowels as a wavetable WAV
  for other synths.

On a Wayland desktop the window opens through XWayland, because the
windowing library's native Wayland backend cannot receive dropped files
yet. Set `TABLEMAKER_WAYLAND=1` to keep it native Wayland; pasting copied
files still works there.

## From the command line

```sh
tablemaker                                          # the app
tablemaker gui recordings/ --speaker woman          # the app, analysing these first
tablemaker voice recordings/ -o alice.json          # analyse and print the voice
tablemaker play alice.json                          # play from MIDI, no window
tablemaker play --list                              # audio outputs and MIDI inputs
tablemaker audition alice.json -o demo.wav          # render a demo phrase
tablemaker audition alice.json --compare -o ab.wav  # recorded, then glottal pulse + formants
tablemaker build clips/ -o table.wav                # a classic wavetable of the cycles
```

`tablemaker voice` prints what it found:

```
Speaker (man)   4 recordings, 8.0 s, 100% voiced
Pitch     124 Hz median, range 109-136 Hz
Voice     jitter 0.97%, shimmer 3.6%, HNR 17.5 dB, H1-H2 -1.2 dB, level -16 dBFS
Vibrato   5.1 Hz, ±28 cents, in 100% of sustained notes

Vowel table (4 entries, in morph order)
   #  vowel     F1     F2     F3     F4  share  pitch   HNR  heard in
   1  i        320   2255   2955   3613  24.4%  118Hz   16  sub/ee.wav at 0.99 s
   2  ɛ        570   1796   2512   3521  28.4%  134Hz   17  sub/eh.wav at 0.23 s
   3  ɑ        767   1323   2447   3421  21.5%  110Hz   20  ah.wav at 0.55 s
   4  u       338    876 + 2314 ~ 3521  25.8%  126Hz   18  oo.wav at 1.69 s
  + found in the vowel's spectrum where the speaker's other vowels put it
  ~ inferred: not in the recording (restored in the instrument unless --no-restore)
```

## How it works

1. **Cycles.** Each recording is oversampled 16× and its rising zero
   crossings located between samples; crossings one tracked pitch period
   apart bound single cycles, kept when they repeat in the next period and
   loop cleanly.
2. **The voice.** Pitch is tracked over time (YIN); formants by Burg linear
   prediction on the signal resampled to twice the highest formant. Each
   cycle's harmonics give its spectral envelope in Hz. Neighbouring cycles
   give jitter and shimmer, their periodicity the harmonics-to-noise ratio,
   and the pitch track's wobble the vibrato.
3. **The vowel table.** All cycles of all recordings are clustered by
   envelope into the vowels and other voiced sounds the speaker made; each
   entry keeps its averaged envelope, formants, breathiness and where it
   was heard, and the entries are ordered so neighbours sound alike.
4. **Completion.** Missing higher formants are predicted from the
   speaker's other vowels (F3 and F4 follow the vocal tract's length,
   which every vowel shares), searched for in the vowel's own spectrum
   with its slope taken out, and otherwise inferred.
5. **The instrument.** Each vowel's envelope becomes a minimum-phase
   response, rendered as a band-limited single cycle for every semitone
   with harmonic amplitudes read from the envelope at that pitch, plus a
   breath-noise loop with the same envelope. Voices crossfade between
   vowels and semitones, with the speaker's vibrato, wander and breath.

## Building

Written in Rust and built with Nix; see
[docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

GNU Affero General Public License v3.0 or later.
