---
name: test-windows-e2e-as-final-user
description: >
  Runs the complete Windows CLI E2E as a final user. Use when the user requests a Windows E2E, validates a release as a final user, asks to install and test on Windows, or requests a full installation and use walkthrough.
disable-model-invocation: true
---

# Test E2E Windows as Final User

<!-- <user_communication> -->
Ask, confirm, and respond to the user in **Spanish** (native Spanish-speaking audience). Keep this artifact's instructions in **English** for token efficiency. Canonical policy: `<language_policy>` in [.claude/skills/artifact-structuring/SKILL.md]. User-facing rules: [AGENTS.md](../../../AGENTS.md) §0.
<!-- </user_communication> -->

<!-- <overview> -->
Reusable task command for the critical manual E2E on the **Windows dev machine** as a final user. It validates the published `vX.Y.Z` through clean-state inspection, installation, model setup, daemon, voice/audio workflows and final installed-state checks. It is not a Cartesian test of every CLI flag: deterministic parser, error-code and isolation contracts belong in Rust tests; this skill covers the published bundle, actual Windows PATH, real model/runtime integration, and available audio hardware. Leave the application, models and generated user data provisioned after the test. The app-owned model/cache/data/scratch roots are defined in `crates/avi-shared/src/paths.rs`; `cleanup`, `self uninstall`, `doctor --repair` and `cargo xtask clean` have distinct scopes and must not be treated as interchangeable.
<!-- </overview> -->

## When it applies

- The user asks for a Windows E2E, release validation as a final user, or a full install-and-use walkthrough.
- Pre-release gate before publishing `vX.Y.Z` or after a release pipeline smoke test.
- Regression after changes to `packaging/bootstrap/install.ps1`, lifecycle install/cleanup, stores, daemon or TTS dispatch.

## Preconditions

- **Host:** Windows 10+ x64 with PowerShell 5.1+ on the development machine. The installation is per-user and does not require UAC.
- **Cost:** approximately 5.86 GB for the six pinned models (`qwen3-tts-0.6b`, `qwen3-tts-0.6b-base`, `parakeet-tdt-v3`, both Opus-MT directions and `dpdfnet8`; see `MODEL_REVISIONS` and `ModelPin::approx_bytes` in `crates/avi-shared/src/paths.rs`). Warn and obtain fresh approval before a purge or model redownload. `dpdfnet8` is startup-optional but still required by the complete setup/doctor criterion. The Base model is always provisioned by `setup`; there is no clone-model flag. Windows snapshots use hardlinks to avoid duplicate model storage.
- **State:** do not modify `docs/`, `Cargo` or `.circleci`. System under test is the installed bundle and its owned roots (`%LOCALAPPDATA%\Programs\ai-voice-interconnector`, `%LOCALAPPDATA%\ai-voice-interconnector\data`, `%LOCALAPPDATA%\ai-voice-interconnector\cache\models`, scratch and staging as resolved by `crates/avi-shared/src/paths.rs`). Each invocation starts by inspecting state; it may purge only after showing the exact dry-run targets and obtaining fresh, explicit user approval for those paths and the expected ~5.86 GB re-download. Approval of a plan or prior E2E does not authorize a later purge. Never delete the shared Hugging Face cache except the app-owned pinned repos that the official cleanup plan explicitly selects. Preserve the installed application, models and generated data after the test.
- **Contract:** CLI envelope `schema_version="5"` (`CLI_SCHEMA_VERSION`, `crates/avi-core/src/json_emitter.rs`); daemon protocol has independent version `"4"` (`DAEMON_SCHEMA_VERSION`). WAV output is PCM16 mono at 24 kHz. When Parakeet is available, short-text TTS must meet WER ≤0.25. Code signing/SmartScreen remains deferred in `docs/GOAL.md`.

<!-- <user_communication> -->
Ask, confirm and respond to the user in Spanish. Keep this skill's instructions in English. Canonical language policy: `<language_policy>` in [artifact-structuring](../artifact-structuring/SKILL.md); user-facing rules: [AGENTS.md](../../../AGENTS.md) §0.
<!-- </user_communication> -->

## Task tracking

