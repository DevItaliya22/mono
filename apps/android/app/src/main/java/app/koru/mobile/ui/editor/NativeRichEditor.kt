// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile.ui.editor

import android.graphics.*
import android.graphics.drawable.ColorDrawable
import android.text.*
import android.text.style.*
import android.view.Gravity
import android.widget.EditText
import androidx.compose.runtime.*
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFontFamilyResolver
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.viewinterop.AndroidView
import app.koru.mobile.domain.*
import kotlin.math.roundToInt

private class ReferenceSpan(private val label: String, private val textColor: Int) : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?): Int =
        paint.measureText(label).roundToInt()

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        val old = paint.color
        val underlined = paint.isUnderlineText
        paint.color = textColor
        paint.isUnderlineText = true
        canvas.drawText(label, x, y.toFloat(), paint)
        paint.color = old
        paint.isUnderlineText = underlined
    }
}

private class RichCoordinator(
    var model: RichText,
    var onText: (String, String) -> Unit,
    var onSplit: (String, String) -> Unit,
    var code: Boolean,
) : TextWatcher {
    var rendering = false
    var focusScheduled = false
    lateinit var view: EditText

    private var before = model.text
    override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) { before = s?.toString().orEmpty() }
    override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
    override fun afterTextChanged(value: Editable?) {
        if (rendering || value == null) return
        val text = value.toString()
        if (!code && text.contains('\n')) {
            onSplit(before, text)
            rendering = true
            value.delete(text.indexOf('\n'), value.length)
            rendering = false
        } else onText(before, text)
    }
}

@Composable
fun NativeRichEditor(
    richText: RichText,
    style: TextStyle,
    draftText: String?,
    onText: (String, String) -> Unit,
    onDone: () -> Unit,
    onSplit: (String, String) -> Unit,
    locked: Boolean = false,
    requestFocus: Boolean = false,
    onFocused: () -> Unit = {},
    code: Boolean = false,
    hint: String = "",
    modifier: Modifier = Modifier,
) {
    val foreground = style.color.toArgb()
    val linkColor = androidx.compose.material3.MaterialTheme.colorScheme.primary.toArgb()
    val density = LocalDensity.current
    val typeface = LocalFontFamilyResolver.current.resolve(style.fontFamily,
        style.fontWeight ?: FontWeight.Normal, style.fontStyle ?: FontStyle.Normal).value as Typeface
    val fontPixels = with(density) { style.fontSize.toPx() }
    val linePixels = with(density) { style.lineHeight.toPx() }
    val bringIntoView = remember { BringIntoViewRequester() }
    var focused by remember { mutableStateOf(false) }
    val keyboardHeight = WindowInsets.ime.getBottom(LocalDensity.current)
    LaunchedEffect(focused, keyboardHeight) {
        if (focused) bringIntoView.bringIntoView()
    }
    AndroidView(
        modifier = modifier.bringIntoViewRequester(bringIntoView),
        factory = { context ->
            val coordinator = RichCoordinator(richText, onText, onSplit, code)
            EditText(context).apply {
                coordinator.view = this
                tag = coordinator
                setOnFocusChangeListener { _, hasFocus -> focused = hasFocus }
                background = ColorDrawable(Color.Transparent.toArgb())
                setTextColor(foreground)
                includeFontPadding = false
                minHeight = 0
                minimumHeight = 0
                isSingleLine = false
                this.hint = hint
                inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE or android.text.InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
                imeOptions = android.view.inputmethod.EditorInfo.IME_FLAG_NO_ENTER_ACTION
                setOnEditorActionListener { _, action, _ ->
                    if (action == android.view.inputmethod.EditorInfo.IME_ACTION_DONE) { onDone(); true } else false
                }
                filters = arrayOf(InputFilter { source, start, end, _, _, _ ->
                    if (coordinator.rendering) null else {
                        val input = source.subSequence(start, end).toString()
                        val filtered = input.replace("\uFFFC", "").replace("\r\n", "\n").replace("\r", "\n")
                        if (input == filtered) null else filtered
                    }
                })
                addTextChangedListener(coordinator)
                coordinator.rendering = true
                renderRichText(this, richText, linkColor)
                draftText?.let { applyDraft(this, it) }
                coordinator.rendering = false
            }
        },
        update = { view ->
            val coordinator = view.tag as RichCoordinator
            coordinator.onText = onText
            coordinator.onSplit = onSplit
            coordinator.code = code
            view.setTextColor(foreground)
            view.isEnabled = !locked
            view.typeface = typeface
            view.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, fontPixels)
            val extra = (linePixels - (view.paint.fontMetrics.descent - view.paint.fontMetrics.ascent)).coerceAtLeast(0f)
            view.setLineSpacing(extra, 1f)
            view.setPadding(0, (extra / 2).toInt(), 0, (extra - extra / 2).roundToInt())
            view.gravity = Gravity.TOP or when (style.textAlign) {
                TextAlign.Center -> Gravity.CENTER_HORIZONTAL
                TextAlign.End -> Gravity.END
                else -> Gravity.START
            }
            view.justificationMode = if (style.textAlign == TextAlign.Justify) android.text.Layout.JUSTIFICATION_MODE_INTER_WORD else android.text.Layout.JUSTIFICATION_MODE_NONE
            view.letterSpacing = if (style.letterSpacing.isSp) style.letterSpacing.value / style.fontSize.value else 0f
            view.setOnEditorActionListener { _, action, _ ->
                if (action == android.view.inputmethod.EditorInfo.IME_ACTION_DONE) { onDone(); true } else false
            }
            if (locked && draftText != null && view.text.toString() != draftText) {
                coordinator.rendering = true
                applyDraft(view, draftText)
                coordinator.rendering = false
            }
            if (requestFocus && !locked && !coordinator.focusScheduled) {
                coordinator.focusScheduled = true
                view.post {
                    coordinator.focusScheduled = false
                    if (!view.isAttachedToWindow) return@post
                    view.requestFocus()
                    view.setSelection(0)
                    (view.context.getSystemService(android.content.Context.INPUT_METHOD_SERVICE) as android.view.inputmethod.InputMethodManager)
                        .showSoftInput(view, android.view.inputmethod.InputMethodManager.SHOW_IMPLICIT)
                    onFocused()
                }
            }
            // A poll/ack must not restart an IME composing range or replace a
            // newer keystroke with the preceding saved snapshot.
            if (draftText == null && (coordinator.model != richText || view.text.toString() != richText.text) &&
                android.view.inputmethod.BaseInputConnection.getComposingSpanStart(view.text) < 0) {
                val selection = view.selectionStart.coerceAtLeast(0)
                coordinator.rendering = true
                renderRichText(view, richText, linkColor)
                view.setSelection(selection.coerceAtMost(view.text.length))
                coordinator.model = richText
                coordinator.rendering = false
            }
        },
    )
}

