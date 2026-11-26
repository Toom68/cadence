//! A tiny SoundFont fixture shared by integration tests (`tests/common`)
//! and the library's unit tests (`src/audio/mod.rs`). Std only.

/// A minimal but valid SoundFont 2 file: one preset (bank 0, program 0) with
/// one instrument playing a short looped sine sample. Lets font tests run
/// without a real `.sf2` (none is committed to the repo).
pub fn tiny_sf2() -> Vec<u8> {
    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }
    fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for c in chunks {
            body.extend_from_slice(c);
        }
        chunk(b"LIST", &body)
    }
    fn name(text: &str) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[..text.len()].copy_from_slice(text.as_bytes());
        out
    }
    fn u16s(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn phdr(label: &str, bag: u16) -> Vec<u8> {
        let mut out = name(label).to_vec();
        out.extend(u16s(&[0, 0, bag])); // program, bank, first zone
        out.extend_from_slice(&[0u8; 12]); // library, genre, morphology
        out
    }
    fn inst(label: &str, bag: u16) -> Vec<u8> {
        let mut out = name(label).to_vec();
        out.extend(u16s(&[bag]));
        out
    }
    fn shdr(label: &str, fields: [u32; 5], pitch: u8, kind: u16) -> Vec<u8> {
        let mut out = name(label).to_vec();
        for f in fields {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&[pitch, 0]);
        out.extend(u16s(&[0, kind]));
        out
    }
    // 100 samples of one sine cycle every 50 samples (441 Hz at 22.05 kHz),
    // followed by the 46 zero samples the spec asks for.
    let mut wave: Vec<u8> = (0..100)
        .map(|i| ((i as f32 / 50.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16)
        .flat_map(|s| s.to_le_bytes())
        .collect();
    wave.extend_from_slice(&[0u8; 92]);
    let info = list(b"INFO", &[chunk(b"ifil", &u16s(&[2, 1]))]);
    let sdta = list(b"sdta", &[chunk(b"smpl", &wave)]);
    let pdta = list(
        b"pdta",
        &[
            chunk(b"phdr", &[phdr("Tiny", 0), phdr("EOP", 1)].concat()),
            chunk(b"pbag", &u16s(&[0, 0, 1, 0])),
            chunk(b"pmod", &[0u8; 10]),
            chunk(b"pgen", &u16s(&[41, 0, 0, 0])), // instrument 0, terminator
            chunk(b"inst", &[inst("TinyInst", 0), inst("EOI", 1)].concat()),
            // One zone holding both generators below (bag terminator at 2).
            chunk(b"ibag", &u16s(&[0, 0, 2, 0])),
            chunk(b"imod", &[0u8; 10]),
            // sampleModes = 1 (loop continuously), sampleID 0, terminator.
            chunk(b"igen", &u16s(&[54, 1, 53, 0, 0, 0])),
            chunk(
                b"shdr",
                &[
                    shdr("TinySine", [0, 100, 0, 100, 22_050], 69, 1),
                    shdr("EOS", [0; 5], 0, 0),
                ]
                .concat(),
            ),
        ],
    );
    let mut riff = b"sfbk".to_vec();
    riff.extend(info);
    riff.extend(sdta);
    riff.extend(pdta);
    chunk(b"RIFF", &riff)
}
