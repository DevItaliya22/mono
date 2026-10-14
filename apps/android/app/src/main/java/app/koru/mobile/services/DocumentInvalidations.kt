// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow

/** Native notification ownership follows collection, not the screen's cached
 * snapshot. Subscribe before reading, conflate wakes while a read is running,
 * and release even when collection is canceled during native registration. */
internal fun documentInvalidations(
    register: (Runnable) -> Long,
    release: (Long) -> Unit,
): Flow<Unit> = callbackFlow {
    val observer = register(Runnable { trySend(Unit) })
    trySend(Unit)
    awaitClose { release(observer) }
}.buffer(Channel.CONFLATED)
