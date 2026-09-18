package net.napstr.nostrfy

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.media.app.NotificationCompat.MediaStyle
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

class MediaNotificationService : Service() {
  private lateinit var mediaSession: MediaSessionCompat
  private var title = "Napstrfy"
  private var artist = ""
  private var playing = false
  private var position = 0L
  private var duration = 0L
  private var canPrevious = false
  private var canNext = false
  private var foregroundStarted = false
  private var screenWakeLock: PowerManager.WakeLock? = null
  private var artworkUrl = ""
  private var artwork: Bitmap? = null
  private var artworkRequest = 0
  private val mainHandler = Handler(Looper.getMainLooper())
  private val artworkLoader = Executors.newSingleThreadExecutor { runnable ->
    Thread(runnable, "napstrfy-artwork").apply { isDaemon = true }
  }

  override fun onCreate() {
    super.onCreate()
    createChannel()
    mediaSession = MediaSessionCompat(this, "NapstrfyPlayback").apply {
      setCallback(object : MediaSessionCompat.Callback() {
        override fun onPlay() = dispatch(ACTION_PLAY)
        override fun onPause() = dispatch(ACTION_PAUSE)
        override fun onSkipToPrevious() = dispatch(ACTION_PREVIOUS)
        override fun onSkipToNext() = dispatch(ACTION_NEXT)
        override fun onSeekTo(pos: Long) {
          MediaControlBridge.dispatch("seek:${pos.coerceAtLeast(0L)}")
        }
      })
      isActive = true
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_UPDATE -> readState(intent)
      ACTION_CLEAR -> {
        stopForeground(STOP_FOREGROUND_REMOVE)
        foregroundStarted = false
        stopSelf()
        return START_NOT_STICKY
      }
      ACTION_PLAY, ACTION_PAUSE, ACTION_PREVIOUS, ACTION_NEXT -> {
        dispatch(intent.action!!)
        return START_NOT_STICKY
      }
      else -> return START_NOT_STICKY
    }

    updateSession()
    val notification = buildNotification()
    startForeground(NOTIFICATION_ID, notification)
    foregroundStarted = true
    return START_NOT_STICKY
  }

  private fun readState(intent: Intent) {
    title = intent.getStringExtra(EXTRA_TITLE)?.ifBlank { "Napstrfy" } ?: "Napstrfy"
    artist = intent.getStringExtra(EXTRA_ARTIST).orEmpty()
    playing = intent.getBooleanExtra(EXTRA_PLAYING, false)
    position = intent.getLongExtra(EXTRA_POSITION, 0L).coerceAtLeast(0L)
    duration = intent.getLongExtra(EXTRA_DURATION, 0L).coerceAtLeast(0L)
    canPrevious = intent.getBooleanExtra(EXTRA_CAN_PREVIOUS, false)
    canNext = intent.getBooleanExtra(EXTRA_CAN_NEXT, false)
    updateArtwork(intent.getStringExtra(EXTRA_ARTWORK).orEmpty())
    updateScreenWakeLock()
  }

  /**
   * The webview pushes the cover URL on every state change, so only a genuine
   * change is worth a fetch; anything already decoded is reused from the cache.
   */
  private fun updateArtwork(url: String) {
    if (url == artworkUrl) return
    artworkUrl = url
    artwork = ArtworkCache.peek(url)
    artworkRequest += 1
    if (artwork != null) return updateSession()
    if (url.isEmpty()) return updateSession()
    val request = artworkRequest
    artworkLoader.execute {
      val loaded = ArtworkCache.load(url)
      mainHandler.post {
        if (loaded == null || request != artworkRequest) return@post
        artwork = loaded
        updateSession()
        refreshNotification()
      }
    }
  }

  /** Re-posts the notification once artwork arrives, without alerting again. */
  private fun refreshNotification() {
    if (!foregroundStarted) return
    NotificationManagerCompat.from(this).notify(NOTIFICATION_ID, buildNotification())
  }

  @Suppress("DEPRECATION")
  private fun updateScreenWakeLock() {
    if (playing) {
      if (screenWakeLock == null) {
        val powerManager = getSystemService(POWER_SERVICE) as PowerManager
        screenWakeLock = powerManager.newWakeLock(
          PowerManager.SCREEN_DIM_WAKE_LOCK,
          "$packageName:playback-screen"
        ).apply { setReferenceCounted(false) }
      }
      if (screenWakeLock?.isHeld != true) screenWakeLock?.acquire()
    } else {
      releaseScreenWakeLock()
    }
  }

  private fun releaseScreenWakeLock() {
    screenWakeLock?.takeIf { it.isHeld }?.release()
  }

