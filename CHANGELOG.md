# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Ari for Linux opens from the launcher. Closing its window hides it: Ari keeps running in
  the background, and opening it again brings the same window back.
- Start at Login, on by default, in Preferences. Inside Flatpak it goes through the
  Background portal. Hyprland starts nothing at login by itself, so there Preferences can
  add Ari to your Hyprland config, and switching Start at Login off takes it out again.
- Italian translation.
- The main window: a sidebar with Conversation, Timers and Alarms, Skills and Remembered.
  Timers and Alarms and Skills are empty for now.
- Type to Ari on the Conversation page. The built-in skills answer: the time, the date,
  sums and greetings. The conversation is kept when Ari restarts; "clear history" or New
  Conversation empties it.
- Ari remembers what you ask it to ("remember that…"). The Remembered page lists it, and
  each can be deleted.
- The Skills page: browse the skill registry, then install, update and remove skills, each
  with its own settings. Skills that need something Ari for Linux can't do yet are shown
  with what they need, and can't be installed.
- Assistants: choose ChatGPT, Claude or Gemini in Preferences to answer what no skill does,
  or say yes when Ari offers one you've just installed. API keys are kept in your keyring.
  Assistants' answers show their formatting, tables included.
- Up and Down in the message field step through what you've sent before.
- Installing a skill that doesn't speak Ari's language asks first.
- Preferences: the language Ari speaks, and whether and for how long it remembers the
  conversation.
- Skills' cards show under their answers: timers count down, the weather shows its
  figures, and the buttons work. A timer rings at its time with a sound and a
  notification; Stop, Esc or typing "stop" silences it.
- Alarms: Ari keeps and rings them itself. One that comes while the computer is off or
  asleep doesn't ring; a notification says it was missed.
- The Timers and Alarms page lists what's counting down and the alarms, each with a way
  to cancel it.
- Web searches, links and directions open in your browser. Preferences picks the search
  engine (DuckDuckGo by default) and the map (OpenStreetMap by default).
- Skills can copy to the clipboard.
- Skills' notifications, and a daily check for skill updates that says when there are
  some.
- When no installed skill can answer but one in the store could, Ari offers it with a
  button that opens it.
