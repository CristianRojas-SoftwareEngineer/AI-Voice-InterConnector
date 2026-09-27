# Suite del bootstrap Windows packaging/bootstrap/install.ps1 (Pester v5).
#
# Corre contra el servidor falso local (support/Serve.ps1, HTTP en 127.0.0.1)
# con el asset de Windows, SHA256SUMS.txt coherente o corrupto y raíces
# reubicadas a temporales. Cubre los criterios 3 (checksum con staging
# borrado), 4 (plataforma no soportada antes de descargar), 5 (binario
# incompatible con diagnóstico), 8 (sin efectos en la sesión salvo el PATH de
# esa sesión; la consola sobrevive al error bajo tubería) y 10 (paso de
# parámetros y variables). Sin casos --check y sin red real.
#
# Ejecutar: Invoke-Pester tests/bootstrap/install.tests.ps1 -CI

BeforeAll {
    . (Join-Path $PSScriptRoot "support/Setup.ps1")

    $script:RepoRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:InstallPs1 = Join-Path $script:RepoRoot "packaging/bootstrap/install.ps1"
    $script:ServeScript = Join-Path $PSScriptRoot "support/Serve.ps1"

    # Falso ejecutable (doble de `self install`), compilado una vez por ejecución.
    $script:RunDir = Join-Path ([IO.Path]::GetTempPath()) ("avi-bootstrap-test-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $script:RunDir | Out-Null
    $script:FakeExe = Join-Path $script:RunDir "fake-exe-bin.exe"
    New-FakeExe -Path $script:FakeExe

    function Get-TestEnvBase {
        param($TestDir)
        $programDir = Join-Path $TestDir "install"
        return @{
            AVI_DOWNLOAD_BASE_URL = $script:LastBaseUrl
            AVI_INSTALL_DIR       = $programDir
            AVI_BIN_DIR           = (Join-Path $TestDir "bin")
            AVI_DATA_DIR          = (Join-Path $TestDir "data")
            AVI_CACHE_DIR         = (Join-Path $TestDir "cache")
            AVI_FAKE_LOG          = (Join-Path $TestDir "fake.log")
            AVI_VERSION           = $null
            AVI_NO_SETUP          = $null
            AVI_NO_MODIFY_PATH    = $null
            AVI_YES               = $null
            AVI_FAKE_MODE         = $null
            AVI_FAKE_PATH_SUBKEY  = $null
        }
    }

    function Assert-NoStagingLeft {
        param($TestDir)
        $left = Get-ChildItem -Path $TestDir -Filter ".ai-voice-interconnector-staging-*" -ErrorAction SilentlyContinue
        $left | Should -BeNullOrEmpty
    }
}

AfterAll {
    Remove-Item -Recurse -Force $script:RunDir -ErrorAction SilentlyContinue
    Remove-Item -Path "HKCU:\Software\AviBootstrapTest" -Recurse -Force -ErrorAction SilentlyContinue
}

Describe "bootstrap de Windows" {
    BeforeEach {
        $script:TestDir = Join-Path $script:RunDir ("t-" + [guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $script:TestDir | Out-Null
        $serveDir = Join-Path $script:TestDir "serve"
        New-Item -ItemType Directory -Force -Path $serveDir | Out-Null
        foreach ($d in @("install", "bin", "data", "cache")) {
            New-Item -ItemType Directory -Force -Path (Join-Path $script:TestDir $d) | Out-Null
        }
        New-HarnessAsset -Dir $serveDir -FakeExe $script:FakeExe | Out-Null
        Write-HarnessChecksum -Dir $serveDir -Mode "ok"
        $script:Server = Start-HarnessServer -Root $serveDir -WorkDir $script:TestDir -ServeScript $script:ServeScript
        $script:LastBaseUrl = "http://127.0.0.1:$($script:Server.Port)"
        $script:RegSubkey = $null
    }

    AfterEach {
        Stop-HarnessServer $script:Server
        Exit-HarnessEnv
        if ($null -ne $script:RegSubkey) {
            Remove-Item -Path "HKCU:\$($script:RegSubkey)" -Recurse -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -Recurse -Force $script:TestDir -ErrorAction SilentlyContinue
    }

    It "instala delegando en self install con los parámetros" {
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        (Get-Content -Raw $envBase.AVI_FAKE_LOG).Trim() | Should -Be "self install --no-setup --no-modify-path --yes"
        Get-Content -Raw $script:Server.RequestLog | Should -Match "GET /v9\.9\.9/ai-voice-interconnector-9\.9\.9-x86_64-windows\.zip"
        Assert-NoStagingLeft $script:TestDir
    }

    It "las variables AVI_* equivalen a los parámetros (criterio 10)" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "true"
        $envBase.AVI_YES = "yes"
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        (Get-Content -Raw $envBase.AVI_FAKE_LOG).Trim() | Should -Be "self install --no-setup --no-modify-path --yes"
    }

    It "el parámetro -Version manda sobre AVI_VERSION" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "8.8.8"
        # El arnés solo sirve 9.9.9: si eligiera 8.8.8, la descarga fallaría.
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
    }

    It "la versión estampada se usa sin -Version ni AVI_VERSION" {
        $stamped = Join-Path $script:TestDir "install-stamped.ps1"
        (Get-Content -Raw $script:InstallPs1).Replace("__AVI_STAMPED_VERSION__", "9.9.9") |
            Set-Content -Path $stamped -NoNewline
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $stamped `
            -Arguments @("-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        Get-Content -Raw $script:Server.RequestLog | Should -Match "GET /v9\.9\.9/"
    }

    It "checksum corrupto aborta sin extraer y con staging borrado (criterio 3)" {
        Write-HarnessChecksum -Dir (Join-Path $script:TestDir "serve") -Mode "corrupt"
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        ($result.Stdout + $result.Stderr) | Should -Match "checksum_mismatch"
        Assert-NoStagingLeft $script:TestDir
        Test-Path (Join-Path $envBase.AVI_INSTALL_DIR "ai-voice-interconnector.exe") | Should -BeFalse
    }

    It "SHA256SUMS sin línea para el asset aborta igual" {
        Write-HarnessChecksum -Dir (Join-Path $script:TestDir "serve") -Mode "missing"
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        ($result.Stdout + $result.Stderr) | Should -Match "checksum_mismatch"
        Assert-NoStagingLeft $script:TestDir
    }

    It "binario incompatible diagnostica y deja lo instalado intacto (criterio 5)" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_FAKE_MODE = "noexec"
        $sentinel = Join-Path $envBase.AVI_INSTALL_DIR "sentinel.txt"
        Set-Content -Path $sentinel -Value "instalación previa" -NoNewline
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        ($result.Stdout + $result.Stderr) | Should -Match "binary_incompatible"
        ($result.Stdout + $result.Stderr) | Should -Match "x86_64-pc-windows-msvc"
        ($result.Stdout + $result.Stderr) | Should -Match "BUILD\.md"
        Get-Content -Raw $sentinel | Should -Be "instalación previa"
        Assert-NoStagingLeft $script:TestDir
    }

    It "arquitectura no soportada falla antes de descargar (criterio 4)" {
        $envBase = Get-TestEnvBase $script:TestDir
        # Puerto cerrado: si intentara descargar, el error sería de red.
        $envBase.AVI_DOWNLOAD_BASE_URL = "http://127.0.0.1:1"
        # Solo en el hijo (ChildEnv): tocarlas en el anfitrión envenena la
        # herencia a los hijos siguientes.
        $childEnv = @{ PROCESSOR_ARCHITECTURE = "ARM64"; PROCESSOR_ARCHITEW6432 = "" }
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase -ChildEnv $childEnv

        $result.ExitCode | Should -Not -Be 0
        ($result.Stdout + $result.Stderr) | Should -Match "unsupported_platform"
    }

    It "no deja variables ni funciones en la sesión tras dot-source (criterio 8)" {
        $varsBefore = Get-Variable -Scope Global | Select-Object -ExpandProperty Name
        $funcsBefore = Get-ChildItem function: | Select-Object -ExpandProperty Name
        $eapBefore = $ErrorActionPreference
        $ppBefore = $ProgressPreference

        . $script:InstallPs1

        $varsAfter = Get-Variable -Scope Global | Select-Object -ExpandProperty Name
        $funcsAfter = Get-ChildItem function: | Select-Object -ExpandProperty Name
        Compare-Object $varsBefore $varsAfter | Should -BeNullOrEmpty
        Compare-Object $funcsBefore $funcsAfter | Should -BeNullOrEmpty
        $ErrorActionPreference | Should -Be $eapBefore
        $ProgressPreference | Should -Be $ppBefore
    }

    It "bajo tubería instala sin cerrar la consola y añade el programa al PATH de la sesión (criterio 8)" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "1"
        $envBase.AVI_YES = "1"
        $scriptText = Get-Content -Raw $script:InstallPs1
        # `$env:Path se evalúa en el hijo (escapado): lo observado es el PATH
        # de la sesión del bootstrap, con el programa añadido.
        $stdinText = $scriptText + "`n[Console]::Out.WriteLine('PATH-EFFECT:' + `$env:Path)"
        $result = Invoke-ChildBootstrap -StdinText $stdinText -ExtraEnv $envBase

        # Sin `exit` bajo tubería: el proceso completa el marcador con código 0.
        $result.ExitCode | Should -Be 0
        $result.Stdout | Should -Match "Checksum verificado"
        $marker = ($result.Stdout -split "`r?`n" | Where-Object { $_ -match "PATH-EFFECT:" } | Select-Object -First 1)
        $marker | Should -Not -BeNullOrEmpty
        ($marker -split ";" ) -contains $envBase.AVI_INSTALL_DIR | Should -BeTrue
    }

    It "bajo tubería un checksum corrupto se informa sin cerrar la consola" {
        Write-HarnessChecksum -Dir (Join-Path $script:TestDir "serve") -Mode "corrupt"
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "1"
        $envBase.AVI_YES = "1"
        $scriptText = Get-Content -Raw $script:InstallPs1
        $stdinText = $scriptText + "`n[Console]::Out.WriteLine('PATH-EFFECT-ALIVE')"
        $result = Invoke-ChildBootstrap -StdinText $stdinText -ExtraEnv $envBase

        $result.Stderr | Should -Match "checksum_mismatch"
        # La consola sigue viva: el marcador posterior se imprime.
        $result.Stdout | Should -Match "PATH-EFFECT-ALIVE"
    }

    It "sin -NoModifyPath integra el PATH en la subclave de prueba" {
        $script:RegSubkey = "Software\AviBootstrapTest\" + [guid]::NewGuid().ToString("N")
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_FAKE_PATH_SUBKEY = $script:RegSubkey
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-NoSetup", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        $pathValue = (Get-ItemProperty -Path "HKCU:\$($script:RegSubkey)").Path
        ($pathValue -split ";") -contains $envBase.AVI_INSTALL_DIR | Should -BeTrue
    }

    It "con -NoModifyPath no toca la subclave de prueba" {
        $script:RegSubkey = "Software\AviBootstrapTest\" + [guid]::NewGuid().ToString("N")
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_FAKE_PATH_SUBKEY = $script:RegSubkey
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        Test-Path "HKCU:\$($script:RegSubkey)" | Should -BeFalse
    }
}
