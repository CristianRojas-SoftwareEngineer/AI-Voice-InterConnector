# Smoke-test Pester (v5) de install-windows.ps1 (docs/SELF-HOSTED-INSTALL.md).
#
# Valida el orquestador Install-TtsSidecar sin red ni instalación reales:
# el dot-source de install-windows.ps1 solo define funciones (guard de
# entrypoint), y los mocks recaen sobre las funciones propias del script —
# no sobre cmdlets nativos — igual que install-linux.bats mockea
# curl/sha256sum por PATH.

BeforeAll {
    . (Join-Path $PSScriptRoot "..\..\install-windows.ps1")

    # Fabrica el release simulado de la API de GitHub (no duplicar el JSON
    # por Context). Incluye un asset de Linux para verificar que la selección
    # de Windows no lo confunde.
    function New-FakeRelease {
        param([switch]$WithoutWindowsAsset)
        $assets = @(
            [pscustomobject]@{
                name                 = "ai-voice-interconnector-9.9.9-x86_64.AppImage"
                browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64.AppImage"
            }
            [pscustomobject]@{
                name                 = "SHA256SUMS.txt"
                browser_download_url = "https://example.invalid/SHA256SUMS.txt"
            }
        )
        if (-not $WithoutWindowsAsset) {
            $assets += [pscustomobject]@{
                name                 = "ai-voice-interconnector-9.9.9-x86_64-setup.exe"
                browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-setup.exe"
            }
        }
        [pscustomobject]@{ tag_name = "v9.9.9"; assets = $assets }
    }

    # Bytes fake del instalador y su hash real (Get-FileHash -InputStream),
    # para que el caso de éxito ejercite la verificación de checksum de verdad.
    $script:FakeSetupBytes = [System.Text.Encoding]::ASCII.GetBytes("fake-installer-bytes")
    $stream = [System.IO.MemoryStream]::new($script:FakeSetupBytes)
    $script:FakeSetupHash = (Get-FileHash -InputStream $stream -Algorithm SHA256).Hash.ToLowerInvariant()
    $stream.Dispose()
}

Describe "Install-TtsSidecar" {
    BeforeEach {
        Mock Install-SetupSilently {}
        Mock Update-SessionPath {}
        Mock Test-LegacyMachinePath {}
        Mock Invoke-TtsSidecarSetup {}
    }

    Context "flujo exitoso" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease }
            Mock Get-RemoteFile {
                if ($OutFile -like "*SHA256SUMS.txt") {
                    Set-Content -Path $OutFile -Value "$script:FakeSetupHash  ai-voice-interconnector-9.9.9-x86_64-setup.exe"
                } else {
                    [System.IO.File]::WriteAllBytes($OutFile, $script:FakeSetupBytes)
                }
            }
        }

        It "descarga, verifica el checksum e instala en silencio" {
            { Install-TtsSidecar } | Should -Not -Throw
            Should -Invoke Get-RemoteFile -Times 2 -Exactly
            Should -Invoke Install-SetupSilently -Times 1 -Exactly
            Should -Invoke Invoke-TtsSidecarSetup -Times 1 -Exactly
        }

        It "revisa la migración per-machine tras instalar" {
            { Install-TtsSidecar } | Should -Not -Throw
            Should -Invoke Test-LegacyMachinePath -Times 1 -Exactly
        }
    }

    Context "checksum corrupto" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease }
            Mock Get-RemoteFile {
                if ($OutFile -like "*SHA256SUMS.txt") {
                    # Hash que no corresponde a los bytes descargados.
                    Set-Content -Path $OutFile -Value ("0" * 64 + "  ai-voice-interconnector-9.9.9-x86_64-setup.exe")
                } else {
                    [System.IO.File]::WriteAllBytes($OutFile, $script:FakeSetupBytes)
                }
            }
        }

        It "aborta sin instalar" {
            { Install-TtsSidecar } | Should -Throw "*checksum*"
            Should -Invoke Install-SetupSilently -Times 0 -Exactly
        }
    }

    Context "release sin asset de Windows" {
        BeforeEach {
            Mock Resolve-LatestRelease { New-FakeRelease -WithoutWindowsAsset }
            Mock Get-RemoteFile {}
        }

        It "aborta antes de descargar nada" {
            { Install-TtsSidecar } | Should -Throw "*x86_64-setup.exe*"
            Should -Invoke Get-RemoteFile -Times 0 -Exactly
            Should -Invoke Install-SetupSilently -Times 0 -Exactly
        }
    }
}

Describe "Find-LegacyMachinePathEntry" {
    # Detección pura de la entrada per-machine heredada (pre-0.4.0),
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
