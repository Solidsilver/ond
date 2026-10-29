# Home Manager

The flake has a Home Manager module, `programs.ond`. It installs Ond on Linux and writes the settings you give it to `managed.json`, next to Ond's own settings file. Ond applies those over whatever was set in the app, and Settings shows each one greyed out with "Set in your Nix config". Anything you leave out stays editable in the app.

```nix
{
  inputs.ond.url = "github:Solidsilver/ond";

  # in your Home Manager configuration
  imports = [ inputs.ond.homeModules.default ];

  programs.ond = {
    enable = true;
    settings = {
      server = "https://music.example.com";
      username = "me";
      theme = "dark";
      accent = "teal";
      alpha_rail = true;
      playback = {
        replay_gain = "album";
        quality = "auto";
        format = "aac";
        kbps = 256;
      };
    };
  };
}
```

On macOS, Ond comes as an app bundle, so the module installs nothing there (`package` is `null`). It still writes the settings, to `~/Library/Application Support/dev.solidsilver.ond/managed.json`. On Linux the file goes in `~/.config/ond/managed.json`. Nothing else ever writes it, so you can also write it by hand if you don't use Home Manager.

Take a setting out of your config and Ond goes back to what the user had picked in the app before. Ond reads the file when it starts, so restart it after `home-manager switch`.

## Settings

Names and values are the same as in Ond's own `settings.json`. Ond logs a warning for a setting it doesn't know or a value it can't read, and ignores that one setting.

### Account

| Setting | Values |
|---|---|
| `server` | The server's address, like `"https://music.example.com"` |
| `username` | Your username on it |

These fill in the sign-in form and lock those two fields. The password isn't a setting: Ond asks for it once and keeps it in the system keyring. If you change the server or username, Ond asks you to sign in again.

### General

| Setting | Values | Default |
|---|---|---|
| `scrobble` | Report plays to the server | `true` |
| `diagnostics_bar` | The diagnostics bar along the bottom | `false` |
| `window.system_title_bar` | Linux: the desktop's title bar instead of Ond's own buttons | `false` |

### Appearance

| Setting | Values | Default |
|---|---|---|
| `theme` | `"system"`, `"light"`, `"dark"` | `"system"` |
| `accent` | `"blue"`, `"teal"`, `"green"`, `"amber"`, `"red"`, `"rose"`, `"violet"`, `"graphite"`, or your own color as `{ custom = [ 245 197 24 ]; }` (red, green, blue) | `"blue"` |
| `adapt_colors` | Colors follow the album art | `false` |
| `backdrop` | Blurred artwork behind the Player and Mini | `false` |
| `waveform` | The track's waveform as the seek bar | `false` |
| `waveform_bars.rounded` | Rounded bar ends | `false` |
| `waveform_bars.width` | Bar thickness in half points, 2 to 8 | `4` |
| `waveform_bars.gap` | Space between bars in half points, 1 to 6 | `2` |
| `alpha_rail` | The alphabet rail beside lists sorted by name | `false` |
| `artist_photos` | Artists' photos from the server | `true` |

### Playback

| Setting | Values | Default |
|---|---|---|
| `playback.replay_gain` | `"off"`, `"track"`, `"album"`, `"auto"` | `"off"` |
| `playback.preamp_db` | Pre-amp in dB, −12 to 12 | `0.0` |
| `playback.untagged_db` | Gain for tracks without ReplayGain tags, in dB, −12 to 0 | `-6.0` |
| `playback.quality` | `"original"`, `"auto"` (original at home, transcoded elsewhere), `"transcode"` | `"original"` |
| `playback.format` | `"aac"`, `"mp3"` | `"aac"` |
| `playback.kbps` | `128`, `192`, `256`, `320` | `256` |

### Output

| Setting | Values | Default |
|---|---|---|
| `output_device` | A device ID to play through; leave it out to follow the system default | |
| `device_modes.<device ID>` | `"normal"` or `"bit-perfect"` for that device | `"normal"` |
| `device_volume_green` | In bit-perfect mode, draw the volume slider in green while it sets the device's own volume | `true` |
| `device_volume_db` | When the device's own volume shows in dB beside the slider: `"adjusting"`, `"always"`, `"never"` | `"adjusting"` |
| `device_change_toast` | Show a notice naming the output whenever playback moves to another device | `false` |
| `output_match_rate` | `true` switches the device to each song's sample rate in normal mode; `false` leaves it as the system has it (macOS only) | `false` |

Device IDs come from the system (CoreAudio's device UID on macOS; on Linux `alsa:<card id>:<device>`, like `alsa:DX3:0`, where the card id is the one in `/proc/asound/cards`) and aren't shown in the app. To find one, pick the device, or its mode in Settings › Output, in Ond once, then look in `settings.json` under `output_device` or `device_modes`.

### Offline and library

| Setting | Values | Default |
|---|---|---|
| `offline_dir` | The folder for downloads | Next to the library |
| `offline_limit_gb` | Cap on downloads in GB; leave it out for no limit | |
| `stream_cache_mb` | Cap on the streaming cache in MB | `2048` |
| `analyze_playing` | Analyze each track as it plays | `false` |

Like a folder picked in Settings, `offline_dir` gets a folder of Ond's own inside it, `Ond Offline`, because Ond deletes files it doesn't recognize in its downloads folder. When you change it, Ond moves the existing downloads there the next time it starts.

### Experimental

| Setting | Values | Default |
|---|---|---|
| `experimental.spectrum` | Live spectrum in the now-playing display and the Player | `false` |
| `experimental.spectrogram` | Spectrogram in the Info tab | `false` |
| `experimental.visualizer` | The visualizer (⌘T) | `false` |
| `experimental.visualizer_pick` | `"flow"`, `"particles"`, `"ribbons"`, `"shuffle"` | `"flow"` |

Experimental settings may change or go away between versions.

The rest of `settings.json` is state the app keeps for itself (window positions, volume, column widths, whether the queue is open, grid or list views), and Ond ignores it in `managed.json`.
