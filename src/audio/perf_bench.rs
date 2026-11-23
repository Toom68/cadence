//! Run with `cargo test --release --locked --lib perf_bench -- --ignored --nocapture --test-threads=1`.

use super::*;
use crate::model::{create_track, Clip, Effects, Instrument, Note, MAX_TRACKS};
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

fn dense_project() -> Project {
    let mut project = Project::empty();
    project.tracks = (0..MAX_TRACKS)
        .map(|i| {
            let mut track = create_track(Instrument::Pad);
            track.effects = Effects {
                cutoff_hz: 5200.0,
                reverb: 1.0,
                delay: 1.0,
                drive: 1.0,
            };
            let pattern = &mut track.patterns[0];
            let offset = i as f64 / (2 * MAX_TRACKS) as f64;
            pattern.notes = (0..16)
                .flat_map(|step| {
                    (0..4).map(move |voice| {
                        let start = step as f64 + offset;
                        Note::new(48 + voice * 5, start, (16.0 - start).min(3.0), 0.8)
                    })
                })
                .collect();
            track.clips = vec![Clip {
                id: crate::model::next_id(),
                pattern_id: pattern.id,
                start: 0.0,
                loops: 8,
                fade_in: 0.0,
                fade_out: 0.0,
            }];
            track
        })
        .collect();
    project.validate().unwrap();
    project
}

fn start_playback(project: &Project, rate: f32) -> (RealTime, Receiver<Retired>) {
    let (tx, rx) = bounded(COMMAND_CAPACITY);
    let (gtx, grx) = bounded(GARBAGE_CAPACITY);
    let rt = RealTime::new(rate, rx, gtx, Arc::default());
    tx.send(
        Command::Play {
            song: Box::new(SongData::from_project(project)),
            start_step: 0.0,
            looping: true,
            loop_region: None,
            count_in: 0.0,
        }
        .into(),
    )
    .unwrap();
    (rt, grx)
}

#[test]
#[ignore = "release-mode performance measurement"]
fn perf_bench_audio() {
    let rate = 48_000;
    let projects = [
        (
            "neon-circuits",
            crate::persistence::load_project(Path::new("examples/songs/neon-circuits.cadence"))
                .unwrap(),
        ),
        (
            "midnight-dub",
            crate::persistence::load_project(Path::new("examples/songs/midnight-dub.cadence"))
                .unwrap(),
        ),
        ("dense-32", dense_project()),
    ];
    for (name, project) in &projects {
        let start = Instant::now();
        for _ in 0..80 {
            black_box(SongData::from_project(project));
        }
        println!(
            "snapshot,{name},{:.1} us",
            start.elapsed().as_secs_f64() * 1e6 / 80.0
        );
        for frames in [128, 256, 512] {
            let mut times = Vec::new();
            for _ in 0..3 {
                let (mut rt, _retired) = start_playback(project, rate as f32);
                let mut data = vec![0.0f32; frames * 2];
                rt.fill(&mut data, 2);
                for _ in
                    0..(project.duration_seconds() * rate as f64 / frames as f64).ceil() as usize
                {
                    let start = Instant::now();
                    rt.fill(black_box(&mut data), 2);
                    times.push(start.elapsed().as_secs_f64() * 1e6);
                }
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            times.sort_unstable_by(f64::total_cmp);
            println!(
                "audio,{name},{frames},mean={mean:.1},p99={:.1},max={:.1},budget={:.1} us",
                times[times.len() * 99 / 100],
                times[times.len() - 1],
                frames as f64 * 1e6 / rate as f64
            );
        }
    }
}

#[test]
#[ignore = "release-mode performance measurement"]
fn perf_bench_meter_cost() {
    let project = dense_project();
    for frames in [128, 256, 512] {
        let (mut on, _on_retired) = start_playback(&project, 48_000.0);
        let (mut off, _off_retired) = start_playback(&project, 48_000.0);
        let mut on_data = vec![0.0f32; frames * 2];
        let mut off_data = on_data.clone();
        let mut on_time = Duration::ZERO;
        let mut off_time = Duration::ZERO;
        let mut meter = || {
            let now = Instant::now();
            on.fill(black_box(&mut on_data), 2);
            on_time += now.elapsed();
        };
        let mut no_meter = || {
            let now = Instant::now();
            off.fill_with_metering::<f32, false>(black_box(&mut off_data), 2);
            off_time += now.elapsed();
        };
        let blocks = 4 * 48_000 / frames;
        for i in 0..blocks {
            if i & 1 == 0 {
                meter();
                no_meter();
            } else {
                no_meter();
                meter();
            }
        }
        assert_eq!(on_data, off_data);
        let us = |duration: Duration| duration.as_secs_f64() * 1e6 / blocks as f64;
        println!(
            "meter,dense-32,{frames},off={:.1},on={:.1} us",
            us(off_time),
            us(on_time)
        );
    }
}
