package com.kithara

import com.kithara.ffi.AudioPlayer as FfiAudioPlayer
import com.kithara.ffi.FfiException
import com.kithara.ffi.FfiKeyOptions
import com.kithara.ffi.FfiKeyProcessor
import com.kithara.ffi.FfiKeyRule
import com.kithara.ffi.FfiPlayerConfig
import com.kithara.ffi.FfiPlayerEvent
import com.kithara.ffi.FfiPlayerStatus
import com.kithara.ffi.FfiTrackStatus
import com.kithara.ffi.FfiTransition
import com.kithara.ffi.PlayerObserver
import com.kithara.ffi.SeekCallback
import com.kithara.ffi.defaultCrossfadeDuration
import com.kithara.ffi.defaultPlayingRate
import com.kithara.ffi.drmLowercaseHexSalt
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Queue-based audio player.
 *
 * Observe [state] for reactive updates, or read convenience accessors for the
 * latest snapshot. Call [Kithara.initialize] before creating any instance.
 *
 * ```kotlin
 * val player = KitharaPlayer()
 * val item = KitharaPlayerItem("https://example.com/audio.mp3")
 * player.insert(item)
 * player.play()
 * ```
 */
class KitharaPlayer(config: Config = Config()) {
    /**
     * A single DRM rule: a key processor bound to one or more domain
     * patterns (exact `"example.com"`, wildcard subdomain
     * `"*.example.com"`, or match-any `"*"`), plus optional headers /
     * query params sent with matching key requests, and an optional
     * per-rule salt forwarded to [KeyProcessor.processKey].
     */
    data class KeyRule(
        val processor: KeyProcessor,
        val domains: List<String>,
        val headers: Map<String, String>? = null,
        val queryParams: Map<String, String>? = null,
        val salt: String? = null,
    ) {
        companion object {
            /**
             * The common "one processor for every host" rule: a match-any
             * domain pattern carrying a freshly generated lowercase-hex salt,
             * mirrored into the player-wide `X-Encrypted-Key` header. A pure
             * constructor — put the result into [Config.keyRules].
             */
            @JvmStatic
            fun wildcard(processor: KeyProcessor): KeyRule =
                KeyRule(processor = processor, domains = listOf("*"), salt = drmLowercaseHexSalt())
        }
    }

    /** Configuration for [KitharaPlayer] creation. */
    data class Config(
        val eqBandCount: Int = 10,
        val keyRules: List<KeyRule> = emptyList(),
        /** Shared asset store used by every item created by this player. */
        val store: AssetStore = Kithara.defaultStore,
        /**
         * Auth token sent on every player HTTP request. Empty means no token;
         * replace it later with [setupNetwork].
         */
        val authToken: String = "",
        /**
         * Crossfade window in seconds applied from construction. Replace it
         * later through [crossfadeDuration].
         */
        val crossfadeDuration: Float = defaultCrossfadeDuration(),
        /**
         * Playback-rate target applied from construction (1.0 = normal).
         * Replace it later through [playingRate].
         */
        val playingRate: Float = defaultPlayingRate(),
    )

    private val inner: FfiAudioPlayer = FfiAudioPlayer(
        config.toFfi()
    )
    private val observer = PlayerObserverBridge(this)
    private val eventsFlow = MutableSharedFlow<KitharaPlayerEvent>(extraBufferCapacity = 16)
    private val stateFlow = MutableStateFlow(PlayerState())

    init {
        inner.setObserver(observer)
    }

    /**
     * Full player state as a single observable snapshot.
     */
    val state: StateFlow<PlayerState> = stateFlow.asStateFlow()

    /**
     * One-shot player events: item changes, playback completion, and failures.
     */
    val events: SharedFlow<KitharaPlayerEvent> = eventsFlow.asSharedFlow()

    /** Current playback time in seconds. */
    val currentTime: Double
        get() = state.value.currentTime

    /** Current playback rate. */
    val rate: Float
        get() = state.value.rate

    /** Current player status. */
    val status: PlayerStatus
        get() = state.value.status

    /** Current item duration in seconds, if known. */
    val duration: Double?
        get() = state.value.duration

