// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import app.koru.mobile.domain.ProfileId
import app.koru.mobile.domain.WorkspaceId
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private val PROFILE = ProfileId("22222222-2222-4222-8222-222222222222")
private val OTHER = ProfileId("33333333-3333-4333-8333-333333333333")

/**
 * The rules of the desktop's Manage Profiles Cloud sync section
 * (`cloud_progress.rs` and its tests), held on the phone's side of the same
 * data: what a row says, what is held, which rows open, how far an open row
 * lists, and what the poll keeps and drops.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class CloudProgressPresentationTest {
    private fun ws(n: Int) = WorkspaceId("00000000-0000-4000-8000-${n.toString().padStart(12, '0')}")

    private val on = CloudPermitFacts(enabled = true, stage = "settled")

    private fun files(
        referenced: Int = 0,
        durable: Int = 0,
        uploading: Int = 0,
        missing: Int = 0,
        pending: String = "",
        direction: String = "",
        path: String = "",
        paused: Boolean = false,
        coverage: CloudCoverageFacts? = null,
    ) = CloudFilesFacts(
        referenced = referenced, present = referenced, durable = durable, uploading = uploading,
        missing = missing, pendingReason = pending, direction = direction, path = path,
        paused = paused, coverage = coverage,
    )

    private fun doc(title: String, files: Int = 1, vararg holders: String) =
        CloudWaitingEntry("id-$title", title, files, holders.toList())

    // ------------------------------------------------------------ one row

    /** *48 of 88 documents · 33 of 72 files · sending · via Koru Cloud*. */
    @Test fun aMovingWorkspaceSaysDocumentsFilesDirectionAndPath() {
        val status = CloudWorkspaceStatus(
            sync = on,
            files = files(
                referenced = 72, durable = 33, uploading = 3, direction = "upload", path = "cloud",
                coverage = CloudCoverageFacts("incomplete", documentsRequired = 88, documentsMissing = 40),
            ),
        )
        assertEquals(
            listOf(
                CloudDetailPart.Documents(48, 88),
                CloudDetailPart.Files(33, 72),
                CloudDetailPart.Moving(sending = true, path = CloudPath.Cloud),
            ),
            cloudRowDetail(status),
        )
        // Straight from a peer is its own path; a download is receiving.
        assertEquals(
            CloudDetailPart.Moving(sending = false, path = CloudPath.Peer),
            cloudRowDetail(status.copy(files = status.files!!.copy(direction = "download", path = "peer_direct"))).last(),
        )
    }

    /** A superseded claim claims nothing, and no claim says nothing about
     *  documents — never zero. */
    @Test fun documentsAreReadOnlyFromALiveClaim() {
        val superseded = CloudWorkspaceStatus(
            sync = on,
            files = files(coverage = CloudCoverageFacts("superseded", 10, 0)),
        )
        assertTrue(cloudRowDetail(superseded).none { it is CloudDetailPart.Documents })
        val unclaimed = CloudWorkspaceStatus(sync = on, files = files())
        assertTrue(cloudRowDetail(unclaimed).none { it is CloudDetailPart.Documents })
        // Missing more than required never reads as negative.
        val odd = CloudWorkspaceStatus(sync = on, files = files(coverage = CloudCoverageFacts("incomplete", 3, 5)))
        assertEquals(CloudDetailPart.Documents(0, 3), cloudRowDetail(odd).first())
    }

    /** A settled Workspace adds no state, and one with nothing measured says
     *  the Sync line's own word. */
    @Test fun aSettledWorkspaceAddsNoStateAndAnEmptyOneSaysOn() {
        val settled = CloudWorkspaceStatus(
            sync = on,
            files = files(
                referenced = 5, durable = 5, direction = "upload", path = "cloud",
                coverage = CloudCoverageFacts("complete", 4, 0),
            ),
        )
        assertEquals(
            listOf(CloudDetailPart.Documents(4, 4), CloudDetailPart.Files(5, 5)),
            cloudRowDetail(settled),
        )
        assertEquals(listOf(CloudDetailPart.SyncOn), cloudRowDetail(CloudWorkspaceStatus(sync = on, files = files())))
        assertEquals(listOf(CloudDetailPart.Unavailable), cloudRowDetail(null))
    }

    /** Held, in order: the person's pause, the transfer's code, the permit's
     *  error. A Workspace that is simply not on Cloud is *off*, not held. */
    @Test fun holdsOutrankEachOtherAndOffIsNotAHold() {
        val paused = CloudWorkspaceStatus(sync = on, files = files(paused = true, pending = "network_metered"))
        assertEquals(CloudHold.PausedByYou, holdOf(paused))
        val waiting = CloudWorkspaceStatus(sync = on.copy(errorCode = "cloud_unit_unavailable"), files = files(pending = "object_missing"))
        assertEquals(CloudHold.Code("object_missing"), holdOf(waiting))
        val refused = CloudWorkspaceStatus(sync = on.copy(errorCode = "cloud_unit_unavailable"), files = files())
        assertEquals(CloudHold.Code("cloud_unit_unavailable"), holdOf(refused))
        assertEquals(CloudDetailPart.Held(CloudHold.Code("cloud_unit_unavailable")), cloudRowDetail(refused).last())

        val off = CloudWorkspaceStatus(sync = null, files = files(referenced = 2, durable = 0))
        assertNull(holdOf(off))
        assertEquals(listOf(CloudDetailPart.Files(0, 2), CloudDetailPart.SyncOff), cloudRowDetail(off))
    }

    @Test fun onlyAHeldRowOrOneWithFilesParkedOpens() {
        assertTrue(cloudRowOpenable(CloudWorkspaceStatus(sync = on, files = files(pending = "object_missing"))))
        assertTrue(cloudRowOpenable(CloudWorkspaceStatus(sync = on, files = files(missing = 2))))
        assertTrue(cloudRowOpenable(CloudWorkspaceStatus(sync = on, files = files(paused = true))))
        assertFalse(cloudRowOpenable(CloudWorkspaceStatus(sync = on, files = files(referenced = 3, durable = 3))))
        assertFalse(cloudRowOpenable(CloudWorkspaceStatus(sync = null, files = files())))
        assertFalse(cloudRowOpenable(null))
    }

    // -------------------------------------------------------- the summary

    /** *13 of 13 workspaces on Koru Cloud · 369 of 583 files · 1 held*, summed
     *  from the rows' own counts; a failed row counts as a Workspace and adds
     *  nothing else. */
    @Test fun theSummaryIsTheRowsAddedUp() {
        val rows = listOf(
            CloudWorkspaceProgress(ws(1), "A", CloudWorkspaceStatus(on, files(referenced = 500, durable = 300))),
            CloudWorkspaceProgress(ws(2), "B", CloudWorkspaceStatus(on, files(referenced = 83, durable = 69, pending = "object_missing"))),
            CloudWorkspaceProgress(ws(3), "C", CloudWorkspaceStatus(null, files())),
            CloudWorkspaceProgress(ws(4), "D", null),
        )
        assertEquals(CloudSummary(onCloud = 2, workspaces = 4, durable = 369, referenced = 583, held = 1), cloudSummary(rows))
        assertEquals(CloudSummary(0, 0, 0, 0, 0), cloudSummary(emptyList()))
    }

    // ------------------------------------------------------ an open row

    @Test fun anOpenRowListsTwentyLinesAtMostTheLastOfThemAndNMore() {
        val many = CloudWaiting.Documents((1..25).map { doc("Doc $it") })
        val lines = cloudWaitingLines(many)
        assertEquals(CLOUD_WAITING_CAP, lines.size)
        assertEquals(CloudWaitingLine.Document(doc("Doc 1")), lines.first())
        assertEquals(CloudWaitingLine.Document(doc("Doc 19")), lines[18])
        assertEquals(CloudWaitingLine.More(6), lines.last())

        val exactly = CloudWaiting.Documents((1..20).map { doc("Doc $it") })
        assertTrue(cloudWaitingLines(exactly).all { it is CloudWaitingLine.Document })
        assertEquals(20, cloudWaitingLines(exactly).size)

        assertEquals(listOf(CloudWaitingLine.None), cloudWaitingLines(CloudWaiting.Documents(emptyList())))
        assertEquals(listOf(CloudWaitingLine.Reading), cloudWaitingLines(CloudWaiting.Reading))
        assertEquals(listOf(CloudWaitingLine.Unavailable), cloudWaitingLines(CloudWaiting.Unavailable))
    }

    // ------------------------------------------------------------ the poll

    private class FakeSource : CloudProgressSource {
        val workspaces = mutableMapOf<ProfileId, List<Pair<WorkspaceId, String>>>()
        val statuses = mutableMapOf<WorkspaceId, CloudWorkspaceStatus?>()
        val waiting = mutableMapOf<WorkspaceId, List<CloudWaitingEntry>?>()
        var gate: CompletableDeferred<Unit>? = null
        var reads = 0
        val waitingAsked = mutableListOf<WorkspaceId>()

        override suspend fun workspaces(profile: ProfileId): List<Pair<WorkspaceId, String>>? {
            reads++
            gate?.await()
            return workspaces[profile]
        }
        override suspend fun status(workspace: WorkspaceId) = statuses[workspace]
        override suspend fun waiting(workspace: WorkspaceId): List<CloudWaitingEntry>? {
            waitingAsked += workspace
            return waiting[workspace]
        }
    }

    private val held = CloudWorkspaceStatus(on, files(referenced = 6, durable = 0, missing = 6, pending = "object_missing"))
    private val done = CloudWorkspaceStatus(on, files(referenced = 2, durable = 2))

    @Test fun aProfileWithoutCloudSyncIsOffAndNothingIsRead() = runTest {
        val source = FakeSource()
        val presenter = CloudProgressPresenter(source, StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = false)
        presenter.refresh()
        assertEquals(CloudProgressScreenState.Off(unattached = false), presenter.state.value)
        presenter.show(PROFILE, enabled = false, unattached = true)
        assertEquals(CloudProgressScreenState.Off(unattached = true), presenter.state.value)
        assertEquals(0, source.reads)
    }

    @Test fun aReadDrawsTheSummaryAndTheRows() = runTest {
        val source = FakeSource().apply {
            workspaces[PROFILE] = listOf(ws(1) to "Photos", ws(2) to "Notes", ws(3) to "Broken")
            statuses[ws(1)] = held
            statuses[ws(2)] = done
            statuses[ws(3)] = null
        }
        val presenter = CloudProgressPresenter(source, StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = true)
        assertEquals(CloudProgressScreenState.Loading, presenter.state.value)
        presenter.refresh()
        val ready = presenter.state.value as CloudProgressScreenState.Ready
        assertEquals(CloudSummary(2, 3, 2, 8, 1), ready.summary)
        assertEquals(listOf("Photos", "Notes", "Broken"), ready.rows.map { it.name })
        assertEquals(listOf(true, false, false), ready.rows.map { it.openable })
        assertEquals(listOf(CloudDetailPart.Unavailable), ready.rows[2].detail)
        assertTrue(ready.rows.all { it.waiting == null })
        // Nobody opened a row, so nobody was asked who is waiting.
        assertTrue(source.waitingAsked.isEmpty())
    }

    @Test fun aHostThatWillNotListSaysUnavailable() = runTest {
        val presenter = CloudProgressPresenter(FakeSource(), StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = true)
        presenter.refresh()
        assertEquals(CloudProgressScreenState.Unavailable, presenter.state.value)
    }

    /** Only a held row opens; it reads at once, lists in the same pass as
     *  its counts, and closing it forgets the list. */
    @Test fun aHeldRowOpensOntoItsWaitingDocuments() = runTest {
        val source = FakeSource().apply {
            workspaces[PROFILE] = listOf(ws(1) to "Photos", ws(2) to "Notes")
            statuses[ws(1)] = held
            statuses[ws(2)] = done
            waiting[ws(1)] = listOf(doc("Trip notes", 6, "Pixel 8"))
        }
        val presenter = CloudProgressPresenter(source, StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = true)
        presenter.refresh()

        presenter.toggle(ws(2))
        assertTrue((presenter.state.value as CloudProgressScreenState.Ready).rows.all { it.waiting == null })

        presenter.toggle(ws(1))
        var rows = (presenter.state.value as CloudProgressScreenState.Ready).rows
        assertEquals(listOf(CloudWaitingLine.Reading), rows[0].waiting)
        presenter.refresh()
        rows = (presenter.state.value as CloudProgressScreenState.Ready).rows
        assertEquals(listOf(CloudWaitingLine.Document(doc("Trip notes", 6, "Pixel 8"))), rows[0].waiting)
        assertEquals(listOf(ws(1)), source.waitingAsked)

        // A list the host would not read is said as unavailable, not as empty.
        source.waiting[ws(1)] = null
        presenter.refresh()
        rows = (presenter.state.value as CloudProgressScreenState.Ready).rows
        assertEquals(listOf(CloudWaitingLine.Unavailable), rows[0].waiting)

        presenter.toggle(ws(1))
        rows = (presenter.state.value as CloudProgressScreenState.Ready).rows
        assertNull(rows[0].waiting)

        // A row that stops being able to open closes by itself.
        presenter.toggle(ws(1))
        source.statuses[ws(1)] = done
        presenter.refresh()
        rows = (presenter.state.value as CloudProgressScreenState.Ready).rows
        assertFalse(rows[0].openable)
        assertNull(rows[0].waiting)
    }

    /** **An answer about a Profile the person has left is dropped**, not
     *  drawn under the new one's name. */
    @Test fun anAnswerForAProfileThePersonLeftIsDropped() = runTest {
        val source = FakeSource().apply {
            workspaces[PROFILE] = listOf(ws(1) to "Old")
            workspaces[OTHER] = listOf(ws(2) to "New")
            statuses[ws(1)] = done
            statuses[ws(2)] = done
            gate = CompletableDeferred()
        }
        val presenter = CloudProgressPresenter(source, StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = true)
        val first = launch { presenter.refresh() }
        runCurrent()
        presenter.show(OTHER, enabled = true)
        source.gate!!.complete(Unit)
        first.join()
        assertEquals(CloudProgressScreenState.Loading, presenter.state.value)

        presenter.refresh()
        val ready = presenter.state.value as CloudProgressScreenState.Ready
        assertEquals(listOf("New"), ready.rows.map { it.name })
    }

    /** Every five seconds while polled, at once when a row is opened, and not
     *  at all once the poll is cancelled — which is what leaving the screen
     *  or backgrounding the app does. */
    @Test fun thePollAsksEveryFiveSecondsAndStopsWhenCancelled() = runTest {
        val source = FakeSource().apply {
            workspaces[PROFILE] = listOf(ws(1) to "Photos")
            statuses[ws(1)] = held
        }
        val presenter = CloudProgressPresenter(source, StandardTestDispatcher(testScheduler))
        presenter.show(PROFILE, enabled = true)
        val poll = launch { presenter.poll() }
        runCurrent()
        assertEquals(1, source.reads)
        advanceTimeBy(4_999)
        assertEquals(1, source.reads)
        advanceTimeBy(2)
        assertEquals(2, source.reads)

        presenter.toggle(ws(1))
        runCurrent()
        assertEquals("opening asks at once", 3, source.reads)

        poll.cancel()
        advanceTimeBy(60_000)
        assertEquals(3, source.reads)
    }
}
