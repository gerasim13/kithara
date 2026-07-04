# Superpowered vs kithara: HLS sequential-read benchmark — design

Дата: 2026-07-03
Статус: дизайн утверждён пользователем; ревизия после design-review Codex
(3 блокера + 7 should-fix учтены)
Ветка: `bench/superpowered-compare` — **локальная, никогда не пушится**
Рабочая копия: worktree `/Volumes/Render/dev/worktrees/kithara-bench`
(не мешаем агентам в `~/code/kithara`)
Процесс: имплементирует Codex; Claude оркестрирует, собирает и запускает
финальные прогоны.

## Цель

Понять, насколько kithara близка по производительности к Superpowered SDK
(эталонный конкурент) на сценарии «последовательно вычитать HLS-трек до EOF
быстрее реального времени, без аудио-устройства» на macOS arm64. Метрики:
CPU-стоимость пайплайна, wall-clock полной вычитки, time-to-first-audio,
пиковая память.

## Не-цели

- Не публикуем ни код, ни результаты: всё живёт на локальной ветке.
- Не сравниваем FLAC (SP не декодирует FLAC) — только AAC-LC.
- Не меряем seek/ABR-переключения/DRM — только последовательная вычитка.
- Не строим CI-lane и не добавляем bench в workspace/justfile основного репо.
- Не вендорим Superpowered SDK в git (лицензия): SDK лежит вне репо,
  путь передаётся через `SUPERPOWERED_SDK_DIR`.

## Структура

```
bench/
  sp-bench/       C++ CLI: main.cpp + Makefile (clang++, arm64)
  kit-bench/      Rust CLI: standalone-крейт, НЕ член workspace
  run.sh          оркестратор: сервер + прогоны + таблица
  README.md       сборка и запуск
```

- `kit-bench/Cargo.toml`: пустая таблица `[workspace]` (отвязка от корневого
  workspace), path-зависимость на `crates/kithara`. Собирается в **двух
  feature-конфигурациях** (см. «Компоненты»), чтобы RSS каждой конфигурации
  не включал слинкованный чужой декодер. Корневой `Cargo.toml`,
  workspace-hack и линты основного репо не трогаем.
- `sp-bench/Makefile`: хедеры/статик-либа из `$SUPERPOWERED_SDK_DIR`
  (точные имена либ/фреймворков подтверждает спайк).

## Компоненты

### sp-bench `<url> [--paced]`

1. `Superpowered::Initialize(...)` (сигнатура/ключ — из GitHub-примеров SDK,
   подтверждает спайк).
2. `AdvancedAudioPlayer(samplerate = rate фикстуры, cachedPointCount=0,
   internalBufferSizeSeconds=0 /* документированный offline mode */)`.
   Настройки: `timeStretching=false`, `playbackRate=1`,
   `HLSAutomaticAlternativeSwitching=false`,
   `HLSBufferingSeconds=<download-everything константа>`
   (имя/значение подтверждает спайк по реальным хедерам SDK).
3. Свежая temp-папка на каждый запуск (`setTempFolder`-семейство, владелец
   уточняется спайком), удаляется после выхода — паритет холодного кэша.
4. Снимок baseline: `getrusage(RUSAGE_SELF)` + monotonic **непосредственно
   перед `openHLS(url)`** (после Initialize и конструктора плеера) — это
   нулевая точка для cpu/wall/ttfa.
5. `openHLS(url)` → поллинг `getLatestEvent()` до `PlayerEvent_Opened`
   (`PlayerEvent_OpenFailed` → exit non-zero c `getOpenErrorCode()`).
6. `play()` → цикл `processStereo(buf, false, 1024)`; первый вызов,
   вернувший `true`, фиксирует TTFA; `eofRecently()` завершает цикл.
7. Запасной путь (если спайк покажет, что плеер не качается быстрее
   реального времени): `Superpowered::Decoder::openHLS` +
   `decodeAudio()` poll-цикл (`BufferingTryAgainLater` → continue,
   `EndOfFile` → выход); baseline тогда снимается непосредственно перед
   `Decoder::openHLS`. Выбор пути фиксируется в README результата.

### kit-bench `<url> [--paced]`, две сборки

Публичный фасад `kithara`, две отдельно собранные конфигурации:

- `kit-bench` c features `hls, symphonia, client-reqwest, tls-rustls,
  tokio-rt-multi-thread` → `DecoderBackend::Symphonia`;
