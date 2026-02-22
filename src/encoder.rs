use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

use crate::metadata::VideoMetadata;

/// Retorna filtro FFmpeg para ajustar duração ao segundo exato.
/// Frames a mais: trim. Frames faltando: congela último frame.
fn duration_adjust_filter(metadata: &VideoMetadata) -> String {
    let target = metadata.duration_secs as f64;
    let raw = metadata.duration_raw;
    let diff = raw - target;

    if diff.abs() < 0.001 {
        // Duração exata, sem ajuste
        String::new()
    } else if diff > 0.0 {
        // Frames a mais: cortar no tempo exato
        format!(",trim=duration={target},setpts=PTS-STARTPTS")
    } else {
        // Frames faltando: congelar último frame
        let pad_duration = -diff;
        format!(",tpad=stop_mode=clone:stop_duration={pad_duration:.4}")
    }
}

pub fn encode(
    slate_path: &Path,
    video_path: &Path,
    output_path: &Path,
    metadata: &VideoMetadata,
) -> Result<()> {
    let slate_duration = 5;
    let black_duration = 2;
    let silence_duration = slate_duration + black_duration;

    // Construir filter_complex baseado no áudio do source
    let filter_complex = build_filter_complex(metadata, silence_duration);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y"]); // Sobrescrever sem perguntar

    // Input 0: slate image (loop)
    cmd.args([
        "-loop",
        "1",
        "-t",
        &slate_duration.to_string(),
        "-framerate",
        "30000/1001",
        "-i",
    ]);
    cmd.arg(slate_path);

    // Input 1: black video
    cmd.args([
        "-f",
        "lavfi",
        "-t",
        &black_duration.to_string(),
        "-i",
        "color=black:s=1920x1080:r=30000/1001",
    ]);

    // Input 2: source video (NVDEC hardware decode)
    cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
    cmd.arg("-i");
    cmd.arg(video_path);

    // Filter complex
    cmd.args(["-filter_complex", &filter_complex]);

    // Mapping
    cmd.args(["-map", "[vout]", "-map", "[aout]"]);

    // Video codec: MPEG-2 XDCAM HD422
    cmd.args([
        "-c:v",
        "mpeg2video",
        "-pix_fmt",
        "yuv422p",
        "-b:v",
        "50000k",
        "-maxrate",
        "50000k",
        "-minrate",
        "50000k",
        "-bufsize",
        "17825792",
        "-flags",
        "+ildct+ilme",
        "-top",
        "1",
        "-dc",
        "10",
        "-intra_vlc",
        "1",
        "-qmax",
        "28",
        "-sc_threshold",
        "1000000000",
        "-g",
        "12",
        "-bf",
        "2",
    ]);

    // Audio codec: PCM 24-bit
    cmd.args(["-c:a", "pcm_s24le", "-ar", "48000", "-ac", "4"]);

    // Output format: MXF
    cmd.args(["-f", "mxf"]);
    cmd.arg(output_path);

    println!("Executando FFmpeg...");
    println!(
        "  Slate: {}s | Black: {}s | Vídeo: {}s",
        slate_duration, black_duration, metadata.duration_secs
    );
    println!(
        "  Duração total: {}s",
        silence_duration as u64 + metadata.duration_secs
    );

    let output = cmd
        .output()
        .context("Falha ao executar FFmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Show only the last meaningful lines (skip version/config noise)
        let last_lines: String = stderr
            .lines()
            .filter(|l| !l.is_empty())
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("FFmpeg falhou:\n{last_lines}");
    }

    println!("Encoding concluído: {}", output_path.display());
    Ok(())
}

