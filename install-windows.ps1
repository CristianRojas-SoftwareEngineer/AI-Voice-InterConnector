# Instalador auto-hospedado de ai-voice-interconnector para Windows.
#
# Uso:
#   irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 | iex
#
# Resuelve el último Release de GitHub, descarga el archivo .zip x86_64 y
# SHA256SUMS.txt, verifica el checksum (abortando si no coincide), lo extrae en
# %LOCALAPPDATA%\Programs\ai-voice-interconnector y registra ese directorio en
# el PATH de usuario (HKCU) de forma idempotente. La instalación es per-user
# (sin UAC). Como ya no hay instalador nativo, el propio script gestiona el
# PATH; al final ejecuta `ai-voice-interconnector setup` para ofrecer la
# descarga del modelo de voz. Ver docs/SELF-HOSTED-INSTALL.md para el diseño
# completo.
#
# La descarga por CLI (Invoke-WebRequest/Invoke-RestMethod) no aplica el
# Mark-of-the-Web, así que el archivo descargado no dispara SmartScreen
# (hallazgo verificado; solo la descarga por navegador marca ZoneId=3).
#
# Alternativa inspeccionable a `irm | iex`:
#   iwr https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 -OutFile install-windows.ps1
#   .\install-windows.ps1

param(
    [string]$Repo = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector",
    [string]$ApiUrl = "https://api.github.com/repos/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest",
    [switch]$NoSetup,
    [switch]$Check
)

$ErrorActionPreference = "Stop"

function Write-Log {
    param([string]$Message)
    Write-Host $Message
}

function Fail {
    # throw (no exit): abortable y mockeable en Pester sin matar el runner.
    param([string]$Message)
    throw "ERROR: $Message"
}

function Resolve-LatestRelease {
    # Devuelve el JSON del último release (objeto PowerShell).
    param([string]$Url)
    Write-Log "Resolviendo el último release de $Repo..."
    try {
        # GitHub API requiere User-Agent; UseBasicParsing por compatibilidad.
        return Invoke-RestMethod -Uri $Url -Headers @{ "User-Agent" = "ai-voice-interconnector-install" } -UseBasicParsing
    } catch {
        Fail "no se pudo consultar ${Url}: $_"
    }
}

function Select-WindowsAsset {
    # Elige el archivo .zip x86_64 y SHA256SUMS.txt del release. Solo hay build
    # x86_64 para Windows, así que no hay selección de arquitectura (a
    # diferencia de install-linux.sh).
    param($Release)
    $archiveAsset = $Release.assets | Where-Object { $_.name -like "ai-voice-interconnector-*-x86_64-windows.zip" } | Select-Object -First 1
    $sumsAsset = $Release.assets | Where-Object { $_.name -eq "SHA256SUMS.txt" } | Select-Object -First 1
    if (-not $archiveAsset) {
        Fail "no se encontró un archivo x86_64-windows.zip en el último release"
    }
    if (-not $sumsAsset) {
        Fail "no se encontró SHA256SUMS.txt en el último release"
    }
    return @{
        ArchiveName = $archiveAsset.name
        ArchiveUrl  = $archiveAsset.browser_download_url
        SumsUrl     = $sumsAsset.browser_download_url
    }
}

function Get-RemoteFile {
    # Descarga por CLI: sin Mark-of-the-Web, sin SmartScreen (ver cabecera).
    param([string]$Url, [string]$OutFile)
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        Fail "descarga fallida de ${Url}: $_"
    }
}

function Test-Sha256Sum {
    # Verifica el archivo contra su línea de SHA256SUMS.txt; aborta si el
    # checksum no coincide o el archivo no figura en la lista.
    param([string]$FilePath, [string]$SumsPath)
    $fileName = Split-Path -Leaf $FilePath
    $expectedLine = Get-Content $SumsPath | Where-Object { $_ -match [regex]::Escape($fileName) + '$' } | Select-Object -First 1
    if (-not $expectedLine) {
        Fail "no hay línea para $fileName en SHA256SUMS.txt"
    }
    $expectedHash = ($expectedLine -split '\s+')[0].ToLowerInvariant()
    $actualHash = (Get-FileHash -Path $FilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        Fail "el checksum de $fileName no coincide con SHA256SUMS.txt; instalación abortada"
    }
    Write-Log "Checksum verificado: $fileName"
}

function Get-InstallDir {
    # Directorio de instalación per-user (sin UAC). Constante del proyecto.
    return Join-Path $env:LOCALAPPDATA "Programs\ai-voice-interconnector"
}

function Expand-ArchiveToInstallDir {
    # Extrae el .zip (layout plano: binario + documentos de licencia) al
    # directorio de instalación, limpiándolo antes para no dejar archivos
    # huérfanos de una versión anterior.
    param([string]$ArchivePath, [string]$InstallDir)
    Write-Log "Extrayendo en $InstallDir..."
    if (Test-Path $InstallDir) {
        Remove-Item -Path $InstallDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Expand-Archive -Path $ArchivePath -DestinationPath $InstallDir -Force
    $exe = Join-Path $InstallDir "ai-voice-interconnector.exe"
    if (-not (Test-Path $exe)) {
        Fail "el binario esperado no está en el archivo extraído: $exe"
    }
}

function Add-UserPathEntry {
    # Registra el directorio en el PATH de usuario (HKCU) de forma idempotente:
    # el instalador Inno desapareció, así que el script gestiona el PATH. No
    # requiere UAC (User, no Machine).
    param([string]$Directory)
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @()
    if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ -ne '' } }
    if ($entries -contains $Directory) {
        Write-Log "El PATH de usuario ya contiene $Directory"
        return
    }
    $newPath = (@($entries) + $Directory) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    Write-Log "Añadido al PATH de usuario: $Directory"
}

