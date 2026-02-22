use std::fmt;

#[derive(Debug)]
#[allow(dead_code)]
pub enum EncoderError {
    FfmpegNotFound,
    FfprobeError(String),
    CodeNotFound(u32),
    TemplateNotFound(String),
    EncodingFailed(String),
    ConfigError(String),
}

impl fmt::Display for EncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FfmpegNotFound => write!(f, "FFmpeg/FFprobe não encontrado no PATH"),
            Self::FfprobeError(msg) => write!(f, "Erro ao ler metadados do vídeo: {msg}"),
            Self::CodeNotFound(code) => {
                write!(f, "Código {code} não encontrado na tabela de registros")
            }
            Self::TemplateNotFound(path) => {
                write!(f, "Template da claquete não encontrado: {path}")
            }
            Self::EncodingFailed(msg) => write!(f, "Falha no encoding: {msg}"),
            Self::ConfigError(msg) => write!(f, "Erro na configuração: {msg}"),
        }
    }
}

impl std::error::Error for EncoderError {}
