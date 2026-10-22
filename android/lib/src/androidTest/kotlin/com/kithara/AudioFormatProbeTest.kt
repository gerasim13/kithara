package com.kithara

import android.content.Context
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.kithara.ffi.FfiException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Probes cpal's default host / output device on Android and asserts the
 * default sample format matches what the firewheel graph produces.
 *
 * The graph output is interleaved stereo f32. If cpal negotiates a
 * different format (e.g. I16) with the Android backend, every frame is
 * converted inside cpal before it reaches AAudio — a likely source of
 * the on-device audio distortion we see in the app but not in
 * [OfflineCaptureTest]'s offline WAV.
 *
 * Read full supported-config list via `adb logcat -v brief | grep cpal`.
 */
@RunWith(AndroidJUnit4::class)
class AudioFormatProbeTest {

    companion object {
        private const val TAG = "AudioFormatProbeTest"

        @JvmStatic
        @BeforeClass
        fun setUpClass() {
            val context = ApplicationProvider.getApplicationContext<Context>()
            Kithara.initialize(context, TestTransport.okHttp, logLevel = LogLevel.Debug)
        }
    }

    @Test
    fun defaultOutputFormatIsF32() {
        val code = Kithara.Test.probeAndroidAudio()
        val name = Kithara.Test.SampleFormat.name(code)
        Log.i(TAG, "cpal default output sample format: $name ($code)")

        assertEquals(
            "cpal default output must be F32 on Android to match the firewheel graph output. " +
                "A different format ($name) means cpal converts every sample before sending to " +
                "AAudio — inspect `cpal supported output config` logs for alternatives.",
            Kithara.Test.SampleFormat.F32,
            code,
        )
    }

    @Test
    fun closedPlayerReleasesItsHostSlotAtCapacity() {
        val players = mutableListOf<KitharaPlayer>()
        try {
            repeat(8) { players += KitharaPlayer() }
            val exhausted = assertThrows(FfiException.Internal::class.java) { KitharaPlayer() }
            assertEquals("all scope slots are in use", exhausted.description)

            val released = players.first()
            released.close()
            released.close()
            val replacement = KitharaPlayer()
            players += replacement

            assertTrue(replacement.items.isEmpty())
            assertEquals(1.0f, players[1].playingRate, 0.0f)
            assertThrows(IllegalStateException::class.java) { released.items }
        } finally {
            players.forEach(KitharaPlayer::close)
        }
    }

    @Test
    fun closedPlayersDoNotConsumeHostSlots() {
        val closedPlayers = mutableListOf<KitharaPlayer>()
        try {
            repeat(16) {
                val player = KitharaPlayer()
                closedPlayers += player
                player.use { assertTrue(it.items.isEmpty()) }
            }

            assertEquals(16, closedPlayers.size)
            closedPlayers.forEach { player ->
                assertThrows(IllegalStateException::class.java) { player.items }
            }
        } finally {
            closedPlayers.forEach(KitharaPlayer::close)
        }
    }
}
