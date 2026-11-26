package net.myterm.app

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Keep the page clear of the status bar, navigation bar and keyboard
    // (spec 14.4): the page gets smaller when the keyboard opens, so the
    // terminal refits to what is visible rather than being covered.
    val root = findViewById<View>(android.R.id.content)
    root.setBackgroundColor(0xFF16181D.toInt())
    WindowCompat.getInsetsController(window, window.decorView).isAppearanceLightStatusBars = false
    ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
      v.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, ime.bottom))
      WindowInsetsCompat.CONSUMED
    }
  }
}
