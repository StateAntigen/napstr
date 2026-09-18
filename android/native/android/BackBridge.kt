package net.napstr.nostrfy

import android.webkit.JavascriptInterface
import android.webkit.WebView
import java.lang.ref.WeakReference

/**
 * Lets the now-playing drawer own the hardware back button.
 *
 * Kotlin cannot ask the page whether the drawer is open synchronously, so the
 * webview pushes that flag on every drawer transition. When the drawer is open,
 * a back press closes it and is consumed; otherwise the press is handed back to
 * the system so back still leaves the app.
 */
class BackBridge {
  @JavascriptInterface
  fun setDrawerOpen(open: Boolean) {
    drawerOpen = open
  }

  companion object {
    private const val BACK_EVENT = "napstrfy-back"
    // Written from the webview's JS bridge thread and read from the UI thread.
    @Volatile private var drawerOpen = false
    private var webView = WeakReference<WebView>(null)

    fun attach(next: WebView) {
      webView = WeakReference(next)
    }

    fun detach() {
      drawerOpen = false
      webView.clear()
    }

    /** True when the press was consumed by the drawer. */
    fun consumeBack(): Boolean {
      if (!drawerOpen) return false
      drawerOpen = false
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