Before executing the E2E, invoke `harness-task-tracking` and create one harness-managed task for each workflow step: `0a`, `0b`, and `1`–`7`. Keep only one task `in_progress` at a time. Mark a task complete only after checking command exit codes, payloads and resulting artifacts. Record failed, skipped and blocked steps explicitly; do not declare the E2E complete while any required step has failed or is blocked. Close the task list and provide its final walkthrough as required by `harness-task-tracking`.

## Workflow

The persistent user PATH value in `HKCU\Environment` and each process's inherited PATH are separate states. Inspect the registry value separately, then create a new terminal after installation to verify `where.exe` and `doctor`; never edit the session PATH to force a pass.

Execute in order and do not parallelize. Report `PASS/FAIL/SKIP/BLOCKED` per step with exact command, exit code, relevant payload and artifact path. `SKIP` requires an observed missing optional capability; `BLOCKED` means a precondition prevents a valid test. Do not call the E2E complete when a required step fails or is blocked.

**Invoke the CLI by full path, never by name.** Set `$bin = "$env:LOCALAPPDATA\Programs\ai-voice-interconnector\ai-voice-interconnector.exe"` once for normal invocations and leave the current session `PATH` untouched. In cleanup verification, use a genuinely new process/terminal when checking command resolution. Step 0a always inspects first; never report `already clean` while any owned installation, data, cache, scratch, staging, receipt or PATH integration remains.

Steps are numbered `0a`, `0b`, `1`…`7`; a reference to "step N" always means the section `### N.` of this Workflow.

### 0a. Initial cleanup — verify zero residue

- **Inventory and consent:** inspect the installation receipt, `$bin cleanup --all --dry-run --json` and `$bin self uninstall --dry-run --json` (when the installed binary exists), HKCU PATH value and owned program/data/cache/scratch/staging/log paths. Compare the announced targets and preserved roots; show exact targets and sizes to the user. Require fresh explicit approval of those targets and the expected model re-download before any destructive command. Do not reuse prior-session consent.
- **Stop and cleanup:** if `$bin` exists, run `$bin daemon stop --json`; accept only a payload with `daemon_fully_stopped: true`, then verify status and registered process identities are stopped. Run `$bin cleanup --all --yes --json` and `$bin self uninstall --yes --json` only after consent. `not_installed` is idempotent, not evidence of uninstall. `removal_scheduled` remains pending until target paths and failure logs disappear. Never use `cargo xtask clean --app` as proof that an installed uninstall occurred.
- **Missing binary with residue:** if `$bin` is absent but installation, data, cache, staging, scratch, logs or PATH integration remain, mark `BLOCKED`; do not manually delete, adopt conventional directories or claim `already clean`. Resolve ownership and supported recovery explicitly before continuing.
- **Verify zero residue:** from a newly created process check `where.exe` returns 1, and verify owned installation/data/cache/scratch/staging/receipt and PATH entry are absent; preserve foreign HF cache, unrelated program paths, and unrelated processes. Report `already clean` only when all owned paths and integration were absent before any action; otherwise report exact uninstall result plus post-clean verification.

### 0b. Resolve the current release

- Read `const VERSION` in `src/main.rs` (y `Cargo.toml:3`) and the fixture `tests/golden/cli_version.json`. Confirm `vX.Y.Z` matches `git describe --tags --abbrev=0`. All `version --json` checks below must return this `X.Y.Z`, and must match the fixture.
- Confirm **7 assets** in the GitHub Release: 4 bundles (`*-x86_64-windows.zip`, `*-x86_64-linux.tar.gz`, `*-arm64-linux.tar.gz`, `*-arm64-macos.tar.gz`) plus `install.sh`, `install.ps1` and `SHA256SUMS.txt` — since `0.16.0` the Windows build is single `x86_64`. The two bootstraps are assets of every release, not files fetched from `main`.

### 1. `install.ps1` — published-release bootstrap

The bootstrap is the only code run before the binary exists. Its scope is **resolve, verify and delegate**: it does not provision models. `irm | iex` does not accept script parameters, so pass options through environment variables.

