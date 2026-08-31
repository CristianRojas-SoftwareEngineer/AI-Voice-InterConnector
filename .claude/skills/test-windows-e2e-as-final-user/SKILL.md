---
name: test-windows-e2e-as-final-user
description: >
  Ejecuta la prueba E2E completa de la CLI en Windows como usuario final. Use
  when the user asks to probar e2e en Windows, test Windows e2e, validar release
  como usuario final, instalar y probar en Windows, recorrido e2e completo,
  prueba de instalación Windows, verificar workflow de usuario final, or
  test-windows-e2e, even if they do not name the skill.
disable-model-invocation: true
---

# Test E2E Windows as Final User

<!-- <user_communication> -->
Ask, confirm, and respond to the user in **Spanish** (native Spanish-speaking audience). Keep this artifact's instructions in **English** for token efficiency. Canonical policy: `<language_policy>` in [.claude/skills/artifact-structuring/SKILL.md]. User-facing rules: [AGENTS.md](../../../AGENTS.md) §0.
<!-- </user_communication> -->

<!-- <overview> -->
Reusable task command for the full manual E2E on the **Windows dev machine** as a final user. Each invocation is interpreted as a request to run `cleanup → install → setup → daemon → voice clone → synthesis matrix → store/transcribe/translate/dub → cleanup/uninstall` from zero, validating the published `vX.Y.Z` without depending on CI. Evolved from `.claude/plans/2026-08-27-prueba-e2e-windows-completa.md` (0.15.2) to `0.18.8` with pipeline heterogéneo (`test-linux`/`coverage` con `target-v2` `os: linux`, `test-windows`/`macos` con `registry+sccache` `os: windows/macos`, `sccache_save_cache_conditional` umbral 85 % `.circleci/config.yml:228` ahorra 1 GiB/~50s cuando `hit ≥85%`), zero-residue purge (`src/main.rs:1387` `crates/avi-store/src/lib.rs:446` `hub+xet`), `exe_dir` resolution (`crates/avi-tts/src/lib.rs:151`) and detached launch without pipe inheritance (`crates/avi-daemon/src/spawn.rs:30`).
<!-- </overview> -->

## When it applies

- User says `probar e2e`, `prueba completa Windows`, `validar release en Windows`, `instalar como usuario final`, `test e2e windows`, `recorrido completo`.
- Pre-release gate before publishing `vX.Y.Z` or after a `0.18.x` pipeline success where CI only did `version` smoke.
- Regression after changes to `install-windows.ps1`, `avi-store` (`VoiceStore`/`SpeechStore`/`ModelStore`), `avi-daemon`, or `avi-tts` dispatch.

## Preconditions

- **Host:** Windows 10+ x64, PowerShell 5.1+ (the dev box). `per-user` install, no UAC.
- **Cost:** `~9 GB` base (`qwen3-tts-0.6b` + `parakeet-tdt-v3` + `marian-es-en/en-es`) and `~11.5 GB` with `--with-base` (`qwen3-tts-0.6b-base` `crates/avi-store/src/lib.rs:410`). Warn before downloading — each E2E re-downloads from zero due to initial purge.
- **State:** repo stays unmutated (`docs/`, `Cargo`, `.circleci` untouched). System under test is the installed artifacts (`%LOCALAPPDATA%\Programs\ai-voice-interconnector`, `%APPDATA%\ai-voice-interconnector\data`, `%USERPROFILE%\.cache\huggingface\hub` + `xet` `crates/avi-store/src/lib.rs:446`). **Each E2E purges them upfront** for determinism (see Workflow 0a).
- **Contract:** `schema_version="3"` (`crates/avi-core/src/exit_codes.rs`), exit codes `0-10/130`, `WAV 24kHz mono 16-bit` (`hound`), `WER ≤0.25` via `ParakeetEngine` when `native-stt` is available. Signing (`SmartScreen`) is deferred per `docs/GOAL.md:209`.

## Workflow

Pipeline vigente **heterogéneo** desde 0.18.8: `test-linux`/`coverage` restauran `target-v2` (`os: linux`, `cargo_restore_caches`), `test-windows`/`macos` restauran solo `registry` (`cargo_restore_registry` + `sccache` sin `target`); `sccache_save_cache_conditional` (`.circleci/config.yml:228`) guarda solo si `hit <85%`. Ver `docs/BUILD.md` §Cacheo y `.circleci/config.yml:75-391`.

Execute in order. Each task needs the previous task's artifacts (purge → binary → models → daemon → `.qvoice` → persisted wav). Do not parallelize. Report `PASS/FAIL` per step with command, exit code, and payload.

