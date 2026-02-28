@echo off
setlocal enabledelayedexpansion

:: Detecta o diretorio onde o .bat esta
set "ENCODER_DIR=%~dp0"
:: Remove a barra final
set "ENCODER_DIR=%ENCODER_DIR:~0,-1%"

set "EXE_PATH=%ENCODER_DIR%\encoder-gui.exe"
set "CONFIG_DIR=%ENCODER_DIR%\config"

:: Verifica se o executavel existe
if not exist "%EXE_PATH%" (
    echo [ERRO] encoder-gui.exe nao encontrado em: %ENCODER_DIR%
    echo Coloque este .bat na mesma pasta do encoder-gui.exe
    pause
    exit /b 1
)

:: Escapa as barras para o .reg
set "REG_PATH=%EXE_PATH:\=\\%"

:: Gera o .reg temporario
set "REG_FILE=%TEMP%\encoder-context-menu.reg"

:: Detecta subpastas de clientes (que contenham defaults.toml e codes.toml)
set "CLIENT_COUNT=0"
set "CLIENTS="
if exist "%CONFIG_DIR%" (
    for /d %%D in ("%CONFIG_DIR%\*") do (
        if exist "%%D\defaults.toml" if exist "%%D\codes.toml" (
            set /a CLIENT_COUNT+=1
            set "CLIENTS=!CLIENTS! %%~nxD"
        )
    )
)

:: Extensoes suportadas
set "EXTS=.mp4 .mov .avi .mkv"

if %CLIENT_COUNT% EQU 0 (
    :: === Modo simples: sem clientes, entrada unica ===
    (
    echo Windows Registry Editor Version 5.00
    echo.
    for %%E in (%EXTS%) do (
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI]
        echo @="Abrir com Encoder"
        echo "Icon"="%REG_PATH%"
        echo.
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\command]
        echo @="\"%REG_PATH%\" \"%%1\""
        echo.
    )
    ) > "%REG_FILE%"

    echo Modo simples: nenhuma subpasta de cliente detectada.
) else (
    :: === Modo multi-cliente: submenu cascata ===
    (
    echo Windows Registry Editor Version 5.00
    echo.
    for %%E in (%EXTS%) do (
        :: Entrada principal com submenu
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI]
        echo @="Abrir com Encoder"
        echo "Icon"="%REG_PATH%"
        echo "SubCommands"=""
        echo.
        :: Shell container
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\shell]
        echo.
        :: Entrada "Padrao" (config raiz)
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\shell\Padrao]
        echo @="Padrao"
        echo.
        echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\shell\Padrao\command]
        echo @="\"%REG_PATH%\" \"%%1\""
        echo.
        :: Entradas por cliente
        for %%C in (!CLIENTS!) do (
            echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\shell\%%C]
            echo @="%%C"
            echo.
            echo [HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\%%E\shell\EncoderGUI\shell\%%C\command]
            echo @="\"%REG_PATH%\" --client %%C \"%%1\""
            echo.
        )
    )
    ) > "%REG_FILE%"

    echo Modo multi-cliente: detectados %CLIENT_COUNT% cliente(s^):!CLIENTS!
)

:: Importa o .reg
regedit /s "%REG_FILE%"

echo.
echo [OK] Menu de contexto instalado com sucesso!
echo Caminho: %EXE_PATH%
echo.
echo Clique direito em .mp4, .mov, .avi ou .mkv para usar "Abrir com Encoder"
pause