- Install only the binary, without chained setup: set `$env:AVI_NO_SETUP = "1"`, then run `irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex`. With `AVI_NO_SETUP=1`, the bootstrap delegates to `self install --no-setup`; step 2 runs `setup` explicitly with a bounded timeout.
- The bootstrap resolves the version (argument/`AVI_VERSION`/stamped/`latest`), verifies the checksum against `SHA256SUMS.txt`, checks that the binary starts and delegates to `self install`. That command validates the bundle, transactionally installs into `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, updates the user PATH in `HKCU\Environment`, writes the receipt and removes staging. Verify the receipt and persistent registry entry; do not treat this process's PATH as proof of persistence.
- After installation, verify the persistent HKCU PATH entry. A `where.exe ai-voice-interconnector` launched from the current installer process may see a process-local PATH update; do not treat that as persistent PATH evidence. In step 7, use a genuinely new PowerShell process to prove inherited resolution without editing `$env:Path`.
- `$bin version --json` `exit 0` `{"name":"ai-voice-interconnector","version":"X.Y.Z","schema_version":"5"}` (`handle_version`, `crates/avi-core/src/json_emitter.rs`)

### 2. `avi-store` — model setup and voice cloning

- **Anti-hang guard:** run `$bin setup --json` through `Start-Process -PassThru`, redirect stdout/stderr to fresh per-run files, call `WaitForExit(1200000)`, then record the process exit code and parse the final JSON payload. If it times out, terminate only the exact PID returned by this invocation (and inspect its owned descendants if any); never use `taskkill /IM`. A timeout is `FAIL`, not success. Before retrying, inspect snapshots and weight files: a plain retry can accept incomplete Qwen snapshots. `setup --force-update --yes` purges pinned snapshots and app-owned xet state, so it requires a new explicit user approval after reporting that it re-downloads about 5.86 GB; do not run it automatically.
- Invoke `$bin doctor --json` from the current process before setup and record its expected integral failure for missing models; after setup, use a newly created PowerShell process whose environment was inherited after installation. Do not mutate the current `$env:Path` to manufacture success. In the new process require `path_resolves` and `path_duplicates` checks to pass, `duplicate_entries: []`, and `resolves_to_this_install: true`. A `doctor` invocation from the old terminal may legitimately fail `path_resolves`; report it as stale inherited environment, not a product failure, and do not count it as final verification.
- Before setup, `$bin doctor --json` should fail its integrated verdict. In a registered install with missing models, `failed` includes `models_provisioned`; in an unmanaged install, receipt and PATH checks also fail. `doctor` is not a partial checker: any failed check yields exit 1. `model_missing` is an operation `reason` (exit 4), not a doctor check.
- `$bin setup --json` must report `status: "completed"` and all six pinned models provisioned under `models_cache_dir()`: `qwen3-tts-0.6b` `85e237c`, `qwen3-tts-0.6b-base` `5d83992`, `parakeet-tdt-v3` `8f23f0c`, `opus-mt-es-en` `6eabf2f`, `opus-mt-en-es` `452971f`, and `dpdfnet8` `dd6818d`. Then require `doctor --json` from the new terminal to exit 0 with `failed: []`, all six models present and no pending artifacts. Do not infer model completeness from snapshot existence alone; a prior interrupted Qwen download may leave config/tokenizer files without weights. If the command fails or times out, inspect the exact payload, process and required model files and stop; any destructive repair via `--force-update --yes` requires fresh user approval.
- The fixed `setup` selection also provisions `qwen3-tts-0.6b-base` (`MODEL_REVISIONS`); `Qwen3TtsEngine` resolves its `base_model_dir` via `resolve_base_model_dir` in `crates/avi-tts/src/lib.rs`.
- `$bin devices --json` must exit 0 and return the current device list with `schema_version: "5"`; if enumeration reports an actual device error, record the payload rather than treating an empty list as proof of hardware availability.

### 3. `src/main.rs` — daemon lifecycle

Start the daemon here and keep it running for voice cloning and ForceDaemon cases. Step 5 owns the stop/failure/direct/restart transitions.

1. `$bin daemon status --json` must initially report `stopped` with schema 5.
2. Start detached: use `Start-Process -PassThru` with stdout/stderr redirected to fresh log files; retain the exact process object and wait at most 15 s for its exit/payload. A successful launcher must return exit 0 and `daemon: "running"`; independently poll `daemon status --json` and the daemon's `/health` until `warm`. Do not pipe the launcher or use `Start-Job` with inherited output handles.
3. Verify status `running`/`warm`; continue with that state into voice cloning and synthesis.
4. On launch timeout, inspect saved process identity/logs and stop only the process tree owned by this run; never `taskkill` by image name.

Use the installed engine from `<exe_dir>\\vendor`; do not depend on current working directory or set `QWEN3_TTS_BIN`.

### 4. `src/main.rs` — voice management with dual models

- `$bin voice list --json` must contain the names `default`, `ryan` and `vivian`; this JSON is a names array and has no factory metadata field.
- Define `$reference = (Resolve-Path "assets/audios/Speech Sampler.wav").Path` from the repository root. Clone `mi_voz` with `--speech-reference $reference --no-daemon --json`; expect exit 0, `precomputed: false` and an existing `reference.qvoice` larger than 1 MB. The directory must contain no copied `speech-reference.wav`.
- Clone a second test voice through `$bin voice clone --name mi_voz_daemon --speech-reference $reference --daemon --json` while running; assert success and inspect its stored qvoice, but do not assume `precomputed` is false on a delegated route.
- Clone `mi_voz` again without `--force` and require exit 6 (`state_conflict`); then retry with `--force --no-daemon` and require exit 0. Retain this cloned voice for later synthesis/WER checks.
- Clone `$bin voice clone --name mi_voz_denoise --speech-reference $reference --denoise --no-daemon --json`; require exit 0 and a non-empty qvoice. This checks opt-in reference cleanup before cloning.
- If `$bin devices --json` reports an available capture device, run `$bin voice clone --name mi_voz_mic --mic --duration 5 --no-daemon --json` and require successful capture; otherwise report `SKIP` with the observed device/error payload. Push-to-talk is covered by parser tests, not a manual TTY requirement.
- Keep `mi_voz` for later checks; remove only temporary voices created for cleanup verification.

### 5. `src/main.rs` + `crates/avi-tts` — synthesis dispatch matrix

Set `$wavShortDefault`, `$wavShortClone` and `$wavRetained` from the JSON `audio_path` payloads; resolve every output to an absolute path.

1. With the daemon running, run `$bin speech synthesize --text "Hola mundo" --label e2e_auto_default --voice default --json` (Auto) and `$bin speech synthesize --text "Hola mundo" --label e2e_daemon_default --voice default --daemon --json` (ForceDaemon). Both require exit 0, `status: "success"`, an existing WAV path and WAV PCM16 mono 24 kHz.
2. Run `$bin speech say --text "Hola clon" --voice mi_voz --no-daemon --json` and `$bin speech say --text "Hola factory" --voice default --daemon --json`. Require `status: "reproduced"`; these commands require an output device and audible playback. If no device is available, report `SKIP` and do not claim playback solely from JSON.
3. Stop the daemon using the exact owned-process checks in step 3, then verify it is stopped. Run `$bin speech synthesize --text "Hello world" --label e2e_direct_en --voice default --source-language en --target-language en --temperature 0.7 --no-daemon --json` (ForceDirect with valid temperature). Then run `$bin speech synthesize --text "Hola mundo" --label e2e_direct_cross --voice default --source-language es --target-language en --no-daemon --json`; require successful payloads and audio output, not merely exit 0.
4. If Parakeet is available, transcribe the retained short WAVs and compute WER using the same normalization/Levenshtein implementation as `tests/cli_golden.rs` (`normalize`, `word_error_rate_and_transcript`). Cover `"Hola mundo"` for `default` and the newly cloned `mi_voz`; record transcript and WER. `WER > 0.25` is `FAIL`; `WER ≤ 0.25` is `PASS`. If STT cannot run, report `SKIP`, never infer a pass from synthesis status.

The daemon remains stopped after this step. Restart it explicitly before any later `--daemon` case.

### 6. `src/main.rs` + `crates/avi-store` — store / transcribe / translate / dub

Define `$sttSample = (Resolve-Path "crates/avi-stt/tests/assets/parakeet_sample_16k.wav").Path` and `$denoiseDir` as a fresh absolute directory outside the repository. Keep the synthesized `$wavRetained` from step 5.

- Verify `$bin speech list --json` contains persisted labels and `$bin speech list --voice mi_voz --json` filters by voice. `$bin speech play --label e2e_auto_default --voice default --json` must report `played` and actual audio playback; if no output device, mark that playback assertion `SKIP`. `$bin speech remove --label e2e_direct_en --voice default --json` must exit 0 and the label must then be absent. Preserve `$wavRetained`.
- Step 3 leaves the daemon running/warm. Execute `$bin speech synthesize --text "Hola clon" --label e2e_daemon_clone --voice mi_voz --daemon --json`; require success and retain its WAV as the cloned-voice short-text WER sample. This verifies the ForceDaemon cloned-voice cell with a persisted artifact.
- First persist one disposable utterance for `mi_voz_denoise` with `$bin speech synthesize --text "Test voice" --voice mi_voz_denoise --label e2e_voice_remove --no-daemon --json`; then `$bin voice remove --name mi_voz_denoise --yes --json` must remove that test voice and its owned speech namespace. Verify both are absent. Do not remove `mi_voz`, `mi_voz_daemon` or any voice/audio artifact promised for final retention.
- With Parakeet available, run `$bin speech transcribe --audio $sttSample --source-language es --no-daemon --json` and verify non-empty text, source language and schema. If a capture device is available, also run `$bin speech transcribe --mic --duration 5 --source-language es --no-daemon --json`; otherwise mic capture is `SKIP`. `transcribe` requires exactly one of `--audio` or `--mic`. The daemon-routed successful STT case is covered by the current Rust resource gate, which must be run and reported separately.
- Verify real translation in both directions: `$bin translate --text "Hola" --source-language es --target-language en --daemon --json` and, after stopping the daemon, `$bin translate --text "Hello" --source-language en --target-language es --no-daemon --json`. Assert translated text is non-empty and differs from input. Keep `$bin translate --text "Hola" --source-language es --target-language es --json` as passthrough control that returns input intact.
- With the daemon running, execute `$bin speech dub --audio $sttSample --source-language es --target-language en --daemon --json` as the ForceDaemon cross-language case. Stop the daemon and run `$bin speech dub --audio $wavRetained --source-language es --target-language es --no-daemon --json` as the ForceDirect passthrough case. Require a non-empty transcript/result and verify the response contract rather than expecting a persisted WAV. `dub` requires audible playback; without an output device report `SKIP`, not a pass.
- Exercise opt-in cleaning with `$bin speech transcribe --audio $sttSample --source-language es --denoise --no-daemon --json` and `$bin speech dub --audio $wavRetained --source-language es --target-language es --denoise --no-daemon --json`. Require successful payloads and independently verify DPDFNet is provisioned. `voice clone --denoise` is covered in step 4; detailed daemon request-field propagation is covered by daemon contract/resource tests, which must be run and reported for the current source revision.
- `$bin speech denoise --version` must identify the bundled FFmpeg. Run `$bin speech denoise $reference --output-dir $denoiseDir --json` once with default studio preset and once with `--preset default`; parse JSONL records and final summary, require zero failures and non-empty output files. This is local-only; do not use `--model-dir` or `--ffmpeg-path` (parser rejection is covered by Rust tests).

### 7. `src/main.rs` — final installed-state verification

- In a fresh PowerShell process created after install, `where.exe ai-voice-interconnector` must resolve to `%LOCALAPPDATA%\Programs\ai-voice-interconnector\ai-voice-interconnector.exe`; verify persistent HKCU PATH contains exactly one matching entry. `$bin version --json` must still match the release and schema 5.
- In that same fresh process, `$bin doctor --json` must exit 0 with `failed: []`, `path.resolves_to_this_install: true`, `duplicate_entries: []`, valid receipt and all six models provisioned; record the installed data/model roots and sizes. The `pending` object must have empty `unknown_owner`, `pending_failures` and `unreadable` arrays.
- Verify the retained cloned voice's `reference.qvoice`, `$wavRetained` and at least one generated denoise output still exist and are non-empty. Confirm final daemon status and report whether it is running/warm or stopped.
- Do not run cleanup, uninstall or `cargo xtask clean` after the E2E; leave the release installed, models provisioned and intended user data retained.

## Output format

After execution, respond in Spanish. Keep instructions in this skill in English; the final headings and content below are user-facing Spanish output requirements:

### Recorrido (walkthrough)

1. **Proceso seguido** — tareas ejecutadas en orden `0a→0b→1→2→3→4→5→6→7`, con comandos exactos, códigos de salida, evidencia de payloads y estado `PASS/FAIL/SKIP/BLOCKED` por paso; informar al final la aplicación, los modelos y los datos conservados.
2. **Desviaciones respecto al plan** — cada divergencia de esta skill y su motivo, incluidas pruebas bloqueadas/omitidas y acciones de limpieza o recuperación no ejecutadas; o declarar explícitamente que no hubo desviaciones.

No mutations to `docs/`, `Cargo` or `.circleci` during E2E. Step 0a inspects every invocation; destructive purge happens only after fresh, path-specific approval. The final cleanup is never run.

## Verification checklist

- [ ] `0a` inventory, dry-run, path-specific fresh consent and post-removal verification are recorded; `not_installed` is not uninstall evidence; `removal_scheduled` is pending until absence is verified; missing binary with residue is `BLOCKED`
- [ ] Published version and checksum, 7 Release assets, install receipt and schema 5 are verified; PATH persistence and fresh-process resolution are separate checks
- [ ] `setup --json` exits 0 with six exact pinned models; doctor in a fresh process exits 0 with `failed: []` and no pending cleanup artifacts
- [ ] Daemon lifecycle follows step 3 sequence, checks process/payload/health, and never kills by image name
- [ ] `devices --json`; clone by reference direct and daemon; cloned qvoice and no copied WAV; duplicate conflict/force; clone denoise; microphone capture only when real device exists
- [ ] Synthesis matrix uses valid full arguments across Auto/ForceDaemon/ForceDirect, factory/cloned voices, WAV PCM16 mono 24 kHz, a valid temperature and one cross-language case; short WER covers factory and newly cloned voice with transcript and reproducible calculation
- [ ] `speech list/play/remove`, `voice remove` on disposable voice with owned speech, STT, effective translate es→en/en→es plus passthrough, dub direct/daemon and cross-language path; device-dependent playback/capture are explicit SKIP when unavailable
- [ ] Denoise opt-in clone/transcribe/dub and actual `speech denoise` JSONL outputs using packaged FFmpeg; detailed parser/error flag matrix remains delegated to Rust tests
- [ ] Final verification retains installed release, one PATH entry, six models, designated cloned qvoice, synthesized WAV and denoise output; no cleanup/uninstall after E2E

## Constraints

- **The bootstrap MUST be invoked with `AVI_NO_SETUP=1`** (`$env:AVI_NO_SETUP = "1"` before `irm | iex`) to avoid unbounded setup inside bootstrap. Step 2 runs setup explicitly with `Start-Process -PassThru`, fresh stdout/stderr logs, and `WaitForExit(1200000)`. On timeout stop only this invocation's PID/tree, inspect state and require fresh approval before `--force-update --yes`; never kill by image name. `curl … install.sh` is not used on this Windows-only test.
- Run only on the Windows dev machine as final user; use the network and actual release assets. Rust contract tests should be run separately against the current source and must report their full command/features, pass/fail and ignored count; a historical release gate is not evidence for later source changes.
- The ~5.86 GB download is expected only after fresh consent. WAV is PCM16 mono 24 kHz. Use the same text normalization and WER algorithm as `tests/cli_golden.rs`; report raw transcript and calculation. Models are pinned in `MODEL_REVISIONS` in `crates/avi-shared/src/paths.rs`: CustomVoice `85e237c`, Base `5d83992`, Parakeet `8f23f0c`, translation es-en `6eabf2f` / en-es `452971f`, DPDFNet `dd6818d`.
- Never launch `daemon start` with pipe capture. Use detached `Start-Process -PassThru` with fresh redirection, bounded waiting, exit code/payload, `daemon status` and `/health`; on timeout only terminate processes owned by this run.
- Never run a CLI command requiring confirmation without its explicit `--yes`. `--force` does not replace confirmation. The initial purge and setup `--force-update` each require distinct, fresh approval; no destructive cleanup runs after the test.