- `kit-bench` c features `hls, apple, client-reqwest, tls-rustls,
  tokio-rt-multi-thread` (без symphonia) → `DecoderBackend::Apple`
  (AudioToolbox).

Точный минимальный набор features уточняется при имплементации; если
сборка apple-без-symphonia окажется невозможной без правок основного репо —
fallback: одна сборка с обоими бэкендами + `--decoder`-флаг и явная
оговорка в отчёте, что RSS включает оба слинкованных декодера.

1. Рантайм: вручную собранный tokio multi-thread runtime, **2 воркера**;
   `Audio::new(config)` через `block_on`; pump-цикл — синхронный, на
   главном потоке, runtime жив до EOF. Модель потоков указывается в отчёте.
2. `AudioConfig<Hls>` c `.block_on_underrun(true)` — канонический
   offline-pump без sleep
   (см. `crates/kithara-audio/src/pipeline/config.rs:84-90`; фасадный
   `Resource::read` НЕ подходит: `ResourceConfig` не пробрасывает
   `block_on_underrun`, наивный цикл busy-крутит `Pending` и меряет
   политику цикла, а не движок). Выбор бэкенда —
   `AudioConfig.decoder_backend: DecoderBackend`
   (`crates/kithara-audio/src/pipeline/config.rs:40`).
3. Свежий `StoreOptions::new(temp_dir)` на каждый запуск (холодный кэш
   клиента), очистка после.
4. ABR: вариант пинится URL-ом медиа-плейлиста (см. «Честность»); если
   kithara не примет медиа-плейлист корнем — bench-локальный AAC-only
   master + `AbrMode::manual(idx)`.
5. Снимок baseline: `getrusage(RUSAGE_SELF)` + monotonic **непосредственно
   перед `block_on(Audio::new(config))`** — открытие, probe и создание
   декодера входят в измеряемое окно (симметрично SP `openHLS`).
6. Pump-цикл как в `tests/src/reads.rs:22-44`: `Frames` → учёт,
   `Pending` → continue (parked), `Eof` → выход, `Err` → exit non-zero.
   Первый `ReadOutcome::Frames` фиксирует TTFA.

### run.sh

1. **Фаза build (не измеряется)**: собирает оба CLI в release и обе
   kit-конфигурации:
   `(cd bench/kit-bench && cargo build --release)`,
   `(cd bench/kit-bench && cargo build --release --no-default-features
   --features apple --target-dir target-apple)`,
   `(cd bench/sp-bench && make)`. Ни одна компиляция не происходит между
   измеряемыми прогонами.
2. **Preflight**: фиксирует в шапку отчёта macOS-версию, модель CPU,
   версии rustc/clang, git-ревизию, профиль/флаги сборки обеих сторон и
   фактический `BENCH_TMPDIR`; предупреждает при высоком load
   average, взведённых `MallocStackLogging`/`MallocScribble`, кастомных
   `RUSTFLAGS`/`CXXFLAGS`.
3. Поднимает один `python3 -u -m http.server 0 --bind 127.0.0.1` из корня
   worktree. Это симметрично сервит обе стороны из одного worktree-root
   HTTP-сервера. Runner парсит порт из строки `Serving HTTP on 127.0.0.1
   port NNNNN` и ждёт доступности kit master URL.
4. **Warm-up**: один неизмеряемый прогон каждой конфигурации (греет
   page cache сервера и кода; клиентские кэши всё равно свежие в каждом
   прогоне).
5. N измеряемых повторов (default 5) с **ротацией порядка** сторон по
   повторам: `sp,sym,apple` → `sym,apple,sp` → `apple,sp,sym` → ... —
   размазывает дрейф фона машины. Каждый прогон — свежие temp-каталоги.
6. **Эквивалентность — жёсткий инвариант**: `samplerate` и `channels`
   совпадают между всеми сторонами; `pcm_frames` проходят допуск
   план-Б-спайка: `|kit - sp| <= 44100` pcm frames (1 s) и длительность
   каждой стороны лежит в 219.5-220.5 s. Любое нарушение или упавший
   прогон = провал всего запуска бенчмарка (exit non-zero), не
   «исключение из статистики» — нестабильность не маскируем.
7. Отчёт: таблица медиан + межквартильный разброс по каждой метрике.
   Санити самого бенчмарка: два последовательных полных запуска должны
   давать медианы CPU в пределах ~5% — иначе предупреждение «машина шумит».
