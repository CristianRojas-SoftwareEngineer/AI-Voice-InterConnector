# Suite del bootstrap Windows packaging/bootstrap/install.ps1 (Pester v5).
#
# Corre contra el servidor falso local (support/Serve.ps1, HTTP en 127.0.0.1)
# con el asset de Windows, SHA256SUMS.txt coherente o corrupto y raices
# reubicadas a temporales. Cubre los criterios 3 (checksum con staging
# borrado), 4 (plataforma no soportada antes de descargar), 5 (binario
# incompatible con diagnostico), 8 (sin efectos en la sesion salvo el PATH de
# esa sesion; la consola sobrevive al error bajo tuberia) y 10 (paso de
# parametros y variables). Cubre ademas el canal real `irm <url> | iex` en
# PowerShell 5.1 y 7 (las pruebas de tuberia necesitan `pwsh` en el PATH para
# no omitirse), el entorno con PSModulePath heredado de PowerShell 7 (se
# construye a proposito, sin depender del anfitrion) y la guarda de
# codificacion ASCII de los .ps1. Sin casos --check y sin red real.
#
# Ejecutar: Invoke-Pester tests/bootstrap/install.tests.ps1 -CI

BeforeAll {
    . (Join-Path $PSScriptRoot "support/Setup.ps1")

    $script:RepoRoot = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $script:InstallPs1 = Join-Path $script:RepoRoot "packaging/bootstrap/install.ps1"
    $script:ServeScript = Join-Path $PSScriptRoot "support/Serve.ps1"

    # Falso ejecutable (doble de `self install`), compilado una vez por ejecucion.
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
        # El bootstrap se sirve byte a byte para que `irm <base>/install.ps1 | iex`
        # lo obtenga como en produccion.
        Copy-Item $script:InstallPs1 (Join-Path $serveDir "install.ps1")
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

    It "instala delegando en self install con los parametros" {
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        (Get-Content -Raw $envBase.AVI_FAKE_LOG).Trim() | Should -Be "self install --no-setup --no-modify-path --yes"
        Get-Content -Raw $script:Server.RequestLog | Should -Match "GET /v9\.9\.9/ai-voice-interconnector-9\.9\.9-x86_64-windows\.zip"
        Assert-NoStagingLeft $script:TestDir
    }

    It "las variables AVI_* equivalen a los parametros (criterio 10)" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "true"
        $envBase.AVI_YES = "yes"
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
        (Get-Content -Raw $envBase.AVI_FAKE_LOG).Trim() | Should -Be "self install --no-setup --no-modify-path --yes"
    }

    It "el parametro -Version manda sobre AVI_VERSION" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "8.8.8"
        # El arnes solo sirve 9.9.9: si eligiera 8.8.8, la descarga fallaria.
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Be 0
    }

    It "la version estampada se usa sin -Version ni AVI_VERSION" {
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
        # El texto del script trae `$archiveName` sin expandir: solo el nombre
        # concreto demuestra que el mensaje salio de la ejecucion.
        ($result.Stdout + $result.Stderr) | Should -Match ([regex]::Escape("el checksum de ai-voice-interconnector-9.9.9-x86_64-windows.zip no coincide"))
        Assert-NoStagingLeft $script:TestDir
        # `self install` nunca se invoco: el doble no registro nada.
        Test-Path $envBase.AVI_FAKE_LOG | Should -BeFalse
    }

    It "SHA256SUMS sin linea para el asset aborta igual" {
        Write-HarnessChecksum -Dir (Join-Path $script:TestDir "serve") -Mode "missing"
        $envBase = Get-TestEnvBase $script:TestDir
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        ($result.Stdout + $result.Stderr) | Should -Match ("no contiene ninguna l.nea para\s+" + [regex]::Escape("ai-voice-interconnector-9.9.9-x86_64-windows.zip"))
        Assert-NoStagingLeft $script:TestDir
        Test-Path $envBase.AVI_FAKE_LOG | Should -BeFalse
    }

    It "binario incompatible diagnostica y deja lo instalado intacto (criterio 5)" {
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_FAKE_MODE = "noexec"
        $sentinel = Join-Path $envBase.AVI_INSTALL_DIR "sentinel.txt"
        Set-Content -Path $sentinel -Value "instalacion previa" -NoNewline
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        # El destino va interpolado en el mensaje; en el texto del script
        # aparece como `$target` sin expandir.
        ($result.Stdout + $result.Stderr) | Should -Match ([regex]::Escape("el binario descargado (x86_64-pc-windows-msvc) no arranca"))
        Get-Content -Raw $sentinel | Should -Be "instalacion previa"
        Assert-NoStagingLeft $script:TestDir
    }

    It "arquitectura no soportada falla antes de descargar (criterio 4)" {
        $envBase = Get-TestEnvBase $script:TestDir
        # Puerto cerrado: si intentara descargar, el error seria de red.
        $envBase.AVI_DOWNLOAD_BASE_URL = "http://127.0.0.1:1"
        # Solo en el hijo (ChildEnv): tocarlas en el anfitrion envenena la
        # herencia a los hijos siguientes.
        $childEnv = @{ PROCESSOR_ARCHITECTURE = "ARM64"; PROCESSOR_ARCHITEW6432 = "" }
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase -ChildEnv $childEnv

        $result.ExitCode | Should -Not -Be 0
        $output = $result.Stdout + $result.Stderr
        # `$osArch` se expande a ARM64 solo en la ejecucion real.
        $output | Should -Match ([regex]::Escape("arquitectura no soportada: ARM64"))
        $output | Should -Not -Match "Instalando ai-voice-interconnector 9.9.9"
    }

    It "una version que no tiene exactamente tres componentes falla sin descargar" -ForEach @(
        @{ Bad = "1" }, @{ Bad = "1.2" }, @{ Bad = "1.2.3.4" }, @{ Bad = "1.2.x" }, @{ Bad = "1..2" }
    ) {
        $envBase = Get-TestEnvBase $script:TestDir
        # Puerto cerrado: si intentara descargar, el error seria de red.
        $envBase.AVI_DOWNLOAD_BASE_URL = "http://127.0.0.1:1"
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", $Bad, "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase

        $result.ExitCode | Should -Not -Be 0
        $output = $result.Stdout + $result.Stderr
        # El mensaje lleva el valor concreto: la salida de error de PowerShell
        # reproduce el texto del script (con `usage_error` sin expandir), asi
        # que buscar solo el codigo daria falsos positivos.
        $output | Should -Match ([regex]::Escape("usage_error]: ") + "versi.n inv.lida: '" + [regex]::Escape($Bad) + "'")
        $output | Should -Not -Match ("Instalando ai-voice-interconnector " + [regex]::Escape($Bad))
    }

    It "no deja variables ni funciones en la sesion tras dot-source (criterio 8)" {
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

    It "bajo tuberia (irm | iex) con <Engine> instala sin cerrar la consola y anade el programa al PATH de la sesion (criterio 8)" -ForEach @(@{ Engine = "powershell.exe" }, @{ Engine = "pwsh" }) {
        $cmd = Get-Command $Engine -ErrorAction SilentlyContinue
        if ($null -eq $cmd) {
            Set-ItResult -Skipped -Because "$Engine no disponible"
            return
        }
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "1"
        $envBase.AVI_YES = "1"
        # `$env:Path se evalua en el hijo (escapado): lo observado es el PATH
        # de la sesion del bootstrap, con el programa anadido.
        $stdinText = "irm $($script:LastBaseUrl)/install.ps1 | iex`n[Console]::Out.WriteLine('PATH-EFFECT:' + `$env:Path)"
        $result = Invoke-ChildBootstrap -StdinText $stdinText -ExtraEnv $envBase -Engine $cmd.Source

        # Sin `exit` bajo tuberia: el proceso completa el marcador con codigo 0.
        $result.ExitCode | Should -Be 0
        $result.Stdout | Should -Match "Checksum verificado"
        $marker = ($result.Stdout -split "`r?`n" | Where-Object { $_ -match "PATH-EFFECT:" } | Select-Object -First 1)
        $marker | Should -Not -BeNullOrEmpty
        ($marker -split ";" ) -contains $envBase.AVI_INSTALL_DIR | Should -BeTrue
    }

    It "bajo tuberia (irm | iex) con <Engine> un checksum corrupto se informa sin cerrar la consola" -ForEach @(@{ Engine = "powershell.exe" }, @{ Engine = "pwsh" }) {
        $cmd = Get-Command $Engine -ErrorAction SilentlyContinue
        if ($null -eq $cmd) {
            Set-ItResult -Skipped -Because "$Engine no disponible"
            return
        }
        Write-HarnessChecksum -Dir (Join-Path $script:TestDir "serve") -Mode "corrupt"
        $envBase = Get-TestEnvBase $script:TestDir
        $envBase.AVI_VERSION = "9.9.9"
        $envBase.AVI_NO_SETUP = "1"
        $envBase.AVI_NO_MODIFY_PATH = "1"
        $envBase.AVI_YES = "1"
        $stdinText = "irm $($script:LastBaseUrl)/install.ps1 | iex`n[Console]::Out.WriteLine('PATH-EFFECT-ALIVE')"
        $result = Invoke-ChildBootstrap -StdinText $stdinText -ExtraEnv $envBase -Engine $cmd.Source

        $result.Stderr | Should -Match ([regex]::Escape("el checksum de ai-voice-interconnector-9.9.9-x86_64-windows.zip no coincide"))
        $result.Stdout | Should -Not -Match "Checksum verificado"
        # La consola sigue viva: el marcador posterior se imprime.
        $result.Stdout | Should -Match "PATH-EFFECT-ALIVE"
    }

    It "con powershell.exe y un PSModulePath heredado de PowerShell 7 instala y verifica el checksum" {
        if ($null -eq (Get-Command powershell.exe -ErrorAction SilentlyContinue) -or $null -eq (Get-Command pwsh -ErrorAction SilentlyContinue)) {
            Set-ItResult -Skipped -Because "powershell.exe o pwsh no disponible"
            return
        }
        $envBase = Get-TestEnvBase $script:TestDir
        $contaminated = Get-ContaminatedModulePath
        $result = Invoke-ChildBootstrap -Bootstrap $script:InstallPs1 `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase `
            -Engine (Get-Command powershell.exe).Source -ChildEnv @{ PSModulePath = $contaminated }

        $result.ExitCode | Should -Be 0
        $result.Stdout | Should -Match "Checksum verificado"
        ($result.Stdout + $result.Stderr) | Should -Not -Match "AVISO: no se pudieron restringir"
        Assert-NoStagingLeft $script:TestDir
    }

    It "con powershell.exe, un PSModulePath heredado de PowerShell 7 y la reparacion rota falla explicando la causa antes de descargar" {
        if ($null -eq (Get-Command powershell.exe -ErrorAction SilentlyContinue) -or $null -eq (Get-Command pwsh -ErrorAction SilentlyContinue)) {
            Set-ItResult -Skipped -Because "powershell.exe o pwsh no disponible"
            return
        }
        # Copia con el import apuntando a un directorio inexistente: la
        # reparacion no puede restaurar los cmdlets.
        $broken = Join-Path $script:TestDir "install-broken.ps1"
        $text = (Get-Content -Raw $script:InstallPs1).Replace('"Modules", $module', '"ModulosInexistentes", $module')
        $text | Should -Not -BeExactly (Get-Content -Raw $script:InstallPs1)
        Set-Content -Path $broken -Value $text -NoNewline
        $envBase = Get-TestEnvBase $script:TestDir
        # Puerto cerrado: si intentara descargar, el error seria de red.
        $envBase.AVI_DOWNLOAD_BASE_URL = "http://127.0.0.1:1"
        $result = Invoke-ChildBootstrap -Bootstrap $broken `
            -Arguments @("-Version", "9.9.9", "-NoSetup", "-NoModifyPath", "-Yes") -ExtraEnv $envBase `
            -Engine (Get-Command powershell.exe).Source -ChildEnv @{ PSModulePath = (Get-ContaminatedModulePath) }

        $result.ExitCode | Should -Not -Be 0
        $output = $result.Stdout + $result.Stderr
        # Los nombres concretos solo aparecen si el mensaje salio de la ejecucion.
        $output | Should -Match ([regex]::Escape("unsupported_platform]: ") + "faltan cmdlets est.ndar de PowerShell: Get-FileHash")
        $output | Should -Not -Match "Instalando ai-voice-interconnector 9.9.9"
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

Describe "codificacion de los .ps1" {
    # `irm` entrega el cuerpo sin `charset` (application/octet-stream en los
    # assets de GitHub): PowerShell 5.1 lo decodifica como ISO-8859-1 y
    # PowerShell 7 como UTF-8 conservando U+FEFF. Un BOM queda delante de `<#`
    # y rompe el parseo; un byte no ASCII corrompe los mensajes. Por eso todo
    # .ps1 del bootstrap y de su suite es ASCII puro y sin BOM.
    It "ningun .ps1 del bootstrap ni de su suite contiene bytes no ASCII" {
        $files = @(Get-ChildItem -Path (Join-Path $script:RepoRoot "packaging/bootstrap") -Filter "*.ps1" -File) +
            @(Get-ChildItem -Path (Join-Path $script:RepoRoot "tests/bootstrap") -Filter "*.ps1" -File -Recurse)
        $files.Count | Should -BeGreaterThan 0
        $offenders = @()
        foreach ($file in $files) {
            $bytes = [IO.File]::ReadAllBytes($file.FullName)
            for ($i = 0; $i -lt $bytes.Length; $i++) {
                if ($bytes[$i] -gt 0x7F) {
                    $offenders += "$($file.FullName) (desplazamiento $i, byte 0x$($bytes[$i].ToString('X2')))"
                    break
                }
            }
        }
        $offenders | Should -BeNullOrEmpty
    }
}
