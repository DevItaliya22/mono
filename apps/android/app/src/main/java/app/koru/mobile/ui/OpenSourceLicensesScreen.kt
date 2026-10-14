// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import android.content.Context
import androidx.annotation.StringRes
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import app.koru.mobile.R
import app.koru.mobile.ui.theme.KoruSpacing
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * **What the build packages in `assets/licenses/`**, named once. Gradle's
 * `koruNotices<Variant>` task writes these files and `checkKoruNotices<Variant>`
 * refuses an APK without them; `OpenSourceLicensesTest` holds the names here to
 * the names there.
 */
object OpenSourceLicenses {
    const val DIRECTORY = "licenses"
    const val LICENSE = "LICENSE.txt"
    const val NOTICE = "NOTICE.txt"
    const val SOURCE = "SOURCE.txt"
    const val THIRD_PARTY = "THIRD-PARTY-NOTICES.txt"

    /** The documents the screen lists, in order, with their titles. */
    val documents: List<Pair<String, Int>> = listOf(
        LICENSE to R.string.open_source_license_koru,
        NOTICE to R.string.open_source_license_notice,
        THIRD_PARTY to R.string.open_source_license_third_party,
    )

    /**
     * **A long text in pieces a lazy list can draw.** The third-party notices
     * are over a megabyte; one `Text` holding all of it lays out every line
     * before the first frame. Pieces break at blank lines, are at most
     * [maxLines] lines unless one paragraph is longer, and join back to the
     * original text with "\n".
     */
    fun chunks(text: String, maxLines: Int = 80): List<String> {
        val lines = text.replace("\r\n", "\n").trimEnd('\n').split('\n')
        val out = mutableListOf<String>()
        var current = mutableListOf<String>()
        for (line in lines) {
            current += line
            if (line.isBlank() && current.size >= maxLines) {
                out += current.joinToString("\n")
                current = mutableListOf()
            }
        }
        if (current.isNotEmpty()) out += current.joinToString("\n")
        return out
    }

    fun read(context: Context, file: String): String? =
        runCatching { context.assets.open("$DIRECTORY/$file").bufferedReader().use { it.readText() } }.getOrNull()
}

/** Settings -> Open-source licenses: the source of this build, and the three documents. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OpenSourceLicensesScreen(onBack: () -> Unit, onOpen: (String) -> Unit) {
    val context = LocalContext.current
    val uriHandler = LocalUriHandler.current
    val source by produceState<String?>(null) {
        value = withContext(Dispatchers.IO) { OpenSourceLicenses.read(context, OpenSourceLicenses.SOURCE)?.trim() }
    }
    Scaffold(topBar = { TopAppBar(
        title = { Text(stringResource(R.string.open_source_licenses)) },
        navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.back)) } },
    ) }) { padding ->
        LazyColumn(
            Modifier.padding(padding),
            contentPadding = PaddingValues(KoruSpacing.md),
            verticalArrangement = Arrangement.spacedBy(KoruSpacing.sm),
        ) {
            item { Text(stringResource(R.string.open_source_licenses_intro), style = MaterialTheme.typography.bodyMedium) }
            source?.takeIf { it.startsWith("https://") }?.let { url ->
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.open_source_source_code)) },
                        supportingContent = { Text(url) },
                        trailingContent = { TextButton(onClick = { uriHandler.openUri(url) }) { Text(stringResource(R.string.open)) } },
                    )
                }
            }
            item { HorizontalDivider() }
            items(OpenSourceLicenses.documents) { (file, title) ->
                ListItem(
                    headlineContent = { Text(stringResource(title)) },
                    trailingContent = { TextButton(onClick = { onOpen(file) }) { Text(stringResource(R.string.open)) } },
                )
            }
        }
    }
}

/** One of the documents, read from the APK's assets off the main thread. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OpenSourceLicenseDocumentScreen(file: String, onBack: () -> Unit) {
    val context = LocalContext.current
    // Only the listed documents: the name arrives through a route.
    @StringRes val title = OpenSourceLicenses.documents.firstOrNull { it.first == file }?.second
    val chunks by produceState<List<String>?>(null, file) {
        value = withContext(Dispatchers.Default) {
            title?.let { OpenSourceLicenses.read(context, file) }?.let { OpenSourceLicenses.chunks(it) } ?: emptyList()
        }
    }
    Scaffold(topBar = { TopAppBar(
        title = { Text(stringResource(title ?: R.string.open_source_licenses)) },
        navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.back)) } },
    ) }) { padding ->
        val loaded = chunks
        when {
            loaded == null -> Box(Modifier.fillMaxSize().padding(padding)) { LinearProgressIndicator(Modifier.fillMaxWidth()) }
            loaded.isEmpty() -> Text(
                stringResource(R.string.open_source_licenses_missing),
                Modifier.padding(padding).padding(KoruSpacing.md),
            )
            else -> LazyColumn(Modifier.padding(padding), contentPadding = PaddingValues(KoruSpacing.md)) {
                items(loaded) { chunk ->
                    Text(chunk, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
                }
            }
        }
    }
}