### 0a. Limpieza inicial — zero-residue desde cero

- **Stop daemon graceful:** `ai-voice-interconnector daemon stop --json` `exit 0/5`, fallback `taskkill /F /IM qwen_tts.exe` + `ai-voice-interconnector.exe` si cuelga, `Remove-Item $env:APPDATA\ai-voice-interconnector\data\daemon.pid -Force` — libera `8765` y `resident` (`crates/avi-daemon/src/lib.rs:88` `src/main.rs:1387`).
- **Purge completa:** `ai-voice-interconnector cleanup --json` `{"status":"cleanup_complete"}` `src/main.rs:1387` → borra `data_dir/models/speech/voices` + `daemon.pid`, `hub` `crates/avi-store/src/lib.rs:618` `models--*`, `xet` `crates/avi-store/src/lib.rs:446` `xet/shard-cache+logs+.locks` y `temp avi_*`/`ai-voice-interconnector-install-*`; luego `ai-voice-interconnector uninstall --force` `src/main.rs:1471` → borra `%LOCALAPPDATA%\Programs\ai-voice-interconnector` + `HKCU` PATH + `data_dir` completo.
- **Verify zero-residue:** `where ai-voice-interconnector` `exit 1` fails, `Test-Path "$env:LOCALAPPDATA\Programs\ai-voice-interconnector"` `$false`, `Test-Path "$env:APPDATA\ai-voice-interconnector\data"` `$false`, `Test-Path "$env:USERPROFILE\.cache\huggingface\hub\models--*"` `$false` y `Test-Path "$env:USERPROFILE\.cache\huggingface\xet"` `$false`, `HKCU` sin entrada `ai-voice-interconnector`. Si ya estaba limpio, reportar `already clean`.

### 0b. Resolve current release

- Read `src/main.rs:27` `const VERSION` and `Cargo.toml:3`. Confirm `vX.Y.Z` matches `git describe --tags --abbrev=0`. All `version --json` checks below must return this `X.Y.Z`, not `0.15.2`.
- Confirm 4 assets in the GitHub Release (`ai-voice-interconnector-*-x86_64-windows.zip` + `*-x86_64-linux.tar.gz` + `*-arm64-linux.tar.gz` + `*-arm64-macos.tar.gz` + `SHA256SUMS.txt`) — since `0.16.0` the Windows build is single `x86_64`.

### 1. `install-windows.ps1` — oneliner install

- `irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 | iex` (`install-windows.ps1:4`)
- Verify `Test-Sha256Sum` `install-windows.ps1:84` prints `Checksum verificado`, `Expand-ArchiveToInstallDir` `install-windows.ps1:106` extracts `ai-voice-interconnector.exe`, `Add-UserPathEntry` `install-windows.ps1:123` registers `HKCU` PATH idempotently, `Update-SessionPath` `install-windows.ps1:140` recomposes `$env:Path`.
- `where ai-voice-interconnector` → `%LOCALAPPDATA%\Programs\ai-voice-interconnector\ai-voice-interconnector.exe` (`install-windows.ps1:103`)
- `ai-voice-interconnector version --json` `exit 0` `{"name":"ai-voice-interconnector","version":"X.Y.Z","schema_version":"3"}` (`src/main.rs:348`, `crates/avi-core/src/json_emitter.rs`)

### 2. `avi-store` — base provision + Base

- `ai-voice-interconnector doctor --json` pre-setup `exit 1` `failed` with `model_missing` (`src/main.rs:531`, `crates/avi-store/src/lib.rs:527` `is_provisioned`)
- `ai-voice-interconnector setup` → snapshots `qwen3-tts-0.6b` `85e237c`, `parakeet-tdt-v3` `8f23f0c` (4 artefacts), `marian-es-en` `c96e2c` / `marian-en-es` in `hf_cache_dir` (`crates/avi-store/src/lib.rs:381` `MODEL_REVISIONS`, `lib.rs:634` `ensure_downloaded`) — `doctor --json` flips to `exit 0`
- `ai-voice-interconnector setup --with-base` → `qwen3-tts-0.6b-base` `5d8399` (`crates/avi-store/src/lib.rs:410`) and `ModelStore::is_provisioned("qwen3-tts-0.6b-base")` `true` (`lib.rs:527`), `Qwen3TtsEngine.base_model_dir` `Some` (`crates/avi-tts/src/lib.rs:248`)
- `ai-voice-interconnector devices --json` `exit 0` `{"devices":[...],"schema_version":"3"}` (`src/main.rs:357`, `crates/avi-audio/src/lib.rs`) independent of models

