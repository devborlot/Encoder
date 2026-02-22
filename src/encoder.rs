use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

use crate::metadata::VideoMetadata;

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

fn build_filter_complex(metadata: &VideoMetadata, silence_duration: i32) -> String {
    // Vídeo: scale → fps → yuv422p → interlace para todos os inputs
    let video_filters = [
        // Slate
        "[0:v]scale=1920:1080,fps=30000/1001,format=yuv422p,setfield=tff[slate]",
        // Black
        "[1:v]format=yuv422p,setfield=tff[black]",
        // Main video (hwdownload from NVDEC, then CPU filters)
        "[2:v]hwdownload,format=nv12,scale=1920:1080,fps=30000/1001,format=yuv422p,setfield=tff[main]",
        // Concat vídeo
        "[slate][black][main]concat=n=3:v=1:a=0[vout]",
    ];

    // Áudio: depende do source
    let audio_filters = build_audio_filters(metadata, silence_duration);

    let mut parts: Vec<String> = video_filters.iter().map(|s| s.to_string()).collect();
    parts.extend(audio_filters);

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
        // 4+ canais: usar os primeiros 4 (referência por índice para compatibilidade)
        filters.push(
            "[2:a]aresample=48000,pan=4c|c0=c0|c1=c1|c2=c2|c3=c3[amain]".to_string(),
        );
    } else {
        // 1-3 canais (tipicamente stereo): mapear L/R nos 2 primeiros, silenciar os outros
        filters.push(
            "[2:a]aresample=48000,pan=4c|c0=c0|c1=c1|c2=0*c0|c3=0*c0[amain]".to_string(),
        );
    }

    // Concatenar silêncio + áudio do vídeo
    filters.push("[silence][amain]concat=n=2:v=0:a=1[aout]".to_string());

    filters
}
