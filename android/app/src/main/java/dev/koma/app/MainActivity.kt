package dev.koma.app

import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import dev.koma.app.data.KomaTheme
import dev.koma.app.ui.KomaApp
import dev.koma.app.ui.KomaViewModel
import dev.koma.app.ui.theme.KomaAppTheme

class MainActivity : ComponentActivity() {
    private val viewModel: KomaViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handleIncomingIntent(intent)

        setContent {
            val state by viewModel.uiState.collectAsState()

            LaunchedEffect(state.theme) {
                val style = if (state.theme == KomaTheme.GOFUN) {
                    SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
                } else {
                    SystemBarStyle.dark(Color.TRANSPARENT)
                }
                enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
            }

            KomaAppTheme(theme = state.theme) {
                KomaApp(
                    state = state,
                    viewModel = viewModel,
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncomingIntent(intent)
    }

    private fun handleIncomingIntent(intent: Intent?) {
        if (intent == null) return
        when (intent.action) {
            Intent.ACTION_SEND -> {
                val uri = extractSingleStream(intent)
                if (uri != null) {
                    viewModel.handleIncomingShare(listOf(uri))
                }
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                val uris = extractMultipleStreams(intent)
                if (uris.isNotEmpty()) {
                    viewModel.handleIncomingShare(uris)
                }
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun extractSingleStream(intent: Intent): Uri? {
        val direct = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_STREAM)
        }
        return direct ?: intent.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
    }

    @Suppress("DEPRECATION")
    private fun extractMultipleStreams(intent: Intent): List<Uri> {
        val list = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
        }
        if (!list.isNullOrEmpty()) return list
        val clip = intent.clipData ?: return emptyList()
        return buildList {
            for (i in 0 until clip.itemCount) {
                clip.getItemAt(i).uri?.let(::add)
            }
        }
    }
}