### 3. `src/main.rs` — daemon lifecycle

- `daemon status --json` initial `{"daemon":"stopped","schema_version":"3"}` `exit 0` (`src/main.rs:1247`, `crates/avi-daemon/src/lib.rs:612`)
- `daemon start --json` **must be launched detached without pipe capture** — use `Start-Process -FilePath ai-voice-interconnector -ArgumentList @("daemon","start","--json") -RedirectStandardOutput $env:TEMP\daemon_start.log -WindowStyle Hidden` and poll `GET /health` (not `& $bin daemon start --json | Out-String` nor `Start-Job` with pipe, which inherits the write-end and hangs the parent 10s — `crates/avi-daemon/src/spawn.rs:30` `Stdio::null` + `0x02000000`). Returns `{"daemon":"running","pid":...}` (`src/main.rs:1176`) within `DAEMON_READY_DEADLINE 10s` (`src/main.rs:35`) via `await_daemon_ready` (`src/main.rs:1167`), `GET /health 200`, `warm` `warming→warm` (now via `<exe_dir>/vendor` `crates/avi-tts/src/lib.rs:151` — no `QWEN3_TTS_BIN` needed from any `CWD`) (`crates/avi-daemon/src/lib.rs`)
- `daemon status --json` in `running` exposes `engine` + `warm`; `daemon restart` `exit 0` and `daemon stop` → `stopped` (`src/main.rs:1214/1182`, `crates/avi-tts/src/lib.rs:308` `shutdown`, `lib.rs:917` `kill_resident_process` no orphan `qwen_tts.exe`)

### 4. `src/main.rs` — voice management (dual-model)

- `voice list --json` `{"voices":["default",...],"schema_version":"3"}` with `default` `is_factory true` (`crates/avi-store/src/lib.rs:66`/`77`, preset `ryan` `crates/avi-tts/src/lib.rs:132`)
- `voice clone --name mi_voz --speech-reference <wav24k≥10s> --json` `{"name":"mi_voz","precomputed":false,"speech":"...reference.qvoice"}` (`src/main.rs:558`) via `clone_voice` `crates/avi-tts/src/lib.rs:744` with `Base`; `VoiceStore::find_reference("mi_voz")` `Some` (`lib.rs:140`)
- `<data_dir>/voices/mi_voz/reference.qvoice` `>1 MB` + `speech-reference.wav` copied (`src/main.rs:545`, `crates/avi-store/src/lib.rs:162`)
- Re-clone without `--force` `exit 6` `StateConflict` (`src/main.rs:519`), with `--force` `exit 0`

### 5. `src/main.rs` + `crates/avi-tts` — synthesis dispatch matrix

- With daemon `running`: `speech synthesize --text "Hola mundo" --label e2e1 --voice default --json` `Auto` `{"status":"success","audio_path":"...","voice":"default"}` (`src/main.rs:831`) via `synthesize_via_residente` (`crates/avi-tts/src/lib.rs:419`); `speech say --text "Hola clon" --voice mi_voz --json` `{"status":"reproduced"}` (`src/main.rs:878`, `lib.rs:381`) — both delegated (`src/main.rs:769`)
- `speech synthesize --daemon` with `running` `exit 0`; with `stopped` `exit 5` `DaemonUnreachable` (`src/main.rs:388`, `DaemonMode::ForceDaemon :76`)
- `speech synthesize --no-daemon` with `stopped` `exit 0` via `synthesize_via_subprocess` (`crates/avi-tts/src/lib.rs:349`) — `ForceDirect`
- Each `audio_path` exists, `WAV 24kHz mono 16-bit` (`crates/avi-store/src/lib.rs:335`), `WER ≤0.25` via `ParakeetEngine::transcribe` when `native-stt` present (`tests/cli_golden.rs:383`)

### 6. `src/main.rs` + `crates/avi-store` — store / transcribe / translate / dub

- `speech list --json` `{"speech":[{"label":"e2e1","voice":"default",...}],"schema_version":"3"}` (`lib.rs:222`) and `speech list --voice mi_voz --json` filtered
- `speech play --label e2e1 --voice default --json` `{"status":"played"}` (`src/main.rs:1104`) and `speech remove --label e2e1 --voice default --json` `{"status":"removed"}` (`src/main.rs:1120`, `lib.rs:288`) — deletes `WAV+.json`
- `speech transcribe --audio crates/avi-stt/tests/assets/whisper_sample_16k.wav --source-language es-latam --json` `{"text":...,"source":"es-latam","schema_version":"3"}` (`src/main.rs:743`, `crates/avi-stt/src/lib.rs:40` `STT_MODEL_DIR` `nemo128.onnx`) — requires `native-stt`
- `translate --text "Hola" --from es --to en --json` `{"translated":...}` (`src/main.rs:460`), passthrough `es→es` returns intact (`src/main.rs:406`), and `speech dub --audio <wav> --from es --to es --voice default --json` `{"status":"dubbed"}` (`src/main.rs:1074`) — `dub` is local-only