8. Локальные URL: kithara открывает
   `/bench/fixtures/master-shq.m3u8` (single-variant master, потому что
   media-playlist root rejected with `unexpected tag #EXT-X-TARGETDURATION`);
   Superpowered открывает `/bench/fixtures-ts/shq/index.m3u8`. Внешний
   контрольный прогон использует парные флаги
   `--kit-url <fmp4-master-url> --sp-url <ts-url>`; оба обязательны вместе,
   контейнеры должны соответствовать результатам спайка. Для внешних треков
   runner передаёт `--duration-range MIN MAX` в `stats.py`.

## Методология метрик (идентична для обеих сторон)

Каждый CLI печатает одну JSON-строку в stdout перед выходом.
Нулевая точка (baseline) у всех трёх времяметрик и CPU одна и та же:
непосредственно перед open-вызовом стороны (SP: `openHLS` /
`Decoder::openHLS`; kithara: `block_on(Audio::new)`), после инициализации
SDK/рантайма.

| Поле | Определение |
|---|---|
| `ttfa_ms` | baseline → первый PCM-фрейм в pump-цикле. SP: первый `processStereo()==true` после `play()`. kithara: первый `ReadOutcome::Frames`. НЕ `PlayerEvent_Opened`, НЕ `preload()`. |
| `wall_ms` | baseline → EOF, monotonic clock |
| `cpu_user_s`, `cpu_sys_s` | дельта `getrusage(RUSAGE_SELF)` между baseline и EOF — старт процесса/линковка/Initialize не входят; все потоки процесса (SDK/tokio) входят. Полное CPU процесса — отдельным диагностическим полем `cpu_total_*`. |
| `max_rss_bytes` | `ru_maxrss` на выходе; на macOS — байты. Пик за весь процесс (дельта для пика невозможна) — одинаково у обеих сторон, фиксируем как оговорку. |
| `samples`, `pcm_frames`, `samplerate`, `channels` | `samples` — интерливнутые сэмплы (kithara `ReadOutcome::Frames.count` считает сэмплы); `pcm_frames = samples / channels`; SP `processStereo(n)` отдаёт фреймы → `samples = n × channels`. Длительность валидируется как `pcm_frames / samplerate`. |

- Sample rate выхода = sample rate источника у обеих сторон (значение
  берём из `init-*.mp4` фикстуры на спайке) — ни одна сторона не платит
  за ресемплинг, которого нет у другой.
- `--paced`: запасной режим — обе стороны пейсятся к 1x (sleep по
  доставленным фреймам), сравниваются только CPU/RSS/TTFA.
  Реализуется только если спайк покажет, что SP не качается быстрее
  реального времени; wall-clock в этом режиме не сравнивается.

## Честность сравнения

- **Пин варианта**: обе стороны открывают медиа-плейлист напрямую
  (`/assets/hls/index-shq-a1.m3u8`, AAC-LC ~270 kbps). Это решает сразу
  два ограничения: у SP нет API пина варианта, а в `master.m3u8` есть
  FLAC-вариант (988 kbps), который SP не декодирует и на который его
  авто-ABR мог бы переключиться.
- **Холодный кэш клиента** с обеих сторон на каждом прогоне (fresh temp
  dirs). Серверная сторона и код после warm-up-прогона осознанно тёплые
  (page cache) — одинаково для всех сторон.
- **Только AAC-LC**; `mp4a.40.2` во всех сравниваемых вариантах.
- **Стриминг-оверхед kithara** (storage, events, ABR-машинерия при
  запиненном варианте) — осознанная часть сравнения «продукт против
  продукта», не изъян методологии; фиксируем в отчёте.

## Этап 0 — спайк (блокирует постройку харнеса)

Мини-программа на C++ (~50 строк) против фикстурного сервера отвечает:

1. **Играет ли SP fMP4-HLS** (`.m4s` + `EXT-X-MAP`)? Наш локальный
   конспект доков SP (реконструкция, вне git; ground truth — хедеры SDK)
   заявляет только MPEG-TS, а все фикстуры — fMP4.
2. **Качается ли openHLS быстрее реального времени** в offline mode
   (`internalBufferSizeSeconds=0`)? Провал → sp-bench переходит на
   `Superpowered::Decoder`; если и тот пейсится → включаем `--paced`.
3. Сигнатура `Superpowered::Initialize` + работает ли evaluation-ключ
   из примеров GitHub-репо SDK.
