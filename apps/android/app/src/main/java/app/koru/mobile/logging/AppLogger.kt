// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.logging

import android.util.Log

class AppLogger(
    private val tag: String = "KoruMobile",
    private val sink: (String, String) -> Unit = { outputTag, message ->
        Log.d(outputTag, message)
        Unit
    },
) {
    companion object {
        @Volatile var peerObjectsEnabled: Boolean = false
        @Volatile var accountEnabled: Boolean = false
    }

    fun event(name: String, fields: Map<String, Any?> = emptyMap()) {
        val safe = fields.entries.joinToString(" ") { (key, value) -> "$key=$value" }
        sink(tag, if (safe.isEmpty()) name else "$name $safe")
    }

    fun peerObject(name: String, fields: Map<String, Any?> = emptyMap()) {
        if (peerObjectsEnabled) event(name, fields)
    }

    suspend fun <T> timed(name: String, fields: Map<String, Any?> = emptyMap(), block: suspend () -> T): T {
        val started = System.nanoTime()
        return try {
            block()
        } finally {
            event(name, fields + ("duration_ms" to ((System.nanoTime() - started) / 1_000_000.0)))
        }
    }
}