### 7. `src/main.rs` — cleanup and uninstall (zero residue)

- `cleanup --json` `{"removed":[...]}` (`src/main.rs:321`, `crates/avi-store/src/lib.rs:615` `remove_hf_snapshot`) and `doctor --json` flips back to `exit 1` `model_missing`
- `uninstall --force` (`cleanup --all` alias `src/main.rs:138/143`) `exit 0` removes `%LOCALAPPDATA%\Programs\ai-voice-interconnector` (`install-windows.ps1:103`), `HKCU` PATH entry, and `data_dir` (`crates/avi-store/src/lib.rs:6` `voices`/`speech`)
- `where ai-voice-interconnector` `exit 1` fails and `Test-Path "$env:APPDATA\ai-voice-interconnector\data"` is `$false` or empty
- Subsequent `irm|iex` reinstall returns to `version X.Y.Z` without `sudo`

## Output format

After execution, emit in **Spanish**:

### Recorrido (walkthrough)

1. **Proceso seguido** — tasks executed in order `T-1(0a)→T0b→T7` with commands, exit codes, and `PASS/FAIL` per step.
2. **Desviaciones respecto al plan** — every divergence from this skill (adapted tasks, added/omitted actions, files touched outside the listed `Action` sources, order changes) with reason; or explicit note that execution matched the plan.

No mutations to `docs/`, `Cargo`, `.circleci` — report only. `0a` always purges; `already provisioned` no longer applies (cada E2E parte de cero).

## Verification checklist

- [ ] `0a` zero-residue inicial: `where` fails, `data_dir`/`hub`/`xet` vacíos, `daemon` stopped, `HKCU` sin entrada
- [ ] `version --json` returns current `X.Y.Z` with `schema_version="3"`
- [ ] `doctor` transitions `failed→PASS→failed` across `setup`/`cleanup`
- [ ] `daemon` `stopped→running→stopped` within `10s` with `warm` lifecycle
- [ ] `voice clone` creates `>1 MB` `.qvoice` via `Base`, `list` shows `default`
- [ ] `synthesize`/`say` matrix covers `Auto`/`ForceDaemon`/`ForceDirect` with `WAV 24kHz` and `WER ≤0.25`
- [ ] `speech list/play/remove`, `transcribe`, `translate` passthrough, `dub` local-only verified
- [ ] `uninstall --force` leaves zero residue (`where` fails, `data_dir` empty, `HKCU` entry removed, `xet` purged)

## Constraints

- Run only on the Windows dev machine as the final user; do not mock `curl`/`sha256sum` or network (unlike `tests/installer`). CI heterogéneo (`test-linux`/`coverage` con `target-v2`, `test-windows`/`macos` sin `target` + `sccache` condicional 85 % `.circleci/config.yml:228` — `hit ≥85%` omite `save_cache` y vacía `~/.cache/sccache`, ahorra 1 GiB/~50s) — no asumir `target` uniforme.
- The `~9 GB` download is expected cost — not an incident. Use `hound` for `WAV` validation and `ParakeetEngine` for `WER` when compiled with `native-stt`. Each E2E re-downloads due to `0a`. Modelos pineados: `qwen3-tts-0.6b` `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` `85e237c`, `parakeet-tdt-v3` `istupakov/parakeet-tdt-0.6b-v3-onnx` `8f23f0c` (4 artefactos `encoder-model.int8.onnx`/`decoder_joint-model.int8.onnx`/`nemo128.onnx`/`vocab.txt`), `marian-es-en` `Helsinki-NLP/opus-mt-es-en` `c96e2c` / `marian-en-es` `5bc4493` (`crates/avi-store/src/lib.rs:381` `MODEL_REVISIONS`).
- Never launch `daemon start` with pipe capture (`| Out-String`, `Start-Job` with `Invoke-Expression`, `Command::output` without `Stdio::null`); use `Start-Process` detached with `RedirectStandardOutput` and a 15s timeout + `taskkill` escape, or the workflow will hang on the same bug it validates.
- `0a` is destructive by design (borra `hub`+`xet`+`data_dir`+`vendor`+`PATH`): confirmar coste `~11.5 GB` con `--with-base` antes de ejecutar.