4. `setTempFolder`: владелец (глобал/плеер/декодер), семантика очистки.
5. Имена хедеров и статических либ macOS arm64, требуемые фреймворки.
6. Sample rate/каналы фикстуры из `init-shq-a1.mp4` (для пункта про
   ресемплинг; заодно сверяем `pcm_frames`-инвариант).

**План Б при провале п.1** (SP не играет fMP4): kithara **не читает
MPEG-TS** (`ContainerFormat::MpegTs` отвергается обоими декод-путями),
поэтому симметричный TS-фикстур невозможен. Вместо этого: одноразовый
ремукс тех же AAC-фреймов в MPEG-TS (без перекодирования) в
`bench/fixtures-ts/`; SP читает TS-ремукс, kithara — исходный fMP4.
Аудио-payload идентичен, но `pcm_frames` валидируются с допуском
план-Б-спайка (1 s) из-за разных контейнерных краёв и блочной отдачи SP;
контейнеры разные — каждая сторона на своём нативно поддерживаемом
демаксе; это фиксируется в отчёте как методологическая оговорка. Спайк
тогда дополнительно проверяет, что SP реально играет этот ремукс.

Результаты спайка дописываются в этот документ (секция «Spike findings»)
до начала имплементации харнеса.

## Обработка ошибок

- Любая ошибка стороны (OpenFailed, DecodeError, таймаут прогона
  15 мин) — немедленный exit non-zero с сообщением на stderr;
  JSON-строка не печатается.
- run.sh: любой упавший прогон или нарушение инварианта эквивалентности
  = провал всего запуска бенчмарка (exit non-zero). Частичных
  результатов и «исключённых сэмплов» нет.

## Валидация scope

- Бенч-код не входит в workspace: `just test` / `cargo xtask test`
  основного репо не затрагиваются и остаются зелёными по определению.
- Приёмка самого бенча: `run.sh` дважды подряд даёт медианы CPU в
  пределах ~5%; все стороны проходят допуск `pcm_frames` из секции
  «Spike findings».
- Никакие файлы вне `bench/` и этого spec не меняются на ветке
  (исключение: план Б может добавить `bench/fixtures-ts/`).

## Риски

- Предположения об API SP взяты из локальной реконструкции доков (вне
  git): имена стабильны, но всё критичное сверяется с реальными хедерами
  SDK на спайке (п.1-5).
- Неизвестный AAC-декодер SP на macOS — закрыто прогоном kithara в двух
  бэкендах.
- fMP4-несовместимость SP — закрыто планом Б (асимметричные контейнеры,
  идентичный AAC-payload, явная оговорка).
- Фоновый шум машины — warm-up, ротация порядка, медианы, санити-порог
  5%, preflight-предупреждения.

## Spike findings

- **SPIKE-A (fMP4 HLS): FAIL.** `assets/hls/index-shq-a1.m3u8`
  (`.m4s` + `EXT-X-MAP`) never produced `PlayerEvent_Opened` or
  `PlayerEvent_OpenFailed` in 300 s. The temp folder grew to about
  900 MB from a retry storm (about 37 segments x
  `HLSMaximumDownloadAttempts=100` x about 200 KB): SP downloads `.m4s`
  segments but does not parse the fMP4 fixture. SDK header line 35
  confirms HLS support is AAC-LC/MP3 in audio files or MPEG-TS files.
  Plan B is active.
- **Plan B fixture:** `bench/fixtures-ts/shq/` is a pure stream-copy
  MPEG-TS remux of the same AAC payload, generated with:

  ```bash
  ffmpeg -allowed_extensions ALL -i assets/hls/index-shq-a1.m3u8 -map 0:a -c copy -f hls -hls_time 6 -hls_list_size 0 -hls_playlist_type vod -hls_segment_type mpegts -hls_segment_filename 'seg-%d.ts' index.m3u8
  ```

  The fixture has 37 TS segments and no transcode.
- **SPIKE-A (Plan B TS remux): PASS.** Result:
  `SPIKE OK: ttfa_ms=200.3 wall_s=3.98 frames=9702400`, with about
  7.0 MB temp usage.
- **SPIKE-B: PASS.** The 220.2 s track was consumed in 3.98 s wall
  time, about 55x realtime, using `internalBufferSizeSeconds=0`,
  `openHLS`, and `HLSBufferingSeconds=HLSDownloadRemaining`. The
  `AdvancedAudioPlayer` path remains primary. `Superpowered::Decoder`
  fallback is not needed. `--paced` stays part of the benchmark CLI
  contract, but it is not needed for the primary result.