private fun applyDraft(view: EditText, draft: String) {
    val splice = textSplice(view.text.toString(), draft)
    view.text.replace(splice.start, splice.end, splice.inserted)
}

private fun renderRichText(view: EditText, rich: RichText, linkColor: Int) {
    val fallback = referenceFallback(view.resources)
    val text = SpannableStringBuilder(rich.text)
    rich.marks.forEach { mark ->
        val start = utf16Index(rich.text, mark.range.startUtf8) ?: return@forEach
        val end = utf16Index(rich.text, mark.range.endUtf8) ?: return@forEach
        val span: Any = when (mark.kind) {
            MarkKind.Bold -> TypefaceSpan(Typeface.create("sans-serif", Typeface.BOLD))
            MarkKind.Italic -> StyleSpan(Typeface.ITALIC)
            MarkKind.Underline -> UnderlineSpan()
            MarkKind.Strike -> StrikethroughSpan()
            MarkKind.Code -> TypefaceSpan("monospace")
            MarkKind.Superscript -> SuperscriptSpan()
            MarkKind.Subscript -> SubscriptSpan()
            MarkKind.Size -> AbsoluteSizeSpan(android.util.TypedValue.applyDimension(
                android.util.TypedValue.COMPLEX_UNIT_SP, mark.value.decodeToString().toFloatOrNull() ?: 18f,
                view.resources.displayMetrics).roundToInt())
            MarkKind.Highlight -> BackgroundColorSpan(0x66FFE066)
            MarkKind.Unknown -> return@forEach
        }
        text.setSpan(span, start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    rich.inlineObjects.forEach { value ->
        val start = utf16Index(rich.text, value.anchorUtf8) ?: return@forEach
        if (start < text.length) text.setSpan(ReferenceSpan(value.label ?: fallback(value.reference), linkColor), start, start + 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    view.setText(text)
}

private fun utf16Index(value: String, utf8Offset: Int): Int? {
    var bytes = 0
    var index = 0
    while (index < value.length) {
        if (bytes == utf8Offset) return index
        val codePoint = value.codePointAt(index)
        bytes += String(Character.toChars(codePoint)).toByteArray(Charsets.UTF_8).size
        index += Character.charCount(codePoint)
    }
    return index.takeIf { bytes == utf8Offset }
}
