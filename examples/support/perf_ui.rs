//! Run with `CADENCE_DATA_DIR` pointing at an isolated test directory and
//! `cargo test --release --locked --lib perf_bench -- --ignored --nocapture --test-threads=1`.

use super::super::Tab;
use super::*;
use crate::audio::AudioEngine;
use std::hint::black_box;
use std::path::Path;
use std::time::{Duration, Instant};

fn input(frame: usize) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
        time: Some(frame as f64 / 60.0),
        ..Default::default()
    }
}

fn frame(ctx: &egui::Context, app: &mut CadenceApp, index: usize) -> Duration {
    ctx.run(input(index), |ctx| app.frame(ctx)).viewport_output[&egui::ViewportId::ROOT]
        .repaint_delay
}

#[test]
#[ignore = "release-mode performance measurement"]
fn perf_bench_frames() {
    let data = std::env::var_os("CADENCE_DATA_DIR").expect("set an isolated CADENCE_DATA_DIR");
    assert!(std::env::var_os("CADENCE_PACKS_DIR").is_none());
    assert!(std::env::var_os("CADENCE_FONTS_DIR").is_none());

    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let now = Instant::now();
    let mut app = CadenceApp::new(&cc);
    let created = now.elapsed();
    frame(&ctx, &mut app, 0);
    println!(
        "startup,new={:.2} ms,through_first_frame={:.2} ms,device={}",
        created.as_secs_f64() * 1000.0,
        now.elapsed().as_secs_f64() * 1000.0,
        app.engine.is_some()
    );
    for i in 1..10 {
        frame(&ctx, &mut app, i);
    }
    println!("repaint,io-idle,{:?}", frame(&ctx, &mut app, 10));
    let (repaint_tx, repaint_rx) = crossbeam_channel::bounded(8);
    ctx.set_request_repaint_callback(move |request| {
        let _ = repaint_tx.try_send(request.delay);
    });
    let idle_ok = repaint_rx
        .recv_timeout(Duration::from_millis(2200))
        .is_err();
    println!("repaint,io-idle-no-requests-over-2200-ms,{idle_ok}");
    assert!(idle_ok);

    let pack_dir = Path::new(&data).join("packs");
    std::fs::create_dir_all(&pack_dir).unwrap();
    let id = format!("perf-watch-probe-{}", std::process::id());
    let pack = crate::packs::template_pack(&id, "Performance watcher probe").unwrap();
    let pack_path = pack_dir.join(format!("{id}.json"));
    assert!(!pack_path.exists());
    std::fs::write(&pack_path, crate::packs::pack_to_json(&pack).unwrap()).unwrap();
    let repaint = repaint_rx.recv_timeout(Duration::from_secs(5));
    frame(&ctx, &mut app, 11);
    std::fs::remove_file(pack_path).unwrap();
    println!("repaint,pack-change,{repaint:?}");
    assert_eq!(repaint.unwrap(), Duration::ZERO);
    assert!(app
        .catalog
        .instruments
        .iter()
        .any(|p| p.id.starts_with(&id)));
    drop(app);
    let now = Instant::now();
    for _ in 0..20 {
        black_box(crate::packs::load_catalog());
    }
    println!("startup,packs={:.2} ms", now.elapsed().as_secs_f64() * 50.0);
    let now = Instant::now();
    for _ in 0..20 {
        black_box(crate::font::load_library());
    }
    println!("startup,fonts={:.2} ms", now.elapsed().as_secs_f64() * 50.0);

    for name in ["neon-circuits", "midnight-dub"] {
        let path = format!("examples/songs/{name}.cadence");
        let project = crate::persistence::load_project(Path::new(&path)).unwrap();
        for tab in [Tab::Editor, Tab::Mixer, Tab::Keys] {
            let ctx = egui::Context::default();
            let mut app = CadenceApp::headless(project.clone());
            let (engine, advance) = AudioEngine::for_frame_benchmark();
            app.engine = Some(engine);
            app.tab = tab;
            for mode in ["idle", "playing", "drag"] {
                let mut repaint = Duration::MAX;
                let mut elapsed = Duration::ZERO;
                for n in 0..240 {
                    let mut mode_index = 0;
                    if mode == "playing" {
                        mode_index = 1;
                        app.transport.playing = true;
                    } else if mode == "drag" {
                        mode_index = 2;
                    }
                    advance(n as f64 * 0.05, mode == "playing");
                    let start = Instant::now();
                    if mode == "drag" {
                        // Model the mixer fader's coalesced drag without relying on
                        // pointer coordinates that move when layouts are restyled.
                        app.edit("Track volume", Some("vol-bench"), |p| {
                            p.tracks[0].volume = (n % 80) as f32 / 80.0;
                        });
                    }
                    repaint = frame(&ctx, &mut app, n + 240 * mode_index);
                    if n >= 40 {
                        elapsed += start.elapsed();
                    }
                }
                println!(
                    "ui,{name},{tab:?},{mode},{:.1} us,repaint={repaint:?}",
                    elapsed.as_secs_f64() * 5000.0
                );
            }
        }
    }

    let ctx = egui::Context::default();
    let mut app = CadenceApp::headless(Project::demo());
    let path = Path::new(&data).join(format!("perf-export-{}.wav", std::process::id()));
    std::fs::create_dir_all(&data).unwrap();
    assert!(!path.exists());
    frame(&ctx, &mut app, 0);
    frame(&ctx, &mut app, 1);
    app.start_wav_export(path.clone());
    let delay = ctx
        .run(input(2), |ctx| app.schedule_repaint(ctx))
        .viewport_output[&egui::ViewportId::ROOT]
        .repaint_delay;
    println!("repaint,export,{delay:?}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.export.is_some() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
        app.poll_export();
    }
    std::fs::remove_file(path).unwrap();
}