    /**
     * Currently playing item, or `null` if the queue is empty. Mirrors
     * the iOS `AudioPlayerProtocol.currentAudioItem`.
     */
    val currentAudioItem: KitharaPlayerItem?
        get() = inner.currentItem()?.let { KitharaPlayerItem(it) }

    /** Last loaded ranges reported by the underlying resource. */
    val loadedRanges: List<ItemLoadedRange>
        get() = state.value.loadedRanges

    /** Last player error, if any. */
    val error: KitharaError?
        get() = state.value.error

    /** Current queue in native order. */
    val items: List<KitharaPlayerItem>
        get() = inner.items().map { KitharaPlayerItem(it) }

    /**
     * Target playback speed used by [play]. While the player is
     * playing, [rate] equals this; on pause [rate] falls to `0`.
     * Mirrors the iOS `AudioPlayerProtocol.playingRate`. The initial
     * value belongs in [Config.playingRate].
     */
    var playingRate: Float
        get() = inner.playingRate()
        set(value) {
            inner.setPlayingRate(value)
        }

    /** Crossfade duration in seconds applied on item transitions. */
    var crossfadeDuration: Float
        get() = inner.crossfadeDuration()
        set(value) {
            inner.setCrossfadeDuration(value)
        }

    /** Volume scalar, usually 0.0 to 1.0. */
    var volume: Float
        get() = inner.volume()
        set(value) {
            inner.setVolume(value)
        }

    /** Mute state. */
    var isMuted: Boolean
        get() = inner.isMuted()
        set(value) {
            inner.setMuted(value)
        }

    /** Starts or resumes playback. */
    fun play() {
        inner.play()
    }

    /** Pauses playback. */
    fun pause() {
        inner.pause()
    }

    /**
     * Stop playback, clear the queue, and reset the current-item slot.
     * Mirrors `AVPlayer.stop` semantics.
     */
    fun stop() {
        inner.stop()
    }

    /**
     * Get EQ gain for a specific band.
     * @param band The band index (0-based).
     */
    fun getEqGain(band: Int): Float {
        return inner.eqGain(band.toUInt())
    }

