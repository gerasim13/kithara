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

1. **Фаза build (не измеряется)**: собирает оба CLI в release, обе
   kit-конфигурации, и `test_server` в release
   (`cargo build --release -p kithara-integration-tests --bin test_server`
   в основном worktree). Ни одна компиляция не происходит между
   измеряемыми прогонами.
2. **Preflight**: фиксирует в шапку отчёта macOS-версию, модель CPU,
   версии rustc/clang, git-ревизию, профиль/флаги сборки обеих сторон и
   фактический `BENCH_TMPDIR`; предупреждает при высоком load
   average, взведённых `MallocStackLogging`/`MallocScribble`, кастомных
   `RUSTFLAGS`/`CXXFLAGS`.
3. Поднимает `test_server` c `TEST_SERVER_PORT=0` (эфемерный порт, чтобы
   не столкнуться с тестами других агентов на этой машине), парсит
   реальный порт из строки «test server listening on ...», ждёт `/health`.
   Фиксированный порт — только по явному флагу.
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
8. `run.sh --url <fmp4-kit-url>` + `SP_URL=<ts-sp-url>` — те же CLI против
   реального интернета (один контрольный прогон; только AAC; контейнеры
   должны соответствовать результатам спайка).

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
