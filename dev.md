# Ari for Linux: development plan

The Linux app: Rust, gtk4-rs and libadwaita, linking ari-engine's crates directly. One process with two windows: the main window (chat, skills, settings) opens from the launcher, and the command bar overlay opens from the shortcut or the wake word. Ari always runs in the background and starts at login by default.

The design is the merged prototype on the canvas: https://claude.ai/artifact/2ULuNmFjmPd9CMJRnNK622#play-Prototype

`[ ]` not started, `[/]` in progress, `[x]` done.

## Rules for every task

- Place each task against the engine boundary (`ari-engine/docs/architecture.md`) before writing code. Tasks marked **(engine)** land in ari-engine first, with Android switched over wherever it's affected. After Keith's phone check, both repos get their CHANGELOG entries and are committed and pushed, and CI is checked.
- Every user-facing change adds its CHANGELOG `[Unreleased]` entry in the same commit.
- Portals for everything. Nothing goes in that would stop a Flathub listing.
- Follow the system's light/dark choice and accent colour. Ari has no palette of its own; the orange gradient is only in the logo.
- UI strings go through gettext, English source only.
- A stage is done when Keith's check passes. The next stage starts after that.

## Stage 1: The app starts and stays running

Keith's check: Ari opens from the app grid, closing the window leaves it running, opening it again brings the same window back, and it starts at login.

- [x] Cargo workspace with the Linux app crate (`crates/ari-gtk`), plus a Justfile: `just run`, `just install-dev`, `just check`.
- [x] App ID `dev.heyari.Ari`, desktop entry, icons and AppStream metainfo.
- [x] libadwaita application that runs as a single instance: a second launch shows the existing window.
- [x] Closing the window hides it and Ari keeps running.
- [x] Start at login, on by default, with a setting to turn it off: the Background portal inside Flatpak, an XDG autostart entry outside it.
- [x] Hyprland ignores XDG autostart, so Preferences offers to add Ari to the user's Hyprland config (`hyprland.lua`, or the older `hyprland.conf`). Switching Start at Login off takes it out.
- [x] gettext set up from the first string. Italian drafted the same way as Android's, and `just check` fails if a string is untranslated.
- [/] CI: format check, clippy, tests, packaging and translations on every push (`just check` in a Fedora container). Runs on the first push.

## Stage 2: Main window and typed chat

Keith's check: "what time is it" and "what's 12 times 7" get answers in the conversation, and the conversation is still there after a restart.

- [ ] Split view from the prototype: sidebar with Conversation, Timers and alarms, Skills and Remembered; Preferences as a dialog from the main menu. Pages other than Conversation stay empty until their stage.
- [ ] Depend on ari-engine's crates by git revision (path override for local work), and build an `ari_runtime::Runtime` with the built-in skills, stored under the app's data directory.
- [ ] Create the `VoicePipeline` with a `VoiceHost`, attach the session and the runtime, and send typed turns through `submit_text`, as Android's chat does.
- [ ] Conversation view: message bubbles, streamed answers, markdown and the thinking indicator.
- [ ] Composer from the prototype: text field with the breathing ring, and a solid accent mic button that turns into Send when there's text. The mic does nothing until stage 5.
- [ ] Conversation log kept on disk and reloaded on start, with "clear history".
- [ ] Remembered facts saved whenever the engine flags a change, listed on the Remembered page with delete.

## Stage 3: Skills, assistants and settings

Keith's check: install Coin flip from the Skills page and "flip a coin" answers; choose ChatGPT, enter a key, and a general question gets an answer.

- [ ] Skills page: browse, install, update and remove community skills through `ari_runtime::skills`.
- [ ] Each skill's settings drawn from `setting_fields` and saved through the runtime's `SettingsStore`.
- [ ] Assistant picker through `ari_runtime::assistants`, with API keys kept by the Secret portal (the Secret Service outside Flatpak).
- [ ] App settings in GSettings.
- [ ] Preferences: General (language, start at login, conversation memory) and Assistant. Voice and shortcut pages come with stage 5.
- [ ] Declare the desktop's host capabilities, so skills it can't run (messaging, launching apps by name) are never offered or routed to.

## Stage 4: Cards, timers, alarms and actions

Keith's check: "set a timer for one minute" shows a counting card, it rings after a minute with a notification, and Stop silences it. "Search for pecorino" opens the browser.

- [ ] **(engine)** Parse the presentation envelope into Rust types in the engine. Android's `PresentationEnvelope.kt` parses it today; Android reads the engine's version through ari-ffi instead.
- [ ] Cards in the conversation and the command bar: generic cards and the timer card with its countdown. Buttons go through the engine.
- [ ] Timers ring from a wall-clock deadline, so suspend doesn't stretch them. The alert sends a notification through the Notification portal, plays a sound, and stops by button, Esc or voice.
- [ ] Alarms: Linux has no shared alarm clock, so Ari keeps and rings them itself. One due while the laptop is asleep doesn't ring, like any other laptop alarm, unless a paired phone has taken it over (stage 7, which moves timers too).
- [ ] Timers and alarms page listing what's running, with cancel.
- [ ] Actions: open links and web searches through the OpenURI portal, clipboard, navigation as a map link, media controls over MPRIS, and dismissing cards, notifications and alerts.

## Stage 5: Voice from the shortcut

Keith's check: the shortcut opens the command bar, "set a timer for ten minutes for the pasta" is heard, answered aloud, and the timer card shows in the bar.