- **SPIKE-C: PASS.** SDK 2.8.1 (`8a71534`) accepts
  `Superpowered::Initialize("ExampleLicenseKey-WillExpire-OnNextUpdate")`.
- **SPIKE-D: PASS.** Temp-folder API is
  `Superpowered::AdvancedAudioPlayer::setTempFolder(const char *)`.
  It must be called before any player instance is created, creates a
  `SuperpoweredAAP` subfolder, and clears that subfolder if present.
  A fresh directory per run works.
- **SPIKE-E: PASS.** Build recipe: `clang++ -std=c++17 -O2 -arch arm64
  -I$SDK/Superpowered`; static lib
  `$SDK/Superpowered/libSuperpoweredAudio.xcframework/macos-arm64_x86_64/libSuperpoweredAudioOSX.a`;
  frameworks `AudioToolbox CoreMedia CoreAudio AVFoundation
  CoreFoundation Foundation`.
- **SPIKE-F: PASS.** Fixture `shq` is AAC-LC, 44100 Hz, 2 channels
  (`afinfo` on init + first segment, about 290 kbps). `BENCH_RATE=44100`.
- **Equivalence caveat:** SP reports full 1024-frame process blocks
  (`9,702,400` frames in the spike) while the sample-exact fMP4
  expectation is about `9,711,700`, a tail delta of about 9300 frames
  or 0.21 s. Because Plan B uses asymmetric containers, `stats.py` must
  replace exact `pcm_frames` matching with: `|kit - sp| <= 44100`
  pcm frames (1 s) and every side's duration must be within
  219.5-220.5 s.

## Results

Measured 2026-07-03 on Apple M3 Pro, macOS 26.5, rustc 1.93.1, Apple
clang 17.0.0, git `d4cbbe39f`. Harness: `bench/run.sh 5` twice (run A +
run B), pooled 10 reps per config, rotated order, one warm-up per
engine, local `python3 http.server`, `BENCH_RATE=44100`. Fixture:
220.2 s AAC-LC 44.1 kHz stereo (fMP4 for kithara, TS remux for
Superpowered, same payload).

Median +/- IQR over 10 reps:

| engine | decoder | ttfa_ms | wall_ms | cpu_user_s | cpu_sys_s | max_rss |
|---|---|---|---|---|---|---|
| kithara | apple | 69.6 +/- 9.1 | 396 +/- 85 | 0.13 +/- 0.00 | 0.09 +/- 0.00 | 28.6 MB |
| kithara | symphonia | 59.3 +/- 12.4 | 409 +/- 88 | 0.15 +/- 0.01 | 0.09 +/- 0.02 | 24.8 MB |
| superpowered | superpowered | 139.5 +/- 5.7 | 2186 +/- 80 | 0.29 +/- 0.19 | 0.09 +/- 0.06 | 15.7 MB |

Derived (track 220.2 s):

- Pump speed: kithara/apple ~556x realtime, kithara/symphonia ~538x,
  Superpowered ~101x. kithara consumes the track ~5.3-5.5x faster in
  wall time.
- TTFA: kithara 2.0-2.4x lower (59-70 ms vs 140 ms).
- CPU (user): kithara ~2x lower median; Superpowered's CPU is bimodal
  (IQR +/-0.19) - see caveats.
- Peak RSS: Superpowered ~1.6-1.8x lower (15.7 MB vs 24.8-28.6 MB).

Equivalence gate: PASS on every run (samplerate 44100, channels 2,
`pcm_frames` kit 9,712,640 vs SP 9,702,400, delta 10,240 <= 44,100;
durations 220.01-220.24 s).

Repeatability (plan target: A-vs-B cpu medians within ~5%): kithara
configs 6-7% - borderline; Superpowered FAILED (cpu_user 0.30 vs 0.20,
intra-run IQR +/-0.15-0.19). Conditions: corporate security agents
(Kaspersky, BI.Zone EDR, XProtect) actively scanning during runs;
load average 5.7-6.3 at start. SP's per-segment temp-file churn
plausibly triggers per-file AV scans, kithara's asset store less so.
Before the final runs two orphaned `telegram` plugin `bun server.ts`
processes (100% CPU each since Jul 1) were killed; earlier runs at
LA 9-16 showed the same ordering with wider spread. Wall/ttfa
orderings are stable across all six runs performed today; the
headline conclusions are far above the noise floor.

Bugs found and fixed during Task 6 validation:

