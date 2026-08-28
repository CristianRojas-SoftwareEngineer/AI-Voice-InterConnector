$install = "$env:LOCALAPPDATA\Programs\ai-voice-interconnector"
$exe = "$install\ai-voice-interconnector.exe"
Write-Host "=== Smoke test ===" -ForegroundColor Green
Write-Host "Instal dir exists: $(Test-Path $install)"
Write-Host "Old binary exists: $(Test-Path $exe)"
# Stop any running daemon
try { & $exe daemon stop 2>$null } catch {}
Start-Sleep -Seconds 2
# Install new binary
Copy-Item "target\debug\ai-voice-interconnector.exe" $exe -Force
Write-Host "Binary installed (with ModelStore fix)"
# Install qwen_tts.exe into vendor/qwen3-tts/
$vd = "$install\vendor\qwen3-tts"
New-Item -ItemType Directory -Force -Path $vd | Out-Null
Copy-Item "vendor\qwen3-tts\qwen_tts.exe" "$vd\qwen_tts.exe" -Force
Write-Host "qwen_tts.exe installed"
Write-Host "onnxruntime.dll: $(Test-Path "$install\onnxruntime.dll")"
# Start daemon (polls /health up to DAEMON_READY_DEADLINE=10s)
Write-Host "=== Starting daemon ===" -ForegroundColor Yellow
try {
    & $exe daemon start --json
} catch {
    Write-Host "daemon start process ended: $_"
}
Start-Sleep -Seconds 3
try { & $exe daemon status --json } catch { Write-Host "status failed" }
try { & $exe daemon stop 2>$null } catch {}
Write-Host "=== Done ===" -ForegroundColor Green
