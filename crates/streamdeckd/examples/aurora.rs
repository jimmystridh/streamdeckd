use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use streamdeckd::aurora::{ScreensaverCanvas, ScreensaverScene};
use streamdeckd::device::hid::HidDeckDevice;
use streamdeckd::device::DeckDevice;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Scene {
    Aurora,
    Matrix,
    Space,
}

#[derive(Debug, Parser)]
#[command(about = "Play a native screensaver across all 15 Stream Deck keys")]
struct Cli {
    #[arg(long, default_value_t = 12.0)]
    duration: f32,

    #[arg(long, default_value_t = 10)]
    fps: u32,

    #[arg(long)]
    serial: Option<String>,

    #[arg(long, default_value_t = 75)]
    brightness: u8,

    #[arg(long, value_enum, default_value_t = Scene::Aurora)]
    scene: Scene,

    #[arg(long, value_name = "PNG")]
    preview: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::ensure!(cli.duration > 0.0, "duration must be greater than zero");
    anyhow::ensure!((1..=30).contains(&cli.fps), "fps must be between 1 and 30");

    if let Some(path) = cli.preview {
        canvas(cli.scene, cli.duration * 0.42, 1.0).save_preview(&path)?;
        println!("{}", path.display());
        return Ok(());
    }

    let device = HidDeckDevice::open(cli.serial.as_deref())?;
    device.set_brightness(cli.brightness).await?;

    let started = Instant::now();
    let frame_period = Duration::from_secs_f32(1.0 / cli.fps as f32);
    let mut frames = 0u64;
    let mut bytes = 0u64;

    loop {
        let elapsed = started.elapsed();
        let progress = elapsed.as_secs_f32() / cli.duration;
        if progress >= 1.0 {
            break;
        }

        let intensity = smoothstep(0.0, 0.08, progress) * (1.0 - smoothstep(0.84, 1.0, progress));
        bytes += canvas(cli.scene, elapsed.as_secs_f32(), intensity)
            .send(&device)
            .await? as u64;
        frames += 1;

        let next = started + frame_period.mul_f32(frames as f32);
        if next > Instant::now() {
            tokio::time::sleep_until(tokio::time::Instant::from_std(next)).await;
        }
    }

    ScreensaverCanvas::black().send(&device).await?;
    device.close().await?;

    let seconds = started.elapsed().as_secs_f32();
    println!(
        "{frames} frames in {seconds:.2}s ({:.1} fps), {:.1} MiB encoded",
        frames as f32 / seconds,
        bytes as f32 / 1_048_576.0
    );
    Ok(())
}

fn canvas(scene: Scene, time: f32, intensity: f32) -> ScreensaverCanvas {
    match scene {
        Scene::Aurora => ScreensaverCanvas::render(ScreensaverScene::Aurora, time, intensity),
        Scene::Matrix => ScreensaverCanvas::render(ScreensaverScene::Matrix, time, intensity),
        Scene::Space => ScreensaverCanvas::render(ScreensaverScene::Space, time, intensity),
    }
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
