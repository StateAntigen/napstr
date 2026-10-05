package net.napstr.nostrfy

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.webkit.JavascriptInterface
import androidx.core.content.FileProvider
import java.io.File

/**
 * The one thing the page cannot do for itself: hand something to another app.
 *
 * A webview can copy to the clipboard and can show a code, but "share to an audio
 * editor" is an Android intent, and a file the page only knows as a path is only
 * readable by another app through a content URI. Both live here rather than in the
 * Rust half because both are about *this* device's app ecosystem, which is what
 * the Kotlin layer is for.
 *
 * Nothing here is trusted: every argument is a string the page produced, checked
 * for shape and bounded, and the file is one the application itself put in its own
 * cache directory. A path outside that directory is refused rather than resolved,
 * because a share that could point anywhere would be a way for a compromised page
 * to hand any file on the device to any app.
 */
class ShareBridge(private val activity: MainActivity) {
  @JavascriptInterface
  fun shareText(text: String, title: String): Boolean {
    val payload = safeText(text, MAX_TEXT)
    if (payload.isEmpty()) return false
    val intent = Intent(Intent.ACTION_SEND).apply {
      type = "text/plain"
      putExtra(Intent.EXTRA_TEXT, payload)
      val name = safeText(title, MAX_TITLE)
      if (name.isNotEmpty()) putExtra(Intent.EXTRA_TITLE, name)
    }
    return start(intent, safeText(title, MAX_TITLE))
  }

  /**
   * Share one audio file, by the path the Rust half put it at.
   *
   * The path is made absolute and checked to be inside the cache directory the
   * provider exposes, so the URI handed to the chooser is one Android will let the
   * chosen app read - and only for as long as the chooser's own grant lasts.
   */
  @JavascriptInterface
  fun shareFile(path: String, mime: String, title: String): Boolean {
    val shareRoot = File(activity.cacheDir, SHARE_DIRECTORY)
    val inside = try {
      File(path).canonicalFile
    } catch (_: Exception) {
      return false
    }
    if (!inside.isFile || !inside.canonicalPath.startsWith(shareRoot.canonicalPath + File.separator)) {
      return false
    }
    val uri = try {
      FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", inside)
    } catch (_: Exception) {
      // A share with no readable URI is worse than no share: the chosen app would
      // open an empty file and the person would not know why.
      return false
    }
    val intent = Intent(Intent.ACTION_SEND).apply {
      type = if (mime.isBlank()) "audio/*" else mime
      putExtra(Intent.EXTRA_STREAM, uri)
      putExtra(Intent.EXTRA_TITLE, safeText(title, MAX_TITLE))
      clipData = ClipData.newRawUri(safeText(title, MAX_TITLE), uri)
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
    return start(intent, safeText(title, MAX_TITLE))
  }

  /** Copy text, for a page that would rather not rely on the webview's clipboard. */
  @JavascriptInterface
  fun copyText(text: String): Boolean {
    val payload = safeText(text, MAX_TEXT)
    if (payload.isEmpty()) return false
    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
      ?: return false
    activity.runOnUiThread {
      clipboard.setPrimaryClip(ClipData.newPlainText("Napstrfy", payload))
    }
    return true
  }

  private fun start(intent: Intent, title: String): Boolean {
    val chooser = Intent.createChooser(intent, title.ifEmpty { "Share" })
    chooser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    return try {
      activity.runOnUiThread { activity.startActivity(chooser) }
      true
    } catch (_: Exception) {
      // A device with nothing that accepts this: the page says so instead of
      // believing a sheet appeared.
      false
    }
  }

  private fun safeText(value: String, limit: Int): String =
    value.filter { it >= ' ' && it != '\u007f' }.take(limit)

  companion object {
    /** Where the Rust half stages a file for sharing. Must match `SHARE_DIRECTORY` there. */
    const val SHARE_DIRECTORY = "share"
    private const val MAX_TEXT = 4_096
    private const val MAX_TITLE = 200
  }
}
