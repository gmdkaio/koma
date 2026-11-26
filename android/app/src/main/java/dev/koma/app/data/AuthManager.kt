package dev.koma.app.data

import android.content.Context
import android.content.Intent
import android.content.IntentSender
import com.google.android.gms.auth.api.identity.AuthorizationClient
import com.google.android.gms.auth.api.identity.AuthorizationRequest
import com.google.android.gms.auth.api.identity.Identity
import com.google.android.gms.common.api.Scope
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.tasks.await

sealed interface SignInOutcome {
    data object SignedIn : SignInOutcome
    data class NeedsResolution(val intentSender: IntentSender) : SignInOutcome
}

class AuthManager(
    context: Context,
) {
    private val appContext = context.applicationContext
    private val prefs = appContext.getSharedPreferences("koma_auth", Context.MODE_PRIVATE)
    private val authClient: AuthorizationClient = Identity.getAuthorizationClient(appContext)
    private val refreshMutex = Mutex()

    private val authRequest: AuthorizationRequest = AuthorizationRequest.Builder()
        .setRequestedScopes(listOf(Scope(DRIVE_SCOPE)))
        .build()

    val isSignedIn: Boolean
        get() = prefs.getBoolean(KEY_SIGNED_IN, false)

    fun loadTheme(): KomaTheme = KomaTheme.fromKey(prefs.getString(KEY_THEME, null))

    fun saveTheme(theme: KomaTheme) {
        prefs.edit().putString(KEY_THEME, theme.key).apply()
    }

    suspend fun requestSignIn(): Result<SignInOutcome> = runCatching {
        val result = authClient.authorize(authRequest).await()
        if (result.hasResolution()) {
            val sender = result.pendingIntent?.intentSender
                ?: error("Google authorization resolution missing PendingIntent")
            SignInOutcome.NeedsResolution(sender)
        } else {
            val token = result.accessToken
            if (token.isNullOrBlank()) {
                error("Google Play Services returned an empty access token")
            }
            saveToken(token)
            SignInOutcome.SignedIn
        }
    }

    fun handleAuthorizationResult(data: Intent?): Result<Unit> = runCatching {
        if (data == null) error("Google sign-in canceled")
        val result = authClient.getAuthorizationResultFromIntent(data)
        val token = result.accessToken
        if (token.isNullOrBlank()) {
            error("Google authorization did not return an access token")
        }
        saveToken(token)
    }

    suspend fun getValidAccessToken(): String = refreshMutex.withLock {
        val cached = currentAccessTokenBlocking()
        if (!cached.isNullOrBlank()) {
            return@withLock cached
        }
        if (!isSignedIn) {
            error("Not signed in to Google Drive")
        }
        val result = authClient.authorize(authRequest).await()
        val token = result.accessToken
        if (result.hasResolution() || token.isNullOrBlank()) {
            signOut()
            error("Google Drive authorization expired. Sign in again.")
        }
        saveToken(token)
        token
    }

    fun currentAccessTokenBlocking(): String? {
        val token = prefs.getString(KEY_ACCESS_TOKEN, null)
        val expiresAt = prefs.getLong(KEY_EXPIRES_AT, 0L)
        return if (!token.isNullOrBlank() && System.currentTimeMillis() < expiresAt) {
            token
        } else {
            null
        }
    }

    fun signOut() {
        prefs.edit()
            .putBoolean(KEY_SIGNED_IN, false)
            .remove(KEY_ACCESS_TOKEN)
            .remove(KEY_EXPIRES_AT)
            .apply()
    }

    private fun saveToken(token: String) {
        val expiresAt = System.currentTimeMillis() + TOKEN_TTL_MS
        prefs.edit()
            .putBoolean(KEY_SIGNED_IN, true)
            .putString(KEY_ACCESS_TOKEN, token)
            .putLong(KEY_EXPIRES_AT, expiresAt)
            .apply()
    }

    companion object {
        private const val DRIVE_SCOPE = "https://www.googleapis.com/auth/drive.file"
        private const val TOKEN_TTL_MS = 50L * 60L * 1000L
        private const val KEY_SIGNED_IN = "signed_in"
        private const val KEY_ACCESS_TOKEN = "access_token"
        private const val KEY_EXPIRES_AT = "expires_at"
        private const val KEY_THEME = "theme"
    }
}