  private fun updateSession() {
    val metadata = MediaMetadataCompat.Builder()
      .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
      .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
      .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, duration)
    // The lock screen, media output picker and Android Auto read the session.
    artwork?.let {
      metadata.putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, it)
      metadata.putBitmap(MediaMetadataCompat.METADATA_KEY_ART, it)
    }
    mediaSession.setMetadata(metadata.build())
    var actions = PlaybackStateCompat.ACTION_PLAY or
      PlaybackStateCompat.ACTION_PAUSE or
      PlaybackStateCompat.ACTION_PLAY_PAUSE or
      PlaybackStateCompat.ACTION_SEEK_TO
    if (canPrevious) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS
    if (canNext) actions = actions or PlaybackStateCompat.ACTION_SKIP_TO_NEXT
    mediaSession.setPlaybackState(
      PlaybackStateCompat.Builder()
        .setActions(actions)
        .setState(
          if (playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED,
          position,
          if (playing) 1f else 0f
        )
        .build()
    )
  }

  private fun buildNotification(): Notification {
    val previous = actionPendingIntent(ACTION_PREVIOUS, 1)
    val playPause = actionPendingIntent(if (playing) ACTION_PAUSE else ACTION_PLAY, 2)
    val next = actionPendingIntent(ACTION_NEXT, 3)
    val launch = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(this, 0, it, pendingFlags())
    }
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_stat_napstrfy)
      .setContentTitle(title)
      .setContentText(artist)
      .setContentIntent(launch)
      .apply { artwork?.let { setLargeIcon(it) } }
      .setOnlyAlertOnce(true)
      .setSilent(true)
      .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
      .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
      .setOngoing(playing)
      .addAction(android.R.drawable.ic_media_previous, "Previous", previous)
      .addAction(
        if (playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
        if (playing) "Pause" else "Play",
        playPause
      )
      .addAction(android.R.drawable.ic_media_next, "Next", next)
      .setStyle(MediaStyle().setMediaSession(mediaSession.sessionToken).setShowActionsInCompactView(0, 1, 2))
      .build()
  }

  private fun actionPendingIntent(action: String, requestCode: Int): PendingIntent =
    PendingIntent.getService(
      this,
      requestCode,
      Intent(this, MediaNotificationService::class.java).setAction(action),
      pendingFlags()
    )

  private fun pendingFlags(): Int =
    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE

  private fun dispatch(action: String) {
    when (action) {
      ACTION_PLAY -> MediaControlBridge.dispatch("play")
      ACTION_PAUSE -> MediaControlBridge.dispatch("pause")
      ACTION_PREVIOUS -> if (canPrevious) MediaControlBridge.dispatch("previous")
      ACTION_NEXT -> if (canNext) MediaControlBridge.dispatch("next")
    }
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val channel = NotificationChannel(
      CHANNEL_ID,
      "Media playback",
      NotificationManager.IMPORTANCE_LOW
    ).apply {
      description = "Controls for music and podcasts playing in Napstrfy"
      setShowBadge(false)
    }
    getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }

  override fun onTaskRemoved(rootIntent: Intent?) {
    releaseScreenWakeLock()
    stopForeground(STOP_FOREGROUND_REMOVE)
    stopSelf()
    super.onTaskRemoved(rootIntent)
  }

  override fun onDestroy() {
    artworkLoader.shutdownNow()
    releaseScreenWakeLock()
    mediaSession.isActive = false
    mediaSession.release()
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  companion object {
    const val ACTION_UPDATE = "net.napstr.nostrfy.media.UPDATE"
    const val ACTION_CLEAR = "net.napstr.nostrfy.media.CLEAR"
    const val ACTION_PLAY = "net.napstr.nostrfy.media.PLAY"
    const val ACTION_PAUSE = "net.napstr.nostrfy.media.PAUSE"
    const val ACTION_PREVIOUS = "net.napstr.nostrfy.media.PREVIOUS"
    const val ACTION_NEXT = "net.napstr.nostrfy.media.NEXT"
    const val EXTRA_TITLE = "title"
    const val EXTRA_ARTIST = "artist"
    const val EXTRA_ARTWORK = "artwork"
    const val EXTRA_PLAYING = "playing"
    const val EXTRA_POSITION = "position"
    const val EXTRA_DURATION = "duration"
    const val EXTRA_CAN_PREVIOUS = "canPrevious"
    const val EXTRA_CAN_NEXT = "canNext"
    private const val CHANNEL_ID = "napstrfy_playback"
    private const val NOTIFICATION_ID = 7302
  }
}

/**
 * Decoded covers for the notification, kept in a tiny LRU so scrolling through
 * tracks does not re-download the same five albums.
 */
private object ArtworkCache {
  private const val MAX_ENTRIES = 4
  private const val MAX_EDGE = 512
  private const val CONNECT_TIMEOUT_MS = 5_000
  private const val READ_TIMEOUT_MS = 8_000
  private val entries = LinkedHashMap<String, Bitmap>(MAX_ENTRIES, 0.75f, true)

  @Synchronized fun peek(url: String): Bitmap? = entries[url]

  fun load(url: String): Bitmap? {
    // Covers are published as HTTPS URLs; anything else is not ours to fetch.
    if (!url.startsWith("https://")) return null
    peek(url)?.let { return it }
    val decoded = try {
      val connection = (URL(url).openConnection() as HttpURLConnection).apply {
        connectTimeout = CONNECT_TIMEOUT_MS
        readTimeout = READ_TIMEOUT_MS
        instanceFollowRedirects = true
        setRequestProperty("Accept", "image/*")
      }
      try {
        connection.inputStream.use { stream -> BitmapFactory.decodeStream(stream) }
      } finally {
        connection.disconnect()
      }
    } catch (_: Exception) {
      null
    } ?: return null
    return remember(url, decoded)
  }

  @Synchronized private fun remember(url: String, decoded: Bitmap): Bitmap {
    val longest = maxOf(decoded.width, decoded.height)
    if (longest <= MAX_EDGE) {
      store(url, decoded)
      return decoded
    }
    val ratio = MAX_EDGE.toFloat() / longest
    val scaled = Bitmap.createScaledBitmap(
      decoded,
      (decoded.width * ratio).toInt().coerceAtLeast(1),
      (decoded.height * ratio).toInt().coerceAtLeast(1),
      true
    )
    if (scaled != decoded) decoded.recycle()
    store(url, scaled)
    return scaled
  }

  @Synchronized private fun store(url: String, bitmap: Bitmap) {
    entries[url] = bitmap
    while (entries.size > MAX_ENTRIES) {
      val oldest = entries.keys.firstOrNull() ?: return
      entries.remove(oldest)
    }
  }
}
