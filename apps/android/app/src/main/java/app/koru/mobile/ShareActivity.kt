// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import app.koru.mobile.ui.KoruApp
import app.koru.mobile.ui.theme.AppearanceStore
import app.koru.mobile.ui.theme.KoruTheme

/** Ephemeral Android Sharesheet entry point. The regular launcher stays in MainActivity. */
class ShareActivity : ComponentActivity() {
    private lateinit var model: AppViewModel

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val container = (application as KoruApplication).container
        val appearance = AppearanceStore.load(applicationContext)
        AppearanceStore.applyLanguage(this, appearance.language)
        model = ViewModelProvider(this, object : ViewModelProvider.Factory {
            override fun <T : ViewModel> create(modelClass: Class<T>): T {
                if (!modelClass.isAssignableFrom(AppViewModel::class.java)) {
                    throw IllegalArgumentException("unknown view model")
                }
                @Suppress("UNCHECKED_CAST")
                return AppViewModel(container) as T
            }
        })[AppViewModel::class.java]
        val share = model.share.value
        if (share.pending == null && !share.receiving && share.savedFiles.isEmpty()) {
            model.receiveShare(intent)
        }
        setContent {
            KoruTheme(appearance) {
                KoruApp(
                    model = model,
                    implementation = container.serviceImplementation,
                    appearance = appearance,
                    onAppearanceChanged = {},
                    onShareFinished = {
                        setResult(RESULT_OK)
                        finish()
                    },
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        model.receiveShare(intent)
    }
}
