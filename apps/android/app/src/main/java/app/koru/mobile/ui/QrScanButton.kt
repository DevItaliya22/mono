// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import android.util.Log
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.koru.mobile.R
import app.koru.mobile.services.AndroidQrScanner
import app.koru.mobile.services.scanQrCode
import app.koru.mobile.services.scannerFailureCode
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch

/** Own the complete scanner attempt with this UI, so leaving the link tab
 * retires preparation and cannot open the scanner later from a stale callback. */
@Composable
internal fun QrScanButton(enabled: Boolean, onScanned: (String) -> Unit) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val scanner = remember(context) { AndroidQrScanner(context) }
    val scope = rememberCoroutineScope()
    val deliver by rememberUpdatedState(onScanned)
    var job by remember { mutableStateOf<Job?>(null) }
    var busy by remember { mutableStateOf(false) }
    var preparing by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    LaunchedEffect(enabled) { if (!enabled) job?.cancel() }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            // Launching the scanner itself stops this Activity too. Only retire
            // a pending preparation, not a scanner that is already on screen.
            if (event == Lifecycle.Event.ON_STOP && preparing) job?.cancel()
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    BackHandler(enabled = busy && preparing) { job?.cancel() }

    Column {
        Button(enabled = enabled && !busy, onClick = {
            busy = true
            preparing = true
            failed = false
            job = scope.launch {
                var stage = "prepare"
                Log.d("KoruMobile", "QR scanner stage=prepare outcome=started")
                try {
                    val code = scanQrCode(scanner) {
                        if (!lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) {
                            throw CancellationException()
                        }
                        preparing = false
                        stage = "scan"
                        Log.d("KoruMobile", "QR scanner stage=scan outcome=started")
                    }
                    if (code.isNullOrBlank()) {
                        failed = true
                        Log.w("KoruMobile", "QR scanner stage=scan outcome=empty")
                    } else {
                        Log.d("KoruMobile", "QR scanner stage=scan outcome=completed")
                        deliver(code)
                    }
                } catch (_: TimeoutCancellationException) {
                    failed = true
                    Log.w("KoruMobile", "QR scanner stage=$stage outcome=timeout")
                } catch (cancelled: CancellationException) {
                    Log.d("KoruMobile", "QR scanner stage=$stage outcome=cancelled")
                    throw cancelled
                } catch (error: Exception) {
                    failed = true
                    Log.w("KoruMobile", "QR scanner stage=$stage outcome=failed code=${scannerFailureCode(error)}")
                } finally {
                    busy = false
                    preparing = false
                }
            }
        }) { Text(stringResource(if (busy && preparing) R.string.qr_preparing else R.string.scan_qr)) }
        if (busy && preparing) {
            TextButton(onClick = { job?.cancel() }) { Text(stringResource(R.string.cancel)) }
        }
        if (failed) Text(stringResource(R.string.qr_unavailable), color = MaterialTheme.colorScheme.error)
    }
}