/// Encode versão agência: MP4 H.264 leve (~7MB) sem claquete
pub fn encode_agency(
    video_path: &Path,
    output_path: &Path,
    metadata: &VideoMetadata,
) -> Result<()> {
    // Calcular bitrate de vídeo para target ~7MB
    // 7MB = 56000 kbit; desconta áudio 160kbps
    let target_kbits = 56000u64;
    let audio_kbps = 160u64;
    let video_kbps = if metadata.duration_secs > 0 {
        (target_kbits / metadata.duration_secs).saturating_sub(audio_kbps)
    } else {
        3000
    };
    // Clamp: mínimo 500kbps, máximo 5000kbps
    let video_kbps = video_kbps.max(500).min(5000);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y"]);

    // Input com NVDEC
    cmd.args(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
    cmd.arg("-i");
    cmd.arg(video_path);

    // Video filters: download da GPU, scale, ajuste de duração
    let dur_adjust = duration_adjust_filter(metadata);
    let vf = format!("hwdownload,format=nv12,scale=1920:1080,fps=30000/1001{dur_adjust}");
    cmd.args(["-vf", &vf]);

    // H.264 medium preset
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-profile:v",
        "high",
        "-b:v",
        &format!("{video_kbps}k"),
        "-maxrate",
        &format!("{}k", video_kbps * 2),
        "-bufsize",
        &format!("{}k", video_kbps * 4),
        "-pix_fmt",
        "yuv420p",
    ]);

    // AAC stereo
    cmd.args(["-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-ac", "2"]);

    // Limitar duração total ao segundo exato
    cmd.args(["-t", &metadata.duration_secs.to_string()]);

    // MP4 output
    cmd.args(["-movflags", "+faststart"]);
    cmd.arg(output_path);

    println!("Encodando versão agência (MP4 ~7MB)...");
    println!("  Bitrate vídeo: {video_kbps}kbps | Áudio: {audio_kbps}kbps");

    let output = cmd.output().context("Falha ao executar FFmpeg (agência)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last_lines: String = stderr
            .lines()
            .filter(|l| !l.is_empty())
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        bail!("FFmpeg (agência) falhou:\n{last_lines}");
    }

    println!("Versão agência concluída: {}", output_path.display());
    Ok(())
}

fn build_filter_complex(metadata: &VideoMetadata, silence_duration: i32) -> String {
    let dur_adjust = duration_adjust_filter(metadata);

    let mut parts: Vec<String> = vec![
        // Slate
        "[0:v]scale=1920:1080,fps=30000/1001,format=yuv422p,setfield=tff[slate]".to_string(),
        // Black
        "[1:v]format=yuv422p,setfield=tff[black]".to_string(),
        // Main video (hwdownload, ajuste de duração, then format)
        format!(
            "[2:v]hwdownload,format=nv12,scale=1920:1080,fps=30000/1001{dur_adjust},format=yuv422p,setfield=tff[main]"
        ),
        // Concat vídeo
        "[slate][black][main]concat=n=3:v=1:a=0[vout]".to_string(),
    ];

    // Áudio: depende do source
    parts.extend(build_audio_filters(metadata, silence_duration));

    parts.join(";\n")
}

fn build_audio_filters(metadata: &VideoMetadata, silence_duration: i32) -> Vec<String> {
    let mut filters = Vec::new();

    // Silêncio para slate + black (4 canais)
    filters.push(format!(
        "anullsrc=r=48000:cl=4c:d={silence_duration}[silence]"
    ));

    if !metadata.has_audio {
        // Sem áudio no source: gerar silêncio para a duração do vídeo também
        filters.push(format!(
            "anullsrc=r=48000:cl=4c:d={}[amain]",
            metadata.duration_secs
        ));
    } else if metadata.audio_channels >= 4 {
        // 4+ canais: normalizar loudness (EBU R128, TP max -3dBTP), usar primeiros 4
        filters.push(
            "[2:a]aresample=48000,loudnorm=I=-24:TP=-3:LRA=18,pan=4c|c0=c0|c1=c1|c2=c2|c3=c3[amain]".to_string(),
        );
    } else {
        // 1-3 canais (tipicamente stereo): normalizar loudness, mapear L/R, silenciar os outros
        filters.push(
            "[2:a]aresample=48000,loudnorm=I=-24:TP=-3:LRA=18,pan=4c|c0=c0|c1=c1|c2=0*c0|c3=0*c0[amain]".to_string(),
        );
    }

    // Concatenar silêncio + áudio do vídeo
    filters.push("[silence][amain]concat=n=2:v=0:a=1[aout]".to_string());

    filters
}
