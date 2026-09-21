package net.napstr.nostrfy

import android.webkit.JavascriptInterface
import android.webkit.WebView
import java.lang.ref.WeakReference

/**
 * Lets the page own the hardware back button.
 *
 * Kotlin cannot ask the page whether it has anything for back to close, so the
 * webview pushes that flag on every transition. While it is set, a back press is
 * handed to the page and consumed; otherwise the press goes back to the system,
 * so back still leaves the app when the page has nothing open.
 */
class BackBridge {
  @JavascriptInterface
  fun setBackAvailable(available: Boolean) {
    backAvailable = available
  }

  /** The older name for the same flag, for a page that predates this one. */
  @JavascriptInterface
  fun setDrawerOpen(open: Boolean) {
    backAvailable = open
  }

  companion object {
    private const val BACK_EVENT = "napstrfy-back"
    // Written from the webview's JS bridge thread and read from the UI thread.
    @Volatile private var backAvailable = false
    private var webView = WeakReference<WebView>(null)

    fun attach(next: WebView) {
      webView = WeakReference(next)
    }

    fun detach() {
      backAvailable = false
      webView.clear()
    }

    /** True when the press was handed to the page rather than the system. */
    fun consumeBack(): Boolean {
      if (!backAvailable) return false
      // Clearing it decides this one press only: the page publishes the flag
      // again as soon as it has closed whatever it had.
      backAvailable = false
      webView.get()?.post {
        webView.get()?.evaluateJavascript(
          "window.dispatchEvent(new CustomEvent('$BACK_EVENT'))",
          null
        )
      }
      return true
    }
  }
}