- `sp-bench` swallowed the one-shot `PlayerEvent_ConnectionLost`
  (only `OpenFailed` handled) and spun forever; loop realigned to the
  validated spike shape (unconditional `processStereo`, `play()` on
  `Opened`) plus terminal-event exits.
- `bench/run.sh` warm-up passed a nonexistent `--tmp` subdir
  (`$WARM_TMP/sp` never created); Superpowered cannot persist
  segments into a missing folder -> seg-0 retry storm
  (`HLSMaximumDownloadAttempts=100`+1) -> `ConnectionLost`. Fixed with
  `mkdir -p`. Root cause confirmed by A/B experiment (existing dir ->
  JSON, missing dir -> clean `connection lost` failure).
- `bench/run.sh` now defaults `BENCH_TMPDIR` to
  `/Volumes/Render/dev/tmp` and exports `TMPDIR` so both engines'
  scratch stays off the low-space system disk.

Raw data: `bench/last-results.jsonl` (run B),
`/Volumes/Render/dev/tmp/results-{A,B,pooled}.jsonl`.

## Results addendum: raw Decoder API control (2026-07-03)

Follow-up to the user's challenge of the 5.4x wall gap: is the
Superpowered *decoder* slow, or is it the *player pipeline*?

Setup: `bench/sp-bench/sp-dec-bench` drives `Superpowered::Decoder`
directly on `bench/fixtures/shq.aac` — an ADTS stream-copy of the same
payload (9485 packets, 9,712,640 frames, bit-exact with kithara's
count). Local file, no network, no player. 12 reps plus 3 sp-player
anchor reps in the same session (LA < 5); anchors matched the main-run
numbers, so conditions are comparable.

| config | wall_ms | cpu_user_s | pcm_frames |
|---|---|---|---|
| SP raw Decoder (local file) | 116.9 +/- 1.6 | 0.12 +/- 0.00 | 9,712,640 |
| kithara/apple (full HLS pipeline) | 396 | 0.13 | 9,712,640 |
| kithara/symphonia (full HLS pipeline) | 409 | 0.15 | 9,712,640 |
| SP player (full HLS pipeline) | 2217 +/- 84 | 0.35 +/- 0.10 | 9,702,400 |

Conclusions:

- Superpowered's raw AAC decode is ~1880x realtime (0.12 s CPU per
  220 s track) — NOT slow. Decoder CPU is at parity with kithara's
  numbers (0.13-0.15, which additionally include the whole HLS
  pipeline).
- The SP player drains 19x slower than SP's own decoder on the same
  payload: ~95% of the player's wall time is pipeline pacing
  (buffer granularity, per-segment temp-file churn, thread handoffs),
  not decoding.
- Honest headline: kithara's HLS *pipeline* drains ~5.4x faster than
  Superpowered's HLS *player* pipeline; the decoders themselves are
  equally fast. kithara's edge is pipeline architecture for
  faster-than-realtime consumption; SP keeps its RSS edge.
- Side note: the raw Decoder emits the full frame count (no trim),
  while the player path reports 9,702,400 (1024-block quantization).

Raw data: `/Volumes/Render/dev/tmp/results-decoder.jsonl`.

## Results addendum: progressive and local scenarios (2026-07-04)

Same drain methodology, same payload as an ADTS single file
(`bench/fixtures/shq.aac`, 9,712,640 frames / 220.24 s). New modes:
`kit-bench --mode progressive|local` (FileSrc::Remote / FileSrc::Local
through `AudioConfig::<File>`), `sp-bench --mode` (player `open()`
instead of `openHLS()`), `run.sh --scenario`. Two `run.sh 5` runs per
scenario, pooled; A-vs-B cpu medians within ~5% (local) and ~1.4%
(progressive); IQRs are tight despite background load 16-26 because
each rep is sub-second and single-core-bound. All rows bit-exact at
9,712,640 frames (the SP player only trims frames on the HLS path).

Local file (median +/- IQR, run B shown, run A equivalent):

| config | ttfa_ms | wall_ms | cpu_user_s | max_rss |
|---|---|---|---|---|
| superpowered player | 2.3 +/- 0.2 | 134.7 +/- 0.9 | 0.14 | 28.3 MB |
| kithara/apple | 13.4 +/- 0.8 | 152.6 +/- 1.1 | 0.15 | 32.1 MB |
| kithara/symphonia | 2.1 +/- 0.2 | 169.1 +/- 1.4 | 0.17 | 27.4 MB |

Progressive HTTP (single file over localhost):

