# Smoke-test Pester (v5) de install-windows.ps1 (docs/SELF-HOSTED-INSTALL.md).
#
# Valida el orquestador Install-AIVoiceInterConnector sin red ni instalacion reales:
# el dot-source de install-windows.ps1 solo define funciones (guard de
# entrypoint), y los mocks recaen sobre las funciones propias del script —
# no sobre cmdlets nativos — igual que install-linux.bats mockea
# curl/sha256sum por PATH.

BeforeAll {
    . (Join-Path $PSScriptRoot "..\..\install-windows.ps1")

    # Fabrica el release simulado de la API de GitHub (no duplicar el JSON
    # por Context). Incluye un asset de Linux para verificar que la seleccion
    # de Windows no lo confunde.
    function New-FakeRelease {
        param([switch]$WithoutWindowsAsset)
        $assets = @(
            [pscustomobject]@{
                name                 = "ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz"
                browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz"
            }
            [pscustomobject]@{
                name                 = "SHA256SUMS.txt"
                browser_download_url = "https://example.invalid/SHA256SUMS.txt"
            }
        )
        if (-not $WithoutWindowsAsset) {
            $assets += [pscustomobject]@{
                name                 = "ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-windows.zip"
            }
        }
        [pscustomobject]@{ tag_name = "v9.9.9"; assets = $assets }
    }

    # Bytes fake del archivo .zip y su hash real (Get-FileHash -InputStream),
    # para que el caso de EXITO ejercite la verificacion de checksum de verdad.
    $script:FakeArchiveBytes = [System.Text.Encoding]::ASCII.GetBytes("fake-archive-bytes")
    $stream = [System.IO.MemoryStream]::new($script:FakeArchiveBytes)
    $script:FakeArchiveHash = (Get-FileHash -InputStream $stream -Algorithm SHA256).Hash.ToLowerInvariant()
    $stream.Dispose()
}

Describe "Install-AIVoiceInterConnector" {
    BeforeEach {
        # Se mockean las funciones que tocan el disco/registro reales: la
        # extraccion del zip, el registro del PATH de usuario (HKCU), el
        # refresco de sesion, la migracion per-machine y la provison de modelos.
        Mock Expand-ArchiveToInstallDir {}
        Mock Add-UserPathEntry {}
        Mock Update-SessionPath {}
        Mock Test-LegacyMachinePath {}
        Mock Invoke-AIVoiceInterConnectorSetup {}
    }

    Context "flujo exitoso" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease }
            Mock Get-RemoteFile {
                if ($OutFile -like "*SHA256SUMS.txt") {
                    Set-Content -Path $OutFile -Value "$script:FakeArchiveHash  ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                } else {
                    [System.IO.File]::WriteAllBytes($OutFile, $script:FakeArchiveBytes)
                }
            }
        }

        It "descarga, verifica el checksum, extrae y registra el PATH de usuario" {
            { Install-AIVoiceInterConnector } | Should -Not -Throw
            Should -Invoke Get-RemoteFile -Times 2 -Exactly
            Should -Invoke Expand-ArchiveToInstallDir -Times 1 -Exactly
            Should -Invoke Add-UserPathEntry -Times 1 -Exactly
            Should -Invoke Invoke-AIVoiceInterConnectorSetup -Times 1 -Exactly
        }

        It "revisa la migracion per-machine tras instalar" {
            { Install-AIVoiceInterConnector } | Should -Not -Throw
            Should -Invoke Test-LegacyMachinePath -Times 1 -Exactly
        }
    }

    Context "checksum corrupto" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease }
            Mock Get-RemoteFile {
                if ($OutFile -like "*SHA256SUMS.txt") {
                    # Hash que no corresponde a los bytes descargados.
                    Set-Content -Path $OutFile -Value ("0" * 64 + "  ai-voice-interconnector-9.9.9-x86_64-windows.zip")
                } else {
                    [System.IO.File]::WriteAllBytes($OutFile, $script:FakeArchiveBytes)
                }
            }
        }

        It "aborta sin extraer ni registrar el PATH" {
            { Install-AIVoiceInterConnector } | Should -Throw "*checksum*"
            Should -Invoke Expand-ArchiveToInstallDir -Times 0 -Exactly
            Should -Invoke Add-UserPathEntry -Times 0 -Exactly
        }
    }

    Context "release sin asset de Windows" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease -WithoutWindowsAsset }
            Mock Get-RemoteFile {}
        }

        It "aborta antes de descargar nada" {
            { Install-AIVoiceInterConnector } | Should -Throw "*x86_64-windows.zip*"
            Should -Invoke Get-RemoteFile -Times 0 -Exactly
            Should -Invoke Expand-ArchiveToInstallDir -Times 0 -Exactly
        }
    }
}

