Cadence — a focused songwriting studio for electronic music
=============================================================

Cadence is a native music sequencer: write drum patterns and melodies on a
piano roll, arrange clips on a timeline, mix with per-track effects, and
export WAV or MIDI. Everything runs offline — no account, no cloud.

Updates
-------
Installed copies (the Windows setup, or Cadence.app in your Applications
folder) check for a new version each time they open (and once a day while
left open) and show an update button in the status bar. Each update is
checked against Cadence's release signature before it installs.
Help > Check for Updates... checks right away and has the switch to turn
the automatic check off; it only asks github.com for the newest version
number. The portable Windows zip doesn't update itself —
use the setup from the download page instead.

THIS BUILD IS UNSIGNED
----------------------
Cadence.exe / Cadence.app (and the Windows setup) is not code-signed, so Windows SmartScreen (or
macOS Gatekeeper) may warn the first time you run it. That is expected for
an indie release and does not mean the app is unsafe.

  Windows: click "More info" on the SmartScreen prompt, then "Run anyway".
  macOS:   right-click the app and choose Open, or approve it in
           System Settings > Privacy & Security.

First run
---------
Launch Cadence and the built-in demo song (Afterglow) opens ready to play —
press Space. Songs are .cadence files; your work autosaves for crash
recovery in your user data folder.

Contents of this package
------------------------
  Cadence.exe / Cadence.app   the application
  README.txt                  this file
  THIRD-PARTY-NOTICES.txt     licenses for every Rust crate and bundled font
  licenses/                   the Inter and DejaVu font license texts
  EULA.txt                    the end-user license (when present)

In the app, Help > About Cadence > "Third-Party Notices" shows the same
license texts.

Support
-------
Report problems and ask questions at https://github.com/Toom68/cadence
(or open an issue on the repository page).
