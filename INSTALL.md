# Encoder - Manual de Instalacao

Guia para instalar e rodar o Encoder em um novo servidor Windows.

---

## Pre-requisitos

| Componente | Versao minima | Obrigatorio |
|------------|---------------|-------------|
| Windows 10/11 ou Windows Server 2016+ | - | Sim |
| Rust (rustup + cargo) | 1.70+ | Sim (para compilar) |
| FFmpeg + FFprobe | 6.0+ | Sim |
| GPU NVIDIA + drivers CUDA | - | Nao (recomendado) |
| Arial Bold (arialbd.ttf) | - | Sim |

---

## Passo 1 - Instalar Rust

Baixe e execute o instalador:

```powershell
winget install Rustlang.Rustup
```

Ou acesse https://rustup.rs e baixe manualmente.

Apos instalar, feche e reabra o terminal. Verifique:

```bash
rustc --version
cargo --version
```

---

## Passo 2 - Instalar FFmpeg e FFprobe

### Opcao A - via winget (recomendado)

```powershell
winget install Gyan.FFmpeg
```

### Opcao B - manual

1. Baixe a versao **full** em https://www.gyan.dev/ffmpeg/builds/
2. Extraia em `C:\ffmpeg`
3. Adicione `C:\ffmpeg\bin` ao PATH do sistema:

```powershell
[Environment]::SetEnvironmentVariable("Path", $env:Path + ";C:\ffmpeg\bin", "Machine")
```

### Verificacao

Feche e reabra o terminal:

```bash
ffmpeg -version
ffprobe -version
```

Ambos devem retornar informacoes de versao sem erro.

---

## Passo 3 - Verificar fonte Arial Bold

O arquivo `C:\Windows\Fonts\arialbd.ttf` deve existir. Ele vem instalado por padrao no Windows. Se nao existir, instale o pacote de fontes Arial.

---

## Passo 4 - Clonar o repositorio

```bash
git clone https://github.com/devborlot/Encoder.git
cd Encoder
```

---

## Passo 5 - Compilar

```bash
cargo build --release
```

Isso gera dois executaveis em `target/release/`:

| Executavel | Descricao |
|------------|-----------|
| `encoder.exe` | Versao linha de comando (CLI) |
| `encoder-gui.exe` | Versao com interface grafica (GUI) |

---

## Passo 6 - Estrutura de arquivos necessaria

Garanta que a seguinte estrutura exista no diretorio de trabalho:

```
Encoder/
├── assets/
│   └── template.png          # Template da claquete (1920x1080)
├── config/
│   ├── codes.toml            # Mapeamento codigo -> registro
│   └── defaults.toml         # Valores padrao dos campos
├── target/release/
│   ├── encoder.exe
│   └── encoder-gui.exe
└── output/                   # Criado automaticamente
    └── agencia/              # Criado automaticamente
```

Os arquivos `assets/` e `config/` ja vem incluidos no repositorio.

---

## Passo 7 - Testar a instalacao

```bash
./target/release/encoder.exe --check
```

Deve retornar confirmacao de que FFmpeg e FFprobe estao acessiveis.

---

## Uso

### CLI - Video unico

```bash
./target/release/encoder.exe video.mp4 -o output -c config
```

### CLI - Lote (batch)

Crie um arquivo `lista.toml`:

```toml
videos = [
    "C:/videos/video1.mp4",
    "C:/videos/video2.mp4",
]
```

```bash
./target/release/encoder.exe batch lista.toml -o output -c config
```

### GUI

```bash
./target/release/encoder-gui.exe
```

Ou com video pre-carregado:

```bash
./target/release/encoder-gui.exe "C:/videos/video.mp4"
```

---

## Integracao com menu de contexto do Windows (opcional)

Para adicionar a opcao "Abrir com Encoder" no clique-direito de arquivos de video:

1. Edite o arquivo `install-context-menu.reg`
2. Ajuste o caminho do executavel para o local correto no servidor
3. Execute o `.reg` como administrador

Para remover: execute `uninstall-context-menu.reg`.

---

## Configuracao

### config/defaults.toml

Define os valores padrao que preenchem os campos da claquete:

```toml
produto = "VARIOS"
produtora = "POST.E"
agencia = "C3 Comunicacao"
anunciante = "SIPOLATTI"
diretor = "XXXXXXXXXXXXXXXXX"
```

Edite conforme o projeto.

### config/codes.toml

Mapeia codigos numericos no nome do arquivo para numeros de registro:

```toml
[codes]
1 = "2024017422001-4"
2 = "2024017422002-2"
```

O encoder extrai o numero do nome do arquivo (ex: `FEV_PROMO_17.mp4` -> codigo 17) e busca o registro correspondente.

---

## Saida gerada

Para cada video processado, o encoder gera:

| Arquivo | Formato | Descricao |
|---------|---------|-----------|
| `output/{titulo}.mxf` | MXF XDCAM HD422 | MPEG-2 50Mbps, PCM 24-bit 4ch, com claquete |
| `output/agencia/{titulo}.mp4` | H.264 MP4 | Versao comprimida (~7MB), sem claquete |

---

## Solucao de problemas

| Problema | Solucao |
|----------|---------|
| `ffmpeg not found` | Adicione FFmpeg ao PATH do sistema |
| `ffprobe not found` | FFprobe vem junto com FFmpeg, verifique o PATH |
| Erro de fonte | Verifique se `C:\Windows\Fonts\arialbd.ttf` existe |
| Template nao encontrado | Garanta que `assets/template.png` esta no diretorio correto |
| Encoding lento | Instale drivers NVIDIA atualizados para aceleracao CUDA |
| Erro de permissao | Execute como administrador ou ajuste permissoes do diretorio de saida |
