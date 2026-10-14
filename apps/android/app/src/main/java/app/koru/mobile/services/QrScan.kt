// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withTimeout

internal interface QrScanPort {
    suspend fun prepare()
    suspend fun scan(): String?
}

/** A request acknowledgement is not readiness. The port must wait for the
 * scanner module to finish installing before returning from prepare. */
internal suspend fun scanQrCode(port: QrScanPort, onPrepared: () -> Unit): String? {
    withTimeout(60_000) { port.prepare() }
    currentCoroutineContext().ensureActive()
    onPrepared()
    return port.scan()
}