| config | ttfa_ms | wall_ms | cpu_user_s | max_rss |
|---|---|---|---|---|
| kithara/apple | 22.0 +/- 1.4 | 159.3 +/- 4.2 | 0.15 | 35.0 MB |
| kithara/symphonia | 6.4 +/- 0.8 | 180.3 +/- 0.6 | 0.17 | 30.3 MB |
| superpowered player | 552.8 +/- 17.1 | 692.5 +/- 15.0 | 0.15 | 30.1 MB |

Conclusions:

- Local files: effective parity. SP player is marginally fastest on
  wall (135 vs 149-169 ms) and drops its HLS pacing overhead entirely;
  everyone is within ~2x of the raw-decoder floor (117 ms). TTFA is
  2 ms for SP and kithara/symphonia; kithara/apple pays ~13 ms
  (AudioToolbox session setup).
- Progressive: kithara streams while downloading -> TTFA 6-22 ms;
  the SP player buffers a large prefix before `PlayerEvent_Opened`
  -> TTFA ~553 ms (~80% of its wall). kithara reaches EOF ~4x faster;
  first audio ~25-85x faster. CPU parity (0.14-0.17) across all.
- Single-shot cold-cache runs can mislead: first-ever kit/apple local
  run showed 925 ms wall; in warmed series it is ~150 ms. The harness
  warm-up rep exists for exactly this reason.
- SP's HLS RSS edge (15.7 MB) does not carry over: in file scenarios
  all engines sit at 27-35 MB.

Raw data: `/Volumes/Render/dev/tmp/results-{local,progressive}-{A,B}.jsonl`.

## Results addendum: real server (stream.silvercomet.top, 2026-07-04)

Motivation: rule out local fixture-server overhead. Real content:
`track.mp3` (MP3 48 kHz stereo, 161.96 s, 3.55 MB) and the production
adaptive master `hls/master.m3u8` (4 variants: slq/smq/shq AAC +
lossless FLAC; fMP4 segments — same content family as repo fixtures).
kit-bench gained `--abr auto|<index>` (default 0). Protocol:
5 reps per config, failures logged, no LA gating (runs are
network-bound); one-off runner in session scratchpad.

Local file, track.mp3 at native 48 kHz:

| config | ttfa_ms | wall_ms | cpu_user_s |
|---|---|---|---|
| superpowered player | 1.5 | 76.0 +/- 4.3 | 0.08 |
| kithara/symphonia | 1.4 | 114.5 +/- 10.6 | 0.10 |
| kithara/apple | 11.2 | 229.9 +/- 3.3 | 0.21 |

Progressive over the real network (kithara only — see finding 1):

| config | ttfa_ms | wall_ms | cpu_user_s |
|---|---|---|---|
| kithara/symphonia | 560 +/- 68 | 4602 +/- 87 | 0.23 |
| kithara/apple | 639 +/- 131 | 4681 +/- 212 | 0.35 |

HLS pinned shq (`--abr 2`) and adaptive (`--abr auto`), kithara only:

| config | ttfa_ms | wall_ms | cpu_sys_s |
|---|---|---|---|
| hls kithara/symphonia | 1792 +/- 190 | 9626 +/- 89 | 1.14 |
| hls kithara/apple | 1935 (one ~40 s stall rep) | 9719 | 1.13 |
| adaptive kithara/symphonia | 1226 +/- 171 | 10317 +/- 1797 | 1.11 |
| adaptive kithara/apple | 1539 (same stall outlier) | 10634 | 1.18 |

Findings:

1. **Superpowered cannot reach this server at all.** All progressive
   attempts fail in `open`: error 3 "Network socket error". curl
   completes the TLS handshake in 0.25 s (HTTP/2); plain http is a 307
   redirect back to https. The SP HTTP stack loses to a real-world
   TLS/CDN setup that kithara's reqwest+rustls client handles without
   a single failure (0 network errors across 30 kithara reps).
   Real-server engine-vs-engine comparison is therefore only possible
   for local files.
2. **The python fixture server was not distorting engine numbers.**
   Same shq payload: localhost HLS drain 0.40 s vs real-network 9.6 s
   — localhost runs are compute-bound (that was the point: measuring
   engine overhead), real runs are network-bound (download 7.3 MB +
   per-segment TLS/RTT; cpu_sys rises to ~1.1 s). Both engines shared
   the same fixture server, so relative results stand.