function Update-SessionPath {
    # El PATH de HKCU recién escrito no llega solo a la sesión en curso: se
    # recompone desde el registro (Machine + User).
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machinePath;$userPath"
}

function Find-LegacyMachinePathEntry {
    # Lógica pura de detección (testeable en Pester sin tocar el registro):
    # devuelve la primera entrada ai-voice-interconnector del PATH de máquina, o $null.
    param([string]$MachinePath)
    if (-not $MachinePath) { return $null }
    return ($MachinePath -split ';' | Where-Object { $_ -match 'ai-voice-interconnector' } | Select-Object -First 1)
}

function Test-LegacyMachinePath {
    # Migración per-machine→per-user: los instaladores pre-0.4.0 eran
    # per-machine y dejaban su entrada en el PATH de máquina (HKLM). El
    # instalador per-user actual no puede limpiarla sin UAC
    # (PrivilegesRequired=lowest), así que se detecta y se indica el comando
    # exacto de limpieza para una PowerShell de administrador.
    $stale = Find-LegacyMachinePathEntry -MachinePath ([Environment]::GetEnvironmentVariable("Path", "Machine"))
    if ($stale) {
        Write-Log "AVISO: quedó una entrada per-machine en el PATH de una instalación anterior (pre-0.4.0): $stale"
        Write-Log "La instalación actual es per-user y no la necesita. Para quitarla, en una PowerShell de administrador:"
        Write-Log '  [Environment]::SetEnvironmentVariable("Path", (([Environment]::GetEnvironmentVariable("Path","Machine") -split ";") | Where-Object { $_ -notmatch "ai-voice-interconnector" }) -join ";", "Machine")'
    }
}

function Invoke-AIVoiceInterConnectorSetup {
    # El `setup` del binario Rust solo provisiona modelos (ya no integra el
    # PATH: eso lo hace este script). Se ofrece aquí tras extraer y registrar.
    $exe = Join-Path (Get-InstallDir) "ai-voice-interconnector.exe"
    if (-not (Test-Path $exe)) {
        Fail "no se encontró $exe tras la instalación"
    }
    Write-Log "Ejecutando 'ai-voice-interconnector setup' (chequeos + descarga del modelo si falta)..."
    & $exe setup
    if ($LASTEXITCODE -ne 0) {
        # El binario ya quedó instalado; solo falló la provisión de modelos.
        # No se aborta la instalación (Fail): se advierte de forma visible y
        # reintentable, evitando reportar éxito en falso.
        Write-Log "AVISO: 'ai-voice-interconnector setup' terminó con código $LASTEXITCODE; la provisión de modelos falló."
        Write-Log "El binario quedó instalado igualmente. Para reintentar la provisión, abre una terminal nueva y ejecuta: ai-voice-interconnector setup"
        return $false
    }
    return $true
}

function Install-AIVoiceInterConnector {
    param([switch]$Check)

    if ($Check) {
        $release = Resolve-LatestRelease -Url $ApiUrl
        $latest = $release.tag_name -replace '^v',''
        if (Get-Command ai-voice-interconnector -ErrorAction SilentlyContinue) {
            $current = (& ai-voice-interconnector --version) -split ' ' | Select-Object -Last 1
            if ($current -eq $latest) {
                Write-Log "Ya estás en la versión $latest"
            } else {
                Write-Log "$current -> $latest"
            }
        } else {
            Write-Log "no instalado -> $latest"
        }
        return
    }

    $release = Resolve-LatestRelease -Url $ApiUrl
    $asset = Select-WindowsAsset -Release $release
    Write-Log "Asset seleccionado: $($asset.ArchiveName)"

    $workDir = Join-Path $env:TEMP ("ai-voice-interconnector-install-" + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $workDir | Out-Null
    try {
        $archivePath = Join-Path $workDir $asset.ArchiveName
        $sumsPath = Join-Path $workDir "SHA256SUMS.txt"

        Write-Log "Descargando $($asset.ArchiveName)..."
        Get-RemoteFile -Url $asset.ArchiveUrl -OutFile $archivePath
        Write-Log "Descargando SHA256SUMS.txt..."
        Get-RemoteFile -Url $asset.SumsUrl -OutFile $sumsPath

        Test-Sha256Sum -FilePath $archivePath -SumsPath $sumsPath
        $installDir = Get-InstallDir
        Expand-ArchiveToInstallDir -ArchivePath $archivePath -InstallDir $installDir
        Add-UserPathEntry -Directory $installDir
        Update-SessionPath
        Test-LegacyMachinePath

        $setupOk = $true
        if (-not $NoSetup) {
            $setupOk = Invoke-AIVoiceInterConnectorSetup
        }
        if ($setupOk) {
            Write-Log "Instalación completa. Abre una terminal nueva para usar 'ai-voice-interconnector'."
        } else {
            Write-Log "Instalación del binario completa, pero la provisión de modelos falló (ver aviso anterior)."
        }
    } finally {
        Remove-Item -Path $workDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Entrypoint: con dot-source (Pester) solo se definen las funciones; con
# `irm | iex` o ejecución directa se corre la instalación.
if ($MyInvocation.InvocationName -ne '.') {
    try {
        Install-AIVoiceInterConnector -Check:$Check
    } catch {
        Write-Error $_
        exit 1
    }
}