Describe "Find-LegacyMachinePathEntry" {
    # Deteccion pura de la entrada per-machine heredada (pre-0.4.0),
    # sin tocar el registro real.

    It "detecta la entrada ai-voice-interconnector al inicio, en medio y al final" {
        Find-LegacyMachinePathEntry -MachinePath "C:\Program Files\ai-voice-interconnector;C:\Windows" |
            Should -Be "C:\Program Files\ai-voice-interconnector"
        Find-LegacyMachinePathEntry -MachinePath "C:\Windows;C:\Program Files\ai-voice-interconnector;C:\Tools" |
            Should -Be "C:\Program Files\ai-voice-interconnector"
        Find-LegacyMachinePathEntry -MachinePath "C:\Windows;C:\Program Files\ai-voice-interconnector" |
            Should -Be "C:\Program Files\ai-voice-interconnector"
    }

    It "devuelve nulo cuando no hay entrada heredada" {
        Find-LegacyMachinePathEntry -MachinePath "C:\Windows;C:\Tools" | Should -BeNullOrEmpty
    }

    It "devuelve nulo con un PATH de máquina vacío" {
        Find-LegacyMachinePathEntry -MachinePath "" | Should -BeNullOrEmpty
    }
}

Describe "modo -Check" {
    BeforeEach {
        Mock Resolve-LatestRelease { New-FakeRelease }
        Mock Expand-ArchiveToInstallDir {}
        Mock Add-UserPathEntry {}
        Mock Update-SessionPath {}
        Mock Test-LegacyMachinePath {}
        Mock Invoke-AIVoiceInterConnectorSetup {}
    }

    It "reporta transición cuando hay versión nueva" {
        $Global:_UpgradeCheckMessages = @()
        Mock Write-Log { param([string]$Message) $Global:_UpgradeCheckMessages += $Message }
        Mock ai-voice-interconnector { return "ai-voice-interconnector 1.0.0" }

        { Install-AIVoiceInterConnector -Check } | Should -Not -Throw

        Should -Invoke Expand-ArchiveToInstallDir -Times 0 -Exactly
        Should -Invoke Add-UserPathEntry -Times 0 -Exactly
        Should -Invoke Invoke-AIVoiceInterConnectorSetup -Times 0 -Exactly
        $Global:_UpgradeCheckMessages -join "`n" | Should -Match "1\.0\.0.*9\.9\.9"
    }

    It "reporta 'ya estás en la versión' cuando no hay actualización" {
        $Global:_UpgradeCheckMessages = @()
        Mock Write-Log { param([string]$Message) $Global:_UpgradeCheckMessages += $Message }
        Mock ai-voice-interconnector { return "ai-voice-interconnector 9.9.9" }

        { Install-AIVoiceInterConnector -Check } | Should -Not -Throw

        Should -Invoke Expand-ArchiveToInstallDir -Times 0 -Exactly
        $Global:_UpgradeCheckMessages -join "`n" | Should -Match "Ya estás en la versión"
    }
}
