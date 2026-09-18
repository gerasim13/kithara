package com.kithara.example.drm

import android.content.Context
import android.content.res.AssetManager
import com.kithara.KeyProcessor
import com.kithara.ffi.FfiCipher
import java.io.FileNotFoundException

/**
 * Read the cipher key for zvuk DRM from the bundled `.env` asset.
 *
 * The salt is supplied per-call by the configured key rule (see
 * `KitharaPlayer.KeyRule.wildcard`) — the demo no longer pre-generates a
 * seed; [ZvukKeyProcessor] builds the cipher on every decrypt from
 * `cipherKey + salt`.
 */
internal fun readZvukCipherKey(context: Context): String =
    context.assets.readEnvValue(key = "DRM_KEY") ?: "BinaryCipherKey"

internal fun readZvukAuthToken(context: Context): String? =
    context.assets.readEnvValue(key = "KITHARA_DRM_AUTH_TOKEN")

/**
 * Decrypt `encryptedKey` with a one-shot kithara cipher built from
 * `secret`. Used by [ZvukKeyProcessor].
 */
internal fun kitharaCipherDecrypt(secret: String, encryptedKey: ByteArray): ByteArray {
    val cipher = FfiCipher(secret)
    return cipher.decrypt(encryptedKey)
}

private fun AssetManager.readEnvValue(key: String): String? =
    ENV_ASSET_NAMES.firstNotNullOfOrNull { name -> readAsset(name)?.envValue(key) }

private fun AssetManager.readAsset(name: String): String? =
    try {
        open(name).bufferedReader().use { it.readText() }
    } catch (_: FileNotFoundException) {
        null
    }

private fun String.envValue(key: String): String? =
    lineSequence()
        .map(String::trim)
        .firstOrNull { line -> line.startsWith("$key=") }
        ?.substringAfter('=')
        ?.trim()

// Asset staging may strip the leading dot.
private val ENV_ASSET_NAMES = listOf(".env", "env")

/**
 * Wildcard `"*"` HLS-AES key processor for zvuk. Derives the cipher
 * per call from the player-supplied salt, so a session that rotates
 * the salt needs no re-registration.
 */
internal class ZvukKeyProcessor(private val cipherKey: String) : KeyProcessor {
    override fun processKey(key: ByteArray, salt: String): ByteArray =
        kitharaCipherDecrypt(cipherKey + salt, key)
}