- [ ] Build `libari_voice_native.so`, sherpa-onnx and ONNX Runtime for the desktop, loaded from beside the binary.
- [ ] Microphone: 16 kHz mono from PipeWire or PulseAudio, pushed into the pipeline in 30 ms reads. With the wake word off, the mic is open only while Ari is listening.
- [ ] `VoiceHost`: model files, locale, speech-to-text events, cloud speech-to-text settings, logs.
- [ ] **(engine)** Move the choice of which speech-to-text model serves which language, and the model manifest format, out of Android's `SttModel.kt` and `ModelManifest.kt`. Downloads stay in each frontend.
- [ ] Speech model downloads with progress, checksums and resume. On-device or cloud choice in Preferences.
- [ ] `SessionHost` voice calls: overlay state, the cue sound, speech, capture mode, model loading.
- [ ] Command bar from the prototype: layer-shell where the compositor has it, an ordinary window on GNOME. Partials while listening, answer and cards below, type to follow up, "Open in Ari", Esc closes.
- [ ] Global shortcut through the GlobalShortcuts portal (GNOME 48 and later, KDE, Hyprland), plus an `ari-gtk` option to bind anywhere else.
- [ ] Speech output with Piper voices through sherpa-onnx's text-to-speech. The maintained Piper (piper1-gpl) is Python only now. Both are GPL-3, like this repo.
- [ ] Mic button in the main window: talk from the composer, and dictate into the text field.
- [ ] Barge-in only where an echo canceller is present (PipeWire's echo-cancel module), off everywhere else.

## Stage 6: Wake word

Keith's check: with the wake word on, "Hey Ari, what time is it" opens the command bar and answers.

- [ ] Wake word setting, off by default. While it's on the mic stays open, and Preferences says so.
- [ ] Wake models shipped with the app from the engine's `assets/voice`.
- [ ] **(engine)** Move the sensitivity tuner's measuring out of Android's `SensitivityTuner.kt`, then build the tuning page.
- [ ] False-wake count and the offer to fix it. The rules are already the engine's (`ari_voice::false_wake`); the app keeps the timestamps and raises the notification.
- [ ] Wake log and opt-in debug captures, as on Android.

## Stage 7: Paired devices

Keith's check: with the phone and laptop paired, "Hey Ari" near both gets one answer; "continue this on my Pixel 10" opens the conversation on the phone; an alarm or timer set on the laptop moves to the phone, with a notification, when the laptop is shut.

Every task here changes ari-android too, with its own CHANGELOG entry.

- [ ] Choose the link between devices: local network, Bluetooth, or both.
- [ ] **(engine)** Pairing and the messages between devices: pair once, a group key every paired device shares, messages encrypted with it. A new device pairs with any device already in the group.
- [ ] Pairing screens: the laptop shows a QR code and the phone scans it. Each device has a name the user can change.
- [ ] **(engine)** Wake vote: every device that hears the wake word shares how well it heard it and waits a fraction of a second, and the best one answers. Losing a vote isn't a false wake. If the vote can't happen, they all answer.
- [ ] **(engine)** Handoff: "continue this on <device>" sends the conversation to that device, which opens the chat with it.
- [ ] **(engine)** Alarms and timers set on the desktop are copied to the paired phone, which keeps them on standby while the desktop answers. When it stops answering (moved away, asleep, shut down), they move to the phone, ring through Ari's own alert so either side can cancel them, and the user is told they moved. The phone checks when it loses contact and again shortly before each one is due.
- [ ] Once an alarm or timer has moved it stays on the phone, and the desktop drops its copy when they next meet, so it never rings twice.

## Stage 8: The user's own data

Keith's check: "remind me to call the dentist tomorrow at 10" lands in his CalDAV calendar, and "what's the weather" uses where he is.

- [ ] Calendar and tasks over CalDAV, with the account set up in Preferences. Task lists pushed in as the `tasks.lists` vocabulary.
- [ ] Location through the Location portal.
- [ ] Skill sign-in (`ari::authorize`): open the browser and take the redirect back.
- [ ] Media services from the MPRIS players on the system.

## Stage 9: On-device assistant

Keith's check: with no cloud assistant set, a general question is answered on the laptop.

- [ ] **(engine)** Pick the desktop's on-device runtime: LiteRT-LM if it builds from source inside Flatpak, otherwise GGUF through llama.cpp or another runtime. A laptop isn't held to the phone's limits, so larger models are on the table.
- [ ] Model download and the on-device model picker in Preferences.

## Stage 10: First run

Keith's check: from a fresh install he reaches a spoken answer without opening Preferences.

- [ ] First-run pages: language, assistant, speech model download, microphone, shortcut, start at login, and the Hyprland offer.
- [ ] Empty states for each sidebar page.

## Stage 11: Packaging and release

Keith's check: Ari installs from FriendlyHub or Flathub and passes the checks of stages 1 to 10 inside the sandbox.

- [ ] Flatpak build of every dependency from source: ONNX Runtime, sherpa-onnx, TFLite Micro, LiteRT-LM.
- [ ] Flatpak manifest, written by Keith: Flathub accepts neither AI-written manifests nor AI-made submission PRs.
- [ ] Sandbox pass: every feature works through portals with only the permissions Flathub allows. The Hyprland offer needs `--filesystem=xdg-config/hypr`, or falls back to showing the line to copy.
- [ ] AppStream release notes, screenshots and the AI disclosure Flathub asks for.
- [ ] Release process: version tags, and CHANGELOG `[Unreleased]` turned into the version.
- [ ] Publish on Flathub, or on FriendlyHub if Flathub refuses.

## Not in this plan

- macOS and Windows.
- Bug reporting (out of scope for ari-desktop, see `docs/bug-reporting.md`).
- Messaging and contacts: no desktop skill needs them yet.
