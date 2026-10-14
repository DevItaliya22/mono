// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import app.koru.mobile.R
import app.koru.mobile.ui.theme.*

/**
 * The picker lists each language **in itself** — the desktop's `Lang::endonym`,
 * for the same reason: a list that offered "Japanese" to somebody who reads
 * only Japanese is a list they cannot use to leave the language they are stuck
 * in. These are the one set of strings on this screen that must never be
 * translated, and the tag beside each is what is stored.
 */
private val languages = listOf(
    "en" to "English", "es" to "Español", "de" to "Deutsch",
    "fr" to "Français", "pt-BR" to "Português (Brasil)",
    "ja" to "日本語", "zh-CN" to "简体中文", "ar" to "العربية",
)

@OptIn(ExperimentalLayoutApi::class, ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    appearance: AppearancePreferences,
    onAppearanceChanged: (AppearancePreferences) -> Unit,
    onBack: () -> Unit,
    onDiagnostics: () -> Unit,
    onAccount: () -> Unit,
    onLicenses: () -> Unit,
    picturesOnCellular: Boolean,
    onPicturesOnCellularChanged: (Boolean) -> Unit,
) {
    Scaffold(topBar = { TopAppBar(
        title = { Text(stringResource(R.string.settings)) },
        navigationIcon = { TextButton(onClick = onBack) { Text(stringResource(R.string.back)) } },
    ) }) { padding ->
        LazyColumn(
            Modifier.padding(padding),
            contentPadding = PaddingValues(KoruSpacing.md),
            verticalArrangement = Arrangement.spacedBy(KoruSpacing.lg),
        ) {
            item { TextButton(onClick = onAccount) { Text(stringResource(R.string.account_title)) } }
            item {
                Text(stringResource(R.string.theme), style = MaterialTheme.typography.titleLarge)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(KoruSpacing.sm)) {
                    KoruThemePreference.entries.forEach { choice ->
                        FilterChip(
                            selected = appearance.theme == choice,
                            onClick = { onAppearanceChanged(appearance.copy(theme = choice)) },
                            label = { Text(stringResource(choice.labelRes)) },
                        )
                    }
                }
            }
            item {
                Text(stringResource(R.string.language), style = MaterialTheme.typography.titleLarge)
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(KoruSpacing.sm),
                    verticalArrangement = Arrangement.spacedBy(KoruSpacing.xs),
                ) {
                    languages.forEach { (tag, label) ->
                        FilterChip(
                            selected = appearance.language == tag,
                            onClick = { onAppearanceChanged(appearance.copy(language = tag)) },
                            label = { Text(label) },
                        )
                    }
                }
            }
            item {
                Text(stringResource(R.string.font), style = MaterialTheme.typography.titleLarge)
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(KoruSpacing.sm),
                    verticalArrangement = Arrangement.spacedBy(KoruSpacing.xs),
                ) {
                    KoruFontChoice.entries.forEach { choice ->
                        val family = rememberKoruFontFamily(choice)
                        FilterChip(
                            selected = appearance.font == choice,
                            onClick = { onAppearanceChanged(appearance.copy(font = choice)) },
                            label = {
                                // Inter and Manrope are product names and stay
                                // as written; the four system families are
                                // descriptions and carry a resource.
                                val name = choice.labelRes?.let { stringResource(it) } ?: choice.label
                                Text(name, fontFamily = family)
                            },
                        )
                    }
                }
            }
            item { HorizontalDivider() }
            // **The one switch D50 gives a person.** Off, files and pictures
            // wait for a connection this device may use; documents sync either
            // way, and a single one can still be asked for from the document
            // itself.
            //
            // It says "files and pictures" because it has always held both and
            // said one (D259): the engine's counts never consulted a media
            // type, so a switch labelled "Pictures" was a switch whose effect
            // on a 1.9 GiB archive was invisible.
            item {
                Text(stringResource(R.string.transfers), style = MaterialTheme.typography.titleLarge)
                ListItem(
                    headlineContent = { Text(stringResource(R.string.transfers_on_cellular)) },
                    supportingContent = {
                        Text(stringResource(R.string.transfers_on_cellular_explainer))
                    },
                    trailingContent = {
                        Switch(
                            checked = picturesOnCellular,
                            onCheckedChange = onPicturesOnCellularChanged,
                        )
                    },
                )
            }
            item { HorizontalDivider() }
            // The notices the build packages, and the source it was built
            // from (MPL-2.0 §3.2).
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.open_source_licenses)) },
                    supportingContent = { Text(stringResource(R.string.open_source_licenses_subtitle)) },
                    trailingContent = { TextButton(onClick = onLicenses) { Text(stringResource(R.string.open)) } },
                )
            }
            item {
                ListItem(
                    headlineContent = { Text(stringResource(R.string.developer)) },
                    supportingContent = { Text(stringResource(R.string.developer_subtitle)) },
                    trailingContent = { TextButton(onClick = onDiagnostics) { Text(stringResource(R.string.open)) } },
                )
            }
            item { Spacer(Modifier.height(8.dp)) }
        }
    }
}
