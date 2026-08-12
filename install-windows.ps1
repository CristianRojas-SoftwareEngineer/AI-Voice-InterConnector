# Instalador auto-hospedado de ai-voice-interconnector para Windows.
#
# Uso:
#   irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 | iex
#
# Resuelve el último Release de GitHub, descarga el instalador Inno Setup
# x86_64 y SHA256SUMS.txt, verifica el checksum (abortando si no coincide) y
# ejecuta el instalador en modo silencioso. La instalación es per-user (sin
# UAC): binarios en %LOCALAPPDATA%\Programs\ai-voice-interconnector y PATH de usuario
# (HKCU). Como la instalación silenciosa omite el checkbox de setup
# (skipifsilent), este script ejecuta `ai-voice-interconnector setup` al final para
# ofrecer la descarga del modelo de voz. Ver docs/SELF-HOSTED-INSTALL.md
# para el diseño completo.
#
# La descarga por CLI (Invoke-WebRequest/Invoke-RestMethod) no aplica el
# Mark-of-the-Web, así que el instalador descargado no dispara SmartScreen
# (hallazgo verificado; solo la descarga por navegador marca ZoneId=3).
#
# Alternativa inspeccionable a `irm | iex`:
#   iwr https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 -OutFile install-windows.ps1
#   .\install-windows.ps1

param(
    [string]$Repo = "CristianRojas-SoftwareEngineer/AI-Voice-InterConnector",
    [string]$ApiUrl = "https://api.github.com/repos/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest",
    [switch]$NoSetup
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
    # Elige el instalador x86_64 y SHA256SUMS.txt del release. Solo hay build
    # x86_64 para Windows, así que no hay selección de arquitectura (a
    # diferencia de install-linux.sh).
    param($Release)
    $setupAsset = $Release.assets | Where-Object { $_.name -like "ai-voice-interconnector-*-x86_64-setup.exe" } | Select-Object -First 1
    $sumsAsset = $Release.assets | Where-Object { $_.name -eq "SHA256SUMS.txt" } | Select-Object -First 1
    if (-not $setupAsset) {
        Fail "no se encontró un instalador x86_64-setup.exe en el último release"
    }
    if (-not $sumsAsset) {
        Fail "no se encontró SHA256SUMS.txt en el último release"
    }
    return @{
        SetupName = $setupAsset.name
        SetupUrl  = $setupAsset.browser_download_url
        SumsUrl   = $sumsAsset.browser_download_url
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

function Install-SetupSilently {
    # Instalación silenciosa per-user: sin -Verb RunAs (no hay UAC con
    # PrivilegesRequired=lowest).
    param([string]$SetupPath)
    Write-Log "Instalando (silencioso, per-user)..."
    $proc = Start-Process -FilePath $SetupPath -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Fail "el instalador terminó con código $($proc.ExitCode)"
    }
}

function Update-SessionPath {
    # El PATH de HKCU recién escrito por el instalador no llega solo a la
    # sesión en curso: se recompone desde el registro (Machine + User).
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

function Invoke-TtsSidecarSetup {
    # La instalación silenciosa omite el checkbox de setup (skipifsilent), así
    # que la provisión del modelo se ofrece aquí.
    $exe = Join-Path $env:LOCALAPPDATA "Programs\ai-voice-interconnector\ai-voice-interconnector.exe"
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

function Install-TtsSidecar {
    $release = Resolve-LatestRelease -Url $ApiUrl
    $asset = Select-WindowsAsset -Release $release
    Write-Log "Asset seleccionado: $($asset.SetupName)"

    $workDir = Join-Path $env:TEMP ("ai-voice-interconnector-install-" + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $workDir | Out-Null
    try {
        $setupPath = Join-Path $workDir $asset.SetupName
        $sumsPath = Join-Path $workDir "SHA256SUMS.txt"

        Write-Log "Descargando $($asset.SetupName)..."
        Get-RemoteFile -Url $asset.SetupUrl -OutFile $setupPath
        Write-Log "Descargando SHA256SUMS.txt..."
        Get-RemoteFile -Url $asset.SumsUrl -OutFile $sumsPath

        Test-Sha256Sum -FilePath $setupPath -SumsPath $sumsPath
        Install-SetupSilently -SetupPath $setupPath
        Update-SessionPath
        Test-LegacyMachinePath

        $setupOk = $true
        if (-not $NoSetup) {
            $setupOk = Invoke-TtsSidecarSetup
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
        Install-TtsSidecar
    } catch {
        Write-Error $_
        exit 1
    }
}
