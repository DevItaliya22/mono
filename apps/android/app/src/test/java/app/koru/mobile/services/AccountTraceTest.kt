// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.logging.AppLogger
import org.junit.Assert.*
import org.junit.Test

class AccountTraceTest {
    @Test fun disabledTraceDoesNotWriteAndCanBeEnabledWithoutReconstruction() {
        val lines = mutableListOf<String>()
        var enabled = false
        val trace = AccountTrace({ enabled }, AppLogger(sink = { _, message -> lines += message }))
        trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Started, 7)
        assertTrue(lines.isEmpty())
        enabled = true
        trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Started, 7)
        assertEquals(1, lines.size)
        assertTrue(lines.single().contains("generation=7"))
    }

    @Test fun exceptionMessagesAndCausesNeverReachTheTrace() {
        val lines = mutableListOf<String>()
        val trace = AccountTrace({ true }, AppLogger(sink = { _, message -> lines += message }))
        val error = IllegalStateException("token_sentinel person@example.com",
            IllegalArgumentException("nonce_sentinel"))
        trace.emit(AccountTrace.Stage.GoogleDialog, AccountTrace.Outcome.Failed,
            reason = AccountTrace.reason(error))
        assertTrue(lines.single().contains("OtherException"))
        assertFalse(lines.single().contains("sentinel"))
        assertFalse(lines.single().contains("example.com"))
        assertFalse(lines.single().contains("IllegalStateException"))
    }
}
