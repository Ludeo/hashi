<p align="center">
  <img src="src-tauri/icons/128x128.png" width="96" height="96" alt="">
</p>

<h1 align="center">Hashi</h1>

<p align="center">
  Sends your osu!mania plays to <a href="https://mania-tracker.com">Mania Tracker</a> as you play them.
</p>

---

Hashi is a small desktop app for Windows and Linux. You connect it to Mania Tracker once
and leave it open while you play. Every mania map you finish is then submitted to your profile
along with its replay, on osu!stable and osu!lazer.

It only sends plays you actually played: watched replays, other game modes and plays with Auto or
Relax are skipped. If Mania Tracker can't be reached, plays wait on your computer and are sent
once it's back. Each play in the list gets a stamp saying whether it was submitted, and if it
wasn't, why. New versions install from inside the app.

## What you need

- Windows 10/11 or Linux (x86_64). macOS isn't supported.
- [tosu](https://github.com/tosuapp/tosu), running while you play. Hashi uses it to see when
  you finish a play and which map it was on. If you don't have tosu, the app offers to download it
  for you, and from then on it starts tosu by itself.
- To be logged in to Mania Tracker in your browser, since that's how you connect the app.

On Linux, osu!lazer needs tosu's Linux build. The Windows build of tosu running under Wine only
sees osu!stable.

## Install

Download the latest version from [mania-tracker.com/bridge](https://mania-tracker.com/bridge),
which picks the right file for your system. Every release is also on this repository's
[Releases](../../releases) page.

### Windows

Open `Hashi.exe`. It sets itself up in a few seconds without asking
anything and then opens Hashi. From then on it's in the Start menu and on your desktop. It
only installs for your user, so it doesn't need admin rights. Opening the setup file again later
just opens the app, unless the setup is a newer version than the one you have.

Windows may say "Windows protected your PC", because the installer isn't signed with a paid
certificate. Click "More info", then "Run anyway". You only see this the first time, since updates
install from inside the app.

### Linux

| Package | How to install |
|---|---|
| AppImage (any distro) | `chmod +x "Hashi_x.y.z_amd64.AppImage"`, then run it |
| .deb (Debian, Ubuntu, Mint) | `sudo apt install "./Hashi_x.y.z_amd64.deb"` |
| .rpm (Fedora, openSUSE) | `sudo dnf install "./Hashi-x.y.z-1.x86_64.rpm"` |

All three update from inside the app. The .deb and .rpm ask for your password when installing an
update.

The tray icon needs a desktop that shows tray icons, like KDE or most panels and bars (GNOME needs
the AppIndicator extension). Without one the app works the same, just without a tray icon.

## Getting started

1. Open Hashi and click "Connect to Mania Tracker".
2. Your browser opens Mania Tracker. Sign in if you need to, and approve the connection.
3. Back in the app you'll see your name and avatar. Leave it open and play.

When you finish a mania map it appears at the top of the list. It shows "Waiting for replay" until
osu! has saved the replay, and gets its stamp once Mania Tracker has checked it. Click a submitted
play to see it on Mania Tracker.

A submitted play also shows where it placed on the map's leaderboard, like #7 of 41. If it didn't
beat your best on that map, the number is your best's spot, drawn as an outline. Click it, or
"Leaderboard" on plays that aren't on the board, to see the map's top 50 with your own row
highlighted. While you're picking a song in osu!, the podium button next to the gear shows the
leaderboard of the selected map and follows it as you scroll. It follows your mods too: with Half
Time or Daycore it shows only HT/DC plays, with Double Time or Nightcore only DT/NC plays, at any
rate and with each play's rate next to it. Invert, Hold Off and No Release each have their own
board as well, and Mirror and the visual mods don't change anything.

A play that hasn't been sent yet has Hold and Remove under it. Hold keeps it back until you press
Send, and Remove drops it so it's never sent. Most plays go out a few seconds after the map ends,
so these mostly matter for plays still waiting for their replay.

Closing the window quits the app. If you'd rather keep it running out of the way, use the hide to
tray button instead. In the tray the window is closed completely and only the part that sends your
plays keeps running, so it takes very little memory.

Plays you finish while the app isn't connected aren't sent later, so connect before you play. If
you'd rather not think about it, turn on "Open Hashi on startup" in the settings.

## Settings

The gear in the top right corner opens the settings.

"Send my plays" and "Share my live status" pick what goes to Mania Tracker, and one of them always
stays on. Plays are on by default and the live status is off. The live status shows what you're
doing in osu! right now (in the menus, picking a song, playing or watching a replay) and on which
map. With plays off, the maps you finish are left alone, and plays still waiting to be sent wait
until you turn plays back on.

"Open tosu with Hashi" is off by default. With it on, the app starts tosu in the
background when it opens and stops it again when it closes. Getting tosu through the app turns
this on for you. It looks for tosu in the usual places, and remembers where tosu was the last time
it saw it running. If yours is somewhere else and you've never had it open alongside Hashi, set the
`TOSU_PATH` environment variable to the full path of the tosu program.

With it on, "Open its dashboard too" picks whether tosu opens its dashboard in your browser each
time it starts. It changes tosu's own setting, so it counts however tosu is started.

"Open Hashi on startup" is off by default. With it on, the app starts with your
computer and goes straight to the tray, so it's already running when you play.

"Start in the tray" is off by default. With it on, opening the app only puts its icon in the tray.
Click the icon, or open the app again, to see the window.

"Show dans" shows or hides the dan next to each play's MSD.
The theme color has the same presets and sliders as Mania Tracker.

Disconnect signs the app out and revokes its access. You can also remove it from your connected
apps on Mania Tracker.

## Updates

The app checks for a new version when it starts and every few hours after that. When one is out,
a note at the top says so. Clicking "Update" downloads and installs it, and the app opens again on
the new version. "Later" hides the note until you next start the app.

Updates are signed, and the app only installs files signed with Hashi's release key. Even
someone who broke into the update server couldn't get it to install anything else.

## What gets sent

For each mania play you finish while connected, the app sends the replay file osu! saved, the
chart file (`.osu`) if Mania Tracker doesn't already have that exact version, the app's version,
and whether the play was on osu!stable or osu!lazer.

If "Share what I'm playing" is on, it also sends what you're doing in osu! at the moment and the
current map's name, mods and speed.

Nothing else is sent. Apart from Mania Tracker, the app only connects to tosu on your own computer,
to osu!'s servers for your avatar, and to GitHub when downloading an update or tosu.

## Troubleshooting

### "Linux blocks tosu from reading the game"

Most Linux distros stop programs from reading another program's memory, which is how tosu works.
The app shows the exact command that allows it, which looks like this:

```sh
sudo setcap cap_sys_ptrace=eip /path/to/tosu
```

Run it once, then restart tosu (or Hashi, if it starts tosu for you). You'll need to run it
again after updating tosu.

### A play doesn't show up

Check that tosu is running. The line under your name says what osu! is doing, and if it says
"tosu · Not running" the app can't see your plays. Only finished plays count, so quitting or
failing a map sends nothing. On Linux with osu!lazer, make sure you're running tosu's Linux build.

### A play says "Not submitted"

The reason is written under the stamp. Usually osu! never saved the replay, the map's file was
changed, or Mania Tracker didn't accept the play.

### Something else

The app keeps a log called `bridge.log` in its data folder (see below). It's the first place to
look, and worth attaching when you report a problem.

## Where your data lives

| Windows | Linux |
|---|---|
| `%APPDATA%\mania-bridge` | `~/.config/mania-bridge` |

That folder holds your settings, plays still waiting to be sent, the log, and the connection (a
sign-in token and the app's own key), readable only by your user. If the app downloaded tosu for
you, tosu is in there too. Deleting it signs the app out on
this computer and forgets any plays that were still waiting.

To uninstall, remove the app the usual way (Installed apps on Windows, your package manager, or
deleting the AppImage). On Windows it asks whether to delete your data too. On Linux, delete
that folder if you want your data gone as well.

## License

MIT, see [LICENSE](LICENSE). The Outfit font in `counter/fonts` is under the SIL Open Font License.
