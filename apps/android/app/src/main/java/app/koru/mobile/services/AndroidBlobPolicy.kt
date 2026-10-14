// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import app.koru.mobile.logging.AppLogger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Where the person's cellular allowance is remembered. Device-local, never
 *  synced — the same place and the same reasoning as the relay switch. */
private const val ALLOW_ON_METERED_KEY = "blob_allow_on_metered"

/**
 * This device's connection, told to the shared engine (D50).
 *
 * **A callback, not a poll.** `ConnectivityManager` reports a change when
 * there is one, which is what §10's fifth trigger wants and is also the only
 * shape D244 permits: a phone holds no timer of its own, so nothing here runs
 * on an interval and nothing here wakes the process.
 *
 * The engine is told the facts, never the decision. Whether a metered
 * connection pauses transfers, whether an explicit request goes through it and
 * what a paused Workspace reports are all the engine's, so the desktop and the
 * phone cannot come to two different answers.
 */
class AndroidBlobPolicy(
    context: Context,
    private val native: NativeDeviceLinkService,
    private val logger: AppLogger,
) {
    private val connectivity =
        context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val preferences = context.getSharedPreferences("device", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val mutableAllowOnMetered =
        MutableStateFlow(preferences.getBoolean(ALLOW_ON_METERED_KEY, false))

    /** Whether this person has allowed picture transfers on a paid connection.
     *  Off by default: a phone that quietly spent somebody's data allowance on
     *  a holiday is the outcome D50 exists to prevent. */
    val allowOnMetered: StateFlow<Boolean> = mutableAllowOnMetered.asStateFlow()

    private val mutableConnected = MutableStateFlow(false)
    private val mutableMetered = MutableStateFlow(false)

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
            mutableConnected.value =
                capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            // `NOT_METERED` is the capability Android actually reports; the
            // absence of it is the metered case, including a metered Wi-Fi the
            // person marked themselves.
            mutableMetered.value =
                !capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
            publish()
        }

        override fun onLost(network: Network) {
            mutableConnected.value = false
            publish()
        }
    }

    init {
        runCatching {
            connectivity.registerNetworkCallback(
                NetworkRequest.Builder()
                    .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
                    .build(),
                callback,
            )
        }.onFailure {
            // Unconditional: a device whose connection nobody is watching will
            // hold every picture on a pause that never clears, and an empty
            // frame is not where somebody should discover that.
            logger.event("blob_policy_unwatched", mapOf("message" to (it.message ?: "?")))
        }
        // The first reading, before any callback: an app that started on a
        // metered connection must not spend an allowance while it waits to be
        // told about one.
        readNow()
        publish()
    }

    private fun readNow() {
        val capabilities = runCatching {
            connectivity.getNetworkCapabilities(connectivity.activeNetwork)
        }.getOrNull()
        mutableConnected.value =
            capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        mutableMetered.value =
            capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true
    }

    /** The person's own answer to "use cellular for pictures". */
    suspend fun setAllowOnMetered(allowed: Boolean) {
        preferences.edit().putBoolean(ALLOW_ON_METERED_KEY, allowed).apply()
        mutableAllowOnMetered.value = allowed
        send()
    }

    private fun publish() {
        scope.launch { send() }
    }

    private suspend fun send() {
        val policy = blobPolicyOf(
            connected = mutableConnected.value,
            metered = mutableMetered.value,
            dataSaver = dataSaverOn(),
            allowOnMetered = mutableAllowOnMetered.value,
        )
        runCatching { native.setBlobPolicy(policy) }
            .onSuccess {
                // News rather than activity: a connection change is rare and a
                // person asking "why is nothing downloading" needs exactly it.
                logger.event(
                    "blob_policy",
                    mapOf(
                        "offline" to policy.offline.toString(),
                        "metered" to policy.metered.toString(),
                        "low_data" to policy.lowData.toString(),
                        "allow_on_metered" to policy.allowOnMetered.toString(),
                    ),
                )
            }
            .onFailure {
                logger.event("blob_policy_error", mapOf("message" to (it.message ?: "?")))
            }
    }

    /** Android's data saver, as the person set it for this app. */
    private fun dataSaverOn(): Boolean = runCatching {
        connectivity.restrictBackgroundStatus ==
            ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED
    }.getOrDefault(false)
}
