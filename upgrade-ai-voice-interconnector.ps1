# Wrapper de auto-actualización de ai-voice-interconnector para Windows.
#
# Uso:
#   .\upgrade-ai-voice-interconnector.ps1          # actualiza (re-ejecuta install-windows.ps1)
#   .\upgrade-ai-voice-interconnector.ps1 -Check   # solo reporta la transición
#
# Detecta la versión instalada, la compara con la última de GitHub,
# reporta la transición (de -> a), y re-ejecuta install-windows.ps1
# salvo que se pase -Check.

param(
    [switch]$Check
)

$ErrorActionPreference = "Stop"

function Write-Log {
    param([string]$Message)
    Write-Host $Message
}

function Fail {
    param([string]$Message)
    throw "ERROR: $Message"
}

# Entrypoint: al dot-sourcear (Pester) solo se definen las funciones;
# al ejecutarse directamente (irm | iex o .\script.ps1) corre la lógica.
if ($MyInvocation.InvocationName -ne '.') {

    # Obtener versión instalada (vacío si no está instalado).
    $current = if (Get-Command ai-voice-interconnector -ErrorAction SilentlyContinue) {
        (& ai-voice-interconnector --version) -split ' ' | Select-Object -Last 1
    } else {
        ""
    }

    # Obtener tag_name del release más reciente.
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest" -Headers @{ "User-Agent" = "ai-voice-interconnector-install" } -UseBasicParsing
    $latest = $release.tag_name -replace '^v',''

    if ($current) {
        if ($current -eq $latest) {
            Write-Log "Ya estás en la versión $latest"
        } else {
            Write-Log "$current -> $latest"
        }
    } else {
        Write-Log "no instalado -> $latest"
    }

    if ($Check) {
        exit 0
    }

    # Re-ejecutar install-windows.ps1 (same directory as this script).
    & "$PSScriptRoot\install-windows.ps1"

}
