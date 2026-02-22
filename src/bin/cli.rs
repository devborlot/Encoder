use anyhow::{bail, Context, Result};
use chrono::Datelike;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use encoder_lib::{config, encoder, metadata, slate};

#[derive(Parser)]
#[command(name = "encoder", about = "Automação de claquete + encoding MXF XDCAM HD422")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Caminho do vídeo MP4 de entrada
    video: Option<PathBuf>,

    /// Diretório de saída (default: ./output)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Diretório de configuração (default: ./config)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Verificar se FFmpeg/FFprobe estão no PATH
    #[arg(long)]
    check: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Processar múltiplos vídeos a partir de um arquivo de lista
    Batch {
        /// Caminho do arquivo de lista (.toml)
        lista: PathBuf,

        /// Diretório de saída
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Diretório de configuração
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.check {
        return check_dependencies();
    }

    match cli.command {
        Some(Commands::Batch {
            lista,
            output,
            config,
        }) => {
            let config_dir = config.unwrap_or_else(|| PathBuf::from("config"));
            let output_dir = output.unwrap_or_else(|| PathBuf::from("output"));
            run_batch(&lista, &config_dir, &output_dir)
        }
        None => {
            let video = cli.video.context(
                "Informe o caminho do vídeo. Uso: encoder <video.mp4> [--output <dir>]",
            )?;
            let config_dir = cli.config.unwrap_or_else(|| PathBuf::from("config"));
            let output_dir = cli.output.unwrap_or_else(|| PathBuf::from("output"));
            process_video(&video, &config_dir, &output_dir)
        }
    }
}

fn check_dependencies() -> Result<()> {
    match metadata::check_ffmpeg() {
        Ok(()) => {
            println!("FFmpeg e FFprobe encontrados no PATH.");
            Ok(())
        }
        Err(e) => {
            eprintln!("ERRO: {e}");
            bail!("Dependências não satisfeitas");
        }
    }
}

fn process_video(video_path: &Path, config_dir: &Path, output_dir: &Path) -> Result<()> {
    // 1. Verificar FFmpeg
    metadata::check_ffmpeg().context("FFmpeg/FFprobe não encontrado no PATH")?;

    // 2. Verificar que o vídeo existe
    if !video_path.exists() {
        bail!("Arquivo de vídeo não encontrado: {}", video_path.display());
    }

    // 3. Carregar configurações
    let defaults = config::load_defaults(config_dir)?;
    let codes = config::load_codes(config_dir)?;

    // 4. Ler metadados do vídeo
    println!("Lendo metadados de {}...", video_path.display());
    let meta = metadata::probe(video_path)?;
    println!(
        "  Resolução: {}x{} | FPS: {}/{} | Duração: {}s | Áudio: {}",
        meta.width,
        meta.height,
        meta.fps_num,
        meta.fps_den,
        meta.duration_secs,
        if meta.has_audio {
            format!("{} canais", meta.audio_channels)
        } else {
            "sem áudio".to_string()
        }
    );

    // 5. Extrair código do nome do arquivo
    let filename = video_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Nome de arquivo inválido")?;

    let registro = resolve_registro(filename, &codes)?;
    println!("  Registro: {registro}");

    // 6. Gerar claquete
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let template_path = encoder_lib::find_template(&exe_dir)?;
    let temp_slate = std::env::temp_dir().join("encoder_temp_slate.png");

    let titulo = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let duracao = meta.duration_display();
    let ano = chrono::Local::now().year().to_string();

    let slate_data = slate::SlateData::new(titulo, &duracao, &registro, &ano, &defaults);

    println!("Gerando claquete...");
    slate::generate_slate(&template_path, &slate_data, &temp_slate)?;

    // 7. Criar diretório de saída
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Não foi possível criar diretório: {}", output_dir.display()))?;

    let output_filename = format!("{}.mxf", titulo);
    let output_path = output_dir.join(&output_filename);

    // 8. Encodar MXF
    encoder::encode(&temp_slate, video_path, &output_path, &meta)?;

    // 9. Encodar versão agência (MP4 sem claquete)
    let agency_dir = output_dir.join("agencia");
    std::fs::create_dir_all(&agency_dir)
        .with_context(|| format!("Não foi possível criar diretório: {}", agency_dir.display()))?;
    let agency_path = agency_dir.join(format!("{}.mp4", titulo));
    encoder::encode_agency(video_path, &agency_path, &meta)?;

    // 10. Limpar temporários
    let _ = std::fs::remove_file(&temp_slate);

    println!("\nResultado:");
    println!("  MXF: {}", output_path.display());
    println!("  Agência: {}", agency_path.display());
    println!(
        "  Duração total: {}s (5s claquete + 2s preto + {}s vídeo)",
        7 + meta.duration_secs,
        meta.duration_secs
    );

    Ok(())
}

fn resolve_registro(filename: &str, codes: &HashMap<u32, String>) -> Result<String> {
    let code = config::extract_code_from_filename(filename);

    match code {
        Some(c) => match config::lookup_registro(c, codes) {
            Some(registro) => Ok(registro),
            None => {
                eprintln!(
                    "Código {c} (extraído de \"{filename}\") não encontrado na tabela de registros."
                );
                ask_registro(c)
            }
        },
        None => {
            eprintln!("Não foi possível extrair código numérico de \"{filename}\".");
            ask_registro_manual()
        }
    }
}

fn ask_registro(code: u32) -> Result<String> {
    eprint!("Digite o número de registro para o código {code}: ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let registro = input.trim().to_string();
    if registro.is_empty() {
        bail!("Registro não pode ser vazio");
    }
    Ok(registro)
}

fn ask_registro_manual() -> Result<String> {
    eprint!("Digite o número de registro manualmente: ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let registro = input.trim().to_string();
    if registro.is_empty() {
        bail!("Registro não pode ser vazio");
    }
    Ok(registro)
}

fn run_batch(lista_path: &Path, config_dir: &Path, output_dir: &Path) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct BatchFile {
        videos: Vec<String>,
    }

    let content = std::fs::read_to_string(lista_path)
        .with_context(|| format!("Não foi possível ler {}", lista_path.display()))?;
    let batch: BatchFile = toml::from_str(&content)
        .with_context(|| format!("Erro ao parsear {}", lista_path.display()))?;

    println!("Processando {} vídeos...\n", batch.videos.len());

    let mut errors = Vec::new();
    for (i, video) in batch.videos.iter().enumerate() {
        println!(
            "=== [{}/{}] {} ===",
            i + 1,
            batch.videos.len(),
            video
        );
        let path = PathBuf::from(video);
        if let Err(e) = process_video(&path, config_dir, output_dir) {
            eprintln!("ERRO: {e}");
            errors.push((video.clone(), e));
        }
        println!();
    }

    if errors.is_empty() {
        println!("Todos os vídeos processados com sucesso!");
    } else {
        eprintln!("\n{} erro(s) encontrado(s):", errors.len());
        for (video, err) in &errors {
            eprintln!("  - {video}: {err}");
        }
    }

    Ok(())
}
