# Servidor falso local para la suite Pester (solo pruebas, HTTP en 127.0.0.1).
#
# Se ejecuta como proceso hijo (Start-Process -PassThru); sirve el árbol de
# <Root> tal cual (<Root>/vX.Y.Z/<fichero>), responde 404 fuera del árbol y
# registra cada petición en <RequestLog> para las aserciones de "antes de
# descargar". HTTP plano basta porque install.ps1 no fija --proto como el
# bootstrap POSIX. Uso: powershell -File Serve.ps1 -Root <dir> -PortFile <f> -RequestLog <f>.

param(
    [string]$Root = "",
    [string]$PortFile = "",
    [string]$RequestLog = ""
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($Root) -or [string]::IsNullOrWhiteSpace($PortFile)) {
    Write-Error "se necesitan -Root y -PortFile"
    exit 2
}
$rootFull = [IO.Path]::GetFullPath($Root)

# Puerto efímero real (HttpListener exige prefijo explícito).
$probe = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
$probe.Start()
$port = $probe.Server.LocalEndPoint.Port
$probe.Stop()

$listener = New-Object Net.HttpListener
$listener.Prefixes.Add("http://127.0.0.1:$port/")
$listener.Start()
Set-Content -Path $PortFile -Value "$port" -NoNewline
try {
    while ($listener.IsListening) {
        $ctx = $listener.GetContext()
        try {
            $rel = $ctx.Request.Url.LocalPath.TrimStart("/")
            Add-Content -Path $RequestLog -Value "$($ctx.Request.HttpMethod) /$rel"
            $full = [IO.Path]::GetFullPath((Join-Path $rootFull ($rel.Replace("/", [IO.Path]::DirectorySeparatorChar))))
            if ($full.StartsWith($rootFull, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path $full -PathType Leaf)) {
                $bytes = [IO.File]::ReadAllBytes($full)
                $ctx.Response.ContentLength64 = $bytes.Length
                $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
            } else {
                $ctx.Response.StatusCode = 404
            }
        } catch {
            $ctx.Response.StatusCode = 500
        } finally {
            $ctx.Response.Close()
        }
    }
} finally {
    $listener.Stop()
}
