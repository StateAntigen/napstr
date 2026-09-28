package net.napstr.nostrfy

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.webkit.JavascriptInterface
import android.webkit.WebView
import java.lang.ref.WeakReference

/**
 * Whether the connection this phone is spending is metered.
 *
 * The page owns the music quality setting, but only the phone knows what the
 * setting is spending: `isActiveNetworkMetered` is true on mobile data AND on a
 * metered hotspot, and false on Wi-Fi - including the Wi-Fi a laptop is sharing -
 * which is why this asks the system rather than looking at the connection type.
 * A phone that cannot tell reports metered, because guessing "free" wrongly costs
 * money and guessing "metered" wrongly costs one held-back track.
 */
class NetworkBridge(private val context: Context) {
  @JavascriptInterface
  fun metered(): Boolean = isMetered(context)

  /** "metered", "unmetered" or "offline", for a page that wants to say which. */
  @JavascriptInterface
  fun kind(): String {
    val connectivity = connectivityManager(context) ?: return METERED
    return runCatching {
      if (connectivity.activeNetwork == null) return@runCatching OFFLINE
      if (connectivity.isActiveNetworkMetered) METERED else UNMETERED
    }.getOrDefault(METERED)
  }

  companion object {
    const val METERED = "metered"
    const val UNMETERED = "unmetered"
    const val OFFLINE = "offline"
    private const val NETWORK_EVENT = "napstrfy-network"
    private var webView = WeakReference<WebView>(null)
    private var manager: ConnectivityManager? = null
    private var listener: ConnectivityManager.NetworkCallback? = null

    fun attach(next: WebView) {
      webView = WeakReference(next)
      if (listener != null) return
      val connectivity = connectivityManager(next.context) ?: return
      // A callback rather than a timer: the answer changes when the network
      // changes, and a phone that switches from Wi-Fi to data mid-album has to
      // hear about it before the next file, not at the next poll.
      val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
          announce()
        }

        override fun onLost(network: Network) {
          announce()
        }
      }
      // A registration that fails is not worth a crash: the page reads the
      // current answer at startup either way and keeps it until something tells
      // it otherwise.
      if (runCatching { connectivity.registerDefaultNetworkCallback(callback) }.isSuccess) {
        manager = connectivity
        listener = callback
      }
    }

    fun detach() {
      listener?.let { callback ->
        manager?.let { runCatching { it.unregisterNetworkCallback(callback) } }
      }
      listener = null
      manager = null
      webView.clear()
    }

    fun isMetered(context: Context): Boolean {
      val connectivity = connectivityManager(context) ?: return true
      return runCatching { connectivity.isActiveNetworkMetered }.getOrDefault(true)
    }

    private fun connectivityManager(context: Context): ConnectivityManager? =
      context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager

    private fun announce() {
      webView.get()?.post {
        webView.get()?.evaluateJavascript(
          "window.dispatchEvent(new Event('$NETWORK_EVENT'))",
          null
        )
      }
    }
  }
}
