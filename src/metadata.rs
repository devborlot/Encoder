use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct VideoMetadata {
    pub duration_secs: u64,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub audio_channels: u32,
    pub has_audio: bool,
}

impl VideoMetadata {
    pub fn duration_display(&self) -> String {
        format!("{}\"", self.duration_secs)
    }
}

pub fn check_ffmpeg() -> Result<()> {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .context("FFmpeg não encontrado no PATH")?;
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .context("FFprobe não encontrado no PATH")?;
    Ok(())
}

pub fn probe(video_path: &Path) -> Result<VideoMetadata> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(video_path)
        .output()
        .context("Falha ao executar FFprobe")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("FFprobe retornou erro: {stderr}");
    }

    let json: Value =
        serde_json::from_slice(&output.stdout).context("Falha ao parsear saída do FFprobe")?;

    let streams = json["streams"]
        .as_array()
        .context("Campo 'streams' não encontrado")?;

    // Encontrar stream de vídeo
    let video_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .context("Nenhum stream de vídeo encontrado")?;

    let width = video_stream["width"]
        .as_u64()
        .context("Campo 'width' não encontrado")? as u32;
    let height = video_stream["height"]
        .as_u64()
        .context("Campo 'height' não encontrado")? as u32;

    // Parse frame rate (ex: "30000/1001", "30/1", "25/1")
    let (fps_num, fps_den) = parse_frame_rate(video_stream)?;

    // Encontrar stream de áudio
    let audio_stream = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("audio"));

    let (has_audio, audio_channels) = match audio_stream {
        Some(stream) => {
            let channels = stream["channels"].as_u64().unwrap_or(2) as u32;
            (true, channels)
        }
        None => (false, 0),
    };

    // Duração
    let duration_str = json["format"]["duration"]
        .as_str()
        .or_else(|| video_stream["duration"].as_str())
        .context("Duração não encontrada")?;

    let duration_secs = duration_str
        .parse::<f64>()
        .context("Falha ao parsear duração")?
        .round() as u64;

    Ok(VideoMetadata {
        duration_secs,
        width,
        height,
        fps_num,
        fps_den,
        audio_channels,
        has_audio,
    })
}

fn parse_frame_rate(video_stream: &Value) -> Result<(u32, u32)> {
    // Tenta r_frame_rate primeiro, depois avg_frame_rate
    let rate_str = video_stream["r_frame_rate"]
        .as_str()
        .or_else(|| video_stream["avg_frame_rate"].as_str())
        .context("Frame rate não encontrado")?;

    if let Some((num, den)) = rate_str.split_once('/') {
        let n = num.parse::<u32>().context("Frame rate inválido (num)")?;
        let d = den.parse::<u32>().context("Frame rate inválido (den)")?;
        Ok((n, d))
    } else {
        let fps = rate_str.parse::<f64>().context("Frame rate inválido")?;
        Ok((fps.round() as u32, 1))
    }
}