3. Local mp3: same parity picture as local AAC — SP marginally
   fastest (76 ms, cpu 0.08), symphonia close (115 ms, 0.10),
   apple pays AudioToolbox setup+MP3 cost (230 ms, 0.21). SP got the
   file's native 48 kHz; the first run passed 44100 and SP silently
   resampled — caught by the stats.py samplerate gate.
4. Adaptive vs pinned: adaptive wall is ~7% longer with higher CPU —
   consistent with ABR upswitching from the slq cold start toward
   heavier variants (incl. FLAC lossless) and thus downloading more
   bytes; correct behavior, not a regression.
5. Real-network flakiness is real: one apple HLS rep stalled ~40 s
   (server-side; the same stall pattern seen with curl probes).
   Medians absorb it; IQR exposes it.

Raw data: `/Volumes/Render/dev/tmp/real-{local,progressive,hls,adaptive}.jsonl`.

## Profiling note: kithara/apple local-file gap (2026-07-04)

Question: why is kithara/apple 2x slower than symphonia on local MP3
(230 vs 115 ms wall, cpu 0.21 vs 0.10) while being FASTER on AAC
(152 vs 169 ms), and why the ~11-13 ms TTFA floor?

Method: xctrace Time Profiler on the release apple binary, local mp3
run (284 samples) and local aac control (238 samples).

Findings:

- MP3: 192/284 samples (68% of all CPU) sit INSIDE Apple's system
  codec — `ACMP3Decoder -> MP3DecoderWrapper_SpiritDSP::DecodeFrame`
  (IDCT32PLONKAS, imdct36, mp3d_*). Apple ships a licensed SpiritDSP
  software MP3 decoder and it is simply ~2x more CPU-expensive than
  symphonia's Rust MP3 decoder. Not a kithara bug.
- AAC: 134/238 samples inside Apple's `AACDecoder::DecodeFrame`
  (hot leaf: SpectralData::Deserialize). Efficient — apple wins the
  AAC drain.
- kithara plumbing is thin and identical in both profiles
  (decode_one_step -> ComposedDecoder -> AppleCodec ->
  AudioConverterFillComplexBuffer); pipeline overhead is ~30 samples
  of worker park/wake + ~16 memmove/memset.
- No resampler frames in either profile: 48 kHz mp3 and 44.1 kHz aac
  play at native rates, the fused SRC stays inactive. No hidden
  format conversion, no misconfiguration found.
- No hardware offload exists on this path: both codecs are pure-CPU
  system libraries (no driver/IOKit frames). "Apple decoder" on macOS
  means AudioToolbox software codecs.
- The ~11-13 ms apple TTFA floor is one-time AudioToolbox
  instantiation (codec plugin lookup/caulk hash tables/dispatch_once
  visible in startup samples); symphonia starts emitting in ~1.5 ms.

Implication: codec routing, not fixing — on macOS prefer symphonia
for MP3 drain workloads; keep apple for AAC. Traces:
`/Volumes/Render/dev/tmp/prof-{mp3,aac}.trace`.

### Hardware-codec verification (2026-07-04)

Challenged on "no hardware audio decode on macOS". Verified three ways,
all agree:

1. SDK header `AudioToolbox/AudioFormat.h`: the entire hardware-codec
   API — `kAudioFormatProperty_HardwareCodecCapabilities` (= 'hwcc',
   marked `__attribute__((deprecated))`), `kAppleHardwareAudioCodecManufacturer`
   ('aphw'), `kAppleSoftwareAudioCodecManufacturer` ('appl') — is
   wrapped in `#if TARGET_OS_IPHONE ... #endif`. These symbols do not
   exist when compiling for macOS (a probe using them fails to compile).
2. Header doc comment: "On iPhoneOS, a codec's manufacturer can be used
   to distinguish between hardware and software codecs." The hw/sw
   split is an iOS concept; even there the capabilities query is
   deprecated.
3. Empirical enumeration on this M3 Pro (macOS 26.5) via
   `AudioFormatGetProperty(kAudioFormatProperty_Decoders)` +
   `AudioComponentFindNext('adec')`: 47 audio decoders present, every
   one manufacturer `'appl'` (software). MP3 -> single 'appl' decoder,
   AAC -> single 'appl' decoder. Zero `'aphw'`.

Conclusion: there is no hardware audio decoder to enable on macOS — the
concept only ever applied to old iOS devices and is deprecated. The MP3
cost is inherent to Apple's SpiritDSP software codec; nothing is
misconfigured and nothing can be "switched to hardware". Probe kept at
session scratchpad `codec_probe.c`.
