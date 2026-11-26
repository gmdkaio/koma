package dev.koma.app.ui

import android.net.Uri
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.ImageLoader
import coil3.compose.AsyncImage
import dev.koma.app.R
import dev.koma.app.data.DriveChannel
import dev.koma.app.data.FileBucket
import dev.koma.app.data.KomaTheme
import dev.koma.app.ui.theme.LocalKomaPalette

@Composable
fun SignInScreen(
    theme: KomaTheme,
    onSelectTheme: (KomaTheme) -> Unit,
    onSignIn: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(palette.bg)
            .padding(28.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Image(
                painter = painterResource(R.drawable.ic_koma_mascot),
                contentDescription = "koma",
                modifier = Modifier.size(84.dp),
            )
            Spacer(Modifier.height(14.dp))
            Text(
                text = "koma",
                color = palette.fg,
                fontSize = 26.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                text = "Your image channels synced via Google Drive.",
                color = palette.muted,
                fontSize = 14.sp,
                textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(28.dp))
            Button(
                onClick = onSignIn,
                colors = ButtonDefaults.buttonColors(
                    containerColor = palette.accent,
                    contentColor = palette.accentFg,
                ),
                shape = RoundedCornerShape(8.dp),
                modifier = Modifier
                    .fillMaxWidth()
                    .height(48.dp),
            ) {
                Text(
                    text = "Sign in with Google Drive",
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 15.sp,
                )
            }
            Spacer(Modifier.height(28.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (t in KomaTheme.entries) {
                    val active = theme == t
                    Surface(
                        onClick = { onSelectTheme(t) },
                        color = if (active) palette.accent else palette.surfaceElevated,
                        contentColor = if (active) palette.accentFg else palette.fg,
                        shape = RoundedCornerShape(7.dp),
                    ) {
                        Text(
                            text = t.label,
                            fontSize = 12.5.sp,
                            fontWeight = FontWeight.Medium,
                            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
fun CreateChannelDialog(
    onDismiss: () -> Unit,
    onCreate: (String) -> Unit,
) {
    val palette = LocalKomaPalette.current
    var name by remember { mutableStateOf("") }

    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = palette.surfaceElevated,
        titleContentColor = palette.fg,
        textContentColor = palette.fg,
        title = { Text("New channel", fontWeight = FontWeight.Bold, fontSize = 17.sp) },
        text = {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                singleLine = true,
                placeholder = { Text("e.g. frieren", color = palette.muted) },
                prefix = { Text("# ", color = palette.muted) },
                modifier = Modifier.fillMaxWidth(),
            )
        },
        confirmButton = {
            Button(
                onClick = { if (name.isNotBlank()) onCreate(name) },
                colors = ButtonDefaults.buttonColors(
                    containerColor = palette.accent,
                    contentColor = palette.accentFg,
                ),
            ) {
                Text("Create")
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel", color = palette.muted)
            }
        },
    )
}

@Composable
fun DeleteConfirmDialog(
    count: Int,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = palette.surfaceElevated,
        titleContentColor = palette.fg,
        textContentColor = palette.muted,
        title = {
            Text(
                text = if (count == 1) "Delete image?" else "Delete $count images?",
                fontWeight = FontWeight.Bold,
                fontSize = 17.sp,
            )
        },
        text = {
            Text(
                text = "Moves the selected file(s) to Google Drive trash. Your PC app will remove them on next sync.",
                fontSize = 13.5.sp,
            )
        },
        confirmButton = {
            Button(
                onClick = onConfirm,
                colors = ButtonDefaults.buttonColors(
                    containerColor = palette.dangerBg,
                    contentColor = palette.danger,
                ),
            ) {
                Text("Delete", fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) {
                Text("Cancel", color = palette.muted)
            }
        },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MoveChannelSheet(
    count: Int,
    channels: List<DriveChannel>,
    onDismiss: () -> Unit,
    onSelectChannel: (DriveChannel) -> Unit,
    onCreateAndMove: (String) -> Unit,
) {
    val palette = LocalKomaPalette.current
    var newName by remember { mutableStateOf("") }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = palette.surface,
        contentColor = palette.fg,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 4.dp),
        ) {
            Text(
                text = "Move $count image(s) to...",
                color = palette.fg,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(10.dp))
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedTextField(
                    value = newName,
                    onValueChange = { newName = it },
                    singleLine = true,
                    placeholder = { Text("New channel...", color = palette.muted) },
                    prefix = { Text("# ", color = palette.muted) },
                    modifier = Modifier.weight(1f),
                )
                Button(
                    onClick = { if (newName.isNotBlank()) onCreateAndMove(newName) },
                    enabled = newName.isNotBlank(),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = palette.accent,
                        contentColor = palette.accentFg,
                    ),
                ) {
                    Text("Create")
                }
            }
            Spacer(Modifier.height(10.dp))
            HorizontalDivider(color = palette.border)
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 320.dp),
                contentPadding = PaddingValues(vertical = 8.dp),
            ) {
                items(channels, key = { it.id }) { ch ->
                    ChannelPickerRow(channel = ch, onClick = { onSelectChannel(ch) })
                }
            }
            Spacer(Modifier.height(16.dp))
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun IncomingShareSheet(
    uris: List<Uri>,
    channels: List<DriveChannel>,
    imageLoader: ImageLoader,
    onDismiss: () -> Unit,
    onSelectBucket: (FileBucket) -> Unit,
    onSelectChannel: (DriveChannel) -> Unit,
    onCreateAndUpload: (String) -> Unit,
) {
    val palette = LocalKomaPalette.current
    var newName by remember { mutableStateOf("") }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        containerColor = palette.surface,
        contentColor = palette.fg,
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 4.dp),
        ) {
            Text(
                text = "Save ${uris.size} item(s) to koma",
                color = palette.fg,
                fontSize = 16.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(10.dp))
            Row(
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Surface(
                    onClick = { onSelectBucket(FileBucket.THROWAWAY) },
                    color = palette.accent,
                    contentColor = palette.accentFg,
                    shape = RoundedCornerShape(8.dp),
                    modifier = Modifier.weight(1f),
                ) {
                    Column(modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp)) {
                        Text("Throwaway", fontWeight = FontWeight.Bold, fontSize = 13.5.sp)
                        Text("Auto-deletes on PC", fontSize = 11.sp, color = palette.accentFg.copy(alpha = 0.8f))
                    }
                }
                Surface(
                    onClick = { onSelectBucket(FileBucket.FILES) },
                    color = palette.surfaceElevated,
                    contentColor = palette.fg,
                    shape = RoundedCornerShape(8.dp),
                    modifier = Modifier
                        .weight(1f)
                        .border(1.dp, palette.border, RoundedCornerShape(8.dp)),
                ) {
                    Column(modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp)) {
                        Text("Files", fontWeight = FontWeight.Bold, fontSize = 13.5.sp)
                        Text("Keep in Drive", fontSize = 11.sp, color = palette.muted)
                    }
                }
            }
            Spacer(Modifier.height(10.dp))
            LazyRow(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.fillMaxWidth(),
            ) {
                items(uris) { uri ->
                    AsyncImage(
                        model = uri,
                        imageLoader = imageLoader,
                        contentDescription = null,
                        contentScale = ContentScale.Crop,
                        modifier = Modifier
                            .size(64.dp)
                            .clip(RoundedCornerShape(4.dp))
                            .border(1.dp, palette.border, RoundedCornerShape(4.dp)),
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedTextField(
                    value = newName,
                    onValueChange = { newName = it },
                    singleLine = true,
                    placeholder = { Text("New channel...", color = palette.muted) },
                    prefix = { Text("# ", color = palette.muted) },
                    modifier = Modifier.weight(1f),
                )
                Button(
                    onClick = { if (newName.isNotBlank()) onCreateAndUpload(newName) },
                    enabled = newName.isNotBlank(),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = palette.accent,
                        contentColor = palette.accentFg,
                    ),
                ) {
                    Text("Save")
                }
            }
            Spacer(Modifier.height(10.dp))
            HorizontalDivider(color = palette.border)
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 300.dp),
                contentPadding = PaddingValues(vertical = 8.dp),
            ) {
                items(channels, key = { it.id }) { ch ->
                    ChannelPickerRow(channel = ch, onClick = { onSelectChannel(ch) })
                }
            }
            Spacer(Modifier.height(16.dp))
        }
    }
}

@Composable
private fun ChannelPickerRow(
    channel: DriveChannel,
    onClick: () -> Unit,
) {
    val palette = LocalKomaPalette.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("#", color = palette.muted, fontSize = 15.sp)
        Spacer(Modifier.width(8.dp))
        Text(
            text = channel.slug,
            color = palette.fg,
            fontSize = 15.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier.weight(1f),
        )
        Text(
            text = "${channel.imageCount}",
            color = palette.muted,
            fontSize = 12.sp,
            fontFamily = FontFamily.Monospace,
        )
    }
}
