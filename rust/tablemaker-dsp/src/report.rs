//! A JSON summary of what was found and what went into the table.

use crate::cycles::ClipAnalysis;
use crate::table::Table;
use std::fmt::Write;

fn esc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => write!(o, "\\u{:04x}", c as u32).unwrap(),
            c => o.push(c),
        }
    }
    o
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{}", (v * 1e6).round() / 1e6)
    } else {
        "null".into()
    }
}

pub fn json(clips: &[(String, ClipAnalysis)], table: &Table) -> String {
    let mut o = String::from("{\n  \"clips\": [\n");
    for (i, (name, a)) in clips.iter().enumerate() {
        writeln!(
            o,
            "    {{\"file\": \"{}\", \"sample_rate\": {}, \"duration\": {}, \"median_f0\": {}, \"voiced_fraction\": {}, \"zero_crossings\": {}, \"candidates\": {}, \"cycles\": {}}}{}",
            esc(name),
            a.sample_rate,
            num(a.duration),
            a.median_f0.map_or("null".into(), num),
            num(a.voiced_fraction),
            a.crossings,
            a.candidates,
            a.cycles.len(),
            if i + 1 < clips.len() { "," } else { "" }
        )
        .unwrap();
    }
    write!(
        o,
        "  ],\n  \"frames\": {},\n  \"frame_size\": {},\n  \"keyframes\": [\n",
        table.frames.len(),
        table.frames.first().map_or(0, Vec::len)
    )
    .unwrap();
    for (i, (pos, c)) in table.keyframes.iter().enumerate() {
        writeln!(
            o,
            "    {{\"frame\": {}, \"file\": \"{}\", \"time\": {}, \"f0\": {}, \"score\": {}}}{}",
            pos,
            esc(&clips[c.clip].0),
            num(c.time),
            num(c.f0),
            num(c.score),
            if i + 1 < table.keyframes.len() {
                ","
            } else {
                ""
            }
        )
        .unwrap();
    }
    o.push_str("  ]\n}\n");
    o
}
