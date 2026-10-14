// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class DocumentInvalidationsTest {
    @Test fun registrationAndConcurrentWakesAreConflatedAndCancellationReleases() = runTest {
        lateinit var callback: Runnable
        var reads = 0
        val released = mutableListOf<Long>()
        val job = launch {
            documentInvalidations(
                register = { callback = it; it.run(); 42L },
                release = { released += it },
            ).collect { reads++ }
        }
        runCurrent()
        assertEquals(1, reads)
        repeat(100) { callback.run() }
        runCurrent()
        assertEquals(2, reads)
        job.cancelAndJoin()
        assertEquals(listOf(42L), released)
        callback.run() // An in-flight JNI callback after close is harmless.
        runCurrent()
        assertEquals(2, reads)
    }

    @Test fun cancellationDuringRegistrationStillReleasesTheReturnedOwner() = runTest {
        val released = mutableListOf<Long>()
        lateinit var collection: Job
        collection = launch {
            documentInvalidations(
                register = { collection.cancel(); 7L },
                release = { released += it },
            ).collect { fail("canceled collector must not read") }
        }
        runCurrent()
        collection.join()
        assertEquals(listOf(7L), released)
    }
}
