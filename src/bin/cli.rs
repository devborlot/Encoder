use anyhow::{bail, Context, Result};
use chrono::Datelike;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use encoder_lib::{config, encoder, metadata, peach, slate};

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

    /// Perfil de cliente (subpasta em config/)
    #[arg(short = 'C', long)]
    client: Option<String>,

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

        /// Perfil de cliente (subpasta em config/)
        #[arg(short = 'C', long)]
        client: Option<String>,
    },
    /// Comandos de integração com a plataforma Peach
    Peach {
        #[command(subcommand)]
        action: PeachAction,
    },
}

#[derive(Subcommand)]
enum PeachAction {
    /// Validar credenciais e sessão
    Login {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Enviar um VT já encodado (MXF) para o Peach
    Upload {
        /// Caminho do arquivo MXF
        video: PathBuf,
        /// Perfil de cliente (subpasta em config/)
        #[arg(short = 'C', long)]
        client: String,
        /// Diretório de configuração
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Override do código ANCINE (sem traço). Se omitido, extrai do nome do arquivo.
        #[arg(long)]
        codigo: Option<String>,
    },
    /// Distribuir spots já uploadados para emissoras
    Send {
        /// Spot IDs (numéricos, extraídos do destination do upload)
        #[arg(required = true)]
        spots: Vec<u64>,
        /// Perfil de cliente (subpasta em config/) — usa os destinos configurados em [peach.destinos]
        #[arg(short = 'C', long)]
        client: String,
        /// Diretório de configuração
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Lista CSV de IDs de destinos pra usar (override). Ex: "BR_GLOBO_112,BR1230". Se omitido, usa todos do [peach.destinos].
        #[arg(long, value_delimiter = ',')]
        destinos: Option<Vec<String>>,
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
            client,
        }) => {
            let config_dir = config.unwrap_or_else(|| PathBuf::from("config"));
            let client_ref = client.as_deref();
            let output_dir = resolve_output_dir(output, &config_dir, None, client_ref);
            run_batch(&lista, &config_dir, &output_dir, client_ref)
        }
        Some(Commands::Peach { action }) => run_peach(action),
        None => {
            let video = cli.video.context(
                "Informe o caminho do vídeo. Uso: encoder <video.mp4> [--output <dir>]",
            )?;
            let config_dir = cli.config.unwrap_or_else(|| PathBuf::from("config"));
            let client_ref = cli.client.as_deref();
            let output_dir = resolve_output_dir(cli.output, &config_dir, Some(&video), client_ref);
            process_video(&video, &config_dir, &output_dir, client_ref)
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

fn process_video(video_path: &Path, config_dir: &Path, output_dir: &Path, client: Option<&str>) -> Result<()> {
    // 1. Verificar FFmpeg
    metadata::check_ffmpeg().context("FFmpeg/FFprobe não encontrado no PATH")?;

    // 2. Verificar que o vídeo existe
    if !video_path.exists() {
        bail!("Arquivo de vídeo não encontrado: {}", video_path.display());
    }

    // 3. Carregar configurações
    let defaults = config::load_defaults_for(config_dir, client)?;
    let codes = config::load_codes_for(config_dir, client)?;

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

/// Resolve o diretório de saída com a seguinte prioridade:
/// 1. Flag --output da linha de comando
/// 2. Campo `output` no defaults.toml (do cliente, se informado)
/// 3. Mesmo diretório do vídeo de entrada (ou "." se não houver vídeo)
fn resolve_output_dir(flag: Option<PathBuf>, config_dir: &Path, video: Option<&PathBuf>, client: Option<&str>) -> PathBuf {
    // 1. Flag explícita
    if let Some(dir) = flag {
        return dir;
    }

    // 2. Config
    if let Ok(defaults) = config::load_defaults_for(config_dir, client) {
        if !defaults.output.is_empty() {
            return PathBuf::from(&defaults.output);
        }
    }

    // 3. Mesmo diretório do vídeo
    if let Some(video_path) = video {
        if let Some(parent) = video_path.parent() {
            if !parent.as_os_str().is_empty() {
                return parent.to_path_buf();
            }
        }
    }

    PathBuf::from(".")
}

fn run_batch(lista_path: &Path, config_dir: &Path, output_dir: &Path, client: Option<&str>) -> Result<()> {
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
        if let Err(e) = process_video(&path, config_dir, output_dir, client) {
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

// ----------------- Peach -----------------

fn run_peach(action: PeachAction) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("falha ao iniciar runtime tokio")?;
    rt.block_on(async {
        match action {
            PeachAction::Login { config } => peach_login(config).await,
            PeachAction::Upload {
                video,
                client,
                config,
                codigo,
            } => peach_upload(video, client, config, codigo).await,
            PeachAction::Send {
                spots,
                client,
                config,
                destinos,
            } => peach_send(spots, client, config, destinos).await,
        }
    })
}

async fn peach_login(config: Option<PathBuf>) -> Result<()> {
    let config_dir = config.unwrap_or_else(|| PathBuf::from("config"));
    let creds = peach::PeachCredentials::load(&config_dir)?;

    println!("Fazendo login em latam.peachvideo.com como {}...", creds.email);
    let client = peach::PeachClient::new()?;
    let session = client.login(&creds.email, &creds.password).await?;

    println!("\n✅ Login OK");
    println!("  Usuário: {} <{}>", session.nombre_usuario_activo, session.id_email);
    println!("  Empresa: {} ({})", session.empresa_nombre, session.id_empresa);
    println!("  Privilégios: {:?}", session.privilegios);
    println!("  Extensões permitidas: {:?}", session.extension_permitida);
    Ok(())
}

async fn peach_upload(
    video: PathBuf,
    client_name: String,
    config: Option<PathBuf>,
    codigo_override: Option<String>,
) -> Result<()> {
    if !video.exists() {
        bail!("Arquivo não encontrado: {}", video.display());
    }

    let config_dir = config.unwrap_or_else(|| PathBuf::from("config"));
    let creds = peach::PeachCredentials::load(&config_dir)?;

    // Carrega defaults com bloco [peach]
    let defaults_full =
        peach::config::DefaultsWithPeach::load(&config_dir, Some(&client_name))?;
    let peach_cfg = defaults_full.peach.ok_or_else(|| {
        anyhow::anyhow!(
            "Cliente '{}' não tem bloco [peach] configurado em defaults.toml",
            client_name
        )
    })?;

    // Carrega tabela de códigos do cliente
    let codes = config::load_codes_for(&config_dir, Some(&client_name))?;

    // Probe metadata do vídeo
    println!("Lendo metadados de {}...", video.display());
    let meta = metadata::probe(&video)?;

    let filename = video
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("nome de arquivo inválido"))?;
    let pieza = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    // Resolve código ANCINE
    let codigo = match codigo_override {
        Some(c) => c,
        None => peach::resolve_codigo_from_filename(filename, &codes).ok_or_else(|| {
            anyhow::anyhow!(
                "Não foi possível extrair código de '{}' a partir da tabela de códigos do cliente",
                filename
            )
        })?,
    };

    let framerate_str = format!("{:.2}", meta.fps_num as f64 / meta.fps_den as f64);

    // Se for MXF gerado pelo nosso encoder, desconta a claquete (5s slate + 2s preto)
    // pra obter a duração comercial — que é o que o Peach espera no campo `segundos`.
    let is_mxf = video
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("mxf"))
        .unwrap_or(false);
    let commercial_secs = if is_mxf {
        meta.duration_secs
            .saturating_sub(encoder::SLATE_BLACK_TOTAL_SECS)
    } else {
        meta.duration_secs
    };

    let params = peach::UploadParams {
        video_path: &video,
        pieza,
        codigo: &codigo,
        framerate: &framerate_str,
        duration_secs: commercial_secs,
    };

    println!("Iniciando upload no Peach:");
    println!("  pieza:    {}", pieza);
    println!("  codigo:   {}", codigo);
    if is_mxf {
        println!(
            "  duração:  {}s (total {}s - {}s claquete)",
            commercial_secs,
            meta.duration_secs,
            encoder::SLATE_BLACK_TOTAL_SECS
        );
    } else {
        println!("  duração:  {}s", commercial_secs);
    }
    println!("  fps:      {}", framerate_str);
    println!("  cliente:  {}", client_name);

    // Login
    println!("\nFazendo login...");
    let pclient = peach::PeachClient::new()?;
    let session = pclient.login(&creds.email, &creds.password).await?;
    println!("✅ Logado como {} ({})", session.nombre_usuario_activo, session.id_empresa);

    // Init upload (obtém STS)
    println!("\nObtendo credenciais STS...");
    let sts = pclient
        .init_upload(&params, &peach_cfg, &creds.productora_id)
        .await?;
    println!("✅ id_envio: {}", sts.id_envio);
    println!("   destination: {}", sts.destination);

    // Upload S3
    println!("\nUpload S3 multipart...");
    let file_size = std::fs::metadata(&video)?.len();
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    let last_pct = Arc::new(AtomicU64::new(0));
    let last_pct_cb = Arc::clone(&last_pct);

    peach::upload::s3_multipart_upload(&video, &sts, move |sent, total| {
        let pct = sent * 100 / total;
        let prev = last_pct_cb.load(Ordering::Relaxed);
        if pct >= prev + 5 || sent == total {
            print!("\r  {} / {} bytes ({}%)", sent, total, pct);
            use std::io::Write;
            let _ = std::io::stdout().flush();
            last_pct_cb.store(pct, Ordering::Relaxed);
        }
    })
    .await?;

    println!("\n\n✅ Upload concluído!");
    println!("  Arquivo: {} ({} bytes)", video.display(), file_size);
    if let Some(sid) = sts.spot_id() {
        println!(
            "  spot_id: {sid}  (use em `encoder peach send {sid} --client {client_name}`)"
        );
    }
    println!("  Verifique no portal latam.peachvideo.com em 'Subir'.");

    Ok(())
}

async fn peach_send(
    spots: Vec<u64>,
    client_name: String,
    config: Option<PathBuf>,
    destinos_filter: Option<Vec<String>>,
) -> Result<()> {
    let config_dir = config.unwrap_or_else(|| PathBuf::from("config"));
    let creds = peach::PeachCredentials::load(&config_dir)?;

    let defaults_full =
        peach::config::DefaultsWithPeach::load(&config_dir, Some(&client_name))?;
    let peach_cfg = defaults_full.peach.ok_or_else(|| {
        anyhow::anyhow!(
            "Cliente '{}' não tem bloco [peach] configurado em defaults.toml",
            client_name
        )
    })?;

    if peach_cfg.destinos.is_empty() {
        bail!(
            "Cliente '{}' não tem destinos configurados em [peach.destinos]. Adicione `hd = [...]` no defaults.toml.",
            client_name
        );
    }

    // Filtra os destinos: se --destinos foi passado, usa apenas os IDs dessa lista
    let filter_set: Option<std::collections::HashSet<String>> = destinos_filter.map(|v| {
        v.into_iter().map(|s| s.trim().to_string()).collect()
    });

    let select_destinos = |list: &Vec<peach::DestinoEntry>| -> Vec<String> {
        list.iter()
            .filter_map(|d| {
                let id = d.id();
                if let Some(set) = &filter_set {
                    if !set.contains(id) {
                        return None;
                    }
                }
                Some(id.to_string())
            })
            .collect()
    };

    let hd_ids = select_destinos(&peach_cfg.destinos.hd);
    let sd_ids = select_destinos(&peach_cfg.destinos.sd);

    if hd_ids.is_empty() && sd_ids.is_empty() {
        bail!("Nenhum destino selecionado após o filtro --destinos. IDs disponíveis: {:?}", peach_cfg.destinos.all_ids());
    }

    println!(
        "Distribuindo {} spot(s) → {} HD + {} SD destino(s):",
        spots.len(),
        hd_ids.len(),
        sd_ids.len(),
    );
    for sid in &spots {
        println!("  spot_id: {sid}");
    }
    for d in &peach_cfg.destinos.hd {
        if hd_ids.contains(&d.id().to_string()) {
            println!("  HD → {} ({})", d.id(), d.label());
        }
    }
    for d in &peach_cfg.destinos.sd {
        if sd_ids.contains(&d.id().to_string()) {
            println!("  SD → {} ({})", d.id(), d.label());
        }
    }

    println!("\nFazendo login...");
    let client = peach::PeachClient::new()?;
    let session = client.login(&creds.email, &creds.password).await?;
    println!(
        "✅ Logado como {} ({})",
        session.nombre_usuario_activo, session.id_empresa
    );

    let req = peach::SendRequest {
        spot_ids: &spots,
        destinos_hd: &hd_ids,
        destinos_sd: &sd_ids,
    };

    println!("\nDistribuindo...");
    let summary = client.send_spots(&req, &peach_cfg).await?;
    println!("\n✅ {summary}");

    // Log CSV
    let all_destinos: Vec<&str> = hd_ids.iter().chain(sd_ids.iter()).map(|s| s.as_str()).collect();
    let log_entry = peach::send::SendLogEntry {
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        pieza: spots.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("+"),
        codigo: String::new(),
        spot_id: spots[0],
        destinos: all_destinos.join(";"),
        id_envio: String::new(),
        agencia_url: String::new(),
    };
    let output_dir = config_dir.join(&client_name);
    let _ = peach::send::append_send_log(&output_dir, &log_entry);

    println!("\nVerifique no portal latam.peachvideo.com → Reportes.");
    Ok(())
}
