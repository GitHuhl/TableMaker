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
* **Save** the voice (to reopen without analysing again), a demo phrase as
  WAV, or the vowels as a wavetable WAV for other synths.

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
tablemaker build clips/ -o table.wav                # a classic wavetable of the cycles
```

`tablemaker voice` prints what it found:

```
Speaker (man)   4 recordings, 8.0 s, 100% voiced
Pitch     124 Hz median, range 109-136 Hz
Voice     jitter 1.19%, shimmer 4.4%, HNR 16.3 dB, H1-H2 -0.2 dB, level -17 dBFS
Vibrato   5.2 Hz, ±30 cents, in 100% of sustained notes

Vowel table (4 entries, in morph order)
   #  vowel    F1    F2    F3    F4  share  pitch   HNR  heard in
   1  i       311  2241  2887  3433  23.6%  118Hz   15  sub/ee.wav at 0.56 s
   2  ɛ       553  1821  2485  3357  29.6%  134Hz   17  sub/eh.wav at 0.85 s
   3  ɑ       748  1356  2429  3275  22.5%  110Hz   17  ah.wav at 0.75 s
   4  u       328   858  2253  3191  24.3%  126Hz   15  oo.wav at 1.19 s
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
4. **The instrument.** Each vowel's envelope becomes a minimum-phase
   response, rendered as a band-limited single cycle for every semitone
   with harmonic amplitudes read from the envelope at that pitch, plus a
   breath-noise loop with the same envelope. Voices crossfade between
   vowels and semitones, with the speaker's vibrato, wander and breath.

## Building

Written in Rust and built with Nix; see
[docs/DEVELOPING.md](docs/DEVELOPING.md).

## License

GNU Affero General Public License v3.0 or later.
