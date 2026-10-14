// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class QrScanTest {
    private class Scanner : QrScanPort {
        val installed = CompletableDeferred<Unit>()
        var scans = 0
        override suspend fun prepare() { installed.await() }
        override suspend fun scan(): String { scans++; return "fixture-link" }
    }

    @Test fun waitsForInstallationBeforeOpeningTheCamera() = runTest {
        val port = Scanner()
        var prepared = false
        var result: String? = null
        launch { result = scanQrCode(port) { prepared = true } }
        runCurrent()
        assertEquals(0, port.scans)
        assertFalse(prepared)
        port.installed.complete(Unit)
        runCurrent()
        assertTrue(prepared)
        assertEquals(1, port.scans)
        assertEquals("fixture-link", result)
    }

    @Test fun leavingDuringPreparationCannotOpenTheScannerLater() = runTest {
        val port = Scanner()
        val operation = launch { scanQrCode(port) { fail("Retired request became ready") } }
        runCurrent()
        operation.cancel()
        port.installed.complete(Unit)
        runCurrent()
        assertEquals(0, port.scans)
    }

    @Test fun failedPreparationDoesNotTryTheCamera() = runTest {
        val port = Scanner()
        val failure = IllegalStateException("fixture")
        port.installed.completeExceptionally(failure)
        val result = runCatching { scanQrCode(port) { fail("Failed request became ready") } }
        // Coroutine stack-trace recovery can copy an exception across await.
        assertTrue(result.exceptionOrNull() is IllegalStateException)
        assertEquals("fixture", result.exceptionOrNull()?.message)
        assertEquals(0, port.scans)
    }

    @Test fun lostForegroundAtReadinessDoesNotOpenTheCamera() = runTest {
        val port = Scanner()
        port.installed.complete(Unit)
        val result = runCatching { scanQrCode(port) { throw CancellationException() } }
        assertTrue(result.exceptionOrNull() is CancellationException)
        assertEquals(0, port.scans)
    }

    @Test fun stalledInstallationTimesOutAndLateCompletionCannotOpenTheCamera() = runTest {
        val port = Scanner()
        var error: Throwable? = null
        launch { error = runCatching { scanQrCode(port) {} }.exceptionOrNull() }
        advanceUntilIdle()
        assertTrue(error is TimeoutCancellationException)
        port.installed.complete(Unit)
        runCurrent()
        assertEquals(0, port.scans)
    }
}
