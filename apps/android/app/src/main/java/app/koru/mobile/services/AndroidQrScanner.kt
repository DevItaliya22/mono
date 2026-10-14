// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.services

import android.content.Context
import com.google.android.gms.common.api.ApiException
import com.google.android.gms.common.moduleinstall.InstallStatusListener
import com.google.android.gms.common.moduleinstall.ModuleInstall
import com.google.android.gms.common.moduleinstall.ModuleInstallRequest
import com.google.android.gms.common.moduleinstall.ModuleInstallStatusUpdate.InstallState
import com.google.android.gms.tasks.Task
import com.google.mlkit.common.MlKitException
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Google Play services owns the camera and permission UI. Koru receives only
 * the link code; do not add an app camera permission or log the scanned value. */
internal class AndroidQrScanner(context: Context) : QrScanPort {
    private val scanner = GmsBarcodeScanning.getClient(context,
        GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build())
    private val modules = ModuleInstall.getClient(context)

    override suspend fun prepare() {
        val ready = CompletableDeferred<Unit>()
        val listener = InstallStatusListener { update ->
            when (update.installState) {
                InstallState.STATE_COMPLETED -> ready.complete(Unit)
                InstallState.STATE_FAILED -> ready.completeExceptionally(ScannerInstallFailure(update.errorCode))
                InstallState.STATE_CANCELED -> ready.cancel()
            }
        }
        try {
            val request = ModuleInstallRequest.newBuilder().addApi(scanner).setListener(listener).build()
            val response = modules.installModules(request).awaitScannerTask()
            if (response.areModulesAlreadyInstalled()) ready.complete(Unit)
            // installModules succeeding only acknowledges the request. Wait for
            // completion, including when its callback arrived before the reply.
            ready.await()
        } finally {
            modules.unregisterListener(listener)
        }
    }

    override suspend fun scan(): String? = scanner.startScan().awaitScannerTask().rawValue
}

internal class ScannerInstallFailure(val status: Int) : Exception()

/** Only bounded SDK codes cross the diagnostic boundary, never messages or QR bytes. */
internal fun scannerFailureCode(error: Exception): Int? = when (error) {
    is ScannerInstallFailure -> error.status
    is MlKitException -> error.errorCode
    is ApiException -> error.statusCode
    else -> null
}

private suspend fun <T> Task<T>.awaitScannerTask(): T = suspendCancellableCoroutine { continuation ->
    addOnCompleteListener { task ->
        if (continuation.isActive) {
            when {
                task.isCanceled -> continuation.cancel()
                task.isSuccessful -> continuation.resume(task.result)
                else -> continuation.resumeWithException(task.exception ?: IllegalStateException())
            }
        }
    }
}
