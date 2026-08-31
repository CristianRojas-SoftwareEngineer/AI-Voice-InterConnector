# Smoke-test Pester (v5) de upgrade-ai-voice-interconnector.ps1
# (docs/SELF-HOSTED-INSTALL.md).
#
# Valida el wrapper sin red ni instalacion reales:
# se mockea Invoke-RestMethod (API de GitHub) y se verifica que
# -Check reporta la transicion sin instalar, y sin -Check re-ejecuta
# install-windows.ps1 (que a su vez mockea sus funciones internas).
#
# Ejecutar: pwsh -File tests/installer/upgrade-windows.tests.ps1

# Funcion auxiliar para el mock de Resolve-LatestRelease.
function New-FakeRelease {
    [pscustomobject]@{
        tag_name = "v9.9.9"
        assets = @(
            [pscustomobject]@{
                name                 = "ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-windows.zip"
            }
            [pscustomobject]@{
                name                 = "SHA256SUMS.txt"
                browser_download_url = "https://example.invalid/SHA256SUMS.txt"
            }
        )
    }
}

Describe "upgrade-ai-voice-interconnector.ps1" {
    BeforeAll {
        # Al dot-sourcear, el guard de entrypoint ($MyInvocation.InvocationName -ne '.')
        # hace que solo se definan las funciones Write-Log/Fail; la logica no corre.
        . (Join-Path $PSScriptRoot "..\..\upgrade-ai-voice-interconnector.ps1")

        # La logica de instalacion reside en install-windows.ps1; dot-sourcearlo
        # para que las funciones (Expand-ArchiveToInstallDir, etc.) existan en
        # sesion y los mocks de BeforeEach las intercepten.
        . (Join-Path $PSScriptRoot "..\..\install-windows.ps1")
    }

    # Mocks de funciones de install-windows.ps1 que tocan el disco/registro reales.
    BeforeEach {
        Mock Expand-ArchiveToInstallDir {}
        Mock Add-UserPathEntry {}
        Mock Update-SessionPath {}
        Mock Test-LegacyMachinePath {}
        Mock Invoke-AIVoiceInterConnectorSetup {}
    }

    Describe "modo -Check" {
        BeforeEach {
            Mock Invoke-RestMethod { & {
                [pscustomobject]@{
                    tag_name = "v9.9.9"
                    assets = @(
                        [pscustomobject]@{
                            name                 = "ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                            browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                        }
                        [pscustomobject]@{
                            name                 = "SHA256SUMS.txt"
                            browser_download_url = "https://example.invalid/SHA256SUMS.txt"
                        }
                    )
                }
            } }
            # Write-Host es llamado por Write-Log (definida en upgrade-ai-voice-interconnector.ps1);
            # capturamos la salida para verificar la transicion reportada.
            $Global:_UpgradeCheckMessages = @()
            Mock Write-Host { param([string]$Object) $Global:_UpgradeCheckMessages += $Object }
        }

        It "reporta transicion sin instalar" {
            $Global:_UpgradeCheckMessages = @()
            Mock ai-voice-interconnector { return "ai-voice-interconnector 1.0.0" }

            { .\upgrade-ai-voice-interconnector.ps1 -Check } | Should -Not -Throw
            $LASTEXITCODE | Should -Be 0

            Should -Invoke Expand-ArchiveToInstallDir -Times 0 -Exactly
            Should -Invoke Add-UserPathEntry -Times 0 -Exactly
            Should -Invoke Invoke-AIVoiceInterConnectorSetup -Times 0 -Exactly
            $Global:_UpgradeCheckMessages -join "`n" | Should -Match "1\.0\.0.*9\.9\.9"
        }
    }

    Describe "modo install" {
        BeforeEach {
            Mock Invoke-RestMethod { & {
                [pscustomobject]@{
                    tag_name = "v9.9.9"
                    assets = @(
                        [pscustomobject]@{
                            name                 = "ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                            browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                        }
                        [pscustomobject]@{
                            name                 = "SHA256SUMS.txt"
                            browser_download_url = "https://example.invalid/SHA256SUMS.txt"
                        }
                    )
                }
            } }
            Mock ai-voice-interconnector { return "ai-voice-interconnector 1.0.0" }
            Mock Resolve-LatestRelease { & {
                [pscustomobject]@{
                    tag_name = "v9.9.9"
                    assets = @(
                        [pscustomobject]@{
                            name                 = "ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                            browser_download_url = "https://example.invalid/ai-voice-interconnector-9.9.9-x86_64-windows.zip"
                        }
                        [pscustomobject]@{
                            name                 = "SHA256SUMS.txt"
                            browser_download_url = "https://example.invalid/SHA256SUMS.txt"
                        }
                    )
                }
            } }
            # Get-RemoteFile y Test-Sha256Sum tocan red/disco reales; mockearlos.
            Mock Get-RemoteFile {}
            Mock Test-Sha256Sum {}
        }

        It "re-ejecuta install-windows.ps1 y completa sin error" {
            { .\upgrade-ai-voice-interconnector.ps1 } | Should -Not -Throw
            $LASTEXITCODE | Should -Be 0
        }
    }
}