    /**
     * Set EQ gain for a specific band.
     * @param band The band index (0-based).
     * @param gainDb The gain in decibels.
     */
    @Throws(KitharaError::class)
    fun setEqGain(band: Int, gainDb: Float) {
        try {
            inner.setEqGain(band.toUInt(), gainDb)
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    /**
     * Reset all EQ bands to 0 dB.
     */
    @Throws(KitharaError::class)
    fun resetEq() {
        try {
            inner.resetEq()
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    /**
     * Set the ABR mode.
     */
    fun setAbrMode(mode: com.kithara.ffi.FfiAbrMode) {
        inner.setAbrMode(mode)
    }

    /**
     * Skip to the next item in the queue. No-op if already on the last
     * item or the queue is empty.
     */
    fun advanceToNextItem() {
        inner.advanceToNextItem()
    }

    /**
     * Replace the auth token sent on every player HTTP request. Pass an
     * empty string to clear. The initial value belongs in [Config.authToken].
     */
    fun setupNetwork(authToken: String) {
        inner.setupNetwork(authToken)
    }

    /**
     * Register a wildcard DRM key processor at runtime with a fresh salt.
     * Items already queued keep their rules. Initial rules belong in
     * [Config.keyRules], which is applied through the same path.
     */
    fun setupHlsAes(keyDecryptor: (key: ByteArray, salt: String) -> ByteArray?) {
        inner.setupHlsAes(ClosureKeyProcessorBridge(keyDecryptor))
    }

    /** Append a domain-scoped DRM key rule at runtime; see [setupHlsAes]. */
    fun setupHlsAes(rule: KeyRule) {
        inner.setupHlsAesWithRule(rule.toFfi())
    }

    /**
     * Per-network bitrate ceilings (bits/sec). Pass `0.0` for either
     * argument to lift that limit.
     */
    fun updatePeakBitrate(wifi: Double, cellular: Double) {
        inner.updatePeakBitrate(wifi, cellular)
    }

    /**
     * Seek the current item to the given position in seconds.
     *
     * @param seconds Target playback position in seconds from the item start.
     * @param tolerance Optional landing-tolerance (advisory).
     * @param onComplete Called with `true` if seek succeeded, `false` otherwise.
     */
    fun seek(seconds: Double, tolerance: Double? = null, onComplete: (Boolean) -> Unit) {
        inner.seek(seconds, tolerance, SeekCallbackAdapter(onComplete))
    }

    /**
     * Inserts an item after [after], or at the head of the queue when
     * [after] is null. Use [append] to add to the tail.
     */
    @Throws(KitharaError::class)
    fun insert(item: KitharaPlayerItem, after: KitharaPlayerItem? = null) {
        try {
            inner.insert(item.inner, after?.inner)
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    /** Adds an item to the tail of the queue. */
    @Throws(KitharaError::class)
    fun append(item: KitharaPlayerItem) {
        try {
            inner.append(item.inner)
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    /** Removes an item from the queue. */
    @Throws(KitharaError::class)
    fun remove(item: KitharaPlayerItem) {
        try {
            inner.remove(item.inner)
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    /** Clears the queue. */
    fun removeAllItems() {
        inner.removeAllItems()
    }

    /** Select the item at the given position of [items]. */
    @Throws(KitharaError::class)
    fun selectItem(at: Int, transition: Transition = Transition.None) {
        val item = items.getOrNull(at)
            ?: throw KitharaError.InvalidArgument("item index $at out of range")
        selectItem(item, transition)
    }

    /** Select an item by identity (AVQueuePlayer-style). */
    @Throws(KitharaError::class)
    fun selectItem(item: KitharaPlayerItem, transition: Transition = Transition.None) {
        try {
            inner.select(item.inner, transition.toFfi())
        } catch (error: FfiException) {
            throw KitharaError.fromFfi(error)
        }
    }

    private fun updateState(update: (PlayerState) -> PlayerState) {
        stateFlow.update(update)
    }

    private fun handleEvent(event: FfiPlayerEvent) {
        when (event) {
            is FfiPlayerEvent.TimeChanged ->
                updateState { it.copy(currentTime = event.seconds) }

            is FfiPlayerEvent.RateChanged ->
                updateState { it.copy(rate = event.rate) }

            is FfiPlayerEvent.DurationChanged ->
                updateState { it.copy(duration = event.seconds) }

            is FfiPlayerEvent.BufferedDurationChanged ->
                updateState {
                    it.copy(
                        loadedRanges = listOf(
                            ItemLoadedRange(start = 0.0, duration = event.seconds)
                        )
                    )
                }

            is FfiPlayerEvent.StatusChanged ->
                updateState { it.copy(status = event.status.toPlayerStatus()) }

            is FfiPlayerEvent.Error ->
                updateState { it.copy(error = KitharaError.Internal(event.error)) }

            is FfiPlayerEvent.CurrentItemChanged ->
                eventsFlow.tryEmit(KitharaPlayerEvent.CurrentItemChanged(event.itemId?.toString()))

            is FfiPlayerEvent.TrackStatusChanged ->
                eventsFlow.tryEmit(
                    KitharaPlayerEvent.TrackStatusChanged(
                        event.itemId.toString(),
                        event.status.toTrackStatus(),
                    )
                )

            is FfiPlayerEvent.QueueEnded ->
                eventsFlow.tryEmit(KitharaPlayerEvent.QueueEnded)

            // Per-track failure is surfaced via TrackStatusChanged(Failed) and item-side DidFail.
            is FfiPlayerEvent.ItemDidFail,
            is FfiPlayerEvent.TimeControlStatusChanged,
            is FfiPlayerEvent.VolumeChanged,
            is FfiPlayerEvent.MuteChanged,
            is FfiPlayerEvent.ItemDidPlayToEnd,
            is FfiPlayerEvent.CrossfadeStarted,
            is FfiPlayerEvent.CrossfadeDurationChanged,
            is FfiPlayerEvent.TrackAdded,
            is FfiPlayerEvent.TrackRemoved,
            is FfiPlayerEvent.TrackLoadFailed,
            is FfiPlayerEvent.RepeatModeChanged,
            is FfiPlayerEvent.NextTrackReady,
            is FfiPlayerEvent.CurrentItemAdvanced,
            is FfiPlayerEvent.EngineStarted,
            is FfiPlayerEvent.EngineStopped,
            is FfiPlayerEvent.CrossfadeCompleted,
            is FfiPlayerEvent.CrossfadeCancelled,
            is FfiPlayerEvent.MasterVolumeChanged,
            is FfiPlayerEvent.AudioRouteChanged,
            is FfiPlayerEvent.DjBpmDetected,
            is FfiPlayerEvent.DjKeylockChanged,
            is FfiPlayerEvent.DjStretchBackendChanged,
            is FfiPlayerEvent.AssetCommitted,
            is FfiPlayerEvent.AssetFailed,
            is FfiPlayerEvent.AssetEvicted -> Unit
        }
    }

    private class PlayerObserverBridge(
        private val player: KitharaPlayer,
    ) : PlayerObserver {
        override fun onEvent(event: FfiPlayerEvent) {
            player.handleEvent(event)
        }
    }

    private class SeekCallbackAdapter(
        private val callback: (Boolean) -> Unit,
    ) : SeekCallback {
        override fun onComplete(finished: Boolean) {
            callback(finished)
        }
    }
}

private fun FfiPlayerStatus.toPlayerStatus(): PlayerStatus = when (this) {
    FfiPlayerStatus.READY_TO_PLAY -> PlayerStatus.ReadyToPlay
    FfiPlayerStatus.FAILED -> PlayerStatus.Failed
    FfiPlayerStatus.UNKNOWN -> PlayerStatus.Unknown
}

private fun FfiTrackStatus.toTrackStatus(): TrackStatus = when (this) {
    is FfiTrackStatus.Pending -> TrackStatus.Pending
    is FfiTrackStatus.Loading -> TrackStatus.Loading
    is FfiTrackStatus.Slow -> TrackStatus.Slow
    is FfiTrackStatus.Loaded -> TrackStatus.Loaded
    is FfiTrackStatus.Failed -> TrackStatus.Failed(reason)
    is FfiTrackStatus.Consumed -> TrackStatus.Consumed
    is FfiTrackStatus.Cancelled -> TrackStatus.Cancelled
}

private fun Transition.toFfi(): FfiTransition = when (this) {
    is Transition.None -> FfiTransition.None
    is Transition.Crossfade -> FfiTransition.Crossfade
    is Transition.CrossfadeWith -> FfiTransition.CrossfadeWith(seconds)
}

/**
 * Callback for processing (decrypting) HLS encryption keys.
 *
 * `salt` is the player-generated value attached to outgoing requests
 * under `X-Encrypted-Key`. Implementations that hold a pre-built
 * cipher can ignore the argument; implementations that derive the
 * cipher per-session should rebuild it from `salt` on every call.
 */
interface KeyProcessor {
    fun processKey(key: ByteArray, salt: String): ByteArray
}

private class ClosureKeyProcessorBridge(
    private val decrypt: (ByteArray, String) -> ByteArray?,
) : FfiKeyProcessor {
    override fun processKey(key: ByteArray, salt: String): ByteArray = decrypt(key, salt) ?: key
}

private class KeyProcessorBridge(private val processor: KeyProcessor) : FfiKeyProcessor {
    override fun processKey(key: ByteArray, salt: String): ByteArray =
        processor.processKey(key, salt)
}

private fun KitharaPlayer.KeyRule.toFfi(): FfiKeyRule =
    FfiKeyRule(
        processor = KeyProcessorBridge(processor),
        headers = headers,
        queryParams = queryParams,
        domains = domains,
        salt = salt,
    )

internal fun KitharaPlayer.Config.toFfi(): FfiPlayerConfig {
    return FfiPlayerConfig(
        keyOptions = FfiKeyOptions(rules = keyRules.map { it.toFfi() }),
        store = store.inner,
        eqBandCount = eqBandCount.toUInt(),
        authToken = authToken,
        crossfadeDuration = crossfadeDuration,
        playingRate = playingRate,
    )
}

